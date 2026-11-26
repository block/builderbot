use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tauri::{AppHandle, Manager};

use crate::actions::events::TauriExecutionListener;
use crate::actions::{ActionExecutor, ActionRegistry, ActionType};
use crate::blox::{self, ExecMode};
use crate::git;
use crate::store::{self, Store};
use crate::{BranchWithWorkdir, PollWorkspaceResult};

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorktreeSetupProgress {
    pub branch_id: String,
    pub phase: String,
    pub detail: Option<String>,
}

/// Emit a `worktree-setup-progress` event for a branch and phase.
pub(crate) fn emit_setup_progress<R: tauri::Runtime>(
    handle: &AppHandle<R>,
    branch_id: &str,
    phase: &str,
    detail: Option<String>,
) {
    crate::web_server::emit_to_all(
        handle,
        "worktree-setup-progress",
        WorktreeSetupProgress {
            branch_id: branch_id.to_string(),
            phase: phase.to_string(),
            detail,
        },
    );
}

/// Default idle timeout (in minutes) for Staged workstations.
pub(crate) const WORKSPACE_IDLE_TIMEOUT_MINUTES: u32 = 10080;

// In-memory cache: workspace name → numeric workstation ID.
// Populated by the status pollers and branch setup when `blox ws info` or
// `ws list` returns an ID; read by `to_branch_with_workdir` when serializing for the frontend.
pub(crate) fn workstation_id_cache() -> &'static Mutex<HashMap<String, u64>> {
    static CACHE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn cached_workstation_id(workspace_name: &str) -> Option<u64> {
    workstation_id_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(workspace_name).copied())
}

/// Per-branch lock that serializes concurrent [`setup_worktree_sync`] calls
/// for the same branch — e.g. the branch card's Retry racing the background
/// project setup, or MCP `add_project_repo`. Follows the same pattern as
/// `REPO_CLONE_LOCKS` in `git/github.rs`.
static WORKTREE_SETUP_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

fn worktree_setup_lock_for(branch_id: &str) -> Arc<Mutex<()>> {
    let locks = WORKTREE_SETUP_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = locks.lock().unwrap_or_else(|p| p.into_inner());
    // Opportunistically drop lock entries that are no longer referenced by any
    // active setup operation so the map does not grow without bound.
    map.retain(|_, v| Arc::strong_count(v) > 1);
    map.entry(branch_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Per-clone lock that serializes choosing a local branch name and creating
/// its worktree (see [`check_out_worktree`]), and undoing a worktree whose
/// setup failed ([`discard_incomplete_worktree`]). Branches of different
/// projects are different rows, so [`WORKTREE_SETUP_LOCKS`] does not keep
/// two of them apart, and they share the clone when they are for the same
/// repository.
///
/// Only local git runs under it. Nothing that reaches the network may: every
/// other setup in the clone waits behind the holder.
///
/// Taken after a branch's [`WORKTREE_SETUP_LOCKS`] entry when both are held,
/// and never the other way round. It is not reentrant.
static CLONE_WORKTREE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

fn clone_worktree_lock_for(repo_path: &Path) -> Arc<Mutex<()>> {
    let locks = CLONE_WORKTREE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = locks.lock().unwrap_or_else(|p| p.into_inner());
    // As for `WORKTREE_SETUP_LOCKS`: drop the entries nobody holds.
    map.retain(|_, v| Arc::strong_count(v) > 1);
    // One clone must be one key however its path was spelled.
    let key = repo_path
        .canonicalize()
        .unwrap_or_else(|_| repo_path.to_path_buf());
    map.entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Lock for one pull request's head in one clone, held around the fetch of
/// it in [`check_out_worktree`]. Every setup of a pull request fetches into
/// the same local ref, `refs/staged/pr/<N>`. Two fetches of a ref that is
/// already there at an older commit both read the old value, and git refuses
/// the second update: `cannot lock ref 'refs/staged/pr/<N>': is at <new> but
/// expected <old>`. One at a time, the second finds the ref up to date.
///
/// The fetch reaches the network, so this is never held together with the
/// clone's [`CLONE_WORKTREE_LOCKS`] entry: it is taken once that one has
/// been let go and released before that one is retaken. Setups of other
/// pull requests and of plain branches do not take it.
static PR_HEAD_FETCH_LOCKS: OnceLock<Mutex<PrHeadFetchLocks>> = OnceLock::new();

/// Keyed by the clone's canonical path and the pull request's number.
type PrHeadFetchLocks = HashMap<(PathBuf, u64), Arc<Mutex<()>>>;

fn pr_head_fetch_lock_for(repo_path: &Path, pr_number: u64) -> Arc<Mutex<()>> {
    let locks = PR_HEAD_FETCH_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = locks.lock().unwrap_or_else(|p| p.into_inner());
    // As for `WORKTREE_SETUP_LOCKS`: drop the entries nobody holds.
    map.retain(|_, v| Arc::strong_count(v) > 1);
    // As for `CLONE_WORKTREE_LOCKS`: one clone is one key.
    let clone = repo_path
        .canonicalize()
        .unwrap_or_else(|_| repo_path.to_path_buf());
    map.entry((clone, pr_number))
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn get_store(store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized — please reset from the startup prompt".into())
}

/// Public wrapper for `to_branch_with_workdir` for use by the web server.
pub fn to_branch_with_workdir_public(
    branch: store::Branch,
    workdir_path: Option<String>,
) -> BranchWithWorkdir {
    to_branch_with_workdir(branch, workdir_path)
}

pub(crate) fn to_branch_with_workdir(
    branch: store::Branch,
    workdir_path: Option<String>,
) -> BranchWithWorkdir {
    let workstation_id = branch
        .workspace_name
        .as_deref()
        .and_then(|name| workstation_id_cache().lock().ok()?.get(name).copied());
    BranchWithWorkdir {
        id: branch.id,
        project_id: branch.project_id,
        project_repo_id: branch.project_repo_id,
        branch_name: branch.branch_name,
        base_branch: branch.base_branch,
        pr_number: branch.pr_number,
        branch_type: branch.branch_type,
        workspace_name: branch.workspace_name,
        workstation_id,
        workspace_status: branch.workspace_status,
        pr_state: branch.pr_state,
        pr_checks_status: branch.pr_checks_status,
        pr_review_decision: branch.pr_review_decision,
        pr_mergeable: branch.pr_mergeable,
        pr_draft: branch.pr_draft,
        pr_url: branch.pr_url,
        pr_updated_at: branch.pr_updated_at,
        pr_fetched_at: branch.pr_fetched_at,
        pr_head_sha: branch.pr_head_sha,
        pr_title: branch.pr_title,
        setup_complete: branch.setup_complete,
        deferred_push: branch.deferred_push.map(|d| d.as_column_name()),
        deferred_push_requested_at: branch.deferred_push.map(|d| d.requested_at),
        worktree_path: workdir_path,
        created_at: branch.created_at,
        updated_at: branch.updated_at,
        commit_count: None,
    }
}

pub(crate) fn project_primary_repo(project: &store::Project) -> Result<&str, String> {
    project
        .primary_repo()
        .ok_or_else(|| format!("Project '{}' has no repository attached", project.name))
}

pub(crate) fn resolve_branch_repo_slug(
    store: &Arc<Store>,
    project: &store::Project,
    branch: &store::Branch,
) -> Result<String, String> {
    if let Some(repo_id) = &branch.project_repo_id {
        if let Some(repo) = store.get_project_repo(repo_id).map_err(|e| e.to_string())? {
            return Ok(repo.github_repo);
        }
    }
    Ok(project_primary_repo(project)?.to_string())
}

/// The Blox workspace a remote project's branches share, or `None` while no
/// remote branch names one — a remote project created without a repo has no
/// workspace until its first repo is added and provisions one. A remote
/// branch with a blank name is passed over rather than taken as "none", since
/// `None` makes the next added repo provision a second workspace.
pub(crate) fn existing_project_workspace_name(
    store: &Store,
    project_id: &str,
) -> Result<Option<String>, String> {
    Ok(store
        .list_branches_for_project(project_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|b| b.branch_type == store::BranchType::Remote)
        .filter_map(|b| b.workspace_name)
        .find(|name| !name.trim().is_empty()))
}

/// Resolve the shared workspace name for a remote project.
///
/// If any remote branch already exists for the project, reuse its workspace
/// name so all project repos stay on the same Blox workspace.
pub(crate) fn resolve_project_workspace_name(
    store: &Arc<Store>,
    project: &store::Project,
    fallback_workspace_name: Option<&str>,
) -> Result<String, String> {
    if let Some(name) = existing_project_workspace_name(store, &project.id)? {
        return Ok(name);
    }

    if let Some(name) = fallback_workspace_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(name.to_string());
    }

    Ok(infer_workspace_name(
        &project.id,
        &infer_branch_name(&project.name),
    ))
}

/// Reject unsafe workspace-relative paths.
pub(crate) fn validate_workspace_subpath(subpath: &str) -> Result<String, String> {
    let trimmed = subpath.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Err("Workspace subpath must not be empty".to_string());
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(format!(
            "Workspace subpath '{trimmed}' must be relative, not absolute"
        ));
    }
    if trimmed
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(format!(
            "Workspace subpath '{trimmed}' contains an invalid path segment"
        ));
    }
    Ok(trimmed.to_string())
}

/// Extract the repository name from a GitHub repo slug (e.g. `squareup/g2` → `g2`).
///
/// The name is sanitised so it is safe to use as a directory name on the
/// workspace filesystem.
pub(crate) fn repo_name_from_github_repo(github_repo: &str) -> String {
    let raw = github_repo.rsplit('/').next().unwrap_or(github_repo);
    let collapsed = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        "repo".to_string()
    } else {
        collapsed
    }
}

/// Build the argv every workspace git call hands to `ws_exec`.
///
/// A repo subpath becomes `git -C <resolved> …`, pinning the command to that
/// clone; without one the command runs bare, at whatever directory the
/// workspace hands a bare exec. That one rule is what every remote read of a
/// branch's HEAD rests on, so it lives in a pure function that can be tested
/// without a workstation.
fn workspace_git_args(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    git_args: &[&str],
) -> Result<Vec<String>, blox::BloxError> {
    let mut owned = Vec::<String>::with_capacity(3 + git_args.len());
    owned.push("git".to_string());
    if let Some(subpath) = repo_subpath.map(str::trim).filter(|s| !s.is_empty()) {
        let resolved = resolve_workspace_repo_path(workspace_name, subpath)?;
        owned.push("-C".to_string());
        owned.push(resolved);
    }
    owned.extend(git_args.iter().map(|arg| (*arg).to_string()));
    Ok(owned)
}

/// Run a git command inside a Blox workspace as Staged plumbing.
///
/// Always [`ExecMode::Transient`]: callers are quick reads and ref-only
/// writes, which would otherwise flood the workstation's Blox command
/// history. Anything that rewrites the worktree (`reset --hard`, `clean`,
/// `merge`, `checkout`) or can run past 60 s goes through
/// [`run_workspace_git_with_mode`] as a tracked exec instead: Blox kills a
/// transient command at 60 s, and a git killed mid-rewrite leaves a
/// half-updated tree and `.git/index.lock`.
pub(crate) fn run_workspace_git(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    git_args: &[&str],
) -> Result<String, blox::BloxError> {
    run_workspace_git_with_mode(workspace_name, repo_subpath, ExecMode::Transient, git_args)
}

/// [`run_workspace_git`] with an explicit [`ExecMode`], for the worktree
/// mutations that must not be killed part-way.
pub(crate) fn run_workspace_git_with_mode(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    mode: ExecMode,
    git_args: &[&str],
) -> Result<String, blox::BloxError> {
    let owned = workspace_git_args(workspace_name, repo_subpath, git_args)?;
    let borrowed = owned.iter().map(String::as_str).collect::<Vec<_>>();
    blox::ws_exec(workspace_name, mode, &borrowed)
}

/// Execute a shell script inside a Blox workspace via `sh -c`.
///
/// Positional arguments are passed as `$1`, `$2`, etc. This allows batching
/// multiple git commands into a single `ws_exec` round-trip. Always
/// [`ExecMode::Transient`], like [`run_workspace_git`].
pub(crate) fn run_workspace_shell(
    workspace_name: &str,
    script: &str,
    args: &[&str],
) -> Result<String, blox::BloxError> {
    let mut owned = Vec::<String>::with_capacity(3 + args.len());
    owned.push("sh".to_string());
    owned.push("-c".to_string());
    owned.push(script.to_string());
    // $0 placeholder (conventional for sh -c)
    owned.push("_".to_string());
    owned.extend(args.iter().map(|arg| (*arg).to_string()));
    let borrowed = owned.iter().map(String::as_str).collect::<Vec<_>>();
    blox::ws_exec(workspace_name, ExecMode::Transient, &borrowed)
}

/// Like [`run_workspace_git`], returning raw stdout bytes. Always
/// [`ExecMode::Transient`]; `blox-cli` base64-encodes the output so binary
/// blobs survive the transient path's UTF-8 requirement.
pub(crate) fn run_workspace_git_bytes(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    git_args: &[&str],
) -> Result<Vec<u8>, blox::BloxError> {
    let owned = workspace_git_args(workspace_name, repo_subpath, git_args)?;
    let borrowed = owned.iter().map(String::as_str).collect::<Vec<_>>();
    blox::ws_exec_bytes(workspace_name, ExecMode::Transient, &borrowed)
}

pub(crate) async fn run_blox_blocking<T, F>(op: F) -> Result<T, blox::BloxError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, blox::BloxError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(op)
        .await
        .map_err(|e| blox::BloxError::CommandFailed(format!("blox task failed: {e}")))?
}

/// The branch on origin a Blox workspace checkout of `local` pushes to — the
/// workspace-side counterpart of [`git::upstream_branch_name`]. Blocking: one
/// workspace exec, two when the branch tracks something on origin.
pub(crate) fn workspace_upstream_branch_name(
    ws_name: &str,
    repo_subpath: Option<&str>,
    local: &str,
    base_branch: &str,
) -> String {
    git::upstream_branch_name_with(
        |args| run_workspace_git(ws_name, repo_subpath, args).map_err(|e| e.to_string()),
        local,
        base_branch,
    )
}

/// The branch on origin `branch` pushes to, pulls from, rebases onto and is
/// looked up on GitHub by: its checkout's upstream (see
/// [`git::upstream_branch_name`]), or its own name when it has no worktree
/// yet or tracks nothing.
///
/// A remote-workspace branch resolves to its own name here: reading its
/// upstream costs a workspace exec, which the sync queue paths that
/// deliberately avoid depending on a running workspace must not block on.
/// Use [`resolve_remote_branch`] where that exec is acceptable.
pub(crate) fn resolve_remote_branch_sync(store: &Arc<Store>, branch: &store::Branch) -> String {
    if branch.branch_type == store::BranchType::Remote {
        return branch.branch_name.clone();
    }
    match store.get_workdir_for_branch(&branch.id) {
        Ok(Some(wd)) if Path::new(&wd.path).exists() => git::upstream_branch_name(
            Path::new(&wd.path),
            &branch.branch_name,
            &branch.base_branch,
        ),
        Ok(_) => branch.branch_name.clone(),
        Err(e) => {
            log::warn!(
                "Could not look up the worktree of '{}' to resolve its remote branch: {e}",
                branch.branch_name
            );
            branch.branch_name.clone()
        }
    }
}

/// [`resolve_remote_branch_sync`] that also asks the Blox workspace for a
/// remote-workspace branch. Blocking work runs off the async runtime.
pub(crate) async fn resolve_remote_branch(store: &Arc<Store>, branch: &store::Branch) -> String {
    let fallback = branch.branch_name.clone();
    if branch.branch_type != store::BranchType::Remote {
        let store = Arc::clone(store);
        let branch = branch.clone();
        return tauri::async_runtime::spawn_blocking(move || {
            resolve_remote_branch_sync(&store, &branch)
        })
        .await
        .unwrap_or(fallback);
    }

    let Some(ws_name) = branch.workspace_name.clone() else {
        return fallback;
    };
    let repo_subpath = match resolve_branch_workspace_subpath(store, branch) {
        Ok(subpath) => subpath,
        Err(e) => {
            log::warn!("Could not resolve the workspace subpath of '{fallback}': {e}");
            return fallback;
        }
    };
    let local = branch.branch_name.clone();
    let base_branch = branch.base_branch.clone();
    run_blox_blocking(move || {
        Ok(workspace_upstream_branch_name(
            &ws_name,
            repo_subpath.as_deref(),
            &local,
            &base_branch,
        ))
    })
    .await
    .unwrap_or(fallback)
}

/// Run `args` inside a Blox workspace on the blocking pool.
///
/// Unlike the sync plumbing helpers this takes a mode: its callers are
/// workspace setup steps, where a clone or fetch can outlast the transient
/// path's 60 s limit.
pub(crate) async fn ws_exec_async(
    workspace_name: &str,
    mode: ExecMode,
    args: &[&str],
) -> Result<String, blox::BloxError> {
    let ws_name = workspace_name.to_string();
    let owned_args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    run_blox_blocking(move || {
        let borrowed = owned_args.iter().map(String::as_str).collect::<Vec<_>>();
        blox::ws_exec(&ws_name, mode, &borrowed)
    })
    .await
}

/// [`run_workspace_git`] on the blocking pool, with an explicit mode for the
/// same reason as [`ws_exec_async`].
pub(crate) async fn run_workspace_git_async(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    mode: ExecMode,
    git_args: &[&str],
) -> Result<String, blox::BloxError> {
    let ws_name = workspace_name.to_string();
    let subpath = repo_subpath.map(|s| s.to_string());
    let owned_args = git_args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    run_blox_blocking(move || {
        let borrowed_args = owned_args.iter().map(String::as_str).collect::<Vec<_>>();
        run_workspace_git_with_mode(&ws_name, subpath.as_deref(), mode, &borrowed_args)
    })
    .await
}

fn workspace_home_dir(_workspace_name: &str) -> Result<String, blox::BloxError> {
    // Blox workspaces always use /home/bloxer as the home directory.
    Ok("/home/bloxer".to_string())
}

pub(crate) fn resolve_workspace_repo_path(
    workspace_name: &str,
    repo_subpath: &str,
) -> Result<String, blox::BloxError> {
    if let Some(rest) = repo_subpath.strip_prefix("home:") {
        let home = workspace_home_dir(workspace_name)?;
        return Ok(format!("{home}/{rest}"));
    }
    Ok(repo_subpath.to_string())
}

/// Return just the repo-root clone directory as `home:<clone_dir>`.
///
/// This is the base path on the workspace filesystem where the repo is
/// cloned, and where its `.git` lives even for a monorepo subpath repo. Use
/// this when you need commands to run from the repo root (e.g. `git diff` in
/// `diff_commands`) or when checking for and creating the clone itself
/// (`clone_repo_into_workspace`).
pub(crate) fn resolve_branch_clone_dir(
    store: &Store,
    branch: &store::Branch,
) -> Result<Option<String>, String> {
    let Some(repo_id) = branch.project_repo_id.as_deref() else {
        return Ok(None);
    };
    let Some(repo) = store.get_project_repo(repo_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let clone_dir = repo_name_from_github_repo(&repo.github_repo);
    Ok(Some(format!("home:{clone_dir}")))
}

/// Resolve the workspace-relative path for a branch's repository.
///
/// The workspace clone directory is derived from `github_repo` (e.g.
/// `squareup/g2` → `~/g2`). If the repo has a subpath (a monorepo
/// subdirectory like `apps/staged`), it is appended to the clone directory.
///
/// Returns a `home:<dir>` string suitable for `resolve_workspace_repo_path`,
/// or `None` when the branch has no associated project repo.
pub(crate) fn resolve_branch_workspace_subpath(
    store: &Store,
    branch: &store::Branch,
) -> Result<Option<String>, String> {
    let Some(repo_id) = branch.project_repo_id.as_deref() else {
        return Ok(None);
    };
    let Some(repo) = store.get_project_repo(repo_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };

    let clone_dir = repo_name_from_github_repo(&repo.github_repo);
    let workspace_path = match &repo.subpath {
        Some(subpath) => {
            let validated = validate_workspace_subpath(subpath)?;
            format!("home:{clone_dir}/{validated}")
        }
        None => format!("home:{clone_dir}"),
    };
    Ok(Some(workspace_path))
}

// ── Per-branch git runners ──────────────────────────────────────────────────
//
// Any read *about* a branch has to happen *in* the branch's own checkout. On a
// Blox workspace that is not the same thing as a bare exec: one project gets
// one workspace (`resolve_project_workspace_name`) and every additional repo
// is cloned into it as a sibling (`clone_repo_into_workspace`), so at most one
// clone can be whatever directory a bare `sq blox ws exec … git rev-parse
// HEAD` lands in. These runners resolve the same directory the rebase
// (`run_remote_pipeline_command`), the remote agent, and the diff collector
// already run in.

/// A git command runner bound to one checkout: local commands run in
/// `working_dir`, remote ones in `remote_dir` on `workspace_name`.
///
/// Boxed rather than an `impl Fn`, since a named type keeps the return
/// signatures readable and every consumer is generic over `Fn` anyway.
pub(crate) type GitRunner<'a> = Box<dyn Fn(&[&str]) -> Result<String, String> + 'a>;

/// Build a runner for an already-resolved remote directory.
///
/// `remote_dir` takes either form the codebase produces — the `home:<clone>`
/// string from [`resolve_branch_workspace_subpath`] or the absolute path a
/// session config carries as `remote_working_dir` — because
/// [`resolve_workspace_repo_path`] maps both to the same `-C` argument. `None`
/// runs bare git, which is correct for a branch with no project repo (and is
/// what every remote read did before). It is owned rather than borrowed
/// because callers that resolve it from a branch row produce it on the fly.
pub(crate) fn git_runner<'a>(
    working_dir: &'a Path,
    workspace_name: Option<&'a str>,
    remote_dir: Option<String>,
) -> GitRunner<'a> {
    Box::new(move |args: &[&str]| match workspace_name {
        Some(ws_name) => {
            run_workspace_git(ws_name, remote_dir.as_deref(), args).map_err(|e| e.to_string())
        }
        None => git::cli_run_smart(working_dir, args).map_err(|e| e.to_string()),
    })
}

/// The same runner, resolving the remote directory from the branch row.
///
/// For callers that hold a `Branch` but no session config. The resolution is
/// the one such a config's `remote_working_dir` was built from, so the two
/// agree byte for byte.
pub(crate) fn branch_git_runner<'a>(
    store: &Store,
    branch: &store::Branch,
    working_dir: &'a Path,
    workspace_name: Option<&'a str>,
) -> Result<GitRunner<'a>, String> {
    let remote_dir = match workspace_name {
        Some(_) => resolve_branch_workspace_subpath(store, branch)?,
        None => None,
    };
    Ok(git_runner(working_dir, workspace_name, remote_dir))
}

/// Resolve the local cwd a branch session should run in.
///
/// The branch worktree itself is the git checkout root. If the branch belongs
/// to a project repo with a subpath, that repo's subpath wins; otherwise the
/// legacy project-level subpath applies.
pub(crate) fn local_branch_session_working_dir(
    store: &Store,
    project: &store::Project,
    branch: &store::Branch,
    worktree_path: impl Into<PathBuf>,
) -> PathBuf {
    let mut working_dir = worktree_path.into();
    let effective_subpath = if let Some(repo_id) = branch.project_repo_id.as_deref() {
        store
            .get_project_repo(repo_id)
            .ok()
            .flatten()
            .and_then(|repo| repo.subpath)
    } else {
        project.subpath.clone()
    };
    if let Some(ref subpath) = effective_subpath {
        working_dir = working_dir.join(subpath);
    }
    working_dir
}

/// Read HEAD through a runner, trimmed.
pub(crate) fn head_sha(git: &GitRunner<'_>) -> Result<String, String> {
    git(&["rev-parse", "HEAD"]).map(|sha| sha.trim().to_string())
}

/// A branch's HEAD, read off the checkout that branch lives in.
///
/// Goes to a blocking thread because on a remote branch it is a `ws exec`
/// round trip to a cloud workstation.
pub(crate) async fn branch_head_sha(
    store: &Arc<Store>,
    branch: &store::Branch,
    working_dir: &Path,
) -> Result<String, String> {
    let store = Arc::clone(store);
    let branch = branch.clone();
    let working_dir = working_dir.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let git = branch_git_runner(
            &store,
            &branch,
            &working_dir,
            branch.workspace_name.as_deref(),
        )?;
        head_sha(&git)
    })
    .await
    .map_err(|e| format!("HEAD lookup task failed: {e}"))?
}

pub(crate) fn normalize_branch_ref(branch: &str) -> String {
    branch.strip_prefix("origin/").unwrap_or(branch).to_string()
}

/// Prints `present` or `absent` for `$1/.git`. Answering through stdout keeps
/// a transport failure (a workstation whose agent hasn't reconnected yet, a
/// deadline) from reading as "no repo here" the way a non-zero exit would.
const GIT_DIR_PROBE_SCRIPT: &str =
    r#"if [ -d "$1/.git" ]; then echo present; else echo absent; fi"#;

/// Clones `$1` into `$2` unless `$2/.git` already exists, so a clone chosen
/// on an unclear probe answer can't fail on an existing checkout.
const GUARDED_CLONE_SCRIPT: &str = r#"test -d "$2/.git" || exec git clone "$1" "$2""#;

/// Whether [`GIT_DIR_PROBE_SCRIPT`]'s result calls for a clone. Only a clear
/// `present` skips it; errors that a tracked exec can't wait out either are
/// returned, and anything else falls back to [`GUARDED_CLONE_SCRIPT`].
fn needs_clone(probe: Result<String, blox::BloxError>) -> Result<bool, blox::BloxError> {
    match probe {
        Ok(out) if out.trim() == "present" => Ok(false),
        Err(e @ (blox::BloxError::NotFound | blox::BloxError::NotAuthenticated)) => Err(e),
        _ => Ok(true),
    }
}

/// Clone a repo into an already-running workspace, fetch the base branch,
/// and check out the feature branch.
///
/// `clone_dir` is the repo-root clone directory from
/// [`resolve_branch_clone_dir`] (`home:<clone>`), never the monorepo subpath
/// from [`resolve_branch_workspace_subpath`]: `.git` lives at the clone root,
/// so a `<clone>/<subpath>` path never finds an existing clone, and cloning
/// into it fails on a workstation that already has the repo (or nests a
/// second clone under the first on one that doesn't).
///
/// This is the checkout step of branch setup ([`crate::workspace_setup`]),
/// whatever led to it: a workstation Blox just bootstrapped (whose clone is
/// already there), a branch re-entering `Starting` on a workstation that has
/// its clone (e.g. after `resume_workspace`), or a repo added to a remote
/// project whose workspace is already running.
pub(crate) async fn clone_repo_into_workspace(
    ws_name: &str,
    clone_dir: &str,
    repo_slug: &str,
    checkout: &BranchCheckout<'_>,
) -> Result<Option<String>, CheckoutError> {
    let failed = CheckoutError::Failed;
    let repo_path = resolve_workspace_repo_path(ws_name, clone_dir).map_err(|e| {
        failed(format!(
            "Failed to resolve workspace repo path '{clone_dir}': {e}"
        ))
    })?;

    // Clone only if the repo directory doesn't already exist.
    let probe = ws_exec_async(
        ws_name,
        ExecMode::Transient,
        &["sh", "-c", GIT_DIR_PROBE_SCRIPT, "_", &repo_path],
    )
    .await;
    let needs_clone = needs_clone(probe).map_err(|e| {
        failed(format!(
            "Failed to verify repo path '{clone_dir}' in workspace '{ws_name}': {e}"
        ))
    })?;
    if needs_clone {
        let repo_url = format!("https://github.com/{repo_slug}.git");
        // Tracked: cloning a large repo outlasts the transient 60 s limit,
        // and a tracked exec waits out a workstation that isn't connected yet.
        ws_exec_async(
            ws_name,
            ExecMode::Tracked,
            &["sh", "-c", GUARDED_CLONE_SCRIPT, "_", &repo_url, &repo_path],
        )
        .await
        .map_err(|e| {
            failed(format!(
                "Failed to clone '{repo_slug}' into workspace '{ws_name}': {e}"
            ))
        })?;
    }

    // Apply staged-managed git config. Idempotent — also serves as the
    // lazy migration for pre-existing remote clones (no registry on blox
    // workstations, so we redo the cheap config check on every visit).
    crate::git::config_apply::apply_to_blox_clone(ws_name, &repo_path).await;

    check_out_branch_in_workspace(ws_name, Some(clone_dir), checkout)
        .await
        .map_err(|e| {
            e.map_message(|m| {
                format!("Failed to set up '{repo_slug}' in workspace '{ws_name}': {m}")
            })
        })
}

/// [`check_out_workspace_branch`] in a workstation's clone, on the blocking
/// pool. `remote_dir` is the clone to run in (`home:<clone>`), or `None` for
/// a branch with no project repo, whose git runs bare in the workstation's
/// default directory.
pub(crate) async fn check_out_branch_in_workspace(
    ws_name: &str,
    remote_dir: Option<&str>,
    checkout: &BranchCheckout<'_>,
) -> Result<Option<String>, CheckoutError> {
    let workspace = ws_name.to_string();
    let remote_dir = remote_dir.map(str::to_string);
    let base = checkout.base_ref.to_string();
    let branch = checkout.branch_name.to_string();
    let base_is_worked_on = checkout.base_is_worked_on;
    let siblings = checkout.siblings.to_vec();
    tauri::async_runtime::spawn_blocking(move || {
        // Tracked for everything but the reads (`rev-parse`, the
        // `for-each-ref` that asks whether the clone has the branch, and the
        // `log`, `rev-list` and `status` that ask what a switch would leave
        // behind):
        // Blox SIGKILLs a transient git at 60 s, which a fetch on a large
        // repo can outlast, and a checkout or fast-forward far from where
        // the tree is (or smudging LFS files) can outlast it too, leaving
        // `.git/index.lock` and a half-updated worktree.
        let git: GitRunner<'_> = Box::new(move |args: &[&str]| {
            let mode = if matches!(
                args.first(),
                Some(&"rev-parse")
                    | Some(&"for-each-ref")
                    | Some(&"log")
                    | Some(&"rev-list")
                    | Some(&"--no-optional-locks")
            ) {
                ExecMode::Transient
            } else {
                ExecMode::Tracked
            };
            run_workspace_git_with_mode(&workspace, remote_dir.as_deref(), mode, args)
                .map_err(|e| e.to_string())
        });
        check_out_workspace_branch(
            &git,
            &BranchCheckout {
                base_ref: &base,
                branch_name: &branch,
                base_is_worked_on,
                siblings: &siblings,
            },
        )
    })
    .await
    .map_err(|e| CheckoutError::Failed(format!("Branch checkout task failed: {e}")))?
}

pub(crate) use crate::workspace_setup::checkout::{
    check_out_workspace_branch, BranchCheckout, CheckoutError,
};

/// Kick the branch's session queue when a workspace-status write moved it to
/// `Running`.
///
/// Every site that writes a workspace status calls this with what
/// [`Store::update_branch_workspace_status`] reported, so a queued session
/// waiting on provisioning starts whichever poller or setup path saw the
/// workspace come up — the frontend's own drain used to be the only one.
pub(crate) fn workspace_status_written<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    branch_id: &str,
    changed: bool,
    status: &store::WorkspaceStatus,
) {
    if changed && *status == store::WorkspaceStatus::Running {
        // The replies held for a transfer that landed the branch while its
        // workspace was still starting go first: they wait on this too.
        crate::session_queue::flush_follow_ups(app_handle, branch_id);
        crate::session_queue::kick(
            app_handle,
            branch_id,
            crate::session_queue::KickSource::WorkspaceRunning,
        );
    }
}

/// Clone a repo into an already-running remote workspace for a given branch,
/// check its branch out and mark it `Running`.
///
/// The shared branch setup ([`crate::workspace_setup`]) under its guard, so
/// the frontend's `start_workspace` for the same `Starting` branch joins this
/// run instead of cloning beside it.
pub(crate) async fn setup_remote_repo_clone<R: tauri::Runtime>(
    store: &Arc<Store>,
    app_handle: &tauri::AppHandle<R>,
    branch_id: &str,
) -> Result<(), String> {
    crate::workspace_setup::ensure_branch_set_up(
        app_handle,
        store,
        branch_id,
        crate::workspace_setup::SetupTrigger::Requested,
    )
    .await
}

/// Result of searching for an existing worktree for a branch name.
enum ExistingWorktree {
    /// A worktree was found under the current project's directory.
    Found(PathBuf),
    /// No worktree exists for this branch name.
    None,
    /// A worktree exists but under a different project's directory (stale).
    /// The local branch name is taken so the caller must use a different one.
    Stale,
}

/// Find an existing git worktree checked out on `branch_name`.
///
/// When `project_root` is provided, only worktrees whose path lives under that
/// directory are considered a match.  Worktrees that match by branch name but
/// live under a *different* project return [`ExistingWorktree::Stale`] so the
/// caller can use a different local branch name.
fn find_existing_worktree_for_branch(
    repo_path: &Path,
    branch_name: &str,
    project_root: Option<&Path>,
) -> Result<ExistingWorktree, String> {
    let worktrees = git::list_worktrees(repo_path).map_err(|e| e.to_string())?;

    for (path, wt_branch) in worktrees {
        if wt_branch.as_deref() != Some(branch_name) {
            continue;
        }

        // If no project_root filter, accept the first match (legacy behaviour).
        let Some(root) = project_root else {
            return Ok(ExistingWorktree::Found(path));
        };

        if path.starts_with(root) {
            return Ok(ExistingWorktree::Found(path));
        }

        // Worktree belongs to another (likely deleted) project — the local
        // branch name is taken so the caller should pick a different one.
        log::info!(
            "[find_existing_worktree_for_branch] Stale worktree '{}' for branch '{}' (outside project root '{}')",
            path.display(),
            branch_name,
            root.display(),
        );
        return Ok(ExistingWorktree::Stale);
    }

    Ok(ExistingWorktree::None)
}

fn is_worktree_path_exists_error(err: &str) -> bool {
    err.contains("Worktree already exists at ")
}

fn fallback_worktree_path_for(desired_path: &Path) -> Option<PathBuf> {
    if !desired_path.exists() {
        return Some(desired_path.to_path_buf());
    }

    let parent = desired_path.parent()?;
    let file_name = desired_path.file_name()?.to_string_lossy();

    for suffix in 2..=50 {
        let candidate = parent.join(format!("{file_name}-{suffix}"));
        if !candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

/// Generate a unique local branch name by appending `-2`, `-3`, etc.
/// Whether a PR branch's head lives in a fork rather than on origin: the
/// `head_repo` the create-project flow records on the project repo when the
/// PR's head repository is not the one being cloned.
pub(crate) fn pr_head_is_in_fork(
    store: &Arc<Store>,
    branch: &store::Branch,
) -> Result<bool, String> {
    let Some(repo_id) = &branch.project_repo_id else {
        return Ok(false);
    };
    let Some(repo) = store.get_project_repo(repo_id).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    Ok(repo
        .head_repo
        .as_deref()
        .is_some_and(|head| !head.eq_ignore_ascii_case(&repo.github_repo)))
}

/// Undo a worktree whose setup could not be completed, so the next attempt
/// starts from nothing rather than from a half-made checkout. Best effort:
/// the caller is already returning the error that got it here.
///
/// Like [`git::remove_worktree`]: the worktree removal runs with the lite
/// env, with the captured-env fallback for an unusable git, since it only
/// touches the repo's admin files. The branch delete goes through
/// [`git::force_delete_branch`], which retries with the captured env when
/// the lite one fails in a way it might fix, such as a
/// `reference-transaction` hook that needs the login shell's `PATH`.
///
/// Both steps run under the clone's worktree lock, so the caller must not
/// hold it. Removing the worktree frees the branch name a moment before the
/// branch itself is deleted. A setup of another project that looked in
/// between found a branch no worktree held, took it for one the user already
/// had, and checked out the branch this was about to delete. With the lock
/// it sees the worktree still holding the name, or neither.
fn discard_incomplete_worktree(
    repo_path: &Path,
    worktree_path: &Path,
    created_branch: Option<&str>,
) {
    let Some(worktree_str) = worktree_path.to_str() else {
        return;
    };
    let lock = clone_worktree_lock_for(repo_path);
    let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
    if let Err(e) =
        git::cli_run_lite_or_captured(repo_path, &["worktree", "remove", "--force", worktree_str])
    {
        log::warn!(
            "Could not remove incomplete worktree '{}': {e}",
            worktree_path.display()
        );
    }
    if let Some(branch) = created_branch {
        if let Err(e) = git::force_delete_branch(repo_path, branch) {
            log::warn!("Could not delete incomplete branch '{branch}': {e}");
        }
    }
}

fn unique_branch_name(repo_path: &Path, base_name: &str) -> Result<String, String> {
    for suffix in 2..=50 {
        let candidate = format!("{base_name}-{suffix}");
        if !git::branch_exists(repo_path, &candidate).map_err(|e| e.to_string())? {
            return Ok(candidate);
        }
    }
    Err(format!(
        "Could not find a unique branch name for '{base_name}' (tried {base_name}-2 through \
         {base_name}-50). Delete unused worktrees or branches with `git branch -d` to free up names."
    ))
}

fn create_worktree_for_existing_branch_with_fallback(
    repo_path: &Path,
    branch_name: &str,
    desired_worktree_path: &Path,
    project_root: Option<&Path>,
) -> Result<PathBuf, String> {
    match git::create_worktree_for_existing_branch_at_path(
        repo_path,
        branch_name,
        desired_worktree_path,
    ) {
        Ok(path) => Ok(path),
        Err(err) => {
            if let ExistingWorktree::Found(path) =
                find_existing_worktree_for_branch(repo_path, branch_name, project_root)?
            {
                log::warn!(
                    "Reusing existing worktree '{}' for branch '{}' after create retry",
                    path.display(),
                    branch_name
                );
                return Ok(path);
            }

            let err_msg = err.to_string();
            if !is_worktree_path_exists_error(&err_msg) {
                return Err(err_msg);
            }

            let Some(fallback_path) = fallback_worktree_path_for(desired_worktree_path) else {
                return Err(err_msg);
            };
            if fallback_path == desired_worktree_path {
                return Err(err_msg);
            }

            log::warn!(
                "Desired worktree path '{}' is occupied for branch '{}'; retrying at '{}'",
                desired_worktree_path.display(),
                branch_name,
                fallback_path.display()
            );
            git::create_worktree_for_existing_branch_at_path(repo_path, branch_name, &fallback_path)
                .map_err(|e| e.to_string())
        }
    }
}

fn create_worktree_with_fallback(
    repo_path: &Path,
    branch_name: &str,
    base_branch: &str,
    desired_worktree_path: &Path,
    project_root: Option<&Path>,
) -> Result<PathBuf, String> {
    match git::create_worktree_at_path(repo_path, branch_name, base_branch, desired_worktree_path) {
        Ok(path) => Ok(path),
        Err(err) => {
            if let ExistingWorktree::Found(path) =
                find_existing_worktree_for_branch(repo_path, branch_name, project_root)?
            {
                log::warn!(
                    "Reusing existing worktree '{}' for branch '{}' after create failure",
                    path.display(),
                    branch_name
                );
                return Ok(path);
            }

            let err_msg = err.to_string();
            if !is_worktree_path_exists_error(&err_msg) {
                return Err(err_msg);
            }

            let Some(fallback_path) = fallback_worktree_path_for(desired_worktree_path) else {
                return Err(err_msg);
            };
            if fallback_path == desired_worktree_path {
                return Err(err_msg);
            }

            log::warn!(
                "Desired worktree path '{}' is occupied for new branch '{}'; retrying at '{}'",
                desired_worktree_path.display(),
                branch_name,
                fallback_path.display()
            );
            git::create_worktree_at_path(repo_path, branch_name, base_branch, &fallback_path)
                .map_err(|e| e.to_string())
        }
    }
}

/// What [`create_worktree_on_new_branch`] ended up with.
struct NewBranchWorktree {
    path: PathBuf,
    /// The worktree is one another process made for this project while the
    /// call was running: theirs to keep.
    adopted_existing: bool,
    /// The local branch was created by this call (or is the one its failed
    /// `worktree add -b` left behind).
    created_branch: bool,
}

/// Create a worktree on a new local branch `local_branch_name`, started at
/// `start_point`. The caller found the name free, and holds the clone's
/// worktree lock from that lookup to here ([`try_check_out_worktree`]), so
/// no other setup in this process can have taken the name in between.
fn create_worktree_on_new_branch(
    repo_path: &Path,
    local_branch_name: &str,
    start_point: &str,
    desired_worktree_path: &Path,
    project_root: Option<&Path>,
) -> Result<NewBranchWorktree, String> {
    let create_err = match create_worktree_with_fallback(
        repo_path,
        local_branch_name,
        start_point,
        desired_worktree_path,
        project_root,
    ) {
        Ok(path) => {
            return Ok(NewBranchWorktree {
                path,
                adopted_existing: false,
                created_branch: true,
            })
        }
        Err(create_err) => create_err,
    };

    match find_existing_worktree_for_branch(repo_path, local_branch_name, project_root)? {
        ExistingWorktree::Found(path) => {
            log::warn!(
                "Reusing existing worktree '{}' for branch '{}' after create failure",
                path.display(),
                local_branch_name
            );
            // Another process created it between the lookup and the
            // attempt: theirs to keep.
            Ok(NewBranchWorktree {
                path,
                adopted_existing: true,
                created_branch: false,
            })
        }
        // Something outside this process checked the name out for another
        // project after the lookup. Retrying with that branch can only
        // fail: git checks a branch out in one worktree.
        ExistingWorktree::Stale => Err(create_err),
        ExistingWorktree::None => {
            if !git::branch_exists(repo_path, local_branch_name).map_err(|e| e.to_string())? {
                return Err(create_err);
            }
            log::warn!(
                "Branch '{}' already exists after create attempt; retrying with existing branch",
                local_branch_name
            );
            // The name did not exist when the caller looked (its
            // `branch_exists` check was false, or `unique_branch_name` had
            // just found it free), and the clone's worktree lock has been
            // held since. So the branch is the one `worktree add -b` left
            // behind when it failed after creating it, and not one another
            // setup is part-way through checking out. It is Staged's to
            // point at origin/<requested> and to delete on a later failure,
            // the same as when the first attempt succeeds.
            let path = create_worktree_for_existing_branch_with_fallback(
                repo_path,
                local_branch_name,
                desired_worktree_path,
                project_root,
            )
            .map_err(|e| e.to_string())?;
            Ok(NewBranchWorktree {
                path,
                adopted_existing: false,
                created_branch: true,
            })
        }
    }
}

/// The checkout [`check_out_worktree`] made or found for a branch.
struct CheckedOutWorktree {
    path: PathBuf,
    /// The local branch the worktree is on: the requested name, or a unique
    /// one when another project's worktree holds that name.
    local_branch_name: String,
    /// Whether the worktree belongs to someone else: one the user already
    /// had, or one another process made while this call was running. Its
    /// upstream is left alone and it is never removed on failure.
    adopted_existing: bool,
    /// Whether the local branch itself was created here, so a failure
    /// further down can delete it along with the worktree instead of
    /// leaving the name taken for the next attempt, and so its upstream may
    /// be cleared.
    created_branch: bool,
}

/// Find or create the worktree for `requested_branch` in the clone at
/// `repo_path`: reuse one this project already has, check out the local
/// branch when it exists and is free, and otherwise create the branch, under
/// a unique local name when another project's worktree holds the requested
/// one.
///
/// The name lookup and the `git worktree add` it leads to run under the
/// clone's worktree lock. They are two steps, and two projects setting up
/// the same branch of one clone used to interleave them: both found the name
/// free, one `git worktree add -b` won, and the loser could not tell the
/// winner's branch from a leftover of its own failed attempt. `git worktree
/// add -b` creates the branch a moment before a worktree holds it, so no
/// check made from outside that window can tell either. With the lock the
/// second setup looks only after the first has finished, finds the name
/// held, and takes a unique one, the way it does for a worktree that was
/// there all along.
///
/// A pull request's new branch starts at the pull request's head, which has
/// to be fetched from the remote first. That fetch is not made under the
/// lock: it can take as long as the network likes, and every other setup in
/// the clone would wait behind it. The lookup finds out that the head is
/// needed, the lock is let go for the fetch, and the lookup is made again
/// from the start once the lock is retaken. Whatever another setup did to
/// the name during the fetch is seen by that second lookup, so the lookup
/// that decides and the creation are still one step.
///
/// Two setups of one pull request would then fetch its head at the same
/// time, and git fails one of them when the local ref is already there at an
/// older commit. The fetch has a lock of its own for that, keyed by clone
/// and pull request ([`PR_HEAD_FETCH_LOCKS`]).
///
/// A setup that needs no fetch runs the same steps in the same order as
/// before the lock. One that does makes its lookups twice, and fetches only
/// when it has a branch to create, as before.
fn check_out_worktree(
    repo_path: &Path,
    requested_branch: &str,
    pr_number: Option<u64>,
    base_branch: &str,
    desired_worktree_path: &Path,
    project_root: Option<&Path>,
) -> Result<CheckedOutWorktree, String> {
    check_out_worktree_with(
        repo_path,
        requested_branch,
        pr_number,
        base_branch,
        desired_worktree_path,
        project_root,
        &|repo_path, pr_number| {
            git::fetch_pr_head_sha(repo_path, pr_number).map_err(|e| e.to_string())
        },
    )
}

/// [`check_out_worktree`] with the fetch of a pull request's head passed in,
/// so a test can see when it runs and act while it does.
fn check_out_worktree_with(
    repo_path: &Path,
    requested_branch: &str,
    pr_number: Option<u64>,
    base_branch: &str,
    desired_worktree_path: &Path,
    project_root: Option<&Path>,
    fetch_pr_head_sha: &dyn Fn(&Path, u64) -> Result<String, String>,
) -> Result<CheckedOutWorktree, String> {
    let lock = clone_worktree_lock_for(repo_path);
    let mut pr_head_sha: Option<String> = None;
    // Twice round at most: an attempt that is given the head never asks for
    // it again.
    loop {
        let attempt = {
            let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
            try_check_out_worktree(
                repo_path,
                requested_branch,
                pr_number,
                pr_head_sha.as_deref(),
                base_branch,
                desired_worktree_path,
                project_root,
            )?
        };
        match attempt {
            CheckOutAttempt::Done(checked_out) => return Ok(checked_out),
            // The clone's lock is free again here. Fetches of this pull
            // request's head take turns, and the turn ends with the fetch,
            // before the clone's lock is retaken.
            CheckOutAttempt::NeedsPrHead(pr_number) => {
                let fetch_lock = pr_head_fetch_lock_for(repo_path, pr_number);
                let _fetching = fetch_lock.lock().unwrap_or_else(|p| p.into_inner());
                pr_head_sha = Some(fetch_pr_head_sha(repo_path, pr_number)?);
            }
        }
    }
}

/// How one pass of [`try_check_out_worktree`] ended.
enum CheckOutAttempt {
    Done(CheckedOutWorktree),
    /// A branch has to be created for this pull request, and its head has
    /// not been fetched. Nothing was created. The caller fetches the head
    /// with the lock released and tries again.
    NeedsPrHead(u64),
}

/// One pass of [`check_out_worktree`], made with the clone's worktree lock
/// held: look the name up, and reuse or create accordingly. `pr_head_sha` is
/// the pull request's head when the caller has fetched it.
fn try_check_out_worktree(
    repo_path: &Path,
    requested_branch: &str,
    pr_number: Option<u64>,
    pr_head_sha: Option<&str>,
    base_branch: &str,
    desired_worktree_path: &Path,
    project_root: Option<&Path>,
) -> Result<CheckOutAttempt, String> {
    // When a stale worktree from another project holds the branch name, pick
    // a unique local branch name so we don't collide.
    let mut local_branch_name = requested_branch.to_string();
    let existing_worktree =
        find_existing_worktree_for_branch(repo_path, requested_branch, project_root)?;
    if let ExistingWorktree::Stale = &existing_worktree {
        local_branch_name = unique_branch_name(repo_path, requested_branch)?;
        log::info!(
            "Branch '{}' is checked out in a stale worktree; using '{}' instead",
            requested_branch,
            local_branch_name,
        );
    }

    if let ExistingWorktree::Found(path) = existing_worktree {
        return Ok(CheckOutAttempt::Done(CheckedOutWorktree {
            path,
            local_branch_name,
            adopted_existing: true,
            created_branch: false,
        }));
    }
    if local_branch_name == requested_branch
        && git::branch_exists(repo_path, &local_branch_name).map_err(|e| e.to_string())?
    {
        let path = create_worktree_for_existing_branch_with_fallback(
            repo_path,
            &local_branch_name,
            desired_worktree_path,
            project_root,
        )
        .map_err(|e| e.to_string())?;
        return Ok(CheckOutAttempt::Done(CheckedOutWorktree {
            path,
            local_branch_name,
            adopted_existing: false,
            created_branch: false,
        }));
    }

    // For PRs, always start from `refs/pull/<N>/head` — the branch
    // name on origin may belong to a different branch when the PR
    // comes from a fork repo.  For non-PR branches, fall back to the
    // remote tracking ref or base_branch.
    let remote_ref = format!("origin/{requested_branch}");
    let start_point = match (pr_number, pr_head_sha) {
        (Some(_), Some(pr_head_sha)) => pr_head_sha,
        // Fetched by the caller, once this pass has let the lock go.
        (Some(pr_number), None) => return Ok(CheckOutAttempt::NeedsPrHead(pr_number)),
        (None, _) => {
            if git::remote_branch_exists(repo_path, requested_branch).map_err(|e| e.to_string())? {
                &remote_ref
            } else {
                base_branch
            }
        }
    };
    let created = create_worktree_on_new_branch(
        repo_path,
        &local_branch_name,
        start_point,
        desired_worktree_path,
        project_root,
    )?;
    Ok(CheckOutAttempt::Done(CheckedOutWorktree {
        path: created.path,
        local_branch_name,
        adopted_existing: created.adopted_existing,
        created_branch: created.created_branch,
    }))
}

fn local_base_ref_for_worktree(repo_path: &Path, base_branch: &str) -> Option<String> {
    let base_ref = git::origin_ref_for_branch(base_branch);
    git::resolve_ref(repo_path, &base_ref).ok()?;
    Some(base_ref)
}

fn is_missing_remote_ref_fetch_error(fetch_err: &str) -> bool {
    let lower = fetch_err.to_ascii_lowercase();
    lower.contains("couldn't find remote ref") || lower.contains("could not find remote ref")
}

pub(crate) fn fetch_for_worktree_with_offline_fallback(
    repo_path: &Path,
    repo_slug: &str,
    branch_name: &str,
    base_branch: &str,
) -> Result<(), String> {
    match git::fetch_for_worktree(repo_path, repo_slug, branch_name, base_branch) {
        Ok(()) => Ok(()),
        Err(fetch_err) => {
            let fetch_err = fetch_err.to_string();
            if is_missing_remote_ref_fetch_error(&fetch_err) {
                return Err(fetch_err);
            }

            if let Some(base_ref) = local_base_ref_for_worktree(repo_path, base_branch) {
                log::warn!(
                    "fetch for worktree branch '{}' in '{}' failed; using stale local ref '{}': {}",
                    branch_name,
                    repo_slug,
                    base_ref,
                    fetch_err
                );
                Ok(())
            } else {
                let base_ref = git::origin_ref_for_branch(base_branch);
                Err(format!(
                    "GitHub is unavailable and the local clone for '{repo_slug}' does not have required base ref '{base_ref}': {fetch_err}"
                ))
            }
        }
    }
}

pub(crate) fn is_blox_onboarding_precondition_error(err: &blox::BloxError) -> bool {
    match err {
        blox::BloxError::CommandFailed(stderr) => {
            let lower = stderr.to_ascii_lowercase();
            lower.contains("failed_precondition")
                && (lower.contains("onboard") || lower.contains("has not completed onboarding"))
        }
        _ => false,
    }
}

/// Whether `branch`'s workstation stays when the branches in `leaving` go:
/// another remote branch of the project uses it, so
/// [`cleanup_branch_resources`] leaves it standing. False for a branch with
/// no workstation.
pub(crate) fn workspace_outlives(
    store: &Store,
    branch: &store::Branch,
    leaving: &[&str],
) -> Result<bool, String> {
    let Some(ws_name) = branch.workspace_name.as_deref() else {
        return Ok(false);
    };
    Ok(store
        .list_branches_for_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .any(|b| {
            !leaving.contains(&b.id.as_str())
                && b.branch_type == store::BranchType::Remote
                && b.workspace_name.as_deref() == Some(ws_name)
        }))
}

pub(crate) fn cleanup_branch_resources(
    store: &Arc<Store>,
    branch: &store::Branch,
) -> Result<(), String> {
    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    // Clean up any cached diffs for this branch.
    if let Err(e) =
        crate::diff_cache::delete_branch_cache(project.location, &project.id, &branch.id)
    {
        log::warn!(
            "Failed to clean up diff cache for branch {}: {e}",
            branch.id
        );
    }

    match branch.branch_type {
        store::BranchType::Local => {
            let workdir = store
                .get_workdir_for_branch(&branch.id)
                .map_err(|e| e.to_string())?;

            if let Some(ref wd) = workdir {
                let repo_slug = resolve_branch_repo_slug(store, &project, branch)?;
                let repo_path = crate::paths::repos_dir()
                    .map(|d| d.join(repo_slug))
                    .ok_or("Cannot determine clone path")?;
                let worktree_path = Path::new(&wd.path);
                git::remove_worktree(&repo_path, worktree_path).map_err(|e| e.to_string())?;
                // Entries a transfer to local added to the clone's
                // `info/exclude` for this worktree would hide a later
                // worktree's files. Best-effort, and only logged.
                crate::project_transfer::release_worktree_excludes(worktree_path);
                store.delete_workdir(&wd.id).map_err(|e| e.to_string())?;
            }
        }
        store::BranchType::Remote => {
            if let Some(ref ws_name) = branch.workspace_name {
                if workspace_outlives(store, branch, &[branch.id.as_str()])? {
                    return Ok(());
                }
                crate::project_transfer::ensure_workspace_not_transferring(ws_name)?;
                match blox::ws_delete(ws_name) {
                    Ok(_) => {}
                    Err(blox::BloxError::CommandFailed(msg))
                        if msg.to_lowercase().contains("not found") => {}
                    Err(e) => {
                        return Err(format!("Failed to delete workspace {ws_name}: {e}"));
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn cleanup_branch_resources_best_effort(store: &Arc<Store>, branch: &store::Branch) {
    if let Err(e) = cleanup_branch_resources(store, branch) {
        log::warn!(
            "project delete cleanup warning for branch '{}': {e}",
            branch.branch_name
        );
        if branch.branch_type == store::BranchType::Local {
            if let Ok(Some(wd)) = store.get_workdir_for_branch(&branch.id) {
                let path = Path::new(&wd.path);
                if path.exists() {
                    if let Err(io_err) = std::fs::remove_dir_all(path) {
                        log::warn!(
                            "failed fallback removal for worktree '{}': {io_err}",
                            wd.path
                        );
                    }
                }
            }
        }
    }
}

/// Reduce `name` to a valid single branch-name component: lowercased,
/// separators collapsed to `-`, everything else dropped. May return an empty
/// string.
fn normalize_branch_component(name: &str) -> String {
    name.to_lowercase()
        .replace([' ', '_'], "-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '/' || *c == '.')
        .collect::<String>()
        .replace(['.', '/'], "-")
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

pub(crate) fn infer_branch_name(project_name: &str) -> String {
    let branch = normalize_branch_component(project_name);
    if branch.is_empty() {
        "feature".to_string()
    } else {
        branch
    }
}

/// Normalize a configured branch prefix with the same rules
/// [`infer_branch_name`] applies to project names, per `/`-separated segment
/// so multi-level prefixes like `team/alice` keep their hierarchy. Empty
/// segments are dropped, so leading, trailing, and doubled slashes cannot
/// produce an invalid ref. Returns `None` when nothing valid remains.
fn normalize_branch_prefix(prefix: &str) -> Option<String> {
    let normalized = prefix
        .split('/')
        .map(normalize_branch_component)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// Read the user's configured branch prefix (General settings → Branch
/// prefix) from the preferences store, normalized via
/// [`normalize_branch_prefix`]. Returns `None` when unset or when nothing
/// valid remains after normalization.
fn read_branch_prefix() -> Option<String> {
    let path = crate::preferences_store_path_buf()?;
    let contents = std::fs::read_to_string(&path).ok()?;
    let json = serde_json::from_str::<serde_json::Value>(&contents).ok()?;
    normalize_branch_prefix(json.get("branch-prefix")?.as_str()?)
}

/// Join the normalized branch prefix onto `branch` with a `/` separator.
fn apply_branch_prefix(prefix: Option<&str>, branch: &str) -> String {
    match prefix {
        Some(p) => format!("{p}/{branch}"),
        None => branch.to_string(),
    }
}

/// Infer a branch name from the project name, applying the user's configured
/// branch prefix. Used whenever a repo is added without an explicit branch
/// name. The [`resolve_project_workspace_name`] fallback stays on the
/// unprefixed [`infer_branch_name`] so existing workspace identities are
/// unaffected by the setting.
pub(crate) fn infer_prefixed_branch_name(project_name: &str) -> String {
    apply_branch_prefix(
        read_branch_prefix().as_deref(),
        &infer_branch_name(project_name),
    )
}

/// The longest workspace name Staged picks, in bytes.
pub(crate) const WORKSPACE_NAME_MAX_LENGTH: usize = 32;

/// Hex digits in the suffix that keeps workspace names apart across projects
/// and across branches that only differ past the cut.
pub(crate) const WORKSPACE_NAME_HASH_LENGTH: usize = 6;

/// A workspace name for `branch_name` in project `project_id`: the branch
/// name reduced the way [`infer_branch_name`] reduces a project name —
/// lowercase ASCII letters, digits and single `-`s, everything else dropped —
/// then `-` plus [`WORKSPACE_NAME_HASH_LENGTH`] hex digits of a SHA-256 over
/// the project id and the full branch name. Blox itself only refuses spaces
/// and names over 63 bytes, but a git branch can be called anything, and the
/// name also ends up in hostnames, paths and shell commands.
///
/// The readable part is cut so the whole fits [`WORKSPACE_NAME_MAX_LENGTH`].
/// The suffix is always there, so two projects on the same branch name get
/// their own workstations, as do branches that only differ past the cut
/// (`…-pr-1`, `…-pr-2`, `…-pr-3`). It is a pure function of its inputs, so
/// the same project always resolves to the same name; callers still prefer a
/// name already stored on a branch over re-deriving it. Names picked before
/// this scheme start with `stg-`; nothing matches on that prefix, every
/// lookup goes by the stored name.
///
/// This is [`infer_workspace_name_nth`] at ordinal 0.
pub(crate) fn infer_workspace_name(project_id: &str, branch_name: &str) -> String {
    infer_workspace_name_nth(project_id, branch_name, 0)
}

/// The `ordinal`th candidate workspace name for `branch_name` in project
/// `project_id`, for a caller that found the earlier ones taken. Ordinal 0
/// is exactly [`infer_workspace_name`]; every later ordinal folds itself into
/// the hash input, so each candidate has the same `{readable}-{hash}` shape
/// and the same readable part, and only the hash differs. The readable part
/// is never cut shorter to make room, as appending `-2` to a name that is
/// already [`WORKSPACE_NAME_MAX_LENGTH`] bytes would have to.
pub(crate) fn infer_workspace_name_nth(
    project_id: &str,
    branch_name: &str,
    ordinal: u32,
) -> String {
    let safe = normalize_branch_component(branch_name);
    let safe = if safe.is_empty() { "feature" } else { &safe };
    let hash = workspace_name_hash(project_id, branch_name, ordinal);
    let kept = truncate_workspace_name(safe, WORKSPACE_NAME_MAX_LENGTH - hash.len() - 1);
    format!("{kept}-{hash}")
}

/// [`WORKSPACE_NAME_HASH_LENGTH`] lowercase hex digits of SHA-256 over
/// `project_id`, a newline and `branch_name`, followed for a non-zero
/// `ordinal` by another newline and the ordinal in decimal. Ordinal 0 adds
/// nothing, so names derived before ordinals existed are unchanged.
fn workspace_name_hash(project_id: &str, branch_name: &str, ordinal: u32) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(project_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(branch_name.as_bytes());
    if ordinal != 0 {
        hasher.update(b"\n");
        hasher.update(ordinal.to_string().as_bytes());
    }
    let digest = hasher.finalize();
    // Two hex digits per byte: only the leading bytes are needed.
    digest
        .iter()
        .take(WORKSPACE_NAME_HASH_LENGTH.div_ceil(2))
        .fold(String::new(), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        })[..WORKSPACE_NAME_HASH_LENGTH]
        .to_string()
}

/// `name` cut to at most `max` bytes, never inside a character, without a
/// trailing `-`.
fn truncate_workspace_name(name: &str, max: usize) -> String {
    let mut end = name.len().min(max);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].trim_end_matches('-').to_string()
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<Option<BranchWithWorkdir>, String> {
    let store = crate::get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        let branch = store.get_branch(&branch_id).map_err(|e| e.to_string())?;

        match branch {
            Some(branch) => {
                let workdir = store
                    .get_workdir_for_branch(&branch.id)
                    .map_err(|e| e.to_string())?;
                Ok(Some(to_branch_with_workdir(
                    branch,
                    workdir.map(|w| w.path),
                )))
            }
            None => Ok(None),
        }
    })
    .await
    .map_err(|e| format!("get_branch task failed: {e}"))?
}

/// Runs [`list_branches_for_project_impl`] on the blocking pool. The
/// frontend asks for it once per project at launch and again on every branch
/// change and window focus, so as a synchronous command it parked the main
/// thread, and so the window, behind whatever held the store.
///
/// As for every command in `OFFLOADED_COMMANDS`, only the store handle
/// lookup may run inline: an `async fn` command runs on a runtime worker,
/// where a store lock wait would stall timers and event sockets instead.
#[tauri::command]
pub async fn list_branches_for_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
) -> Result<Vec<BranchWithWorkdir>, String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        list_branches_for_project_impl(&store, &project_id)
    })
    .await
    .map_err(|e| format!("list_branches_for_project task failed: {e}"))?
}

/// Every project's branch list in one reply, keyed by project id: what
/// [`list_branches_for_project`] returns for each project, for the views that
/// show all of them at once. Asking per project cost the frontend one reply,
/// and one pass over every card, per project. Offloaded for the same reason
/// as `list_branches_for_project`.
#[tauri::command]
pub async fn list_branches_by_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<BTreeMap<String, Vec<BranchWithWorkdir>>, String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::project_commands::list_branches_by_project_impl(&store)
    })
    .await
    .map_err(|e| format!("list_branches_by_project task failed: {e}"))?
}

/// The body of [`list_branches_for_project`]: store reads only, each taking
/// and releasing the store lock on its own.
pub(crate) fn list_branches_for_project_impl(
    store: &Store,
    project_id: &str,
) -> Result<Vec<BranchWithWorkdir>, String> {
    let _project = store
        .get_project(project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    project_branches_with_workdirs(store, project_id)
}

/// A project's branches as the frontend lists them: each with its worktree
/// path and its count of finalized commits. An unknown project has none.
/// Shared by the one-project and all-projects lists, on both transports, so
/// a branch reads the same whichever of them returned it.
pub(crate) fn project_branches_with_workdirs(
    store: &Store,
    project_id: &str,
) -> Result<Vec<BranchWithWorkdir>, String> {
    let branches = store
        .list_branches_for_project(project_id)
        .map_err(|e| e.to_string())?;

    // NOTE: workstation IDs for remote branches are populated lazily by
    // poll_all_workspace_statuses (a single batched `ws_list` call) rather
    // than eagerly here. Eager per-workspace `ws_info` calls were serial and
    // each took ~1s, causing multi-second UI freezes on project load.

    let commit_counts = store
        .count_finalized_commits_by_branch_for_project(project_id)
        .map_err(|e| e.to_string())?;

    let mut result = Vec::with_capacity(branches.len());
    for branch in branches {
        let workdir = store
            .get_workdir_for_branch(&branch.id)
            .map_err(|e| e.to_string())?;

        let count = commit_counts.get(&branch.id).copied().unwrap_or(0);
        let mut bw = to_branch_with_workdir(branch, workdir.map(|w| w.path));
        bw.commit_count = Some(count);
        result.push(bw);
    }
    Ok(result)
}

/// Create a local branch record (DB only — no git worktree yet).
///
/// Returns immediately with `worktree_path = None`. Call `setup_worktree`
/// separately to create the git worktree in the background.
#[tauri::command(rename_all = "camelCase")]
pub async fn create_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    branch_name: String,
    base_branch: Option<String>,
    project_repo_id: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::project_transfer::ensure_project_not_transferring(&project_id)?;

        // Detect default branch if none specified (via GitHub API, no local clone needed)
        let target_repo = match project_repo_id {
            Some(repo_id) => store
                .get_project_repo(&repo_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project repo not found: {repo_id}"))?,
            None => store
                .get_primary_project_repo(&project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project '{project_id}' has no repository attached"))?,
        };
        let effective_base = match base_branch {
            Some(b) => b,
            None => git::detect_default_branch_for_repo(&target_repo.github_repo)
                .map_err(|e| e.to_string())?,
        };

        // Normalise to "origin/<branch>" so diffs and worktree creation always
        // use the remote-tracking ref rather than a stale local branch.
        let effective_base = if effective_base.starts_with("origin/") {
            effective_base
        } else {
            format!("origin/{effective_base}")
        };

        // Create branch record only — no git worktree yet
        let branch = store::Branch::new(&project_id, &branch_name, &effective_base)
            .with_project_repo(&target_repo.id);
        store.create_branch(&branch).map_err(|e| e.to_string())?;

        Ok(to_branch_with_workdir(branch, None))
    })
    .await
    .map_err(|e| format!("create_branch task failed: {e}"))?
}

/// Core worktree creation + DB linkage logic behind [`setup_worktree_sync`].
///
/// Acquires the per-branch worktree lock, creates (or reuses) a git worktree,
/// sets upstream tracking for PR branches, persists any branch-name changes,
/// and links the workdir in the database.
///
/// Returns the worktree path as a string.
///
/// Kicks the branch's session queue once the worktree is linked: a session
/// queued on an unprovisioned branch waits for exactly this, and every setup
/// entry point (both transports, the project-setup pass, MCP) funnels here.
pub(crate) fn create_and_link_worktree<R: tauri::Runtime>(
    store: &Arc<Store>,
    app_handle: &tauri::AppHandle<R>,
    branch: &store::Branch,
    repo_path: &std::path::Path,
    desired_worktree_path: &std::path::Path,
) -> Result<String, String> {
    // Serialize worktree creation per branch so that concurrent
    // `setup_worktree_sync` callers do not race.
    let lock = worktree_setup_lock_for(&branch.id);
    let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());

    // Re-check the DB fast-path under the lock — the other caller may have
    // finished while we were waiting.
    if let Some(existing) = store
        .get_workdir_for_branch(&branch.id)
        .map_err(|e| e.to_string())?
    {
        return Ok(existing.path);
    }

    // Reuse any existing worktree for this branch; otherwise create one.
    // Only consider worktrees within this project's directory to avoid
    // picking up stale worktrees from deleted projects.
    let project_root = git::project_worktree_root_for(&branch.project_id)
        .inspect_err(|e| {
            log::debug!(
                "Could not determine worktree root for project '{}': {e}; \
                 falling back to accepting any matching worktree",
                branch.project_id
            );
        })
        .ok();
    let project_root_ref = project_root.as_deref();

    // The branch the user asked for — the PR head, for an import. The local
    // name may be de-duplicated below, but this stays the branch on origin
    // the checkout pushes to, carried by git's upstream tracking (set after
    // the worktree exists) rather than by anything stored.
    let requested_branch = branch.branch_name.as_str();

    // A PR whose head lives in a fork has no branch on origin: its commits
    // come from `refs/pull/N/head`, and an `origin/<requested>` that happens
    // to exist is an unrelated branch that shares the name. It must never
    // become the push target, so such a checkout is left tracking nothing.
    let head_in_fork = branch.pr_number.is_some() && pr_head_is_in_fork(store, branch)?;

    // The name lookup and the `git worktree add` it leads to, serialized per
    // clone: another project's setup may be working in this clone too.
    let CheckedOutWorktree {
        path: worktree_path,
        local_branch_name,
        adopted_existing,
        created_branch,
    } = check_out_worktree(
        repo_path,
        requested_branch,
        branch.pr_number,
        &branch.base_branch,
        desired_worktree_path,
        project_root_ref,
    )?;

    // Git's upstream tracking is what every push, pull, rebase and
    // comparison reads the branch on origin from, so a worktree Staged
    // created must leave it right: git sets no upstream for a PR branch
    // created from a SHA, and sets `origin/<base>` for a fresh branch
    // started there, which must never be read as a push target. Make the
    // checkout track origin/<requested> when that branch exists and nothing
    // otherwise. This is deliberate for de-duplicated non-PR branches too:
    // the user asked for `feature-x`, and the `-2` suffix exists only
    // because another project's worktree holds the local name.
    //
    // A worktree the user already had is left as they configured it, and so
    // is the upstream of a local branch that existed before this call (it
    // only gets a worktree here): one tracking another remote, or a fork,
    // is the user's own setup, and the resolver ignores it rather than
    // Staged clearing it. Such a branch is only pointed at origin when it
    // tracks nothing at all. A fork PR's checkout (see `head_in_fork`) is
    // not pointed at origin either.
    if !adopted_existing && head_in_fork {
        log::info!("Branch '{local_branch_name}' is a fork PR head; it tracks nothing on origin");
    } else if !adopted_existing {
        let keep_own_upstream = !created_branch
            && git::has_upstream(repo_path, &local_branch_name).map_err(|e| e.to_string())?;
        if keep_own_upstream {
            log::info!(
                "Branch '{local_branch_name}' existed before its worktree; keeping the upstream it has"
            );
        } else {
            // A PR import has only fetched `refs/pull/N/head` so far, and a
            // de-duplicated name may never have had its remote fetched. The
            // branch not existing on origin is fine; it is the `Ok(false)`
            // below.
            if branch.pr_number.is_some() || local_branch_name != requested_branch {
                if let Err(e) = git::fetch_origin_branch(repo_path, requested_branch) {
                    log::debug!(
                        "Could not fetch origin/{requested_branch} before setting upstream: {e}"
                    );
                }
            }
            let set = if created_branch {
                git::set_upstream_to_origin_or_unset(
                    repo_path,
                    &local_branch_name,
                    requested_branch,
                )
            } else {
                git::set_upstream_to_origin(repo_path, &local_branch_name, requested_branch)
            };
            match set {
                Ok(true) => {
                    log::info!("Branch '{local_branch_name}' tracks origin/{requested_branch}")
                }
                Ok(false) => log::debug!(
                    "origin/{requested_branch} does not exist; '{local_branch_name}' tracks nothing until pushed"
                ),
                Err(e) => {
                    // Silently continuing would recreate the stray-remote-branch
                    // bug on the first push; leaving the worktree behind would
                    // make the retry find `feature-x-2` taken and move on to `-3`.
                    discard_incomplete_worktree(
                        repo_path,
                        &worktree_path,
                        created_branch.then_some(local_branch_name.as_str()),
                    );
                    return Err(format!(
                        "Could not set the upstream of '{local_branch_name}' to origin/{requested_branch}: {e}. \
                         The worktree was removed; retry to create it again."
                    ));
                }
            }
        }
    }

    // If we picked a different local branch name, persist it. The branch on
    // origin is not stored; the upstream set above carries it.
    if local_branch_name != branch.branch_name {
        store
            .update_branch_name(&branch.id, &local_branch_name)
            .map_err(|e| e.to_string())?;
        if let Some(repo_id) = &branch.project_repo_id {
            store
                .update_project_repo_branch_name(&branch.project_id, repo_id, &local_branch_name)
                .map_err(|e| e.to_string())?;
        }
    }

    let worktree_str = worktree_path
        .to_str()
        .ok_or("Invalid worktree path")?
        .to_string();

    // Link this path to the branch in DB (create or assign existing record).
    let tracked_workdir = store
        .list_workdirs_for_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|wd| wd.path == worktree_str);

    match tracked_workdir {
        Some(wd) => match wd.branch_id.as_deref() {
            Some(existing_branch_id) if existing_branch_id != branch.id => {
                return Err(format!(
                    "Worktree '{}' is already assigned to another branch",
                    wd.path
                ));
            }
            Some(_) => {}
            None => {
                store
                    .assign_workdir(&wd.id, &branch.id)
                    .map_err(|e| e.to_string())?;
            }
        },
        None => {
            let workdir =
                store::Workdir::new(&branch.project_id, &worktree_str).with_branch(&branch.id);
            store.create_workdir(&workdir).map_err(|e| e.to_string())?;
        }
    }

    // The replies held for a transfer that landed the branch without a
    // worktree wait on this too.
    crate::session_queue::flush_follow_ups(app_handle, &branch.id);
    crate::session_queue::kick(
        app_handle,
        &branch.id,
        crate::session_queue::KickSource::WorktreeCreated,
    );

    // _guard dropped here, releasing the per-branch lock.
    Ok(worktree_str)
}

/// Create the git worktree for a local branch and record its workdir.
///
/// Separated from `create_branch` so the frontend can dismiss the modal
/// immediately and show a "Creating worktree…" spinner on the branch card
/// while this runs in the background.
#[tauri::command(rename_all = "camelCase")]
pub async fn setup_worktree(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: AppHandle,
    branch_id: String,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;
    setup_worktree_impl(store, app_handle, branch_id).await
}

/// The body of [`setup_worktree`], shared with the web transport.
///
/// Runs [`setup_worktree_sync`] on the blocking pool: it may clone, and it
/// fetches and creates the worktree, all of which wait on git or `gh`.
/// The branch is read back afterwards so a local name de-duplicated by
/// [`create_and_link_worktree`] is what the caller sees.
pub(crate) async fn setup_worktree_impl(
    store: Arc<Store>,
    app_handle: AppHandle,
    branch_id: String,
) -> Result<BranchWithWorkdir, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let worktree_str = setup_worktree_sync(&store, &branch_id, &app_handle)?;
        let branch = store
            .get_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
        Ok(to_branch_with_workdir(branch, Some(worktree_str)))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Like [`setup_worktree`], but also runs prerun actions after the worktree is
/// ready.  Used by the frontend retry path so that a failed initial setup
/// (which skips prerun actions) can be fully recovered by the user.
///
/// Resolves at worktree-ready: the prerun run is detached
/// ([`spawn_prerun_actions`]) because the frontend holds the branch in
/// `pendingSetupBranches` — and the card in "Setting up…" — until this command
/// returns, and nothing in what it returns comes from prerun.
#[tauri::command(rename_all = "camelCase")]
pub async fn setup_worktree_and_run_prerun(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: AppHandle,
    branch_id: String,
    provider: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;
    let result =
        setup_worktree_impl(Arc::clone(&store), app_handle.clone(), branch_id.clone()).await?;

    spawn_prerun_actions(
        store,
        app_handle,
        branch_id,
        provider,
        "setup_worktree_and_run_prerun",
    );

    Ok(result)
}

/// Import a GitHub PR as a local branch with a worktree.
///
/// Fetches the PR's head ref, creates a local branch at that commit, sets up
/// a git worktree, and records everything in the DB. Returns the branch with
/// `worktree_path` already populated so the frontend doesn't need to call
/// `setup_worktree` separately.
#[tauri::command(rename_all = "camelCase")]
pub async fn setup_worktree_from_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    pr_number: u64,
    head_ref: String,
    base_ref: String,
    project_repo_id: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;
    setup_worktree_from_pr_impl(
        store,
        project_id,
        pr_number,
        head_ref,
        base_ref,
        project_repo_id,
    )
    .await
}

/// The body of [`setup_worktree_from_pr`], shared with the web transport. Runs
/// on the blocking pool: it may clone, and it fetches the PR head and creates
/// the worktree.
pub(crate) async fn setup_worktree_from_pr_impl(
    store: Arc<Store>,
    project_id: String,
    pr_number: u64,
    head_ref: String,
    base_ref: String,
    project_repo_id: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    crate::project_transfer::ensure_project_not_transferring(&project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let target_repo = match project_repo_id {
            Some(repo_id) => store
                .get_project_repo(&repo_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project repo not found: {repo_id}"))?,
            None => store
                .get_primary_project_repo(&project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project '{project_id}' has no repository attached"))?,
        };
        // Ensure we have a local clone
        let repo_path =
            git::ensure_local_clone(&target_repo.github_repo).map_err(|e| e.to_string())?;
        let desired_worktree_path =
            git::project_worktree_path_for(&project_id, &target_repo.github_repo, &head_ref)
                .map_err(|e| e.to_string())?;

        // Fetch PR head and create worktree
        let (worktree_path, branch_name, base_branch) = git::create_worktree_from_pr_at_path(
            &repo_path,
            pr_number,
            &head_ref,
            &base_ref,
            &desired_worktree_path,
        )
        .map_err(|e| e.to_string())?;

        let worktree_str = worktree_path
            .to_str()
            .ok_or("Invalid worktree path")?
            .to_string();

        // Create branch record with PR number
        let branch = store::Branch::new(&project_id, &branch_name, &base_branch)
            .with_project_repo(&target_repo.id)
            .with_pr(pr_number);
        store.create_branch(&branch).map_err(|e| e.to_string())?;

        // Create workdir record
        let workdir = store::Workdir::new(&project_id, &worktree_str).with_branch(&branch.id);
        store.create_workdir(&workdir).map_err(|e| e.to_string())?;

        Ok(to_branch_with_workdir(branch, Some(worktree_str)))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Create a remote branch record.
///
/// Creates the branch DB record with type=remote and status=Starting.
/// No workspace is started here — call `start_workspace` separately.
#[tauri::command(rename_all = "camelCase")]
pub async fn create_remote_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    branch_name: String,
    base_branch: Option<String>,
    workspace_name: String,
    project_repo_id: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;
    create_remote_branch_impl(
        store,
        project_id,
        branch_name,
        base_branch,
        workspace_name,
        project_repo_id,
    )
    .await
}

/// The body of [`create_remote_branch`], shared with the web transport. Runs
/// on the blocking pool: with no `base_branch` it asks GitHub for the default
/// branch.
pub(crate) async fn create_remote_branch_impl(
    store: Arc<Store>,
    project_id: String,
    branch_name: String,
    base_branch: Option<String>,
    workspace_name: String,
    project_repo_id: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    crate::project_transfer::ensure_project_not_transferring(&project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let project = store
            .get_project(&project_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Project not found: {project_id}"))?;
        let resolved_workspace_name =
            resolve_project_workspace_name(&store, &project, Some(&workspace_name))?;

        // Detect default branch if none specified (via GitHub API, no local clone needed)
        let target_repo = match project_repo_id {
            Some(repo_id) => store
                .get_project_repo(&repo_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project repo not found: {repo_id}"))?,
            None => store
                .get_primary_project_repo(&project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project '{project_id}' has no repository attached"))?,
        };
        let effective_base = match base_branch {
            Some(b) => b,
            None => git::detect_default_branch_for_repo(&target_repo.github_repo)
                .map_err(|e| e.to_string())?,
        };

        // Normalise to "origin/<branch>" so diffs and worktree creation always
        // use the remote-tracking ref rather than a stale local branch.
        let effective_base = if effective_base.starts_with("origin/") {
            effective_base
        } else {
            format!("origin/{effective_base}")
        };

        // Create the branch record (starts in Starting status)
        let branch = store::Branch::new_remote(
            &project_id,
            &branch_name,
            &effective_base,
            &resolved_workspace_name,
        )
        .with_project_repo(&target_repo.id);
        store.create_branch(&branch).map_err(|e| e.to_string())?;

        Ok(to_branch_with_workdir(branch, None))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Start the Blox workspace for a remote branch and set its branch up.
///
/// Separated from `create_remote_branch` so the frontend can dismiss the
/// dialog immediately and show the card in its "Provisioning…" state while
/// this runs in the background.
///
/// Also the error view's Retry: a branch in `Error` is set up again.
///
/// Only asks: this returns once the setup run is under way, joined to the
/// one in flight when there is one. The backend carries the run through, its
/// outcome lands on the branch's row, and a failure's reason is announced as
/// a `workspace-setup-error` event. An error here means the request could
/// not be made. See [`crate::workspace_setup::request_setup`].
#[tauri::command(rename_all = "camelCase")]
pub async fn start_workspace(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: AppHandle,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    crate::workspace_setup::request_branch_setup(&app_handle, &store, &branch_id)
}

/// Return the value of the `BLOX_ENV` environment variable, if set.
#[tauri::command]
pub fn get_blox_env() -> Option<String> {
    std::env::var("BLOX_ENV").ok()
}

/// Get info about a remote branch's Blox workspace.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_workspace_info(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<blox::WorkspaceInfo, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let ws_name = branch
        .workspace_name
        .ok_or("Branch is not a remote workspace branch")?;

    run_blox_blocking(move || blox::ws_info(&ws_name))
        .await
        .map_err(|e| e.to_string())
}

/// Whether a raw Blox status string says the workstation exists and is
/// coming up: being created and bootstrapped, or resumed.
pub(crate) fn is_blox_booting_status(blox_status: &str) -> bool {
    matches!(
        blox_status.to_ascii_lowercase().as_str(),
        "starting" | "provisioning" | "creating"
    )
}

/// Map a raw Blox status string to our `WorkspaceStatus` enum.
///
/// During initial startup, Blox may briefly report "stopped" before the
/// workspace transitions to "running". If the DB still says Starting,
/// treat a Blox "stopped" as still Starting so we keep polling.
pub(crate) fn map_blox_status_to_workspace_status(
    blox_status: Option<&str>,
    db_status: Option<&store::WorkspaceStatus>,
) -> store::WorkspaceStatus {
    let normalized = blox_status.map(|s| s.to_ascii_lowercase());
    match normalized.as_deref() {
        Some("running") | Some("ready") | Some("active") => store::WorkspaceStatus::Running,
        Some("stopped") => {
            if db_status == Some(&store::WorkspaceStatus::Starting) {
                store::WorkspaceStatus::Starting
            } else {
                store::WorkspaceStatus::Stopped
            }
        }
        Some(status) if is_blox_booting_status(status) => store::WorkspaceStatus::Starting,
        Some("suspended") => store::WorkspaceStatus::Suspended,
        Some("deleted") => store::WorkspaceStatus::Stopped,
        Some("shutting_down") => {
            if db_status == Some(&store::WorkspaceStatus::Starting) {
                store::WorkspaceStatus::Starting
            } else {
                store::WorkspaceStatus::Stopped
            }
        }
        Some("degraded") => store::WorkspaceStatus::Error,
        Some("error") | Some("failed") => store::WorkspaceStatus::Error,
        _ => store::WorkspaceStatus::Starting,
    }
}

/// Poll a remote branch's workspace status, update the DB, and return the new status.
///
/// Queries the Blox CLI, maps the reported status to our `WorkspaceStatus`
/// enum, persists the change, and returns the updated status string. It
/// never moves a branch from `Starting` to `Running`: a workstation that is
/// up while its row is still `Starting` gets branch setup triggered instead
/// (see [`crate::workspace_setup::poll`]).
#[tauri::command(rename_all = "camelCase")]
pub async fn poll_workspace_status(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: AppHandle,
    branch_id: String,
) -> Result<PollWorkspaceResult, String> {
    let store = get_store(&store)?;
    crate::workspace_setup::poll::poll_workspace_status(&store, &app_handle, &branch_id).await
}

/// Map a blox orchestrator `CommandType` to a stable string identifier that
/// the frontend can map to a display label.
pub(crate) fn bootstrap_command_type_name(command_type: blox::CommandType) -> &'static str {
    match command_type {
        blox::CommandType::Checkout => "checkout",
        blox::CommandType::ExecuteProcess => "execute_process",
        blox::CommandType::ProjectBootstrap => "project_bootstrap",
        blox::CommandType::ProvisionWorkspace => "provision_workspace",
        blox::CommandType::RestoreSnapshot => "restore_snapshot",
        blox::CommandType::ExecuteBootstrap => "execute_bootstrap",
        blox::CommandType::Unspecified => "unknown",
    }
}

/// Derive workspace setup progress from bootstrap commands and emit events
/// for all branches sharing the workspace.
pub(crate) fn emit_workspace_setup_progress(
    app_handle: &AppHandle,
    branch_ids: &[String],
    commands: &[blox::WorkspaceCommand],
) {
    let bootstrap: Vec<&blox::WorkspaceCommand> =
        commands.iter().filter(|c| c.is_bootstrap).collect();
    if bootstrap.is_empty() {
        return;
    }

    let total = bootstrap.len();
    let completed = bootstrap.iter().filter(|c| c.status.is_finished()).count();

    // All done — workspace will transition to running (or error), no need
    // to emit.
    if completed == total {
        return;
    }

    // Pick the first currently-running command.  If none are running yet,
    // fall back to the first unfinished command.
    // Safety: `bootstrap` is non-empty (checked above) and `completed < total`,
    // so at least one command is unfinished.
    let current_cmd = bootstrap
        .iter()
        .find(|c| c.status == blox::WorkspaceCommandStatus::Running)
        .or_else(|| bootstrap.iter().find(|c| !c.status.is_finished()))
        .unwrap();

    let phase = bootstrap_command_type_name(current_cmd.command_type);
    let detail = Some(format!("Step {} of {}", completed + 1, total));

    for bid in branch_ids {
        crate::web_server::emit_to_all(
            app_handle,
            "workspace-setup-progress",
            WorktreeSetupProgress {
                branch_id: bid.clone(),
                phase: phase.to_string(),
                detail: detail.clone(),
            },
        );
    }
}

/// Poll workspace statuses for multiple branches in a single `sq blox ws list` call.
///
/// Returns a map from branch ID to `PollWorkspaceResult`. Branches that cannot be
/// resolved (e.g. missing workspace name, not found in store) are silently omitted
/// from the result rather than failing the entire batch. Like
/// [`poll_workspace_status`], it triggers branch setup rather than promoting
/// a `Starting` branch.
#[tauri::command(rename_all = "camelCase")]
pub async fn poll_all_workspace_statuses(
    app_handle: AppHandle,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_ids: Vec<String>,
) -> Result<HashMap<String, PollWorkspaceResult>, String> {
    let store = get_store(&store)?;
    crate::workspace_setup::poll::poll_all_workspace_statuses(&store, &app_handle, &branch_ids)
        .await
}

/// Resume a suspended Blox workspace.
///
/// Transitions all branches sharing this workspace to Starting and calls
/// `sq blox ws resume`. Returns the IDs of all affected branches.
#[tauri::command(rename_all = "camelCase")]
pub async fn resume_workspace(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: AppHandle,
    workspace_name: String,
) -> Result<Vec<String>, String> {
    let store = get_store(&store)?;
    resume_workspace_impl(&store, &app_handle, &workspace_name).await
}

/// [`resume_workspace`], shared with the web server arm.
///
/// The branches stay `Starting` when this returns: resuming brings the
/// workstation back, and only branch setup marks a branch `Running`, once
/// its checkout has succeeded. Setup is triggered here for each branch so a
/// resume finishes without depending on a window to ask for it.
pub(crate) async fn resume_workspace_impl(
    store: &Arc<Store>,
    app_handle: &AppHandle,
    workspace_name: &str,
) -> Result<Vec<String>, String> {
    crate::project_transfer::ensure_workspace_not_transferring(workspace_name)?;

    // Transition all peer branches to Starting so the UI begins polling.
    let branch_ids = store
        .update_workspace_status_by_workspace_name(
            workspace_name,
            &store::WorkspaceStatus::Starting,
        )
        .map_err(|e| e.to_string())?;

    if branch_ids.is_empty() {
        return Err(format!("No branches found for workspace: {workspace_name}"));
    }

    let ws = workspace_name.to_string();
    if let Err(e) = run_blox_blocking(move || blox::ws_resume(&ws)).await {
        log::warn!(
            "[resume_workspace] workspace={} resume failed: {}",
            workspace_name,
            e
        );
        store
            .update_workspace_status_by_workspace_name(
                workspace_name,
                &store::WorkspaceStatus::Error,
            )
            .ok();
        // `Error` with its reason on show and a retry timer, which sets the
        // branches up once the workstation does come back.
        let reason = format!("Could not resume workspace '{workspace_name}': {e}");
        for branch_id in &branch_ids {
            crate::workspace_setup::branch_failed(app_handle, store, branch_id, &reason);
        }
        return Err(e.to_string());
    }

    // The user's resume: a setup that fails after it is theirs to hear of.
    for branch_id in &branch_ids {
        crate::workspace_setup::trigger_branch_setup(app_handle, store, branch_id, true);
    }

    Ok(branch_ids)
}

#[tauri::command]
pub async fn delete_branch(
    app_handle: tauri::AppHandle,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    executor: tauri::State<'_, Arc<ActionExecutor>>,
    registry: tauri::State<'_, Arc<ActionRegistry>>,
    session_registry: tauri::State<'_, Arc<crate::session_runner::SessionRegistry>>,
    branch_id: String,
) -> Result<(), String> {
    let services = crate::branch_teardown::TeardownServices {
        store: get_store(&store)?,
        action_executor: Arc::clone(&executor),
        action_registry: Arc::clone(&registry),
        session_registry: Arc::clone(&session_registry),
        app_handle,
    };
    crate::branch_teardown::delete_branch_impl(services, branch_id).await
}

/// Move a branch into another project, taking its notes, commits, reviews,
/// sessions and images with it.
#[tauri::command(rename_all = "camelCase")]
pub async fn move_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    executor: tauri::State<'_, Arc<ActionExecutor>>,
    registry: tauri::State<'_, Arc<ActionRegistry>>,
    branch_id: String,
    target_project_id: String,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;
    move_branch_impl(&store, &executor, &registry, &branch_id, &target_project_id).await
}

/// The `(github_repo, subpath)` identity of a repo inside a project, matching
/// `idx_project_repos_unique`'s `(project_id, github_repo, COALESCE(subpath,
/// ''))`: a NULL subpath and an empty one are the same repo.
fn repo_subpath_key(github_repo: &str, subpath: Option<&str>) -> String {
    format!("{github_repo}\u{0}{}", subpath.unwrap_or(""))
}

fn describe_repo(github_repo: &str, subpath: Option<&str>) -> String {
    match subpath.filter(|s| !s.is_empty()) {
        Some(subpath) => format!("{github_repo} ({subpath})"),
        None => github_repo.to_string(),
    }
}

/// A validated branch move: what to rewrite, and where the worktree has to end
/// up, resolved against the database before anything is mutated.
#[derive(Debug)]
struct BranchMovePlan {
    source_project: store::Project,
    /// The branch's repo slug, resolved through `project_repos` or the source
    /// project's primary. Names the local clone the worktree belongs to.
    github_repo: String,
    mv: store::BranchMove,
}

/// Check every precondition on a move and resolve what it will rewrite.
///
/// Split out from [`move_branch_impl`] because this is the whole of the move's
/// decision-making — which `project_repos` row travels, which is cloned, what
/// counts as a destination that already has the repo — and it needs nothing but
/// the store to run.
fn plan_branch_move(
    store: &Arc<Store>,
    branch_id: &str,
    target_project_id: &str,
) -> Result<BranchMovePlan, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    if branch.project_id == target_project_id {
        return Err("This branch is already in that project".to_string());
    }

    let source_project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;
    let target_project = store
        .get_project(target_project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {target_project_id}"))?;

    // Remote branches of a project share one Blox workspace, so moving one out
    // would mean cross-workspace surgery on a filesystem we don't own.
    for project in [&source_project, &target_project] {
        if project.location == store::ProjectLocation::Remote {
            return Err(format!(
                "Project '{}' runs on a remote workspace; branches can only move between local projects",
                project.name
            ));
        }
    }
    if branch.branch_type == store::BranchType::Remote {
        return Err("Remote branches can't be moved between projects".to_string());
    }

    // The move pulls the worktree out from under anything running in it.
    // [`apply_branch_move`] plans again under the branch's session launch lock,
    // which is what keeps a session from starting between this check and the
    // rename that acts on it.
    if store
        .has_running_session_for_branch(branch_id)
        .map_err(|e| e.to_string())?
    {
        return Err("A session is running on this branch — wait for the running session to finish before moving it".to_string());
    }

    let source_repo = match &branch.project_repo_id {
        Some(repo_id) => store.get_project_repo(repo_id).map_err(|e| e.to_string())?,
        None => None,
    };
    // Mirrors `resolve_branch_repo_slug`: a branch with no repo link falls back
    // to its project's primary repo.
    let (github_repo, subpath) = match &source_repo {
        Some(repo) => (repo.github_repo.clone(), repo.subpath.clone()),
        None => (
            project_primary_repo(&source_project)?.to_string(),
            source_project.subpath.clone(),
        ),
    };

    let key = repo_subpath_key(&github_repo, subpath.as_deref());
    if store
        .list_project_repos(target_project_id)
        .map_err(|e| e.to_string())?
        .iter()
        .any(|repo| repo_subpath_key(&repo.github_repo, repo.subpath.as_deref()) == key)
    {
        return Err(format!(
            "'{}' already has {} attached",
            target_project.name,
            describe_repo(&github_repo, subpath.as_deref())
        ));
    }

    // A `project_repos` row can be shared by several branches, so it only
    // travels when this branch is the last one on it.
    let placement = match &source_repo {
        Some(repo) => {
            let shared = store
                .list_branches_for_project(&source_project.id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .any(|b| {
                    b.id != branch.id && b.project_repo_id.as_deref() == Some(repo.id.as_str())
                });
            if shared {
                let mut clone = store::ProjectRepo::new(
                    target_project_id,
                    &repo.github_repo,
                    &repo.branch_name,
                    repo.subpath.clone(),
                );
                clone.reason = repo.reason.clone();
                clone.head_repo = repo.head_repo.clone();
                store::RepoPlacement::Clone(clone)
            } else {
                store::RepoPlacement::Reparent {
                    repo_id: repo.id.clone(),
                }
            }
        }
        // Legacy branch with no repo link: materialize a row rather than carry
        // the NULL across, which `resolve_branch_repo_slug` would resolve to the
        // *destination's* primary repo — a wrong-repo read.
        None => store::RepoPlacement::Clone(store::ProjectRepo::new(
            target_project_id,
            &github_repo,
            &branch.branch_name,
            subpath.clone(),
        )),
    };

    // `workdirs.path` is the source of truth for where the worktree actually
    // is — some branches still sit in the legacy non-project-scoped layout.
    let workdir = match store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?
    {
        Some(wd) => {
            let new_path = git::project_worktree_path_for(
                target_project_id,
                &github_repo,
                &branch.branch_name,
            )
            .map_err(|e| e.to_string())?;
            Some(store::WorkdirMove {
                workdir_id: wd.id,
                old_path: wd.path,
                new_path: new_path.to_string_lossy().to_string(),
            })
        }
        None => None,
    };

    Ok(BranchMovePlan {
        mv: store::BranchMove {
            branch_id: branch_id.to_string(),
            source_project_id: source_project.id.clone(),
            target_project_id: target_project_id.to_string(),
            repo: placement,
            workdir,
        },
        source_project,
        github_repo,
    })
}

/// The body of [`move_branch`], shared with the web router.
///
/// The move runs in three stages that have to agree with each other: the
/// worktree relocation on disk, the re-parent transaction, and the image files.
/// The first two go together in [`apply_branch_move`], under the branch's
/// session launch lock; a failed transaction puts the worktree back.
pub(crate) async fn move_branch_impl(
    store: &Arc<Store>,
    executor: &ActionExecutor,
    registry: &ActionRegistry,
    branch_id: &str,
    target_project_id: &str,
) -> Result<BranchWithWorkdir, String> {
    // Pre-flight, so an impossible move is refused before anything the branch is
    // running gets stopped for it. [`apply_branch_move`] plans again under the
    // launch lock, which is what makes the answer safe to act on.
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;
    crate::project_transfer::ensure_project_not_transferring(target_project_id)?;
    plan_branch_move(store, branch_id, target_project_id)?;

    crate::actions::commands::stop_actions_for_branches(executor, registry, &[branch_id]);

    let source_project = {
        let store = Arc::clone(store);
        let branch_id = branch_id.to_string();
        let target_project_id = target_project_id.to_string();
        tauri::async_runtime::spawn_blocking(move || {
            apply_branch_move(&store, &branch_id, &target_project_id)
        })
        .await
        .map_err(|e| format!("Failed to move branch: {e}"))??
    };

    move_branch_image_files(store, branch_id, &source_project.id, target_project_id);

    // Cheaper to drop than to move, and it rebuilds on the next diff read.
    if let Err(e) = crate::diff_cache::delete_branch_cache(
        source_project.location,
        &source_project.id,
        branch_id,
    ) {
        log::warn!("Failed to clear diff cache for moved branch {branch_id}: {e}");
    }

    let updated = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?
        .map(|w| w.path);
    Ok(to_branch_with_workdir(updated, workdir))
}

/// Plan the move and apply it: the worktree on disk first, then the transaction
/// that re-parents the branch, with the rename undone if the transaction fails.
/// Returns the source project, which the caller still needs to relocate the
/// branch's images and drop its diff cache.
///
/// Synchronous and holding the branch's session launch lock throughout, the way
/// [`crate::prs`] holds it around its queue-or-start decisions: the plan's "no
/// session is running on this branch" precondition is only worth acting on for
/// as long as no session can start underneath it, and two moves of the same
/// branch — the Tauri command and the web router are separate entry points —
/// would otherwise race the same rename. The plan is resolved in here for that
/// reason rather than handed in from outside the lock.
fn apply_branch_move(
    store: &Arc<Store>,
    branch_id: &str,
    target_project_id: &str,
) -> Result<store::Project, String> {
    let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
    let _guard = launch_lock.lock().unwrap();

    let BranchMovePlan {
        source_project,
        github_repo,
        mv,
    } = plan_branch_move(store, branch_id, target_project_id)?;

    let repo_path = crate::paths::repos_dir()
        .map(|d| d.join(&github_repo))
        .ok_or("Cannot determine clone path")?;

    // A branch whose setup never finished has nothing on disk to relocate — and
    // neither does one whose move was interrupted between this rename and the
    // commit below, because `new_path` is derived rather than stored: re-running
    // the move finds the worktree already sitting where the transaction is about
    // to record it, and completes.
    let mut moved_on_disk = false;
    if let Some(wd) = &mv.workdir {
        let old_path = Path::new(&wd.old_path);
        if old_path.exists() {
            git::move_worktree(&repo_path, old_path, Path::new(&wd.new_path))
                .map_err(|e| format!("Failed to move worktree: {e}"))?;
            moved_on_disk = true;
        }
    }

    if let Err(e) = store.move_branch_to_project(&mv) {
        // Disk and database have to agree, so undo the half that landed.
        if let (true, Some(wd)) = (moved_on_disk, &mv.workdir) {
            if let Err(revert) =
                git::move_worktree(&repo_path, Path::new(&wd.new_path), Path::new(&wd.old_path))
            {
                log::warn!(
                    "Failed to move worktree for branch {branch_id} back to {}: {revert}",
                    wd.old_path
                );
                // Reporting only the store error would read as "nothing changed"
                // while the worktree sits where neither project expects it.
                return Err(format!(
                    "{e} The worktree could not be moved back either, so it is now at {} instead of {}: {revert}",
                    wd.new_path, wd.old_path
                ));
            }
        }
        return Err(e.to_string());
    }

    Ok(source_project)
}

/// Relocate a moved branch's image files into the destination project's
/// `images/` directory.
///
/// Tolerant per entry, like [`crate::paths::migrate_directory_contents`]: a file
/// that won't move costs one broken attachment, not the whole move — and the
/// rows already point at the destination by the time this runs.
fn move_branch_image_files(
    store: &Arc<Store>,
    branch_id: &str,
    source_project_id: &str,
    target_project_id: &str,
) {
    let images = match store.list_all_images_for_branch(branch_id) {
        Ok(images) => images,
        Err(e) => {
            log::warn!("Cannot list images for moved branch {branch_id}: {e}");
            return;
        }
    };

    for image in images {
        let from = store::images::image_file_path(source_project_id, &image.id, &image.filename);
        let to = store::images::image_file_path(target_project_id, &image.id, &image.filename);
        let (Ok(from), Ok(to)) = (from, to) else {
            continue;
        };
        // A PDF's cached thumbnail travels with it; losing one only costs a
        // re-render, so its failure is not worth more than a log line.
        if let (Ok(thumb_from), Ok(thumb_to)) = (
            store::images::thumbnail_file_path(source_project_id, &image.id),
            store::images::thumbnail_file_path(target_project_id, &image.id),
        ) {
            if thumb_from.exists() {
                if let Some(parent) = thumb_to.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::rename(&thumb_from, &thumb_to) {
                    log::warn!("Failed to move thumbnail {}: {e}", thumb_from.display());
                }
            }
        }
        if !from.exists() {
            continue;
        }
        if let Some(parent) = to.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                log::warn!("Cannot create image directory {}: {e}", parent.display());
                continue;
            }
        }
        if let Err(e) = std::fs::rename(&from, &to) {
            log::warn!(
                "Failed to move image {} -> {}: {e}",
                from.display(),
                to.display()
            );
        }
    }
}

#[tauri::command(rename_all = "camelCase")]
pub async fn rename_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    branch_name: String,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;
    crate::project_transfer::ensure_branch_not_transferring(&branch_id)?;
    let new_name = branch_name.trim();
    if new_name.is_empty() {
        return Err("Branch name is required".to_string());
    }

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if branch.branch_name == new_name {
        let workdir = store
            .get_workdir_for_branch(&branch.id)
            .map_err(|e| e.to_string())?
            .map(|w| w.path);
        return Ok(to_branch_with_workdir(branch, workdir));
    }

    match branch.branch_type {
        store::BranchType::Local => {
            let store = Arc::clone(&store);
            let branch = branch.clone();
            let new_name = new_name.to_owned();
            tauri::async_runtime::spawn_blocking(move || {
                rename_local_branch(&store, &branch, &new_name)
            })
            .await
            .map_err(|e| e.to_string())??
        }
        store::BranchType::Remote => {
            // `git branch -m` carries the upstream across the rename, so the
            // branch keeps pushing where it did — the PR head, for a PR
            // branch. Best effort: the workspace may be stopped.
            if let Some(ws_name) = &branch.workspace_name {
                let repo_subpath = resolve_branch_workspace_subpath(&store, &branch)?;
                if let Err(e) = run_workspace_git_async(
                    ws_name,
                    repo_subpath.as_deref(),
                    ExecMode::Transient,
                    &["branch", "-m", new_name],
                )
                .await
                {
                    log::warn!("Failed to rename branch in workspace '{ws_name}': {e}");
                }
            }
            record_branch_rename(&store, &branch, new_name)?;
        }
    }

    let updated = store
        .get_branch(&branch.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {}", branch.id))?;
    let workdir = store
        .get_workdir_for_branch(&updated.id)
        .map_err(|e| e.to_string())?
        .map(|w| w.path);
    Ok(to_branch_with_workdir(updated, workdir))
}

/// Rename a desktop branch's local git branch to `new_name` and record the
/// new name.
///
/// `git branch -m` carries the branch's upstream tracking across the rename,
/// so where the branch pushes — the PR head, for a PR branch — does not
/// move; only the local name does.
pub(crate) fn rename_local_branch(
    store: &Arc<Store>,
    branch: &store::Branch,
    new_name: &str,
) -> Result<(), String> {
    if let Some(wd) = store
        .get_workdir_for_branch(&branch.id)
        .map_err(|e| e.to_string())?
    {
        git::cli_run(Path::new(&wd.path), &["branch", "-m", new_name])
            .map_err(|e| format!("Failed to rename local git branch: {e}"))?;
    }

    record_branch_rename(store, branch, new_name)
}

/// Store the new local name for `branch` (and its project-repo row, if any).
pub(crate) fn record_branch_rename(
    store: &Arc<Store>,
    branch: &store::Branch,
    new_name: &str,
) -> Result<(), String> {
    crate::project_transfer::ensure_branch_not_transferring(&branch.id)?;
    store
        .update_branch_name(&branch.id, new_name)
        .map_err(|e| e.to_string())?;
    if let Some(repo_id) = &branch.project_repo_id {
        store
            .update_project_repo_branch_name(&branch.project_id, repo_id, new_name)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Parse a git/gh progress line into a phase name and percentage.
///
/// Examples:
/// - `"remote: Counting objects:   3% (62/2054)"` -> `("Counting objects", 3)`
/// - `"Receiving objects:  27% (11893/44046), 6.93 MiB | 13.84 MiB/s"` -> `("Receiving objects", 27)`
fn parse_git_progress_line(line: &str) -> Option<(String, u32)> {
    // Strip optional "remote: " prefix
    let line = line.strip_prefix("remote: ").unwrap_or(line);
    // Find the colon separator
    let colon_pos = line.find(':')?;
    let phase_name = line[..colon_pos].trim().to_string();
    if phase_name.is_empty() {
        return None;
    }
    // Find percentage
    let rest = &line[colon_pos + 1..];
    let pct_pos = rest.find('%')?;
    let num_start = rest[..pct_pos].rfind(|c: char| !c.is_ascii_digit())? + 1;
    let pct: u32 = rest[num_start..pct_pos].parse().ok()?;
    Some((phase_name, pct))
}

/// Set up a git worktree for a branch synchronously.
///
/// Every worktree setup goes through here: both transports'
/// [`setup_worktree`] (via [`setup_worktree_impl`]), project creation, the
/// add-repo setup pass and MCP. Handles cloning with progress reporting, then
/// delegates to [`create_and_link_worktree`] for worktree creation and DB
/// linkage. It clones, fetches and creates the worktree inline, so call it
/// from the blocking pool.
pub(crate) fn setup_worktree_sync<R: tauri::Runtime>(
    store: &Arc<Store>,
    branch_id: &str,
    app_handle: &tauri::AppHandle<R>,
) -> Result<String, String> {
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;
    let emit_progress = |phase: &str, detail: Option<String>| {
        emit_setup_progress(app_handle, branch_id, phase, detail)
    };

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    // A caller that read the branch before a transfer to remote switched it
    // must not give it a worktree now: during teardown that would adopt the
    // old worktree, which is about to be removed from under the new row.
    if branch.branch_type == store::BranchType::Remote {
        return Err(format!(
            "{} is on a remote workspace; it has no local worktree to set up.",
            branch.branch_name
        ));
    }

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    // Idempotent fast-path: if the branch already has a workdir, reuse it.
    if let Some(existing) = store
        .get_workdir_for_branch(&branch.id)
        .map_err(|e| e.to_string())?
    {
        return Ok(existing.path);
    }

    // Resolve the repo slug for this branch
    let repo_slug = resolve_branch_repo_slug(store, &project, &branch)?;
    emit_progress("cloning", None);
    let repo_path = {
        let handle = app_handle.clone();
        let bid = branch_id.to_string();
        let mut last_emit = std::time::Instant::now();
        crate::git::ensure_local_clone_with_progress(&repo_slug, |line| {
            if let Some((phase_name, pct)) = parse_git_progress_line(line) {
                let now = std::time::Instant::now();
                if now.duration_since(last_emit) >= std::time::Duration::from_millis(250) {
                    last_emit = now;
                    let detail = format!("{phase_name} \u{2014} {pct}%");
                    crate::web_server::emit_to_all(
                        &handle,
                        "worktree-setup-progress",
                        WorktreeSetupProgress {
                            branch_id: bid.clone(),
                            phase: "cloning".to_string(),
                            detail: Some(detail),
                        },
                    );
                }
            }
        })
        .map_err(|e| e.to_string())?
    };
    emit_progress("fetching", None);
    fetch_for_worktree_with_offline_fallback(
        &repo_path,
        &repo_slug,
        &branch.branch_name,
        &branch.base_branch,
    )?;
    let desired_worktree_path =
        crate::git::project_worktree_path_for(&branch.project_id, &repo_slug, &branch.branch_name)
            .map_err(|e| e.to_string())?;

    emit_progress("creating_worktree", None);
    let worktree_str = create_and_link_worktree(
        store,
        app_handle,
        &branch,
        &repo_path,
        &desired_worktree_path,
    )?;

    Ok(worktree_str)
}

/// What [`claim_and_run_prerun_actions`] did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PrerunOutcome {
    /// This caller won the setup claim and the branch's prerun actions all ran
    /// to completion; the count is how many.
    Ran(usize),
    /// Nothing ran — another caller had already claimed this branch's setup, or
    /// the claim or the run failed. Failures are logged, not returned: no
    /// caller has anything to do with one.
    NotRun,
}

/// Claim setup ownership of a branch and, if this caller wins the claim, run
/// the branch's prerun actions.
///
/// `mark_branch_setup_complete` is a one-shot atomic claim, so the claim and
/// the run belong in one place: a caller that takes the claim and then doesn't
/// run leaves that worktree without its setup actions forever, and nothing
/// retries it. Keeping them together is why this is the only way in to
/// [`run_prerun_actions_for_branch`], which is private for that reason.
///
/// **Never await this on a caller's critical path.** Detection inside it waits
/// out another caller's detection window for up to five minutes
/// ([`crate::actions::commands::ensure_actions_detected`]), and each prerun
/// action then runs to completion in turn — a dependency install alone can
/// outlast any request timeout. Every entry point either already runs in a
/// background task or reaches this through [`spawn_prerun_actions`].
///
/// `tag` names the entry point in this function's log lines.
pub(crate) async fn claim_and_run_prerun_actions(
    store: &Arc<Store>,
    app_handle: &AppHandle,
    branch_id: &str,
    executor: &Arc<ActionExecutor>,
    act_registry: &Arc<ActionRegistry>,
    provider: Option<&str>,
    tag: &str,
) -> PrerunOutcome {
    // Before the claim, which a refused run would otherwise use up. The
    // transfer resets the claim when it switches the branch over.
    if crate::project_transfer::is_branch_transferring(branch_id) {
        log::info!("[{tag}] branch {branch_id} is being transferred, skipping prerun");
        return PrerunOutcome::NotRun;
    }
    match store.mark_branch_setup_complete(branch_id) {
        Ok(true) => {
            emit_setup_progress(app_handle, branch_id, "running_setup_actions", None);
            match run_prerun_actions_for_branch(
                store,
                app_handle,
                branch_id,
                executor,
                act_registry,
                provider,
            )
            .await
            {
                Ok(count) => {
                    log::info!("[{tag}] ran {count} prerun actions");
                    PrerunOutcome::Ran(count)
                }
                Err(e) => {
                    log::warn!("[{tag}] prerun actions failed: {e}");
                    PrerunOutcome::NotRun
                }
            }
        }
        Ok(false) => {
            log::info!("[{tag}] branch {branch_id} already setup complete, skipping prerun");
            PrerunOutcome::NotRun
        }
        Err(e) => {
            log::warn!("[{tag}] failed to mark setup complete: {e}");
            PrerunOutcome::NotRun
        }
    }
}

/// [`claim_and_run_prerun_actions`], detached from the caller.
///
/// For the entry points whose caller is on a clock — the branch card's Retry
/// button, over Tauri and over HTTP — where prerun's result is discarded
/// anyway. They return as soon as the worktree exists and leave this running:
/// the worktree has been on disk for seconds by then, while prerun can take
/// minutes, and the frontend holds the branch in "Setting up…" until the
/// command resolves.
///
/// The claim goes into the task with the run so it can't be consumed by a task
/// that never runs the prerun; see [`claim_and_run_prerun_actions`].
pub(crate) fn spawn_prerun_actions(
    store: Arc<Store>,
    app_handle: AppHandle,
    branch_id: String,
    provider: Option<String>,
    tag: &'static str,
) {
    tauri::async_runtime::spawn(async move {
        let executor = app_handle.state::<Arc<ActionExecutor>>().inner().clone();
        let act_registry = app_handle.state::<Arc<ActionRegistry>>().inner().clone();
        claim_and_run_prerun_actions(
            &store,
            &app_handle,
            &branch_id,
            &executor,
            &act_registry,
            provider.as_deref(),
            tag,
        )
        .await;
    });
}

/// Run detect_actions (if needed) and all prerun actions for a branch.
///
/// This replicates the core logic from `actions::commands::run_prerun_actions`
/// without requiring Tauri state.
///
/// Private on purpose: prerun runs exactly once per branch, behind the
/// `mark_branch_setup_complete` claim, so [`claim_and_run_prerun_actions`] is
/// the only caller — the two can't drift apart if there is nowhere else to
/// call this from.
async fn run_prerun_actions_for_branch(
    store: &Arc<Store>,
    app_handle: &AppHandle,
    branch_id: &str,
    executor: &Arc<ActionExecutor>,
    act_registry: &Arc<ActionRegistry>,
    provider_id: Option<&str>,
) -> Result<usize, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Branch not found".to_string())?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Project not found".to_string())?;

    // Resolve the repo/subpath for this branch
    let (github_repo, subpath) = if let Some(project_repo_id) = &branch.project_repo_id {
        let project_repo = store
            .get_project_repo(project_repo_id)
            .map_err(|e| format!("Failed to get project repo: {e}"))?
            .ok_or_else(|| format!("Project repo not found: {project_repo_id}"))?;
        (project_repo.github_repo, project_repo.subpath)
    } else {
        let repo = project
            .primary_repo()
            .ok_or_else(|| "Project has no repository attached".to_string())?;
        (repo.to_string(), project.subpath.clone())
    };

    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| format!("Failed to get action context: {e}"))?;

    // If actions haven't been detected yet for this repo+subpath, detect now —
    // waiting out another caller's detection rather than reading a list it
    // hasn't finished writing.
    let actions =
        crate::actions::commands::ensure_actions_detected(app_handle, store, &context, provider_id)
            .await?;
    let prerun_actions: Vec<_> = actions
        .into_iter()
        .filter(|a| matches!(a.action_type, ActionType::Prerun))
        .collect();

    if prerun_actions.is_empty() {
        return Ok(0);
    }

    // Get the worktree path for this branch
    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| format!("Failed to get workdir: {e}"))?
        .ok_or_else(|| "No worktree found for branch".to_string())?;

    let working_dir = if let Some(ref sp) = subpath {
        std::path::PathBuf::from(&workdir.path)
            .join(sp)
            .to_string_lossy()
            .to_string()
    } else {
        workdir.path
    };

    // Execute each prerun action, waiting for each to complete
    let mut count = 0;
    for action in prerun_actions {
        let listener = Arc::new(TauriExecutionListener::new(
            app_handle.clone(),
            branch_id.to_string(),
            action.id.clone(),
            action.name.clone(),
            action.action_type.as_str().to_string(),
            Arc::clone(act_registry),
        ));

        // execute_and_wait runs the action and waits for it to finish,
        // regardless of success or failure (task requirement)
        match executor
            .execute_and_wait(action.command, working_dir.clone(), listener)
            .await
        {
            Ok(_execution_id) => {
                count += 1;
            }
            Err(e) => {
                log::warn!(
                    "[project_mcp] prerun action '{}' failed (continuing): {e}",
                    action.id
                );
                count += 1; // count even if failed — we waited for it
            }
        }
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_clone_only_skips_on_a_clear_present() {
        use blox::BloxError;
        assert!(!needs_clone(Ok("present\n".into())).unwrap());
        assert!(needs_clone(Ok("absent\n".into())).unwrap());
        assert!(needs_clone(Ok(String::new())).unwrap());
        assert!(needs_clone(Err(BloxError::CommandFailed("not connected".into()))).unwrap());
        assert!(needs_clone(Err(BloxError::Timeout(60))).unwrap());
        assert!(matches!(
            needs_clone(Err(BloxError::NotAuthenticated)),
            Err(BloxError::NotAuthenticated)
        ));
        assert!(matches!(
            needs_clone(Err(BloxError::NotFound)),
            Err(BloxError::NotFound)
        ));
    }

    #[test]
    fn git_dir_probe_and_guarded_clone_respect_an_existing_repo() {
        let repo = crate::test_utils::TempGitRepo::new();
        let repo_path = repo.path().to_str().unwrap();
        let sh = |script: &str, args: &[&str]| {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(script)
                .arg("_")
                .args(args)
                .output()
                .unwrap()
        };

        let probe = sh(GIT_DIR_PROBE_SCRIPT, &[repo_path]);
        assert_eq!(String::from_utf8_lossy(&probe.stdout), "present\n");
        let missing = format!("{repo_path}/nope");
        let probe = sh(GIT_DIR_PROBE_SCRIPT, &[&missing]);
        assert_eq!(String::from_utf8_lossy(&probe.stdout), "absent\n");

        // An unreachable URL proves no clone runs over the existing repo.
        let clone = sh(
            GUARDED_CLONE_SCRIPT,
            &["/nonexistent/origin.git", repo_path],
        );
        assert!(clone.status.success(), "{clone:?}");
        let clone = sh(GUARDED_CLONE_SCRIPT, &["/nonexistent/origin.git", &missing]);
        assert!(!clone.status.success());
    }
    use crate::test_utils::TempGitRepo;

    #[test]
    fn listing_a_projects_branches_counts_their_commits() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();

        let listed = list_branches_for_project_impl(&store, &project.id).unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, branch.id);
        assert_eq!(listed[0].commit_count, Some(0));
    }

    #[test]
    fn listing_the_branches_of_an_unknown_project_is_an_error() {
        let store = Store::in_memory().unwrap();

        let error = list_branches_for_project_impl(&store, "no-such-project").unwrap_err();

        assert_eq!(error, "Project not found: no-such-project");
    }

    /// What the web and control `list_branches_for_project` arm returns: it
    /// calls this body directly (`web_project_list_arms_share_the_tauri_bodies`
    /// pins that), where it used to build the rows itself without commit
    /// counts. So a `stagedctl` or web caller now gets `commitCount` on every
    /// branch, as the desktop command always sent. Unlike the desktop
    /// command, the arm keeps answering an unknown project with an empty
    /// list: a control caller is scoped before it gets here, and the web
    /// client reads a project it has just seen deleted.
    #[test]
    fn the_shared_branch_list_counts_commits_and_lists_nothing_for_an_unknown_project() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let committed = store::Branch::new(&project.id, "committed", "main");
        store.create_branch(&committed).unwrap();
        store
            .create_workdir(
                &store::Workdir::new(&project.id, "/tmp/wt/committed").with_branch(&committed.id),
            )
            .unwrap();
        store
            .create_commit(&store::Commit::new_with_sha(&committed.id, "abc123"))
            .unwrap();
        let fresh = store::Branch::new(&project.id, "fresh", "main");
        store.create_branch(&fresh).unwrap();

        let listed = project_branches_with_workdirs(&store, &project.id).unwrap();

        assert_eq!(
            listed
                .iter()
                .map(|branch| (branch.id.as_str(), branch.commit_count))
                .collect::<Vec<_>>(),
            vec![
                (committed.id.as_str(), Some(1)),
                (fresh.id.as_str(), Some(0))
            ]
        );
        assert_eq!(
            listed[0].worktree_path.as_deref(),
            Some("/tmp/wt/committed")
        );
        // On the wire the count is `commitCount`, present even when zero.
        let wire = serde_json::to_value(&listed).unwrap();
        assert_eq!(wire[0]["commitCount"], 1);
        assert_eq!(wire[1]["commitCount"], 0);
        // The same rows the desktop command returns for a project it knows.
        assert_eq!(
            wire,
            serde_json::to_value(list_branches_for_project_impl(&store, &project.id).unwrap())
                .unwrap()
        );

        assert!(project_branches_with_workdirs(&store, "no-such-project")
            .unwrap()
            .is_empty());
    }

    /// A shared workstation outlives a teardown that leaves another branch
    /// on it, and goes when every branch using it does.
    #[test]
    fn a_workspace_outlives_a_teardown_only_while_another_branch_uses_it() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let first = store::Branch::new_remote(&project.id, "first", "main", "ws-1");
        let second = store::Branch::new_remote(&project.id, "second", "main", "ws-1");
        let other = store::Branch::new_remote(&project.id, "other", "main", "ws-2");
        let local = store::Branch::new(&project.id, "local", "main");
        for branch in [&first, &second, &other, &local] {
            store.create_branch(branch).unwrap();
        }

        assert!(workspace_outlives(&store, &first, &[first.id.as_str()]).unwrap());
        assert!(
            !workspace_outlives(&store, &first, &[first.id.as_str(), second.id.as_str()]).unwrap()
        );
        assert!(!workspace_outlives(&store, &other, &[other.id.as_str()]).unwrap());
        assert!(!workspace_outlives(&store, &local, &[local.id.as_str()]).unwrap());
    }

    /// The rule every remote read of a branch's HEAD rests on: a branch with
    /// a project repo is pinned to that repo's clone directory, so it can't
    /// answer about a sibling clone that happens to be the bare exec cwd.
    #[test]
    fn workspace_git_pins_a_branch_with_a_repo_to_its_clone_dir() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("squareup/g2");
        store.create_project(&project).unwrap();
        let repo = store::ProjectRepo::new(
            &project.id,
            "block/builderbot",
            "feature",
            Some("apps/staged".to_string()),
        );
        store.create_project_repo(&repo).unwrap();
        let branch = store::Branch::new_remote(&project.id, "feature", "main", "ws-1")
            .with_project_repo(&repo.id);

        let subpath = resolve_branch_workspace_subpath(&store, &branch)
            .unwrap()
            .unwrap();
        assert_eq!(
            workspace_git_args("ws-1", Some(&subpath), &["rev-parse", "HEAD"]).unwrap(),
            vec![
                "git",
                "-C",
                "/home/bloxer/builderbot/apps/staged",
                "rev-parse",
                "HEAD"
            ]
        );
    }

    /// A remote branch with a blank workspace name doesn't hide the name a
    /// later one carries: `None` would have the next added repo provision a
    /// second workspace for the project.
    #[test]
    fn existing_project_workspace_name_passes_over_a_blank_name() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("squareup/g2");
        store.create_project(&project).unwrap();
        let mut blank = store::Branch::new_remote(&project.id, "blank", "main", " ");
        blank.created_at -= 1_000;
        store.create_branch(&blank).unwrap();
        store
            .create_branch(&store::Branch::new_remote(
                &project.id,
                "feature",
                "main",
                "ws-1",
            ))
            .unwrap();

        assert_eq!(
            existing_project_workspace_name(&store, &project.id)
                .unwrap()
                .as_deref(),
            Some("ws-1")
        );
    }

    /// A branch with no project repo (a pre-Staged-repo branch) has no clone
    /// dir to resolve, so it runs bare — byte-identical to a plain `ws_exec`.
    #[test]
    fn workspace_git_runs_bare_without_a_repo() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("squareup/g2");
        store.create_project(&project).unwrap();
        let branch = store::Branch::new_remote(&project.id, "feature", "main", "ws-1");

        assert!(resolve_branch_workspace_subpath(&store, &branch)
            .unwrap()
            .is_none());
        assert_eq!(
            workspace_git_args("ws-1", None, &["rev-parse", "HEAD"]).unwrap(),
            vec!["git", "rev-parse", "HEAD"]
        );
    }

    #[test]
    fn local_session_working_dir_uses_project_repo_subpath_when_present() {
        let store = Store::in_memory().unwrap();
        let mut project = store::Project::new("block/builderbot");
        project.subpath = Some("apps/root".to_string());
        store.create_project(&project).unwrap();
        let repo = store::ProjectRepo::new(
            &project.id,
            "block/builderbot",
            "feature",
            Some("apps/staged".to_string()),
        );
        store.create_project_repo(&repo).unwrap();

        let branch = store::Branch::new(&project.id, "feature", "main").with_project_repo(&repo.id);
        let working_dir = local_branch_session_working_dir(
            &store,
            &project,
            &branch,
            PathBuf::from("/tmp/worktree"),
        );

        assert_eq!(working_dir, PathBuf::from("/tmp/worktree/apps/staged"));
    }

    #[test]
    fn local_session_working_dir_falls_back_to_project_subpath() {
        let store = Store::in_memory().unwrap();
        let mut project = store::Project::new("block/builderbot");
        project.subpath = Some("apps/staged".to_string());
        store.create_project(&project).unwrap();
        let branch = store::Branch::new(&project.id, "feature", "main");

        let working_dir = local_branch_session_working_dir(
            &store,
            &project,
            &branch,
            PathBuf::from("/tmp/worktree"),
        );

        assert_eq!(working_dir, PathBuf::from("/tmp/worktree/apps/staged"));
    }

    /// A session config's already-absolute `remote_working_dir` resolves to the
    /// same `-C` as the `home:` form, so the two runners can't disagree.
    #[test]
    fn workspace_git_accepts_an_already_resolved_remote_dir() {
        assert_eq!(
            workspace_git_args(
                "ws-1",
                Some("/home/bloxer/builderbot/apps/staged"),
                &["rev-parse", "HEAD"]
            )
            .unwrap(),
            workspace_git_args(
                "ws-1",
                Some("home:builderbot/apps/staged"),
                &["rev-parse", "HEAD"]
            )
            .unwrap()
        );
    }

    /// A monorepo repo is cloned, and its `.git` lives, at the repo root:
    /// `clone_repo_into_workspace` checks for and creates the clone at the
    /// clone dir, while sessions run in the subpath beneath it. Handing it the
    /// subpath made it miss the existing clone and `git clone` into a
    /// non-empty directory.
    #[test]
    fn clone_dir_is_the_repo_root_for_a_subpath_repo() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("block/builderbot");
        store.create_project(&project).unwrap();
        let repo = store::ProjectRepo::new(
            &project.id,
            "block/builderbot",
            "feature",
            Some("apps/staged".to_string()),
        );
        store.create_project_repo(&repo).unwrap();
        let branch = store::Branch::new_remote(&project.id, "feature", "main", "ws-1")
            .with_project_repo(&repo.id);

        let clone_dir = resolve_branch_clone_dir(&store, &branch).unwrap().unwrap();
        assert_eq!(clone_dir, "home:builderbot");
        assert_eq!(
            resolve_workspace_repo_path("ws-1", &clone_dir).unwrap(),
            "/home/bloxer/builderbot"
        );
        assert_eq!(
            resolve_branch_workspace_subpath(&store, &branch)
                .unwrap()
                .as_deref(),
            Some("home:builderbot/apps/staged")
        );
    }

    /// Without a subpath the clone dir and the session dir are the same.
    #[test]
    fn clone_dir_matches_the_workspace_subpath_without_a_subpath() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("squareup/g2");
        store.create_project(&project).unwrap();
        let repo = store::ProjectRepo::new(&project.id, "squareup/g2", "feature", None);
        store.create_project_repo(&repo).unwrap();
        let branch = store::Branch::new_remote(&project.id, "feature", "main", "ws-1")
            .with_project_repo(&repo.id);

        let clone_dir = resolve_branch_clone_dir(&store, &branch).unwrap();
        assert_eq!(clone_dir.as_deref(), Some("home:g2"));
        assert_eq!(
            resolve_branch_workspace_subpath(&store, &branch).unwrap(),
            clone_dir
        );
    }

    /// A branch with no project repo has no clone to check for or create.
    #[test]
    fn clone_dir_is_none_without_a_repo() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("squareup/g2");
        store.create_project(&project).unwrap();
        let branch = store::Branch::new_remote(&project.id, "feature", "main", "ws-1");

        assert!(resolve_branch_clone_dir(&store, &branch).unwrap().is_none());
    }

    #[test]
    fn apply_branch_prefix_joins_with_slash() {
        assert_eq!(
            apply_branch_prefix(Some("alice"), "my-project"),
            "alice/my-project"
        );
    }

    #[test]
    fn apply_branch_prefix_without_prefix_returns_branch_unchanged() {
        assert_eq!(apply_branch_prefix(None, "my-project"), "my-project");
    }

    const PROJECT: &str = "0140b9fd-3ce1-43d9-ae5d-9050d73c5bfc";

    /// Lowercase ASCII letters, digits and single `-`s, within the limit,
    /// not starting or ending on a `-`, with a readable part before the
    /// hash: what Blox, hostnames, paths and shell commands all take.
    fn assert_valid_workspace_name(name: &str, from: &str) {
        assert!(name.len() <= WORKSPACE_NAME_MAX_LENGTH, "{from}: {name}");
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "{from}: {name}"
        );
        assert!(
            !name.starts_with('-') && !name.ends_with('-'),
            "{from}: {name}"
        );
        assert!(!name.contains("--"), "{from}: {name}");
        let (readable, hash) = name.rsplit_once('-').expect(name);
        assert!(!readable.is_empty(), "{from}: {name}");
        assert_eq!(hash.len(), WORKSPACE_NAME_HASH_LENGTH, "{from}: {name}");
        assert!(
            hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "{from}: {name}"
        );
    }

    /// `{readable}-` plus the hex digits for `branch` in [`PROJECT`].
    fn expected(readable: &str, branch: &str) -> String {
        format!("{readable}-{}", workspace_name_hash(PROJECT, branch, 0))
    }

    #[test]
    fn infer_workspace_name_always_carries_the_project_hash() {
        assert_eq!(
            infer_workspace_name(PROJECT, "feature"),
            expected("feature", "feature")
        );
        assert_eq!(
            infer_workspace_name(PROJECT, "alice/my-project"),
            expected("alice-my-project", "alice/my-project")
        );
        assert_eq!(
            infer_workspace_name(PROJECT, "--a//b--"),
            expected("a-b", "--a//b--")
        );
        assert_eq!(infer_workspace_name(PROJECT, ""), expected("feature", ""));
        for branch in ["feature", "alice/my-project", "--a//b--", ""] {
            assert_valid_workspace_name(&infer_workspace_name(PROJECT, branch), branch);
        }
    }

    /// Two projects on one short branch name used to share `stg-feature`:
    /// their sessions ran in one checkout and the workstation belonged to
    /// both.
    #[test]
    fn infer_workspace_name_tells_apart_projects_on_the_same_branch() {
        let a = infer_workspace_name("project-a", "feature");
        let b = infer_workspace_name("project-b", "feature");
        assert_ne!(a, b);
        assert!(a.starts_with("feature-"), "{a}");
        assert!(b.starts_with("feature-"), "{b}");
        assert_ne!(
            infer_workspace_name("project-a", "agent-flows-point-nowhere-pr-1"),
            infer_workspace_name("project-b", "agent-flows-point-nowhere-pr-1"),
        );
    }

    #[test]
    fn infer_workspace_name_cuts_long_names_to_fit_the_hash() {
        // 25 kept + 1 (`-`) + 6 hex = 32.
        let name = infer_workspace_name(PROJECT, "a-very-long-branch-name-that-goes-on");
        assert_valid_workspace_name(&name, "long");
        assert_eq!(name.len(), WORKSPACE_NAME_MAX_LENGTH);
        assert!(name.starts_with("a-very-long-branch-name-t-"), "{name}");
        // The readable part keeps at most 25 bytes; 25 fit whole, 26 are cut.
        let fits = "a".repeat(25);
        assert_eq!(infer_workspace_name(PROJECT, &fits), expected(&fits, &fits));
        let over = "a".repeat(26);
        assert_eq!(infer_workspace_name(PROJECT, &over), expected(&fits, &over));
        // Never ends on a `-` the cut left before the suffix.
        let name = infer_workspace_name(PROJECT, "abcdefghijklmnopqrstuvwxy-zab-cd");
        assert_valid_workspace_name(&name, "cut on hyphen");
        assert!(name.starts_with("abcdefghijklmnopqrstuvwxy-"), "{name}");
        assert_eq!(name.len(), 25 + 1 + 6);
        let name = infer_workspace_name(PROJECT, "abcdefghijklmnopqrstuvwx-yzab-cd");
        assert_valid_workspace_name(&name, "cut lands on hyphen");
        assert!(name.starts_with("abcdefghijklmnopqrstuvwx-"), "{name}");
        assert_eq!(name.len(), 24 + 1 + 6);
    }

    /// The bug: three projects with branches `…-pr-1`, `…-pr-2`, `…-pr-3`
    /// all resolved to `stg-agent-flows-point-nowhere-pr` and shared one
    /// workstation.
    #[test]
    fn infer_workspace_name_tells_apart_branches_that_differ_past_the_cut() {
        let names: Vec<String> = (1..=3)
            .map(|n| {
                infer_workspace_name(
                    &format!("project-{n}"),
                    &format!("agent-flows-point-nowhere-pr-{n}"),
                )
            })
            .collect();
        for name in &names {
            assert_valid_workspace_name(name, "pr");
            assert!(name.starts_with("agent-flows-point-nowhere-"), "{name}");
        }
        assert_ne!(names[0], names[1]);
        assert_ne!(names[1], names[2]);
        assert_ne!(names[0], names[2]);
        // Even inside one project, two long branches that only differ past
        // the cut stay apart.
        assert_ne!(
            infer_workspace_name(PROJECT, "agent-flows-point-nowhere-pr-1"),
            infer_workspace_name(PROJECT, "agent-flows-point-nowhere-pr-2"),
        );
    }

    #[test]
    fn infer_workspace_name_is_deterministic() {
        let a = infer_workspace_name(PROJECT, "agent-flows-point-nowhere-pr-1");
        let b = infer_workspace_name(PROJECT, "agent-flows-point-nowhere-pr-1");
        assert_eq!(a, b);
        // Pinned so a change to the hash input or width is a deliberate one:
        // existing projects re-deriving their name must keep landing here.
        assert_eq!(a, "agent-flows-point-nowhere-2f4639");
        assert_eq!(infer_workspace_name(PROJECT, "feature"), "feature-d7e6f9");
    }

    /// A caller that finds the derived name taken asks for the next
    /// ordinal: same readable part, same shape, a different hash. Ordinal 0
    /// is the plain derivation, so stored names and pinned values hold.
    #[test]
    fn infer_workspace_name_nth_keeps_the_shape_across_ordinals() {
        for branch in ["feature", "agent-flows-point-nowhere-pr-1", "", "🚀"] {
            assert_eq!(
                infer_workspace_name_nth(PROJECT, branch, 0),
                infer_workspace_name(PROJECT, branch)
            );
            let base = infer_workspace_name(PROJECT, branch);
            let (readable, _) = base.rsplit_once('-').unwrap();
            let names: Vec<String> = (0..20)
                .map(|n| infer_workspace_name_nth(PROJECT, branch, n))
                .collect();
            for name in &names {
                assert_valid_workspace_name(name, branch);
                assert!(name.starts_with(&format!("{readable}-")), "{name}");
                assert_eq!(name.len(), readable.len() + 1 + WORKSPACE_NAME_HASH_LENGTH);
            }
            let distinct: std::collections::HashSet<&String> = names.iter().collect();
            assert_eq!(distinct.len(), names.len(), "{branch}: {names:?}");
        }
        // Pinned: the base is unchanged and the next candidate is not `-2`.
        assert_eq!(
            infer_workspace_name_nth(PROJECT, "agent-flows-point-nowhere-pr-1", 0),
            "agent-flows-point-nowhere-2f4639"
        );
        let second = infer_workspace_name_nth(PROJECT, "agent-flows-point-nowhere-pr-1", 1);
        assert!(second.starts_with("agent-flows-point-nowhere-"), "{second}");
        assert_eq!(second.len(), WORKSPACE_NAME_MAX_LENGTH);
        assert_ne!(second, "agent-flows-point-nowhere-2f4639");
    }

    #[test]
    fn infer_workspace_name_reduces_any_branch_name_to_lowercase_ascii() {
        assert_eq!(
            infer_workspace_name(PROJECT, "MT/Fix_Thing.v2"),
            expected("mt-fix-thing-v2", "MT/Fix_Thing.v2")
        );
        assert_eq!(
            infer_workspace_name(PROJECT, "café/naïve"),
            expected("caf-nave", "café/naïve")
        );
        // Nothing left: the default readable part, still hashed apart.
        assert_eq!(
            infer_workspace_name(PROJECT, "日本語/ブランチ"),
            expected("feature", "日本語/ブランチ")
        );
        assert_eq!(
            infer_workspace_name(PROJECT, "🚀"),
            expected("feature", "🚀")
        );
        assert_ne!(
            infer_workspace_name(PROJECT, "日本語/ブランチ"),
            infer_workspace_name(PROJECT, "🚀")
        );
    }

    /// A long branch name with a multibyte character across byte 32 used to
    /// panic slicing `full[..32]`, failing a transfer to Blox.
    #[test]
    fn infer_workspace_name_never_panics_on_long_unicode_names() {
        // 27 ASCII bytes put `é` (2 bytes) at bytes 28–29 of the sanitized
        // name, and `🚀` (4 bytes) at 28–31: both across the 25-byte cut.
        let ascii = "a".repeat(27);
        for name in [
            format!("{ascii}é-and-more"),
            format!("{ascii}🚀-and-more"),
            "ü".repeat(40),
            format!("{}-{}", "分支".repeat(20), "x".repeat(40)),
            "🚀".repeat(16),
        ] {
            let inferred = infer_workspace_name(PROJECT, &name);
            assert_valid_workspace_name(&inferred, &name);
        }
        // `é` is dropped; the kept part stops at 25 `a`s before the suffix.
        let inferred = infer_workspace_name(PROJECT, &format!("{ascii}é-and-more"));
        assert!(
            inferred.starts_with(&format!("{}-", "a".repeat(25))),
            "{inferred}"
        );
        assert_eq!(inferred.len(), WORKSPACE_NAME_MAX_LENGTH);
        let umlauts = "ü".repeat(40);
        assert_eq!(
            infer_workspace_name(PROJECT, &umlauts),
            expected("feature", &umlauts)
        );
    }

    #[test]
    fn truncate_workspace_name_cuts_on_a_character_boundary() {
        // `é` is bytes 3–4: cutting at 4 would split it.
        assert_eq!(truncate_workspace_name("abcé", 4), "abc");
        assert_eq!(truncate_workspace_name("abcé", 5), "abcé");
        assert_eq!(truncate_workspace_name("ab🚀", 5), "ab");
        assert_eq!(truncate_workspace_name("ab-🚀", 5), "ab");
        assert_eq!(truncate_workspace_name("abc", 10), "abc");
        assert_eq!(truncate_workspace_name("🚀", 3), "");
    }

    #[test]
    fn normalize_branch_prefix_sanitizes_like_project_names() {
        assert_eq!(
            normalize_branch_prefix("Alice Doe"),
            Some("alice-doe".to_string())
        );
        assert_eq!(
            normalize_branch_prefix("branch.lock"),
            Some("branch-lock".to_string())
        );
        assert_eq!(
            normalize_branch_prefix("~fix..up"),
            Some("fix-up".to_string())
        );
    }

    #[test]
    fn normalize_branch_prefix_preserves_multi_level_prefixes() {
        assert_eq!(
            normalize_branch_prefix("team/alice"),
            Some("team/alice".to_string())
        );
    }

    #[test]
    fn normalize_branch_prefix_drops_empty_segments() {
        assert_eq!(normalize_branch_prefix("alice/"), Some("alice".to_string()));
        assert_eq!(normalize_branch_prefix("/alice"), Some("alice".to_string()));
        assert_eq!(
            normalize_branch_prefix("foo//bar"),
            Some("foo/bar".to_string())
        );
    }

    #[test]
    fn normalize_branch_prefix_rejects_prefixes_with_nothing_valid() {
        assert_eq!(normalize_branch_prefix(""), None);
        assert_eq!(normalize_branch_prefix("  "), None);
        assert_eq!(normalize_branch_prefix("///"), None);
        assert_eq!(normalize_branch_prefix("~/.."), None);
    }

    #[test]
    fn local_base_ref_for_worktree_accepts_existing_origin_ref() {
        let repo = TempGitRepo::new();
        repo.write_file("README.md", "hello");
        repo.commit("initial");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);

        assert_eq!(
            local_base_ref_for_worktree(repo.path(), "origin/main"),
            Some("origin/main".to_string())
        );
        assert_eq!(
            local_base_ref_for_worktree(repo.path(), "main"),
            Some("origin/main".to_string())
        );
    }

    #[test]
    fn local_base_ref_for_worktree_rejects_missing_origin_ref() {
        let repo = TempGitRepo::new();
        repo.write_file("README.md", "hello");
        repo.commit("initial");

        assert_eq!(
            local_base_ref_for_worktree(repo.path(), "origin/main"),
            None
        );
    }

    #[test]
    fn fetch_offline_fallback_allows_worktree_creation_from_existing_origin_ref() {
        let repo = TempGitRepo::new();
        repo.write_file("README.md", "hello");
        repo.commit("initial");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);

        let github_repo = "owner/repo";
        let https_url = format!("https://github.com/{github_repo}.git");
        let missing_origin = repo.path().join("missing-origin");
        let missing_url = format!("file://{}", missing_origin.display());
        repo.run_git(&["remote", "add", "origin", &https_url]);
        repo.run_git(&[
            "config",
            "--add",
            &format!("url.{missing_url}.insteadOf"),
            &https_url,
        ]);

        fetch_for_worktree_with_offline_fallback(
            repo.path(),
            github_repo,
            "feature/offline",
            "origin/main",
        )
        .expect("existing origin/main should allow offline fallback");

        let worktree_parent = tempfile::tempdir().expect("worktree tempdir");
        let worktree_path = worktree_parent.path().join("feature-offline");
        let created = create_worktree_with_fallback(
            repo.path(),
            "feature/offline",
            "origin/main",
            &worktree_path,
            None,
        )
        .expect("worktree should be created from stale origin/main");

        assert_eq!(created, worktree_path);
        assert!(created.join("README.md").is_file());
    }

    #[test]
    fn fetch_offline_fallback_rejects_deleted_remote_base_ref() {
        let remote = TempGitRepo::new();
        remote.write_file("README.md", "hello from remote");
        remote.commit("initial");

        let repo = TempGitRepo::new();
        repo.write_file("README.md", "hello");
        repo.commit("initial");
        repo.run_git(&["update-ref", "refs/remotes/origin/deleted-base", "HEAD"]);

        let github_repo = "owner/repo";
        let https_url = format!("https://github.com/{github_repo}.git");
        let remote_url = format!("file://{}", remote.path().display());
        repo.run_git(&["remote", "add", "origin", &remote_url]);
        repo.run_git(&[
            "config",
            "--add",
            &format!("url.{remote_url}.insteadOf"),
            &https_url,
        ]);

        let err = fetch_for_worktree_with_offline_fallback(
            repo.path(),
            github_repo,
            "feature/offline",
            "origin/deleted-base",
        )
        .expect_err("missing remote base ref should not use stale origin/deleted-base");

        assert!(
            is_missing_remote_ref_fetch_error(&err),
            "expected missing remote ref error, got: {err}"
        );
    }

    // ── plan_branch_move ────────────────────────────────────────────────────

    struct MoveFixture {
        store: Arc<Store>,
        source: store::Project,
        target: store::Project,
        repo: store::ProjectRepo,
        branch: store::Branch,
    }

    fn move_fixture() -> MoveFixture {
        let store = Arc::new(Store::in_memory().unwrap());
        let source = store::Project::named("source").with_primary_repo("acme/widgets");
        let target = store::Project::named("target");
        store.create_project(&source).unwrap();
        store.create_project(&target).unwrap();

        let repo = store::ProjectRepo::new(&source.id, "acme/widgets", "feature", None).primary();
        store.create_project_repo(&repo).unwrap();
        let branch =
            store::Branch::new(&source.id, "feature", "origin/main").with_project_repo(&repo.id);
        store.create_branch(&branch).unwrap();

        MoveFixture {
            store,
            source,
            target,
            repo,
            branch,
        }
    }

    #[test]
    fn plan_move_carries_a_sole_branch_repo_row_across() {
        let f = move_fixture();

        let plan = plan_branch_move(&f.store, &f.branch.id, &f.target.id).unwrap();

        match &plan.mv.repo {
            store::RepoPlacement::Reparent { repo_id } => assert_eq!(repo_id, &f.repo.id),
            other => panic!("expected the repo row to travel, got {other:?}"),
        }
        assert_eq!(plan.github_repo, "acme/widgets");
        // Nothing on disk yet, so there is no worktree to relocate.
        assert!(plan.mv.workdir.is_none());
    }

    #[test]
    fn plan_move_clones_a_repo_row_a_sibling_branch_shares() {
        let f = move_fixture();
        let sibling =
            store::Branch::new(&f.source.id, "other", "origin/main").with_project_repo(&f.repo.id);
        f.store.create_branch(&sibling).unwrap();

        let plan = plan_branch_move(&f.store, &f.branch.id, &f.target.id).unwrap();

        match plan.mv.repo {
            store::RepoPlacement::Clone(repo) => {
                assert_ne!(repo.id, f.repo.id);
                assert_eq!(repo.github_repo, "acme/widgets");
                assert_eq!(repo.project_id, f.target.id);
            }
            other => panic!("expected a cloned repo row, got {other:?}"),
        }
    }

    /// A NULL `project_repo_id` would resolve to the *destination's* primary
    /// repo after the move, so the plan materializes a row instead.
    #[test]
    fn plan_move_materializes_a_row_for_a_branch_with_no_repo_link() {
        let f = move_fixture();
        let legacy = store::Branch::new(&f.source.id, "legacy", "origin/main");
        f.store.create_branch(&legacy).unwrap();

        let plan = plan_branch_move(&f.store, &legacy.id, &f.target.id).unwrap();

        match plan.mv.repo {
            store::RepoPlacement::Clone(repo) => {
                assert_eq!(repo.github_repo, "acme/widgets");
                assert_eq!(repo.branch_name, "legacy");
            }
            other => panic!("expected a materialized repo row, got {other:?}"),
        }
    }

    #[test]
    fn plan_move_relocates_the_worktree_into_the_destination_project() {
        let f = move_fixture();
        let workdir = store::Workdir::new(&f.source.id, "/wt/old/path").with_branch(&f.branch.id);
        f.store.create_workdir(&workdir).unwrap();

        let plan = plan_branch_move(&f.store, &f.branch.id, &f.target.id).unwrap();

        let wd = plan.mv.workdir.expect("worktree should be relocated");
        assert_eq!(wd.workdir_id, workdir.id);
        // The old path comes from the row, not a recomputed one — legacy-layout
        // worktrees move from wherever they actually are.
        assert_eq!(wd.old_path, "/wt/old/path");
        let expected = git::project_worktree_path_for(&f.target.id, "acme/widgets", "feature")
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(wd.new_path, expected);
    }

    #[test]
    fn plan_move_rejects_a_remote_project_on_either_end() {
        let f = move_fixture();
        let mut remote = store::Project::named("remote-target");
        remote.location = store::ProjectLocation::Remote;
        f.store.create_project(&remote).unwrap();

        let err = plan_branch_move(&f.store, &f.branch.id, &remote.id).unwrap_err();
        assert!(err.contains("remote workspace"), "unexpected error: {err}");

        // …and the same when the branch is leaving a remote project.
        let remote_repo =
            store::ProjectRepo::new(&remote.id, "acme/other", "feature", None).primary();
        f.store.create_project_repo(&remote_repo).unwrap();
        let remote_branch = store::Branch::new(&remote.id, "feature", "origin/main")
            .with_project_repo(&remote_repo.id);
        f.store.create_branch(&remote_branch).unwrap();

        let err = plan_branch_move(&f.store, &remote_branch.id, &f.target.id).unwrap_err();
        assert!(err.contains("remote workspace"), "unexpected error: {err}");
    }

    #[test]
    fn plan_move_rejects_a_branch_with_a_session_running_on_it() {
        let f = move_fixture();
        let session = store::Session::new_running("work", Path::new("/wt/old/path"))
            .with_branch(&f.branch.id);
        f.store.create_session(&session).unwrap();

        let err = plan_branch_move(&f.store, &f.branch.id, &f.target.id).unwrap_err();

        assert!(
            err.contains("session is running"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn plan_move_rejects_a_destination_that_already_has_the_repo() {
        let f = move_fixture();
        f.store
            .create_project_repo(&store::ProjectRepo::new(
                &f.target.id,
                "acme/widgets",
                "main",
                None,
            ))
            .unwrap();

        let err = plan_branch_move(&f.store, &f.branch.id, &f.target.id).unwrap_err();

        assert!(
            err.contains("already has acme/widgets"),
            "unexpected error: {err}"
        );
    }

    /// `idx_project_repos_unique` coalesces the subpath, so a NULL subpath and
    /// an empty one are the same repo — the check has to agree.
    #[test]
    fn plan_move_treats_a_null_and_an_empty_subpath_as_the_same_repo() {
        let f = move_fixture();
        f.store
            .create_project_repo(&store::ProjectRepo::new(
                &f.target.id,
                "acme/widgets",
                "main",
                Some(String::new()),
            ))
            .unwrap();

        let err = plan_branch_move(&f.store, &f.branch.id, &f.target.id).unwrap_err();

        assert!(
            err.contains("already has acme/widgets"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn plan_move_rejects_a_move_into_the_branchs_own_project() {
        let f = move_fixture();

        let err = plan_branch_move(&f.store, &f.branch.id, &f.source.id).unwrap_err();

        assert!(
            err.contains("already in that project"),
            "unexpected error: {err}"
        );
    }
}

/// Worktree de-duplication for PR imports: the local name may move, the
/// branch on origin — carried by git's upstream tracking — must not.
#[cfg(test)]
mod upstream_tracking_tests {
    use super::*;
    use crate::test_utils::{run_git_in as git, TempDir, TempGitRepo};

    /// A bare origin with `main` and `feature-x`, the latter also PR #7's
    /// head (`refs/pull/7/head`).
    fn bare_origin() -> TempDir {
        let seed = TempGitRepo::new();
        seed.write_file("file.txt", "base\n");
        seed.commit("base");
        seed.run_git(&["checkout", "-b", "feature-x"]);
        seed.write_file("file.txt", "base\nfeature\n");
        let feature_sha = seed.commit("feature");
        seed.run_git(&["checkout", "main"]);

        let origin = TempDir::new("origin");
        git(
            seed.path(),
            &["clone", "--bare", ".", origin.path().to_str().unwrap()],
        );
        git(
            origin.path(),
            &["update-ref", "refs/pull/7/head", feature_sha.trim()],
        );
        origin
    }

    /// `git clone <clone_args> <origin>`, with a commit identity.
    fn clone_of(origin: &TempDir, clone_args: &[&str]) -> TempDir {
        let clone = TempDir::new("clone");
        let mut args = vec!["clone"];
        args.extend_from_slice(clone_args);
        args.push(origin.path().to_str().unwrap());
        args.push(clone.path().to_str().unwrap());
        git(origin.path(), &args);
        git(clone.path(), &["config", "user.email", "test@example.com"]);
        git(clone.path(), &["config", "user.name", "Test"]);
        clone
    }

    /// A full clone of [`bare_origin`]. With `stale`, the clone also has a
    /// worktree in a directory outside any Staged project root that already
    /// has `feature-x` checked out — what an earlier project's PR import
    /// leaves behind.
    ///
    /// All three directories must stay alive for the clone's worktree list
    /// to resolve.
    fn clone_of_origin(stale: bool) -> (TempDir, TempDir, Option<TempDir>) {
        let origin = bare_origin();
        let clone = clone_of(&origin, &[]);

        let stale_dir = stale.then(|| {
            let stale = TempDir::new("stale-worktree");
            let stale_path = stale.path().join("feature-x");
            git(
                clone.path(),
                &[
                    "worktree",
                    "add",
                    stale_path.to_str().unwrap(),
                    "-b",
                    "feature-x",
                    "origin/feature-x",
                ],
            );
            stale
        });
        (origin, clone, stale_dir)
    }

    fn clone_with_stale_feature_x() -> (TempDir, TempDir, TempDir) {
        let (origin, clone, stale) = clone_of_origin(true);
        (origin, clone, stale.unwrap())
    }

    /// The clone a real PR import works in: only `main` was ever fetched, the
    /// PR arrived as `refs/pull/7/head`, and an earlier import's stale
    /// worktree holds `feature-x` — so there is no `origin/feature-x` for
    /// the new checkout to track until something fetches it.
    fn single_branch_clone_with_stale_feature_x() -> (TempDir, TempDir, TempDir) {
        let origin = bare_origin();
        let clone = clone_of(&origin, &["--single-branch", "-b", "main"]);
        git(
            clone.path(),
            &["fetch", "origin", "+refs/pull/7/head:refs/staged/pr/7"],
        );
        let stale = TempDir::new("stale-worktree");
        let stale_path = stale.path().join("feature-x");
        git(
            clone.path(),
            &[
                "worktree",
                "add",
                stale_path.to_str().unwrap(),
                "-b",
                "feature-x",
                "refs/staged/pr/7",
            ],
        );
        assert!(!git::remote_branch_exists(clone.path(), "feature-x").unwrap());
        (origin, clone, stale)
    }

    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app")
    }

    /// [`create_and_link_worktree`] under a mock app with no kicker managed,
    /// for the tests that are about the worktree rather than the queue.
    fn link_worktree(
        store: &Arc<Store>,
        branch: &store::Branch,
        repo_path: &Path,
        desired_worktree_path: &Path,
    ) -> Result<String, String> {
        let app = mock_app();
        create_and_link_worktree(
            store,
            app.handle(),
            branch,
            repo_path,
            desired_worktree_path,
        )
    }

    fn store_with_project() -> (Arc<Store>, store::Project) {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = store::Project::new("acme/widgets");
        store.create_project(&project).unwrap();
        (store, project)
    }

    fn merge_config(repo: &Path, local: &str) -> String {
        git(
            repo,
            &["config", "--default", "", &format!("branch.{local}.merge")],
        )
    }

    #[test]
    fn a_worktree_outside_the_project_root_is_stale_not_found() {
        let (_origin, clone, _stale) = clone_with_stale_feature_x();
        let project_root = TempDir::new("project-root");

        assert!(matches!(
            find_existing_worktree_for_branch(clone.path(), "feature-x", Some(project_root.path())),
            Ok(ExistingWorktree::Stale)
        ));
        assert!(matches!(
            find_existing_worktree_for_branch(clone.path(), "feature-x", None),
            Ok(ExistingWorktree::Found(_))
        ));
        assert!(matches!(
            find_existing_worktree_for_branch(clone.path(), "feature-y", Some(project_root.path())),
            Ok(ExistingWorktree::None)
        ));
    }

    #[test]
    fn unique_branch_name_skips_taken_suffixes() {
        let (_origin, clone, _stale) = clone_with_stale_feature_x();
        assert_eq!(
            unique_branch_name(clone.path(), "feature-x").unwrap(),
            "feature-x-2"
        );

        git(clone.path(), &["branch", "feature-x-2", "origin/main"]);
        assert_eq!(
            unique_branch_name(clone.path(), "feature-x").unwrap(),
            "feature-x-3"
        );
    }

    /// [`check_out_worktree`] for a project whose worktrees live under
    /// `project_root`, on a branch started from `origin/main` when new.
    fn check_out(
        clone: &Path,
        branch: &str,
        project_root: &Path,
    ) -> Result<CheckedOutWorktree, String> {
        check_out_worktree(
            clone,
            branch,
            None,
            "origin/main",
            &project_root.join(branch),
            Some(project_root),
        )
    }

    fn head_branch(worktree: &Path) -> String {
        git(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
    }

    /// Found live, and then again in review: two projects created seconds
    /// apart on the same branch of one clone. Each setup looked the name up
    /// before the other had created it. The loser reached `worktree add -b`
    /// with a name the winner had taken, and could not tell the winner's
    /// branch from a leftover of its own failed attempt.
    ///
    /// The lookup and the creation are one step per clone now. However the
    /// setups are scheduled, each gets a worktree, on a local name nobody
    /// else has, at the branch they all asked for.
    #[test]
    fn projects_setting_up_the_same_branch_at_once_get_a_worktree_each() {
        const PROJECTS: usize = 6;
        let (_origin, clone, _) = clone_of_origin(false);
        let roots: Vec<TempDir> = (0..PROJECTS)
            .map(|_| TempDir::new("project-root"))
            .collect();
        let start = std::sync::Barrier::new(PROJECTS);

        let checked_out: Vec<CheckedOutWorktree> = std::thread::scope(|scope| {
            let setups: Vec<_> = roots
                .iter()
                .map(|root| {
                    let (clone, start) = (clone.path(), &start);
                    scope.spawn(move || {
                        start.wait();
                        check_out(clone, "feature-x", root.path())
                    })
                })
                .collect();
            setups
                .into_iter()
                .map(|setup| setup.join().unwrap().expect("every setup gets a worktree"))
                .collect()
        });

        let mut names: Vec<&str> = checked_out
            .iter()
            .map(|worktree| worktree.local_branch_name.as_str())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "feature-x",
                "feature-x-2",
                "feature-x-3",
                "feature-x-4",
                "feature-x-5",
                "feature-x-6"
            ]
        );
        let requested = git(clone.path(), &["rev-parse", "origin/feature-x"]);
        for (worktree, root) in checked_out.iter().zip(&roots) {
            assert!(worktree.created_branch, "{}", worktree.local_branch_name);
            assert!(!worktree.adopted_existing, "{}", worktree.local_branch_name);
            assert!(worktree.path.starts_with(root.path()));
            assert_eq!(head_branch(&worktree.path), worktree.local_branch_name);
            assert_eq!(git(&worktree.path, &["rev-parse", "HEAD"]), requested);
        }
    }

    /// The same race with its timing pinned: a setup that arrives while
    /// another holds the clone does nothing until that one is done, and then
    /// looks the name up, so it sees what the other made. A lookup made
    /// before the wait would have found the name free.
    #[test]
    fn a_setup_looks_the_name_up_only_once_it_has_the_clone() {
        let (_origin, clone, _) = clone_of_origin(false);
        let winner_root = TempDir::new("winner-root");
        let loser_root = TempDir::new("loser-root");
        let lock = clone_worktree_lock_for(clone.path());
        let winner = lock.lock().unwrap();

        std::thread::scope(|scope| {
            let (done, finished) = std::sync::mpsc::channel();
            let (clone_path, loser_path) = (clone.path(), loser_root.path());
            scope.spawn(move || {
                done.send(check_out(clone_path, "feature-x", loser_path))
                    .unwrap();
            });

            // The loser is held off while the winner works.
            assert!(
                finished
                    .recv_timeout(std::time::Duration::from_millis(400))
                    .is_err(),
                "a setup must wait for the clone's worktree lock"
            );
            assert!(!git::branch_exists(clone.path(), "feature-x").unwrap());
            assert!(!loser_root.path().join("feature-x").exists());

            // The winner creates the branch and its worktree, as its own
            // `git worktree add -b` does, and lets go.
            let winner_path = winner_root.path().join("feature-x");
            git(
                clone.path(),
                &[
                    "worktree",
                    "add",
                    winner_path.to_str().unwrap(),
                    "-b",
                    "feature-x",
                    "origin/feature-x",
                ],
            );
            drop(winner);

            let loser = finished
                .recv_timeout(std::time::Duration::from_secs(120))
                .expect("the waiting setup runs once the clone is free")
                .unwrap();
            assert_eq!(loser.local_branch_name, "feature-x-2");
            assert!(loser.created_branch);
            assert!(!loser.adopted_existing);
            assert_eq!(head_branch(&loser.path), "feature-x-2");
            // The winner's checkout is untouched.
            assert_eq!(head_branch(&winner_path), "feature-x");
        });
    }

    /// One clone is one lock however its path is spelled, and two clones
    /// never wait on each other.
    #[test]
    fn the_worktree_lock_is_per_clone() {
        let (_origin, clone, _) = clone_of_origin(false);
        let (_other_origin, other_clone, _) = clone_of_origin(false);

        let lock = clone_worktree_lock_for(clone.path());
        let respelled = clone_worktree_lock_for(&clone.path().join("."));
        let other = clone_worktree_lock_for(other_clone.path());

        assert!(Arc::ptr_eq(&lock, &respelled));
        assert!(!Arc::ptr_eq(&lock, &other));
        let _held = lock.lock().unwrap();
        assert!(other.try_lock().is_ok());
    }

    /// [`check_out_worktree_with`] for pull request #7's branch, in a
    /// project whose worktrees live under `project_root`.
    fn check_out_pr(
        clone: &Path,
        project_root: &Path,
        fetch_pr_head_sha: &dyn Fn(&Path, u64) -> Result<String, String>,
    ) -> Result<CheckedOutWorktree, String> {
        check_out_worktree_with(
            clone,
            "feature-x",
            Some(7),
            "origin/main",
            &project_root.join("feature-x"),
            Some(project_root),
            fetch_pr_head_sha,
        )
    }

    fn fetch_pr_head(repo: &Path, pr_number: u64) -> Result<String, String> {
        git::fetch_pr_head_sha(repo, pr_number).map_err(|e| e.to_string())
    }

    /// The pull request's head comes over the network, and the clone's lock
    /// is not held while it does: a setup of another branch in the same
    /// clone runs to the end in the middle of the fetch.
    #[test]
    fn a_pull_request_head_is_fetched_with_the_clone_free() {
        let (_origin, clone, _) = clone_of_origin(false);
        let project_root = TempDir::new("project-root");
        let other_root = TempDir::new("other-root");
        let fetches = std::cell::Cell::new(0);

        let checked_out = check_out_pr(clone.path(), project_root.path(), &|repo, pr_number| {
            fetches.set(fetches.get() + 1);
            assert_eq!(pr_number, 7);
            assert!(
                clone_worktree_lock_for(repo).try_lock().is_ok(),
                "the clone's worktree lock is held across the fetch"
            );
            // Nothing was created ahead of the fetch.
            assert!(!git::branch_exists(repo, "feature-x").unwrap());
            // Another project sets up another branch meanwhile. It would
            // wait here for good if the fetch held the lock.
            let other = check_out(repo, "feature-y", other_root.path()).unwrap();
            assert_eq!(head_branch(&other.path), "feature-y");
            fetch_pr_head(repo, pr_number)
        })
        .unwrap();

        assert_eq!(fetches.get(), 1);
        assert_eq!(checked_out.local_branch_name, "feature-x");
        assert!(checked_out.created_branch && !checked_out.adopted_existing);
        assert_eq!(head_branch(&checked_out.path), "feature-x");
        assert_eq!(
            git(&checked_out.path, &["rev-parse", "HEAD"]),
            git(clone.path(), &["rev-parse", "refs/staged/pr/7"])
        );
    }

    /// The race stays closed with the lock let go for the fetch. Another
    /// project takes the name while the head is being fetched. The name was
    /// free when this setup first looked, and it looks again once it has the
    /// lock back, so it sees the name held and takes a unique one. Acting on
    /// the first lookup, it would have run `git worktree add -b` with a name
    /// another worktree holds.
    #[test]
    fn a_name_taken_during_the_fetch_is_seen_when_the_lock_is_retaken() {
        let (_origin, clone, _) = clone_of_origin(false);
        let project_root = TempDir::new("project-root");
        let other_root = TempDir::new("other-root");
        let other_path = std::cell::RefCell::new(None);

        let checked_out = check_out_pr(clone.path(), project_root.path(), &|repo, pr_number| {
            let other = check_out(repo, "feature-x", other_root.path()).unwrap();
            assert_eq!(other.local_branch_name, "feature-x");
            assert!(other.created_branch);
            *other_path.borrow_mut() = Some(other.path);
            fetch_pr_head(repo, pr_number)
        })
        .unwrap();

        assert_eq!(checked_out.local_branch_name, "feature-x-2");
        assert!(checked_out.created_branch && !checked_out.adopted_existing);
        assert!(checked_out.path.starts_with(project_root.path()));
        assert_eq!(head_branch(&checked_out.path), "feature-x-2");
        assert_eq!(
            git(&checked_out.path, &["rev-parse", "HEAD"]),
            git(clone.path(), &["rev-parse", "refs/staged/pr/7"])
        );
        // The other project's checkout is untouched.
        let other_path = other_path.into_inner().unwrap();
        assert_eq!(head_branch(&other_path), "feature-x");
    }

    /// Two projects set up the same pull request at once, in a clone that
    /// fetched its head before the pull request moved, so
    /// `refs/staged/pr/7` is there at an older commit. Both setups fetch
    /// into that ref with the clone's lock free. At the same time, both git
    /// processes read the old value, and the second to write is refused:
    /// `cannot lock ref 'refs/staged/pr/7': is at <new> but expected <old>`.
    /// The fetches take turns instead, and both setups come out at the new
    /// head.
    #[test]
    fn two_setups_of_one_pull_request_fetch_its_head_in_turn() {
        let (origin, clone, _) = clone_of_origin(false);
        let stale_head = fetch_pr_head(clone.path(), 7).unwrap();
        // The pull request gains a commit on origin.
        let tree = git(origin.path(), &["rev-parse", "refs/pull/7/head^{tree}"]);
        let new_head = git(
            origin.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit-tree",
                &tree,
                "-p",
                &stale_head,
                "-m",
                "more",
            ],
        );
        git(
            origin.path(),
            &["update-ref", "refs/pull/7/head", &new_head],
        );
        assert_ne!(stale_head, new_head);

        let roots = [TempDir::new("first-root"), TempDir::new("second-root")];
        // (setups inside the fetch now, the most there ever were, arrivals)
        let fetching = Mutex::new((0_u32, 0_u32, 0_u32));
        let arrived = std::sync::Condvar::new();
        let fetch_together = |repo: &Path, pr_number: u64| {
            {
                let mut state = fetching.lock().unwrap();
                state.0 += 1;
                state.1 = state.1.max(state.0);
                state.2 += 1;
                arrived.notify_all();
                // Give the other setup the time to get here too, so that
                // two fetches nothing keeps apart start together. One that
                // is kept out never arrives, and the wait runs out.
                let _both = arrived
                    .wait_timeout_while(state, std::time::Duration::from_millis(500), |state| {
                        state.2 < 2
                    })
                    .unwrap();
            }
            let fetched = fetch_pr_head(repo, pr_number);
            fetching.lock().unwrap().0 -= 1;
            fetched
        };

        let checked_out: Vec<_> = std::thread::scope(|scope| {
            let setups: Vec<_> = roots
                .iter()
                .map(|root| {
                    let (clone_path, fetch) = (clone.path(), &fetch_together);
                    scope.spawn(move || check_out_pr(clone_path, root.path(), fetch))
                })
                .collect();
            setups.into_iter().map(|s| s.join().unwrap()).collect()
        });

        let mut names = Vec::new();
        for (worktree, root) in checked_out.into_iter().zip(&roots) {
            let worktree = worktree.expect("both setups fetch the pull request's head");
            assert!(worktree.created_branch && !worktree.adopted_existing);
            assert!(worktree.path.starts_with(root.path()));
            assert_eq!(git(&worktree.path, &["rev-parse", "HEAD"]), new_head);
            names.push(worktree.local_branch_name);
        }
        names.sort();
        assert_eq!(names, ["feature-x", "feature-x-2"]);
        assert_eq!(
            git(clone.path(), &["rev-parse", "refs/staged/pr/7"]),
            new_head
        );
        let (now, most, arrivals) = *fetching.lock().unwrap();
        assert_eq!((now, arrivals), (0, 2));
        assert_eq!(most, 1, "two fetches of one pull request's head overlapped");
    }

    /// The fetch lock is for one pull request in one clone, however the
    /// clone's path is spelled. Another pull request, another clone and the
    /// clone's worktree lock are all free while it is held.
    #[test]
    fn the_pr_head_fetch_lock_is_per_clone_and_pull_request() {
        let (_origin, clone, _) = clone_of_origin(false);
        let (_other_origin, other_clone, _) = clone_of_origin(false);

        let lock = pr_head_fetch_lock_for(clone.path(), 7);
        let respelled = pr_head_fetch_lock_for(&clone.path().join("."), 7);
        let other_pr = pr_head_fetch_lock_for(clone.path(), 8);
        let other_clone = pr_head_fetch_lock_for(other_clone.path(), 7);

        assert!(Arc::ptr_eq(&lock, &respelled));
        assert!(!Arc::ptr_eq(&lock, &other_pr));
        assert!(!Arc::ptr_eq(&lock, &other_clone));
        let _held = lock.lock().unwrap();
        assert!(other_pr.try_lock().is_ok());
        assert!(other_clone.try_lock().is_ok());
        assert!(clone_worktree_lock_for(clone.path()).try_lock().is_ok());
    }

    /// The head is fetched only for a branch that has to be created, as
    /// before the lock was let go for it. A setup that finds its worktree or
    /// its local branch reaches for nothing over the network, so it works
    /// offline as it did.
    #[test]
    fn a_pull_request_setup_with_nothing_to_create_fetches_nothing() {
        let (_origin, clone, _) = clone_of_origin(false);
        let project_root = TempDir::new("project-root");
        let root = &project_root.path().canonicalize().unwrap();
        let no_fetch = |_: &Path, _: u64| -> Result<String, String> {
            panic!("nothing to create, so nothing to fetch")
        };

        // The local branch exists and no worktree holds it.
        git(clone.path(), &["branch", "feature-x", "origin/feature-x"]);
        let existing = check_out_pr(clone.path(), root, &no_fetch).unwrap();
        assert!(!existing.created_branch && !existing.adopted_existing);
        assert_eq!(head_branch(&existing.path), "feature-x");

        // This project's own worktree for the branch is there.
        let again = check_out_pr(clone.path(), root, &no_fetch).unwrap();
        assert_eq!(again.path, existing.path);
        assert!(again.adopted_existing && !again.created_branch);
    }

    /// A fetch that fails is the setup's error, and leaves nothing behind:
    /// the pass that found the head missing created nothing.
    #[test]
    fn a_failed_pull_request_fetch_creates_nothing() {
        let (_origin, clone, _) = clone_of_origin(false);
        let project_root = TempDir::new("project-root");

        let err = match check_out_pr(clone.path(), project_root.path(), &|_, _| {
            Err("could not reach origin".to_string())
        }) {
            Ok(checked_out) => panic!("checked out at {}", checked_out.path.display()),
            Err(err) => err,
        };

        assert_eq!(err, "could not reach origin");
        assert!(!git::branch_exists(clone.path(), "feature-x").unwrap());
        assert!(!project_root.path().join("feature-x").exists());
        assert!(clone_worktree_lock_for(clone.path()).try_lock().is_ok());
    }

    /// Undoing a failed setup removes the worktree and then deletes the
    /// branch. Between the two the branch exists with no worktree holding
    /// it, which is what a branch the user already had looks like, and a
    /// setup of another project that looked just then checked out the branch
    /// about to be deleted. Both steps now run under the clone's worktree
    /// lock, so a setup sees the worktree and the branch, or neither.
    #[test]
    fn discarding_a_worktree_holds_the_clone_for_both_steps() {
        let (_origin, clone, _) = clone_of_origin(false);
        let failed_root = TempDir::new("failed-root");
        let next_root = TempDir::new("next-root");
        let failed = check_out(clone.path(), "feature-y", failed_root.path()).unwrap();
        assert!(failed.created_branch);
        let lock = clone_worktree_lock_for(clone.path());
        let held = lock.lock().unwrap();

        std::thread::scope(|scope| {
            let (done, finished) = std::sync::mpsc::channel();
            let (clone_path, failed_path) = (clone.path(), failed.path.as_path());
            scope.spawn(move || {
                discard_incomplete_worktree(clone_path, failed_path, Some("feature-y"));
                done.send(()).unwrap();
            });

            // Held off, with neither step taken, while a setup has the clone.
            assert!(
                finished
                    .recv_timeout(std::time::Duration::from_millis(400))
                    .is_err(),
                "a discard must wait for the clone's worktree lock"
            );
            assert!(failed.path.exists());
            assert!(git::branch_exists(clone.path(), "feature-y").unwrap());

            drop(held);
            finished
                .recv_timeout(std::time::Duration::from_secs(120))
                .expect("the discard runs once the clone is free");
        });

        assert!(!failed.path.exists());
        assert!(!git::branch_exists(clone.path(), "feature-y").unwrap());
        // The lock is let go again, and the next setup starts from nothing:
        // it creates the branch, and owns it.
        let next = check_out(clone.path(), "feature-y", next_root.path()).unwrap();
        assert_eq!(next.local_branch_name, "feature-y");
        assert!(next.created_branch && !next.adopted_existing);
    }

    /// With nothing racing it, a setup does what it did before the lock, in
    /// each of the four cases it tells apart.
    #[test]
    fn a_setup_on_its_own_checks_out_as_before() {
        let (_origin, clone, _other_project) = clone_with_stale_feature_x();
        let project_root = TempDir::new("project-root");
        // Spelled the way git lists a worktree, so the project's own one is
        // recognised under it: the temp directory is behind a symlink on
        // macOS.
        let root = &project_root.path().canonicalize().unwrap();

        // A name nobody holds is created as asked, from the base.
        let fresh = check_out(clone.path(), "feature-y", root).unwrap();
        assert_eq!(fresh.local_branch_name, "feature-y");
        assert!(fresh.created_branch && !fresh.adopted_existing);
        assert_eq!(head_branch(&fresh.path), "feature-y");
        assert_eq!(
            git(&fresh.path, &["rev-parse", "HEAD"]),
            git(clone.path(), &["rev-parse", "origin/main"])
        );

        // This project's own worktree for the branch is reused, not made
        // again.
        let again = check_out(clone.path(), "feature-y", root).unwrap();
        assert_eq!(again.path, fresh.path);
        assert_eq!(again.local_branch_name, "feature-y");
        assert!(again.adopted_existing && !again.created_branch);

        // A local branch with no worktree gets one, and is not Staged's to
        // delete.
        git(clone.path(), &["branch", "feature-z", "origin/main"]);
        let existing = check_out(clone.path(), "feature-z", root).unwrap();
        assert_eq!(existing.local_branch_name, "feature-z");
        assert!(!existing.created_branch && !existing.adopted_existing);
        assert_eq!(head_branch(&existing.path), "feature-z");

        // A name another project's worktree holds is de-duplicated, and
        // starts where the branch on origin is.
        let deduplicated = check_out(clone.path(), "feature-x", root).unwrap();
        assert_eq!(deduplicated.local_branch_name, "feature-x-2");
        assert!(deduplicated.created_branch && !deduplicated.adopted_existing);
        assert_eq!(head_branch(&deduplicated.path), "feature-x-2");
        assert_eq!(
            git(&deduplicated.path, &["rev-parse", "HEAD"]),
            git(clone.path(), &["rev-parse", "origin/feature-x"])
        );
        // The other project's checkout is untouched.
        assert!(matches!(
            find_existing_worktree_for_branch(clone.path(), "feature-x", Some(root)),
            Ok(ExistingWorktree::Stale)
        ));
    }

    /// A name that something outside this process checked out for another
    /// project after the lookup is not retried under the same name: git
    /// checks a branch out in one worktree, so the attempt's own error is
    /// the answer.
    #[test]
    fn a_name_taken_outside_the_lock_fails_with_the_create_error() {
        let (_origin, clone, _other_project) = clone_with_stale_feature_x();
        let project_root = TempDir::new("project-root");

        let err = match create_worktree_on_new_branch(
            clone.path(),
            "feature-x",
            "origin/feature-x",
            &project_root.path().join("feature-x"),
            Some(project_root.path()),
        ) {
            Ok(created) => panic!("checked out twice at {}", created.path.display()),
            Err(err) => err,
        };

        assert!(err.contains("feature-x"), "{err}");
        assert!(!project_root.path().join("feature-x").exists());
    }

    #[test]
    fn deduplicated_pr_worktree_tracks_the_pr_head() {
        let (_origin, clone, _stale) = clone_with_stale_feature_x();
        let (store, project) = store_with_project();
        // Created the way `create_project` does for a PR import: the branch
        // row carries the PR head as its name until the worktree exists.
        let branch = store::Branch::new(&project.id, "feature-x", "main").with_pr(7);
        store.create_branch(&branch).unwrap();

        let worktrees = TempDir::new("worktrees");
        let path = link_worktree(
            &store,
            &branch,
            clone.path(),
            &worktrees.path().join("feature-x"),
        )
        .unwrap();
        let path = Path::new(&path);

        // The local name moved out of the way of the stale worktree…
        assert_eq!(
            git(path, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-x-2"
        );
        let stored = store.get_branch(&branch.id).unwrap().unwrap();
        assert_eq!(stored.branch_name, "feature-x-2");
        // …but git's upstream tracking says the branch pushes to the PR head.
        assert_eq!(
            merge_config(clone.path(), "feature-x-2"),
            "refs/heads/feature-x"
        );
        assert_eq!(
            git::upstream_branch_name(path, "feature-x-2", "main"),
            "feature-x"
        );
        assert_eq!(resolve_remote_branch_sync(&store, &stored), "feature-x");
        // And it started from the PR head, not from main.
        assert_eq!(
            git(path, &["rev-parse", "HEAD"]),
            git(clone.path(), &["rev-parse", "origin/feature-x"])
        );
    }

    /// Not just PRs: a plain branch whose name is held by another project's
    /// worktree still means `origin/feature-x`. The de-duplicated local name
    /// is an artifact of the collision, not a new branch to push.
    #[test]
    fn deduplicated_non_pr_branch_tracks_the_branch_it_was_named_for() {
        let (_origin, clone, _stale) = clone_with_stale_feature_x();
        let (store, project) = store_with_project();
        let branch = store::Branch::new(&project.id, "feature-x", "main");
        store.create_branch(&branch).unwrap();

        let worktrees = TempDir::new("worktrees");
        let path = link_worktree(
            &store,
            &branch,
            clone.path(),
            &worktrees.path().join("feature-x"),
        )
        .unwrap();
        let path = Path::new(&path);

        assert_eq!(
            git(path, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-x-2"
        );
        let stored = store.get_branch(&branch.id).unwrap().unwrap();
        assert_eq!(stored.branch_name, "feature-x-2");
        assert_eq!(
            merge_config(clone.path(), "feature-x-2"),
            "refs/heads/feature-x"
        );
        assert_eq!(resolve_remote_branch_sync(&store, &stored), "feature-x");
        // Started from the branch on origin, not from main.
        assert_eq!(
            git(path, &["rev-parse", "HEAD"]),
            git(clone.path(), &["rev-parse", "origin/feature-x"])
        );
    }

    /// `git worktree add -b feature-new <path> origin/main` makes the new
    /// branch track `origin/main`. Left alone, the push target would read
    /// as `main`; Staged clears it so the branch tracks nothing until its
    /// first `push -u`.
    #[test]
    fn a_fresh_branch_started_from_its_base_tracks_nothing() {
        let (_origin, clone, _stale) = clone_of_origin(false);
        let (store, project) = store_with_project();
        let branch = store::Branch::new(&project.id, "feature-new", "main");
        store.create_branch(&branch).unwrap();

        let worktrees = TempDir::new("worktrees");
        let path = link_worktree(
            &store,
            &branch,
            clone.path(),
            &worktrees.path().join("feature-new"),
        )
        .unwrap();
        let path = Path::new(&path);

        assert_eq!(
            git(path, &["rev-parse", "HEAD"]),
            git(clone.path(), &["rev-parse", "origin/main"])
        );
        assert_eq!(merge_config(clone.path(), "feature-new"), "");
        assert_eq!(
            git::upstream_branch_name(path, "feature-new", "main"),
            "feature-new"
        );
        assert_eq!(resolve_remote_branch_sync(&store, &branch), "feature-new");
    }

    /// Whatever its caller read before a transfer switched the branch to a
    /// workspace, setup gives it no worktree and no workdir row.
    #[test]
    fn worktree_setup_refuses_a_remote_branch() {
        let (store, project) = store_with_project();
        let branch = store::Branch::new_remote(&project.id, "feature", "main", "stg-feature");
        store.create_branch(&branch).unwrap();
        let app = mock_app();

        let err = setup_worktree_sync(&store, &branch.id, app.handle()).unwrap_err();

        assert!(err.contains("remote workspace"), "{err}");
        assert!(store.get_workdir_for_branch(&branch.id).unwrap().is_none());
    }

    /// Provisioning is one of the events the queue waits on: a session queued
    /// on a branch with no worktree yet starts when the worktree appears, and
    /// the backend — not a client's follow-up drain — is what moves it.
    #[test]
    fn worktree_creation_kicks_the_branch() {
        let (_origin, clone, _stale) = clone_of_origin(false);
        let (store, project) = store_with_project();
        let branch = store::Branch::new(&project.id, "feature-new", "main");
        store.create_branch(&branch).unwrap();

        let app = mock_app();
        let kicker = Arc::new(crate::session_queue::BranchQueueKicker::recording());
        app.manage(Arc::clone(&kicker));

        let worktrees = TempDir::new("worktrees");
        create_and_link_worktree(
            &store,
            app.handle(),
            &branch,
            clone.path(),
            &worktrees.path().join("feature-new"),
        )
        .unwrap();
        assert_eq!(kicker.kicked_branches(), vec![branch.id.clone()]);
        // And asks for the replies held while the branch had no worktree.
        assert_eq!(kicker.flushed(), vec![branch.id.clone()]);

        // The fast path — the worktree already exists — provisions nothing,
        // so it kicks nothing.
        create_and_link_worktree(
            &store,
            app.handle(),
            &branch,
            clone.path(),
            &worktrees.path().join("feature-new"),
        )
        .unwrap();
        assert_eq!(kicker.kicked_branches(), vec![branch.id.clone()]);
        assert_eq!(kicker.flushed(), vec![branch.id.clone()]);
    }

    /// A workspace reaching `Running` asks for the replies held while it
    /// was starting, as it kicks; any other write does neither.
    #[test]
    fn a_workspace_coming_up_flushes_held_follow_ups_and_kicks() {
        let app = mock_app();
        let kicker = Arc::new(crate::session_queue::BranchQueueKicker::recording());
        app.manage(Arc::clone(&kicker));

        workspace_status_written(app.handle(), "b", false, &store::WorkspaceStatus::Running);
        workspace_status_written(app.handle(), "b", true, &store::WorkspaceStatus::Starting);
        assert!(kicker.kicked().is_empty());
        assert!(kicker.flushed().is_empty());

        workspace_status_written(app.handle(), "b", true, &store::WorkspaceStatus::Running);
        assert_eq!(kicker.flushed(), vec!["b".to_string()]);
        assert_eq!(
            kicker.kicked(),
            vec![(
                "b".to_string(),
                crate::session_queue::KickSource::WorkspaceRunning
            )]
        );
    }

    /// The ordinary case: a branch that exists on origin, with no collision,
    /// tracks itself.
    #[test]
    fn a_branch_that_exists_on_origin_tracks_itself() {
        let (_origin, clone, _stale) = clone_of_origin(false);
        let (store, project) = store_with_project();
        let branch = store::Branch::new(&project.id, "feature-x", "main");
        store.create_branch(&branch).unwrap();

        let worktrees = TempDir::new("worktrees");
        let path = link_worktree(
            &store,
            &branch,
            clone.path(),
            &worktrees.path().join("feature-x"),
        )
        .unwrap();

        assert_eq!(
            merge_config(clone.path(), "feature-x"),
            "refs/heads/feature-x"
        );
        assert_eq!(
            git::upstream_branch_name(Path::new(&path), "feature-x", "main"),
            "feature-x"
        );
        let steps = crate::prs::build_push_pipeline_steps(
            "feature-x",
            "feature-x",
            crate::prs::PushVariant::default(),
        );
        let store::PipelineStep::Command { command, .. } = &steps[0] else {
            panic!("expected a push command step");
        };
        assert!(
            command.contains("git push -u origin feature-x:feature-x"),
            "{command}"
        );
    }

    /// End to end: the de-duplicated checkout, re-read from the store and
    /// resolved the way the push command does, produces a refspec that
    /// lands on the PR head rather than creating `origin/feature-x-2`.
    #[test]
    fn deduplicated_pr_branch_reloaded_from_the_store_pushes_to_the_pr_head() {
        let (_origin, clone, _stale) = clone_with_stale_feature_x();
        let (store, project) = store_with_project();
        let branch = store::Branch::new(&project.id, "feature-x", "main").with_pr(7);
        store.create_branch(&branch).unwrap();

        let worktrees = TempDir::new("worktrees");
        link_worktree(
            &store,
            &branch,
            clone.path(),
            &worktrees.path().join("feature-x"),
        )
        .unwrap();

        let stored = store.get_branch(&branch.id).unwrap().unwrap();
        let remote_branch = resolve_remote_branch_sync(&store, &stored);
        let steps = crate::prs::build_push_pipeline_steps(
            &stored.branch_name,
            &remote_branch,
            crate::prs::PushVariant::default(),
        );
        let store::PipelineStep::Command { command, .. } = &steps[0] else {
            panic!("expected a push command step");
        };
        assert!(
            command.contains("git push -u origin feature-x-2:feature-x"),
            "{command}"
        );
        assert!(!command.contains("feature-x-2:feature-x-2"), "{command}");
    }

    /// The whole path, in the clone a real import has: the upstream step
    /// must fetch `origin/feature-x` itself, and the push command the
    /// pipeline generates, actually run, lands on the PR head, creates no
    /// `feature-x-2` on origin, and (being `-u` with an explicit refspec)
    /// leaves the tracking entry on the PR head.
    #[test]
    fn deduplicated_pr_import_from_a_single_branch_clone_pushes_to_the_pr_head() {
        let (origin, clone, _stale) = single_branch_clone_with_stale_feature_x();
        let (store, project) = store_with_project();
        let branch = store::Branch::new(&project.id, "feature-x", "main").with_pr(7);
        store.create_branch(&branch).unwrap();

        let worktrees = TempDir::new("worktrees");
        let path = link_worktree(
            &store,
            &branch,
            clone.path(),
            &worktrees.path().join("feature-x"),
        )
        .unwrap();
        let path = Path::new(&path);

        assert!(git::remote_branch_exists(clone.path(), "feature-x").unwrap());
        assert_eq!(
            merge_config(clone.path(), "feature-x-2"),
            "refs/heads/feature-x"
        );

        std::fs::write(path.join("file.txt"), "base\nfeature\nmore\n").unwrap();
        git(path, &["commit", "-am", "more"]);
        let head = git(path, &["rev-parse", "HEAD"]);

        let stored = store.get_branch(&branch.id).unwrap().unwrap();
        let remote_branch = resolve_remote_branch_sync(&store, &stored);
        let steps = crate::prs::build_push_pipeline_steps(
            &stored.branch_name,
            &remote_branch,
            crate::prs::PushVariant::default(),
        );
        let store::PipelineStep::Command { command, .. } = &steps[0] else {
            panic!("expected a push command step");
        };
        assert!(
            command.contains("git push -u origin feature-x-2:feature-x"),
            "{command}"
        );
        let mut push = std::process::Command::new("sh");
        push.arg("-c").arg(command).current_dir(path);
        git::strip_git_env(&mut push);
        let output = push.output().unwrap();
        assert!(
            output.status.success(),
            "{command}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );

        assert_eq!(
            git(origin.path(), &["rev-parse", "refs/heads/feature-x"]),
            head
        );
        assert!(!git::branch_exists(origin.path(), "feature-x-2").unwrap());
        assert_eq!(
            merge_config(clone.path(), "feature-x-2"),
            "refs/heads/feature-x"
        );
        assert_eq!(
            git::upstream_branch_name(path, "feature-x-2", "main"),
            "feature-x"
        );
        // The clone's fetch refspecs are as the user left them: nothing was
        // added for `feature-x`, so a bare `git fetch origin` keeps working
        // after the PR head is deleted.
        assert_eq!(
            git(
                clone.path(),
                &["config", "--get-all", "remote.origin.fetch"]
            ),
            "+refs/heads/main:refs/remotes/origin/main"
        );
        assert!(git::cli_run(clone.path(), &["fetch", "origin"]).is_ok());
    }

    /// "Push without verifying" against a real `pre-push` hook that always
    /// fails and counts its runs. The verifying push fails and runs the hook
    /// once: the HTTPS fallback does not retry a push that reached the hook
    /// (`prs::tests::push_fallback` covers the fallback itself). The
    /// no-verify push lands without running it at all.
    #[test]
    fn no_verify_push_skips_a_failing_pre_push_hook() {
        use std::os::unix::fs::PermissionsExt;

        let (origin, clone, _) = clone_of_origin(false);
        let path = clone.path();
        git(path, &["checkout", "-b", "feature-x", "origin/feature-x"]);
        std::fs::write(path.join("file.txt"), "base\nfeature\nmore\n").unwrap();
        git(path, &["commit", "-am", "more"]);
        let head = git(path, &["rev-parse", "HEAD"]);
        let origin_before = git(origin.path(), &["rev-parse", "refs/heads/feature-x"]);

        // A repo-local hooksPath, so neither the test helpers' per-command
        // `core.hooksPath=/dev/null` nor a global hooksPath hides the hook.
        let hooks = TempDir::new("hooks");
        let runs = hooks.path().join("runs");
        let hook = hooks.path().join("pre-push");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\necho run >> '{}'\nexit 1\n", runs.display()),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        git(
            path,
            &["config", "core.hooksPath", hooks.path().to_str().unwrap()],
        );
        let hook_runs = || {
            std::fs::read_to_string(&runs)
                .map(|runs| runs.lines().count())
                .unwrap_or(0)
        };
        let run_push = |variant: crate::prs::PushVariant| {
            let steps = crate::prs::build_push_pipeline_steps("feature-x", "feature-x", variant);
            let store::PipelineStep::Command { command, .. } = &steps[0] else {
                panic!("expected a push command step");
            };
            let mut push = std::process::Command::new("sh");
            push.arg("-c").arg(command).current_dir(path);
            git::strip_git_env(&mut push);
            push.output().unwrap()
        };

        let verifying = run_push(crate::prs::PushVariant::default());
        assert!(!verifying.status.success());
        assert_eq!(hook_runs(), 1);
        assert_eq!(
            git(origin.path(), &["rev-parse", "refs/heads/feature-x"]),
            origin_before
        );

        let no_verify = run_push(crate::prs::PushVariant {
            force: false,
            no_verify: true,
        });
        assert!(
            no_verify.status.success(),
            "{}",
            String::from_utf8_lossy(&no_verify.stderr)
        );
        assert_eq!(hook_runs(), 1);
        assert_eq!(
            git(origin.path(), &["rev-parse", "refs/heads/feature-x"]),
            head
        );
    }

    /// A local branch the user already had, with no worktree, is only given a
    /// worktree here: an upstream they configured — on another remote, say —
    /// is theirs, even when origin has a branch of the same name. A branch
    /// of theirs that tracks nothing is pointed at origin like any other.
    #[test]
    fn a_pre_existing_branch_keeps_the_upstream_the_user_gave_it() {
        let (origin, clone, _) = clone_of_origin(false);
        let origin_str = origin.path().to_str().unwrap();
        git(clone.path(), &["remote", "add", "fork", origin_str]);
        git(clone.path(), &["fetch", "fork"]);
        git(
            clone.path(),
            &["branch", "--track", "feature-x", "fork/feature-x"],
        );
        git(
            clone.path(),
            &["push", "origin", "main:refs/heads/feature-y"],
        );
        git(clone.path(), &["fetch", "origin"]);
        git(
            clone.path(),
            &["branch", "--no-track", "feature-y", "origin/feature-y"],
        );
        assert!(git::remote_branch_exists(clone.path(), "feature-x").unwrap());

        let (store, project) = store_with_project();
        let tracked = store::Branch::new(&project.id, "feature-x", "main");
        store.create_branch(&tracked).unwrap();
        let loose = store::Branch::new(&project.id, "feature-y", "main");
        store.create_branch(&loose).unwrap();
        let worktrees = TempDir::new("worktrees");

        let path = link_worktree(
            &store,
            &tracked,
            clone.path(),
            &worktrees.path().join("feature-x"),
        )
        .unwrap();
        assert_eq!(
            git(Path::new(&path), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-x"
        );
        assert_eq!(
            git(clone.path(), &["config", "branch.feature-x.remote"]),
            "fork"
        );
        assert_eq!(
            merge_config(clone.path(), "feature-x"),
            "refs/heads/feature-x"
        );
        // Staged itself ignores the fork upstream and pushes to origin by name.
        assert_eq!(
            git::upstream_branch_name(Path::new(&path), "feature-x", "main"),
            "feature-x"
        );

        let path = link_worktree(
            &store,
            &loose,
            clone.path(),
            &worktrees.path().join("feature-y"),
        )
        .unwrap();
        assert_eq!(
            git(clone.path(), &["config", "branch.feature-y.remote"]),
            "origin"
        );
        assert_eq!(
            merge_config(clone.path(), "feature-y"),
            "refs/heads/feature-y"
        );
        assert_eq!(
            git::upstream_branch_name(Path::new(&path), "feature-y", "main"),
            "feature-y"
        );
    }

    /// A PR from a fork: its commits come from `refs/pull/7/head`, and the
    /// `origin/feature-x` that happens to exist is somebody else's branch of
    /// the same name. The checkout must not be made to push there.
    #[test]
    fn a_fork_pr_worktree_tracks_nothing_even_when_origin_has_the_same_name() {
        let (origin, clone, _stale) = clone_of_origin(false);
        let (store, project) = store_with_project();
        let mut repo =
            store::ProjectRepo::new(&project.id, "acme/widgets", "feature-x", None).primary();
        repo.head_repo = Some("fork-owner/widgets".to_string());
        store.create_project_repo(&repo).unwrap();
        let branch = store::Branch::new(&project.id, "feature-x", "main")
            .with_pr(7)
            .with_project_repo(&repo.id);
        store.create_branch(&branch).unwrap();
        assert!(git::remote_branch_exists(clone.path(), "feature-x").unwrap());

        let worktrees = TempDir::new("worktrees");
        let path = link_worktree(
            &store,
            &branch,
            clone.path(),
            &worktrees.path().join("feature-x"),
        )
        .unwrap();
        let path = Path::new(&path);

        assert_eq!(merge_config(clone.path(), "feature-x"), "");
        assert_eq!(
            git::upstream_branch_name(path, "feature-x", "main"),
            "feature-x"
        );
        let stored = store.get_branch(&branch.id).unwrap().unwrap();
        assert_eq!(resolve_remote_branch_sync(&store, &stored), "feature-x");
        assert_eq!(
            git(path, &["rev-parse", "HEAD"]),
            git(origin.path(), &["rev-parse", "refs/pull/7/head"])
        );
    }

    #[test]
    fn a_head_repo_is_a_fork_only_when_it_differs_from_the_repo() {
        let (store, project) = store_with_project();
        // One repo slug per case: a repo can be attached to a project once.
        let repo_with = |github_repo: &str, head_repo: Option<&str>| {
            let mut repo = store::ProjectRepo::new(&project.id, github_repo, "feature-x", None);
            repo.head_repo = head_repo.map(str::to_string);
            store.create_project_repo(&repo).unwrap();
            store::Branch::new(&project.id, "feature-x", "main")
                .with_pr(7)
                .with_project_repo(&repo.id)
        };

        let fork = repo_with("acme/widgets", Some("fork-owner/widgets"));
        assert!(pr_head_is_in_fork(&store, &fork).unwrap());
        let same_repo = repo_with("acme/gadgets", Some("ACME/Gadgets"));
        assert!(!pr_head_is_in_fork(&store, &same_repo).unwrap());
        let no_head_repo = repo_with("acme/gizmos", None);
        assert!(!pr_head_is_in_fork(&store, &no_head_repo).unwrap());
        let no_repo = store::Branch::new(&project.id, "feature-x", "main").with_pr(7);
        assert!(!pr_head_is_in_fork(&store, &no_repo).unwrap());
    }

    /// When the upstream cannot be set after `git worktree add` succeeded,
    /// the worktree and the branch it created go too: otherwise the retry
    /// would find `feature-x-2` taken and silently move on to `-3`.
    #[test]
    fn a_failed_upstream_setup_removes_the_worktree_it_created() {
        let (_origin, clone, _stale) = clone_with_stale_feature_x();
        let (store, project) = store_with_project();
        let branch = store::Branch::new(&project.id, "feature-x", "main").with_pr(7);
        store.create_branch(&branch).unwrap();

        // A held config lock: `worktree add` from a SHA and the fetch write
        // refs, not config, so they succeed, and the first thing to fail is
        // `git branch --set-upstream-to`.
        let config_lock = clone.path().join(".git").join("config.lock");
        std::fs::write(&config_lock, "").unwrap();

        let worktrees = TempDir::new("worktrees");
        let desired = worktrees.path().join("feature-x");
        let err = link_worktree(&store, &branch, clone.path(), &desired).unwrap_err();
        assert!(err.contains("upstream"), "{err}");

        assert!(!desired.exists());
        assert!(!git(clone.path(), &["worktree", "list", "--porcelain"]).contains("feature-x-2"));
        assert!(!git::branch_exists(clone.path(), "feature-x-2").unwrap());
        let stored = store.get_branch(&branch.id).unwrap().unwrap();
        assert_eq!(stored.branch_name, "feature-x");
        assert!(store.get_workdir_for_branch(&branch.id).unwrap().is_none());

        // The retry gets `-2` again, not `-3`.
        std::fs::remove_file(&config_lock).unwrap();
        let path = link_worktree(&store, &branch, clone.path(), &desired).unwrap();
        assert_eq!(
            git(Path::new(&path), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-x-2"
        );
        assert_eq!(
            merge_config(clone.path(), "feature-x-2"),
            "refs/heads/feature-x"
        );
    }

    /// A clone with `feature-x` checked out and tracking `origin/feature-x`,
    /// linked in the store as `branch`'s workdir — a PR branch imported
    /// without any name collision.
    fn store_with_tracked_feature_x(
        pr_number: Option<u64>,
    ) -> (TempDir, TempDir, Arc<Store>, store::Branch) {
        let (origin, clone, _) = clone_of_origin(false);
        git(clone.path(), &["checkout", "feature-x"]);
        assert_eq!(
            merge_config(clone.path(), "feature-x"),
            "refs/heads/feature-x"
        );

        let (store, project) = store_with_project();
        let mut branch = store::Branch::new(&project.id, "feature-x", "main");
        if let Some(pr) = pr_number {
            branch = branch.with_pr(pr);
        }
        store.create_branch(&branch).unwrap();
        let workdir = store::Workdir::new(&project.id, clone.path().to_str().unwrap())
            .with_branch(&branch.id);
        store.create_workdir(&workdir).unwrap();
        (origin, clone, store, branch)
    }

    /// `git branch -m` carries the upstream along, so a renamed PR branch
    /// keeps pushing to the PR head: nothing has to be pinned anywhere.
    #[test]
    fn renaming_a_tracked_pr_branch_keeps_its_push_target() {
        let (_origin, clone, store, branch) = store_with_tracked_feature_x(Some(7));

        rename_local_branch(&store, &branch, "feature-y").unwrap();

        let stored = store.get_branch(&branch.id).unwrap().unwrap();
        assert_eq!(stored.branch_name, "feature-y");
        assert_eq!(
            git(clone.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-y"
        );
        assert_eq!(
            merge_config(clone.path(), "feature-y"),
            "refs/heads/feature-x"
        );
        assert_eq!(resolve_remote_branch_sync(&store, &stored), "feature-x");
    }

    /// A branch that tracks nothing yet has no push target to keep: after a
    /// rename its first push goes to the new name.
    #[test]
    fn renaming_an_unpushed_branch_moves_its_push_target_with_it() {
        let (_origin, clone, store, branch) = store_with_tracked_feature_x(None);
        git(clone.path(), &["branch", "--unset-upstream", "feature-x"]);

        rename_local_branch(&store, &branch, "feature-y").unwrap();

        let stored = store.get_branch(&branch.id).unwrap().unwrap();
        assert_eq!(stored.branch_name, "feature-y");
        assert_eq!(
            git(clone.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-y"
        );
        assert_eq!(merge_config(clone.path(), "feature-y"), "");
        assert_eq!(resolve_remote_branch_sync(&store, &stored), "feature-y");
    }

    // ── Workspace checkout on (re-)entry ────────────────────────────────────

    /// An existing clone of [`bare_origin`] with `feature-x` checked out and
    /// one commit on it that origin doesn't have. Returns the commit's sha.
    fn clone_on_feature_x_with_an_unpushed_commit() -> (TempDir, TempDir, String) {
        let origin = bare_origin();
        let clone = clone_of(&origin, &[]);
        git(
            clone.path(),
            &["checkout", "-b", "feature-x", "origin/feature-x"],
        );
        std::fs::write(clone.path().join("file.txt"), "base\nfeature\nunpushed\n").unwrap();
        git(clone.path(), &["commit", "-am", "unpushed"]);
        let sha = git(clone.path(), &["rev-parse", "HEAD"]);
        (origin, clone, sha)
    }

    /// A branch re-entering `Starting` on a workstation that already has its
    /// checkout keeps it: `checkout -B feature-x origin/feature-x` would reset
    /// the branch and drop the unpushed commit.
    #[test]
    fn an_existing_clone_already_on_the_branch_is_not_reset() {
        let (_origin, clone, unpushed) = clone_on_feature_x_with_an_unpushed_commit();

        check_out_workspace_branch(
            &git_runner(clone.path(), None, None),
            &BranchCheckout::new("main", "feature-x"),
        )
        .unwrap();

        assert_eq!(
            git(clone.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-x"
        );
        assert_eq!(git(clone.path(), &["rev-parse", "HEAD"]), unpushed);
    }

    /// Every remote branch of a repo shares one clone, so a branch can have a
    /// local ref with unpushed commits while HEAD is on a sibling branch —
    /// e.g. after `resume_workspace` restarts both. Switching back must not
    /// reset it onto origin.
    #[test]
    fn a_local_branch_with_an_unpushed_commit_is_not_reset_from_another_branch() {
        let (_origin, clone, unpushed) = clone_on_feature_x_with_an_unpushed_commit();
        git(
            clone.path(),
            &["checkout", "-b", "feature-y", "origin/main"],
        );

        check_out_workspace_branch(
            &git_runner(clone.path(), None, None),
            &BranchCheckout::new("main", "feature-x"),
        )
        .unwrap();

        assert_eq!(
            git(clone.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-x"
        );
        assert_eq!(git(clone.path(), &["rev-parse", "HEAD"]), unpushed);
    }

    /// Push a commit to origin's `feature-x` from a separate clone, the way
    /// another machine would while the workstation is away. Returns its sha.
    fn push_to_origin_feature_x(origin: &TempDir) -> String {
        let other = clone_of(origin, &[]);
        git(
            other.path(),
            &["checkout", "-b", "feature-x", "origin/feature-x"],
        );
        std::fs::write(other.path().join("pushed.txt"), "pushed\n").unwrap();
        git(other.path(), &["add", "pushed.txt"]);
        git(other.path(), &["commit", "-m", "pushed"]);
        git(other.path(), &["push", "origin", "feature-x"]);
        git(other.path(), &["rev-parse", "HEAD"])
    }

    /// A local branch that is only behind origin is fast-forwarded to it, so
    /// it picks up what was pushed elsewhere.
    #[test]
    fn a_local_branch_behind_origin_is_fast_forwarded() {
        let origin = bare_origin();
        let clone = clone_of(&origin, &[]);
        git(clone.path(), &["branch", "feature-x", "origin/feature-x"]);
        let pushed = push_to_origin_feature_x(&origin);

        check_out_workspace_branch(
            &git_runner(clone.path(), None, None),
            &BranchCheckout::new("main", "feature-x"),
        )
        .unwrap();

        assert_eq!(
            git(clone.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-x"
        );
        assert_eq!(git(clone.path(), &["rev-parse", "HEAD"]), pushed);
        assert_eq!(
            merge_config(clone.path(), "feature-x"),
            "refs/heads/feature-x"
        );
    }

    /// A local branch that has diverged from origin can't fast-forward. It
    /// keeps its local commits rather than being reset, and setup still
    /// succeeds.
    #[test]
    fn a_local_branch_diverged_from_origin_keeps_its_commits() {
        let (origin, clone, unpushed) = clone_on_feature_x_with_an_unpushed_commit();
        git(clone.path(), &["checkout", "main"]);
        let pushed = push_to_origin_feature_x(&origin);

        check_out_workspace_branch(
            &git_runner(clone.path(), None, None),
            &BranchCheckout::new("main", "feature-x"),
        )
        .unwrap();

        assert_eq!(
            git(clone.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-x"
        );
        assert_eq!(git(clone.path(), &["rev-parse", "HEAD"]), unpushed);
        // Origin's commit was fetched, just not merged.
        assert_eq!(
            git(clone.path(), &["rev-parse", "origin/feature-x"]),
            pushed
        );
    }

    /// An existing clone on some other branch that doesn't have the branch
    /// yet is moved onto it, tracking it on origin.
    #[test]
    fn an_existing_clone_on_another_branch_checks_out_the_branch() {
        let origin = bare_origin();
        let clone = clone_of(&origin, &[]);

        check_out_workspace_branch(
            &git_runner(clone.path(), None, None),
            &BranchCheckout::new("main", "feature-x"),
        )
        .unwrap();

        assert_eq!(
            git(clone.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-x"
        );
        assert_eq!(
            git(clone.path(), &["rev-parse", "HEAD"]),
            git(clone.path(), &["rev-parse", "origin/feature-x"])
        );
        assert_eq!(
            merge_config(clone.path(), "feature-x"),
            "refs/heads/feature-x"
        );
    }

    /// A branch not on origin starts from the base branch and tracks
    /// nothing, so its first push goes to its own name.
    #[test]
    fn a_branch_missing_from_origin_starts_untracked_from_the_base() {
        let origin = bare_origin();
        let clone = clone_of(&origin, &[]);

        check_out_workspace_branch(
            &git_runner(clone.path(), None, None),
            &BranchCheckout::new("main", "feature-new"),
        )
        .unwrap();

        assert_eq!(
            git(clone.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature-new"
        );
        assert_eq!(
            git(clone.path(), &["rev-parse", "HEAD"]),
            git(clone.path(), &["rev-parse", "origin/main"])
        );
        assert_eq!(merge_config(clone.path(), "feature-new"), "");
    }
}
