use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

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
// Populated by `poll_workspace_status` and `start_workspace` when `blox ws info`
// returns an ID; read by `to_branch_with_workdir` when serializing for the frontend.
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
/// This is used both by `start_workspace` (secondary repo in a shared
/// workspace, or a branch re-entering `Starting` on a workstation that
/// already has its clone, e.g. after `resume_workspace`) and by
/// `add_project_repo` (adding a repo to a remote project whose workspace is
/// already running).
pub(crate) async fn clone_repo_into_workspace(
    ws_name: &str,
    clone_dir: &str,
    repo_slug: &str,
    base_ref: &str,
    branch_name: &str,
) -> Result<(), String> {
    let repo_path = resolve_workspace_repo_path(ws_name, clone_dir)
        .map_err(|e| format!("Failed to resolve workspace repo path '{clone_dir}': {e}"))?;

    // Clone only if the repo directory doesn't already exist.
    let probe = ws_exec_async(
        ws_name,
        ExecMode::Transient,
        &["sh", "-c", GIT_DIR_PROBE_SCRIPT, "_", &repo_path],
    )
    .await;
    let needs_clone = needs_clone(probe).map_err(|e| {
        format!("Failed to verify repo path '{clone_dir}' in workspace '{ws_name}': {e}")
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
        .map_err(|e| format!("Failed to clone '{repo_slug}' into workspace '{ws_name}': {e}"))?;
    }

    // Apply staged-managed git config. Idempotent — also serves as the
    // lazy migration for pre-existing remote clones (no registry on blox
    // workstations, so we redo the cheap config check on every visit).
    crate::git::config_apply::apply_to_blox_clone(ws_name, &repo_path).await;

    let workspace = ws_name.to_string();
    let remote_dir = clone_dir.to_string();
    let base = base_ref.to_string();
    let branch = branch_name.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        // Tracked for everything but `rev-parse`: Blox SIGKILLs a transient
        // git at 60 s, which a fetch on a large repo can outlast (reading as
        // "no remote branch" and silently branching from base), and a
        // checkout or fast-forward far from where the tree is (or smudging LFS
        // files) can outlast it too, leaving `.git/index.lock` and a half-updated worktree.
        let git: GitRunner<'_> = Box::new(move |args: &[&str]| {
            let mode = if args.first() == Some(&"rev-parse") {
                ExecMode::Transient
            } else {
                ExecMode::Tracked
            };
            run_workspace_git_with_mode(&workspace, Some(&remote_dir), mode, args)
                .map_err(|e| e.to_string())
        });
        check_out_workspace_branch(&git, &base, &branch)
    })
    .await
    .map_err(|e| format!("Branch checkout task failed: {e}"))?
    .map_err(|e| format!("Failed to set up '{repo_slug}' in workspace '{ws_name}': {e}"))
}

/// Fetch `base_ref` into the clone `git` runs in, and put the clone on
/// `branch_name`.
///
/// A branch the clone already has is checked out as it is, never recreated.
/// Every remote branch of a repo shares one clone, and this runs again
/// whenever any of them re-enters `Starting` (e.g. after `resume_workspace`),
/// so `checkout -B` would reset a branch onto origin and drop its unpushed
/// commits, whichever branch HEAD happened to be on. When origin has the
/// branch too, it is then fast-forwarded if it can be: a branch that is only
/// behind picks up what was pushed, and one that has diverged keeps its local
/// commits. Only a branch the clone doesn't have yet is created, from origin's
/// copy or from the base.
fn check_out_workspace_branch(
    git: &GitRunner<'_>,
    base_ref: &str,
    branch_name: &str,
) -> Result<(), String> {
    git(&["fetch", "origin", base_ref])
        .map_err(|e| format!("could not fetch base branch '{base_ref}': {e}"))?;

    // If the branch already exists on the remote (e.g. from an existing PR),
    // fetch it so we pick up its commits: a new branch starts from there and
    // an existing one fast-forwards to it. Otherwise a new branch falls back
    // to starting from the base branch.
    let has_remote_branch =
        branch_name != base_ref && git(&["fetch", "origin", branch_name]).is_ok();
    let remote_branch = format!("origin/{branch_name}");

    let local_ref = format!("refs/heads/{branch_name}");
    if git(&["rev-parse", "--verify", "--quiet", &local_ref]).is_ok() {
        git(&["checkout", branch_name])
            .map_err(|e| format!("could not check out branch '{branch_name}': {e}"))?;
        if has_remote_branch {
            // Fails when the branch has diverged from origin (or local
            // changes are in the way). The branch then stays as it is, local
            // commits and all, which is the point: never reset it.
            let _ = git(&["merge", "--ff-only", &remote_branch]);
        }
        return Ok(());
    }

    // `checkout -B <branch> origin/<branch>` makes the checkout track the
    // branch it pushes to. A fresh branch must not track `origin/<base>`
    // instead — git's upstream is read back as the push target.
    let checkout = if has_remote_branch {
        git(&["checkout", "-B", branch_name, &remote_branch])
    } else {
        git(&[
            "checkout",
            "--no-track",
            "-B",
            branch_name,
            &format!("origin/{base_ref}"),
        ])
    };
    checkout
        .map(|_| ())
        .map_err(|e| format!("could not create branch '{branch_name}': {e}"))
}

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

/// Clone a repo into an already-running remote workspace for a given branch.
///
/// Resolves the workspace name, repo slug, subpath, and base ref from the
/// branch and project, then delegates to `clone_repo_into_workspace`. Updates
/// the branch's workspace status to Running on success.
pub(crate) async fn setup_remote_repo_clone<R: tauri::Runtime>(
    store: &Arc<Store>,
    app_handle: &tauri::AppHandle<R>,
    branch_id: &str,
) -> Result<(), String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let ws_name = branch
        .workspace_name
        .as_deref()
        .ok_or("Branch has no workspace name")?;
    let clone_dir =
        resolve_branch_clone_dir(store, &branch)?.ok_or("Branch has no project repo to clone")?;
    let ref_name = normalize_branch_ref(&branch.base_branch);
    let repo_slug = resolve_branch_repo_slug(store, &project, &branch)?;

    clone_repo_into_workspace(
        ws_name,
        &clone_dir,
        &repo_slug,
        &ref_name,
        &branch.branch_name,
    )
    .await?;

    let changed = store
        .update_branch_workspace_status(branch_id, &store::WorkspaceStatus::Running)
        .map_err(|e| e.to_string())?;
    workspace_status_written(
        app_handle,
        branch_id,
        changed,
        &store::WorkspaceStatus::Running,
    );

    Ok(())
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
fn discard_incomplete_worktree(
    repo_path: &Path,
    worktree_path: &Path,
    created_branch: Option<&str>,
) {
    let Some(worktree_str) = worktree_path.to_str() else {
        return;
    };
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
pub fn get_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<Option<BranchWithWorkdir>, String> {
    let store = crate::get_store(&store)?;
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
}

#[tauri::command]
pub fn list_branches_for_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
) -> Result<Vec<BranchWithWorkdir>, String> {
    let store = get_store(&store)?;
    let _project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    let branches = store
        .list_branches_for_project(&project_id)
        .map_err(|e| e.to_string())?;

    // NOTE: workstation IDs for remote branches are populated lazily by
    // poll_all_workspace_statuses (a single batched `ws_list` call) rather
    // than eagerly here. Eager per-workspace `ws_info` calls were serial and
    // each took ~1s, causing multi-second UI freezes on project load.

    let commit_counts = store
        .count_finalized_commits_by_branch_for_project(&project_id)
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
pub fn create_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    branch_name: String,
    base_branch: Option<String>,
    project_repo_id: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;
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

    // When a stale worktree from another project holds the branch name, pick
    // a unique local branch name so we don't collide.
    let mut local_branch_name = branch.branch_name.clone();
    let existing_worktree =
        find_existing_worktree_for_branch(repo_path, requested_branch, project_root_ref)?;
    if let ExistingWorktree::Stale = &existing_worktree {
        local_branch_name = unique_branch_name(repo_path, &branch.branch_name)?;
        log::info!(
            "Branch '{}' is checked out in a stale worktree; using '{}' instead",
            branch.branch_name,
            local_branch_name,
        );
    }

    // Whether the worktree belongs to someone else — one the user already had,
    // or one another process made while this call was running. Its upstream
    // is left alone and it is never removed on failure.
    let mut adopted_existing = matches!(existing_worktree, ExistingWorktree::Found(_));
    // Whether the local branch itself was created here, so a failure further
    // down can delete it along with the worktree instead of leaving the name
    // taken for the next attempt, and so its upstream may be cleared.
    let mut created_branch = false;
    let worktree_path = if let ExistingWorktree::Found(path) = existing_worktree {
        path
    } else if local_branch_name == branch.branch_name
        && git::branch_exists(repo_path, &local_branch_name).map_err(|e| e.to_string())?
    {
        create_worktree_for_existing_branch_with_fallback(
            repo_path,
            &local_branch_name,
            desired_worktree_path,
            project_root_ref,
        )
        .map_err(|e| e.to_string())?
    } else {
        // For PRs, always start from `refs/pull/<N>/head` — the branch
        // name on origin may belong to a different branch when the PR
        // comes from a fork repo.  For non-PR branches, fall back to the
        // remote tracking ref or base_branch.
        let remote_ref = format!("origin/{requested_branch}");
        let pr_head_sha;
        let start_point = if let Some(pr_num) = branch.pr_number {
            pr_head_sha = git::fetch_pr_head_sha(repo_path, pr_num).map_err(|e| e.to_string())?;
            &pr_head_sha
        } else if git::remote_branch_exists(repo_path, requested_branch)
            .map_err(|e| e.to_string())?
        {
            &remote_ref
        } else {
            &branch.base_branch
        };
        match create_worktree_with_fallback(
            repo_path,
            &local_branch_name,
            start_point,
            desired_worktree_path,
            project_root_ref,
        ) {
            Ok(path) => {
                created_branch = true;
                path
            }
            Err(create_err) => {
                if let ExistingWorktree::Found(path) = find_existing_worktree_for_branch(
                    repo_path,
                    &local_branch_name,
                    project_root_ref,
                )? {
                    log::warn!(
                        "Reusing existing worktree '{}' for branch '{}' after create failure",
                        path.display(),
                        local_branch_name
                    );
                    // Another process created it between the lookup above
                    // and the attempt: theirs to keep.
                    adopted_existing = true;
                    path
                } else if git::branch_exists(repo_path, &local_branch_name)
                    .map_err(|e| e.to_string())?
                {
                    log::warn!(
                        "Branch '{}' already exists after create attempt; retrying with existing branch",
                        local_branch_name
                    );
                    // This arm is only reached for a name that did not exist
                    // when this call started (the `branch_exists` check above
                    // was false, or `unique_branch_name` had just found it
                    // free), so the branch is the one `worktree add -b` left
                    // behind when it failed after creating it. It is Staged's
                    // to point at origin/<requested> and to delete on a later
                    // failure, the same as when the first attempt succeeds.
                    created_branch = true;
                    create_worktree_for_existing_branch_with_fallback(
                        repo_path,
                        &local_branch_name,
                        desired_worktree_path,
                        project_root_ref,
                    )
                    .map_err(|e| e.to_string())?
                } else {
                    return Err(create_err);
                }
            }
        }
    };

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

/// Start the Blox workspace for a remote branch.
///
/// Separated from `create_remote_branch` so the frontend can dismiss the
/// dialog immediately and show the card in its "Provisioning…" state while
/// this runs in the background.
#[tauri::command(rename_all = "camelCase")]
pub async fn start_workspace(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: AppHandle,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    crate::project_transfer::ensure_branch_not_transferring(&branch_id)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let ws_name = branch
        .workspace_name
        .as_deref()
        .ok_or("Branch has no workspace name")?;
    let repo_subpath = resolve_branch_workspace_subpath(&store, &branch)?;
    let clone_dir = resolve_branch_clone_dir(&store, &branch)?;
    let ref_name = normalize_branch_ref(&branch.base_branch);
    let repo_slug = resolve_branch_repo_slug(&store, &project, &branch)?;

    // Pre-flight: verify the user is authenticated with Blox before starting
    // a workspace. Without this check, `blox ws start` can hang or fail
    // opaquely, leaving the frontend spinning for minutes (issue #99).
    if let Err(e) = run_blox_blocking(blox::check_auth).await {
        store
            .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
            .ok();
        return Err(e.to_string());
    }

    // Secondary repo setup in an already-running shared workspace — or a
    // branch re-entering `Starting` (e.g. after `resume_workspace`) on a
    // workstation that already has this clone.
    if let Some(clone_dir) = clone_dir.as_deref() {
        if let Ok(info) = run_blox_blocking({
            let ws_name = ws_name.to_string();
            move || blox::ws_info(&ws_name)
        })
        .await
        {
            let ws_status = info
                .status
                .as_deref()
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            if let Some(ws_id) = info.workstation_id {
                if let Ok(mut cache) = workstation_id_cache().lock() {
                    cache.insert(ws_name.to_string(), ws_id);
                }
            }
            if ws_status == "running" {
                clone_repo_into_workspace(
                    ws_name,
                    clone_dir,
                    &repo_slug,
                    &ref_name,
                    &branch.branch_name,
                )
                .await?;
                let changed = store
                    .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Running)
                    .map_err(|e| e.to_string())?;
                workspace_status_written(
                    &app_handle,
                    &branch_id,
                    changed,
                    &store::WorkspaceStatus::Running,
                );

                return Ok(());
            }
        }
    }

    // Construct the HTTPS git URL directly from github_repo.
    let resolved_source = Some(format!(
        "https://github.com/{}.git?ref={}",
        repo_slug, ref_name
    ));
    let start_command_preview = match resolved_source.as_deref() {
        Some(source) => format!(
            "sq blox ws start {} --idle-timeout {} {}",
            ws_name, WORKSPACE_IDLE_TIMEOUT_MINUTES, source
        ),
        None => format!(
            "sq blox ws start {} --idle-timeout {}",
            ws_name, WORKSPACE_IDLE_TIMEOUT_MINUTES
        ),
    };
    log::info!(
        "[start_workspace] branch={} workspace={} invoking command=\"{}\"",
        branch_id,
        ws_name,
        start_command_preview
    );
    let ws_start_started_at = Instant::now();

    match run_blox_blocking({
        let ws_name = ws_name.to_string();
        let source = resolved_source.clone();
        move || {
            blox::ws_start(
                &ws_name,
                source.as_deref(),
                Some(WORKSPACE_IDLE_TIMEOUT_MINUTES),
            )
        }
    })
    .await
    {
        Ok(_) => {
            log::info!(
                "[start_workspace] branch={} workspace={} ws_start completed elapsed_ms={}",
                branch_id,
                ws_name,
                ws_start_started_at.elapsed().as_millis()
            );
            // If the branch already exists on the remote (e.g. from an
            // existing PR), fetch it and start from there so we pick up its
            // commits. Otherwise create a fresh branch from the base ref.
            // Tracked: a transient fetch killed at 60 s would read as "no
            // remote branch" and silently branch from base.
            let has_remote_branch = run_workspace_git_async(
                ws_name,
                repo_subpath.as_deref(),
                ExecMode::Tracked,
                &["fetch", "origin", &branch.branch_name],
            )
            .await
            .is_ok();

            let remote_ref = format!("origin/{}", branch.branch_name);
            let checkout_args: &[&str] = if has_remote_branch {
                &["checkout", "-B", &branch.branch_name, &remote_ref]
            } else {
                &["checkout", "-b", &branch.branch_name]
            };
            // Tracked: Blox SIGKILLs a transient git at 60 s, and a killed
            // checkout leaves `.git/index.lock` and a half-updated worktree.
            let checkout_result = run_workspace_git_async(
                ws_name,
                repo_subpath.as_deref(),
                ExecMode::Tracked,
                checkout_args,
            )
            .await;

            if let Err(e) = checkout_result {
                log::warn!(
                    "failed to create branch '{}' in workspace '{}': {e}",
                    branch.branch_name,
                    ws_name
                );
            }

            Ok(())
        }
        Err(blox::BloxError::NotAuthenticated) => {
            log::warn!(
                "[start_workspace] branch={} workspace={} ws_start failed elapsed_ms={} error=NotAuthenticated",
                branch_id,
                ws_name,
                ws_start_started_at.elapsed().as_millis()
            );
            // Auth errors are definitive — mark as Error so the frontend
            // stops polling and shows an actionable message.
            store
                .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                .ok();
            Err("Not authenticated with Blox. Run: sq login".to_string())
        }
        Err(e) => {
            log::warn!(
                "[start_workspace] branch={} workspace={} ws_start failed elapsed_ms={} error={}",
                branch_id,
                ws_name,
                ws_start_started_at.elapsed().as_millis(),
                e
            );
            if is_blox_onboarding_precondition_error(&e) {
                // Blox onboarding precondition failures are definitive.
                // Keep the exact CLI error text so the user sees the
                // onboarding URL and required action.
                store
                    .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                    .ok();
                return Err(e.to_string());
            }
            // Don't set Error status here — `blox ws start` can fail (e.g.
            // timeout, transient network issue) even though the workspace was
            // created and is still booting. Let the frontend's status polling
            // determine the real state; it will keep polling while the DB says
            // Starting and will eventually converge on the correct status.
            log::warn!(
                "blox ws start failed for '{}', leaving status as Starting for polling to resolve: {e}",
                ws_name
            );
            Ok(())
        }
    }
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
        Some("starting") | Some("provisioning") | Some("creating") => {
            store::WorkspaceStatus::Starting
        }
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
/// This is the primary mechanism for the frontend to detect when a workspace
/// transitions from `Starting` to `Running` (or `Error`). It queries the
/// Blox CLI, maps the reported status to our `WorkspaceStatus` enum, persists
/// the change, and returns the updated status string.
#[tauri::command(rename_all = "camelCase")]
pub async fn poll_workspace_status(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: AppHandle,
    branch_id: String,
) -> Result<PollWorkspaceResult, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let ws_name = branch
        .workspace_name
        .as_deref()
        .ok_or("Branch is not a remote workspace branch")?;

    // Secondary repo setup uses `workspace_status=Starting` as a per-branch
    // loading state while clone/fetch/checkout runs inside an already-running
    // shared workspace. Keep it in Starting until that setup command marks
    // this branch as Running.
    //
    // Important: this must apply only to "secondary clone" branches. Initial
    // remote branches also have a workspace subpath, so keying only on
    // `resolve_branch_workspace_subpath().is_some()` would pin them in
    // Starting forever and block normal status polling.
    let is_secondary_clone_setup = if branch.workspace_status
        == Some(store::WorkspaceStatus::Starting)
        && resolve_branch_workspace_subpath(&store, &branch)?.is_some()
    {
        if let Some(ws_name) = branch.workspace_name.as_deref() {
            let peers = store
                .list_branches_for_project(&branch.project_id)
                .map_err(|e| e.to_string())?;
            peers.into_iter().any(|peer| {
                peer.id != branch.id
                    && peer.branch_type == store::BranchType::Remote
                    && peer.workspace_name.as_deref() == Some(ws_name)
                    && peer.workspace_status == Some(store::WorkspaceStatus::Running)
            })
        } else {
            false
        }
    } else {
        false
    };

    if is_secondary_clone_setup {
        log::debug!(
            "[poll_workspace_status] branch={} ws={} held at Starting for secondary clone setup",
            branch_id,
            ws_name
        );
        return Ok(PollWorkspaceResult {
            status: store::WorkspaceStatus::Starting.as_str().to_string(),
            workstation_id: cached_workstation_id(ws_name),
        });
    }

    let info = match run_blox_blocking({
        let ws_name = ws_name.to_string();
        move || blox::ws_info(&ws_name)
    })
    .await
    {
        Ok(info) => info,
        Err(blox::BloxError::NotAuthenticated) => {
            // Auth errors are definitive — stop polling and surface the error.
            store
                .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                .ok();
            return Err("Not authenticated with Blox. Run: sq login".to_string());
        }
        Err(e) => {
            let is_not_found = matches!(&e, blox::BloxError::CommandFailed(msg) if msg.to_lowercase().contains("not found"));

            // During initial creation, `blox ws start` may still be running
            // when the frontend's first poll fires `blox ws info`. The
            // workspace doesn't exist yet, so the CLI returns "not found".
            // If the DB still says Starting, swallow the error and tell the
            // frontend to keep polling.
            if branch.workspace_status == Some(store::WorkspaceStatus::Starting) {
                log::debug!(
                    "blox ws info failed for '{}' while still Starting, treating as Starting: {e}",
                    ws_name
                );
                return Ok(PollWorkspaceResult {
                    status: store::WorkspaceStatus::Starting.as_str().to_string(),
                    workstation_id: cached_workstation_id(ws_name),
                });
            }
            // If the workspace was Running but Blox explicitly says "not
            // found" (e.g. deleted externally), transition to Stopped.
            // Other errors (network blips, timeouts, etc.) are transient —
            // keep the Running status so the poller retries.
            if branch.workspace_status == Some(store::WorkspaceStatus::Running) {
                if is_not_found {
                    log::debug!(
                        "blox ws info returned not-found for '{}' while Running, treating as Stopped: {e}",
                        ws_name
                    );
                    store
                        .update_branch_workspace_status(
                            &branch_id,
                            &store::WorkspaceStatus::Stopped,
                        )
                        .ok();
                    return Ok(PollWorkspaceResult {
                        status: store::WorkspaceStatus::Stopped.as_str().to_string(),
                        workstation_id: cached_workstation_id(ws_name),
                    });
                }
                log::warn!(
                    "blox ws info failed for '{}' while Running, keeping Running status: {e}",
                    ws_name
                );
                return Ok(PollWorkspaceResult {
                    status: store::WorkspaceStatus::Running.as_str().to_string(),
                    workstation_id: cached_workstation_id(ws_name),
                });
            }
            return Err(e.to_string());
        }
    };

    log::debug!(
        "[poll_workspace_status] branch={} ws={} db_status={:?} blox_status={:?}",
        branch_id,
        ws_name,
        branch.workspace_status,
        info.status
    );

    let new_status = map_blox_status_to_workspace_status(
        info.status.as_deref(),
        branch.workspace_status.as_ref(),
    );

    let previous = branch
        .workspace_status
        .as_ref()
        .map(|s| s.as_str())
        .unwrap_or("none");
    if previous != new_status.as_str() {
        log::debug!(
            "[poll_workspace_status] branch={} ws={} transition {} -> {} (raw={:?})",
            branch_id,
            ws_name,
            previous,
            new_status.as_str(),
            info.status
        );
    }
    // Cache the numeric workstation ID for proxy URL construction.
    if let Some(ws_id) = info.workstation_id {
        if let Some(name) = branch.workspace_name.as_deref() {
            if let Ok(mut cache) = workstation_id_cache().lock() {
                cache.insert(name.to_string(), ws_id);
            }
        }
    }

    let changed = store
        .update_branch_workspace_status(&branch_id, &new_status)
        .map_err(|e| e.to_string())?;
    workspace_status_written(&app_handle, &branch_id, changed, &new_status);

    Ok(PollWorkspaceResult {
        status: new_status.as_str().to_string(),
        workstation_id: cached_workstation_id(ws_name),
    })
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
/// from the result rather than failing the entire batch.
#[tauri::command(rename_all = "camelCase")]
pub async fn poll_all_workspace_statuses(
    app_handle: AppHandle,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_ids: Vec<String>,
) -> Result<HashMap<String, PollWorkspaceResult>, String> {
    let store = get_store(&store)?;

    // Fetch all workspaces in one CLI call.
    let entries = run_blox_blocking(blox::ws_list).await.map_err(|e| {
        if matches!(e, blox::BloxError::NotAuthenticated) {
            "Not authenticated with Blox. Run: sq login".to_string()
        } else {
            e.to_string()
        }
    })?;

    // Build a lookup from workspace name → list entry.
    let ws_map: HashMap<String, &blox::WorkspaceListEntry> =
        entries.iter().map(|e| (e.name.clone(), e)).collect();

    // Map workspace name → branch IDs for workspaces in "starting" state so we
    // can fetch their bootstrap command progress and emit events.
    let mut starting_workspaces: HashMap<String, Vec<String>> = HashMap::new();

    let mut results = HashMap::new();

    for branch_id in &branch_ids {
        let branch = match store.get_branch(branch_id) {
            Ok(Some(b)) => b,
            _ => continue,
        };

        let ws_name = match branch.workspace_name.as_deref() {
            Some(n) => n,
            None => continue,
        };

        // Secondary clone setup: if this branch is Starting and shares a workspace
        // with a Running peer (multi-repo setup), hold it at Starting until the
        // setup command marks it as Running. This mirrors the individual
        // poll_workspace_status logic.
        if branch.workspace_status == Some(store::WorkspaceStatus::Starting)
            && resolve_branch_workspace_subpath(&store, &branch)
                .unwrap_or_else(|e| {
                    log::warn!(
                        "Failed to resolve workspace subpath for branch {}: {e}",
                        branch_id
                    );
                    None
                })
                .is_some()
        {
            let is_secondary = if let Some(ws) = branch.workspace_name.as_deref() {
                store
                    .list_branches_for_project(&branch.project_id)
                    .unwrap_or_default()
                    .into_iter()
                    .any(|peer| {
                        peer.id != *branch_id
                            && peer.branch_type == store::BranchType::Remote
                            && peer.workspace_name.as_deref() == Some(ws)
                            && peer.workspace_status == Some(store::WorkspaceStatus::Running)
                    })
            } else {
                false
            };

            if is_secondary {
                log::debug!(
                    "[poll_all_workspace_statuses] branch={} ws={} held at Starting for secondary clone setup",
                    branch_id,
                    ws_name
                );
                results.insert(
                    branch_id.clone(),
                    PollWorkspaceResult {
                        status: store::WorkspaceStatus::Starting.as_str().to_string(),
                        workstation_id: cached_workstation_id(ws_name),
                    },
                );
                continue;
            }
        }

        // Look up the workspace in the list.
        match ws_map.get(ws_name) {
            Some(entry) => {
                let new_status = map_blox_status_to_workspace_status(
                    entry.status.as_deref(),
                    branch.workspace_status.as_ref(),
                );

                let previous = branch
                    .workspace_status
                    .as_ref()
                    .map(|s| s.as_str())
                    .unwrap_or("none");
                if previous != new_status.as_str() {
                    log::debug!(
                        "[poll_all_workspace_statuses] branch={} ws={} transition {} -> {} (raw={:?})",
                        branch_id,
                        ws_name,
                        previous,
                        new_status.as_str(),
                        entry.status
                    );
                }

                // Cache the numeric workstation ID.
                if let Some(ws_id) = entry.workstation_id {
                    if let Ok(mut cache) = workstation_id_cache().lock() {
                        cache.insert(ws_name.to_string(), ws_id);
                    }
                }

                if let Ok(changed) = store.update_branch_workspace_status(branch_id, &new_status) {
                    workspace_status_written(&app_handle, branch_id, changed, &new_status);
                }

                // Track workspaces still starting so we can fetch bootstrap progress.
                if new_status == store::WorkspaceStatus::Starting {
                    starting_workspaces
                        .entry(ws_name.to_string())
                        .or_default()
                        .push(branch_id.clone());
                }

                results.insert(
                    branch_id.clone(),
                    PollWorkspaceResult {
                        status: new_status.as_str().to_string(),
                        workstation_id: cached_workstation_id(ws_name),
                    },
                );
            }
            None => {
                // Workspace not in list — same "not found" logic as individual polling.
                if branch.workspace_status == Some(store::WorkspaceStatus::Starting) {
                    log::debug!(
                        "[poll_all_workspace_statuses] branch={} ws={} not in list while Starting, keeping Starting",
                        branch_id,
                        ws_name
                    );
                    starting_workspaces
                        .entry(ws_name.to_string())
                        .or_default()
                        .push(branch_id.clone());
                    results.insert(
                        branch_id.clone(),
                        PollWorkspaceResult {
                            status: store::WorkspaceStatus::Starting.as_str().to_string(),
                            workstation_id: cached_workstation_id(ws_name),
                        },
                    );
                } else if branch.workspace_status == Some(store::WorkspaceStatus::Running) {
                    log::debug!(
                        "[poll_all_workspace_statuses] branch={} ws={} not in list while Running, treating as Stopped",
                        branch_id,
                        ws_name
                    );
                    store
                        .update_branch_workspace_status(branch_id, &store::WorkspaceStatus::Stopped)
                        .ok();
                    results.insert(
                        branch_id.clone(),
                        PollWorkspaceResult {
                            status: store::WorkspaceStatus::Stopped.as_str().to_string(),
                            workstation_id: cached_workstation_id(ws_name),
                        },
                    );
                }
                // Other statuses (Stopped, Error, etc.) — omit from results, no change needed.
            }
        }
    }

    // For workspaces that are still starting, fetch bootstrap command progress
    // and emit events so the UI can show which step is running.
    if !starting_workspaces.is_empty() {
        let app_handle_clone = app_handle.clone();

        // Fetch commands for each starting workspace in a blocking task to
        // avoid holding up the response.
        tauri::async_runtime::spawn_blocking(move || {
            for (ws_name, branch_ids) in &starting_workspaces {
                match blox::ws_commands(ws_name) {
                    Ok(cmds) => {
                        emit_workspace_setup_progress(&app_handle_clone, branch_ids, &cmds);
                    }
                    Err(e) => {
                        log::debug!(
                            "[poll_all_workspace_statuses] ws_commands({}) failed: {}",
                            ws_name,
                            e
                        );
                    }
                }
            }
        });
    }

    Ok(results)
}

/// Resume a suspended Blox workspace.
///
/// Transitions all branches sharing this workspace to Starting and calls
/// `sq blox ws resume`. Returns the IDs of all affected branches.
#[tauri::command(rename_all = "camelCase")]
pub async fn resume_workspace(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    workspace_name: String,
) -> Result<Vec<String>, String> {
    let store = get_store(&store)?;
    crate::project_transfer::ensure_workspace_not_transferring(&workspace_name)?;

    // Transition all peer branches to Starting so the UI begins polling.
    let branch_ids = store
        .update_workspace_status_by_workspace_name(
            &workspace_name,
            &store::WorkspaceStatus::Starting,
        )
        .map_err(|e| e.to_string())?;

    if branch_ids.is_empty() {
        return Err(format!("No branches found for workspace: {workspace_name}"));
    }

    // Call resume in the background (it may take a while).
    let ws = workspace_name.clone();
    if let Err(e) = run_blox_blocking(move || blox::ws_resume(&ws)).await {
        log::warn!(
            "[resume_workspace] workspace={} resume failed: {}",
            workspace_name,
            e
        );
        store
            .update_workspace_status_by_workspace_name(
                &workspace_name,
                &store::WorkspaceStatus::Error,
            )
            .ok();
        return Err(e.to_string());
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
    /// twice, because the HTTPS fallback retries on any failure; the no-verify
    /// push lands without running it at all.
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
        assert_eq!(hook_runs(), 2);
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
        assert_eq!(hook_runs(), 2);
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

        check_out_workspace_branch(&git_runner(clone.path(), None, None), "main", "feature-x")
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

        check_out_workspace_branch(&git_runner(clone.path(), None, None), "main", "feature-x")
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

        check_out_workspace_branch(&git_runner(clone.path(), None, None), "main", "feature-x")
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

        check_out_workspace_branch(&git_runner(clone.path(), None, None), "main", "feature-x")
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

        check_out_workspace_branch(&git_runner(clone.path(), None, None), "main", "feature-x")
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

        check_out_workspace_branch(&git_runner(clone.path(), None, None), "main", "feature-new")
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
