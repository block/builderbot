use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use tauri::{AppHandle, Manager};

use crate::actions::events::TauriExecutionListener;
use crate::actions::{ActionExecutor, ActionMetadata, ActionRegistry, ActionType};
use crate::blox;
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
pub(crate) fn emit_setup_progress(
    handle: &AppHandle,
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

/// Per-branch lock that serializes worktree setup so that `setup_worktree`
/// (frontend) and `setup_worktree_sync` (backend/MCP) do not race each other.
/// Follows the same pattern as `REPO_CLONE_LOCKS` in `git/github.rs`.
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
        setup_complete: branch.setup_complete,
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

/// Resolve the shared workspace name for a remote project.
///
/// If any remote branch already exists for the project, reuse its workspace
/// name so all project repos stay on the same Blox workspace.
pub(crate) fn resolve_project_workspace_name(
    store: &Arc<Store>,
    project: &store::Project,
    fallback_workspace_name: Option<&str>,
) -> Result<String, String> {
    let existing = store
        .list_branches_for_project(&project.id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|b| b.branch_type == store::BranchType::Remote)
        .and_then(|b| b.workspace_name)
        .filter(|name| !name.trim().is_empty());
    if let Some(name) = existing {
        return Ok(name);
    }

    if let Some(name) = fallback_workspace_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(name.to_string());
    }

    Ok(infer_workspace_name(&infer_branch_name(&project.name)))
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

pub(crate) fn run_workspace_git(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    git_args: &[&str],
) -> Result<String, blox::BloxError> {
    let owned = workspace_git_args(workspace_name, repo_subpath, git_args)?;
    let borrowed = owned.iter().map(String::as_str).collect::<Vec<_>>();
    blox::ws_exec(workspace_name, &borrowed)
}

/// Execute a shell script inside a Blox workspace via `sh -c`.
///
/// Positional arguments are passed as `$1`, `$2`, etc. This allows batching
/// multiple git commands into a single `ws_exec` round-trip.
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
    blox::ws_exec(workspace_name, &borrowed)
}

pub(crate) fn run_workspace_git_bytes(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    git_args: &[&str],
) -> Result<Vec<u8>, blox::BloxError> {
    let owned = workspace_git_args(workspace_name, repo_subpath, git_args)?;
    let borrowed = owned.iter().map(String::as_str).collect::<Vec<_>>();
    blox::ws_exec_bytes(workspace_name, &borrowed)
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

pub(crate) async fn ws_exec_async(
    workspace_name: &str,
    args: &[&str],
) -> Result<String, blox::BloxError> {
    let ws_name = workspace_name.to_string();
    let owned_args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    run_blox_blocking(move || {
        let borrowed = owned_args.iter().map(String::as_str).collect::<Vec<_>>();
        blox::ws_exec(&ws_name, &borrowed)
    })
    .await
}

pub(crate) async fn run_workspace_git_async(
    workspace_name: &str,
    repo_subpath: Option<&str>,
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
        run_workspace_git(&ws_name, subpath.as_deref(), &borrowed_args)
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

/// Resolve the workspace-relative path for a branch's repository.
///
/// The workspace clone directory is derived from `github_repo` (e.g.
/// `squareup/g2` → `~/g2`). If the repo has a subpath (a monorepo
/// subdirectory like `apps/staged`), it is appended to the clone directory.
///
/// Returns a `home:<dir>` string suitable for `resolve_workspace_repo_path`,
/// or `None` when the branch has no associated project repo.
/// Return just the repo-root clone directory as `home:<clone_dir>`.
///
/// This is the base path on the workspace filesystem where the repo is
/// cloned. Use this when you need commands to run from the repo root
/// (e.g. `git diff` in `diff_commands`).
pub(crate) fn resolve_branch_clone_dir(
    store: &Arc<Store>,
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

/// Clone a repo into an already-running workspace, fetch the base branch,
/// and create the feature branch.
///
/// This is used both by `start_workspace` (secondary repo in a shared
/// workspace) and by `add_project_repo` (adding a repo to a remote project
/// whose workspace is already running).
pub(crate) async fn clone_repo_into_workspace(
    ws_name: &str,
    repo_subpath: &str,
    repo_slug: &str,
    base_ref: &str,
    branch_name: &str,
) -> Result<(), String> {
    let repo_path = resolve_workspace_repo_path(ws_name, repo_subpath)
        .map_err(|e| format!("Failed to resolve workspace repo path '{repo_subpath}': {e}"))?;

    // Clone only if the repo directory doesn't already exist.
    match ws_exec_async(ws_name, &["test", "-d", &format!("{repo_path}/.git")]).await {
        Ok(_) => {}
        Err(blox::BloxError::CommandFailed(_)) => {
            let repo_url = format!("https://github.com/{repo_slug}.git");
            ws_exec_async(ws_name, &["git", "clone", &repo_url, &repo_path])
                .await
                .map_err(|e| {
                    format!("Failed to clone '{repo_slug}' into workspace '{ws_name}': {e}")
                })?;
        }
        Err(e) => {
            return Err(format!(
                "Failed to verify repo path '{repo_subpath}' in workspace '{ws_name}': {e}"
            ));
        }
    }

    // Apply staged-managed git config. Idempotent — also serves as the
    // lazy migration for pre-existing remote clones (no registry on blox
    // workstations, so we redo the cheap config check on every visit).
    crate::git::config_apply::apply_to_blox_clone(ws_name, &repo_path).await;

    run_workspace_git_async(ws_name, Some(repo_subpath), &["fetch", "origin", base_ref])
        .await
        .map_err(|e| format!(
            "Failed to fetch base branch '{base_ref}' for '{repo_slug}' in workspace '{ws_name}': {e}"
        ))?;

    // If the branch already exists on the remote (e.g. from an existing PR),
    // fetch it and start from there so we pick up its commits. Otherwise fall
    // back to starting from the base branch.
    let has_remote_branch = if branch_name != base_ref {
        run_workspace_git_async(
            ws_name,
            Some(repo_subpath),
            &["fetch", "origin", branch_name],
        )
        .await
        .is_ok()
    } else {
        false
    };

    let start_point = if has_remote_branch {
        format!("origin/{branch_name}")
    } else {
        format!("origin/{base_ref}")
    };

    run_workspace_git_async(
        ws_name,
        Some(repo_subpath),
        &["checkout", "-B", branch_name, &start_point],
    )
    .await
    .map_err(|e| {
        format!(
        "Failed to create branch '{branch_name}' for '{repo_slug}' in workspace '{ws_name}': {e}"
    )
    })?;

    Ok(())
}

/// Clone a repo into an already-running remote workspace for a given branch.
///
/// Resolves the workspace name, repo slug, subpath, and base ref from the
/// branch and project, then delegates to `clone_repo_into_workspace`. Updates
/// the branch's workspace status to Running on success.
pub(crate) async fn setup_remote_repo_clone(
    store: &Arc<Store>,
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
    let repo_subpath = resolve_branch_workspace_subpath(store, &branch)?
        .ok_or("Branch has no workspace subpath")?;
    let ref_name = normalize_branch_ref(&branch.base_branch);
    let repo_slug = resolve_branch_repo_slug(store, &project, &branch)?;

    clone_repo_into_workspace(
        ws_name,
        &repo_subpath,
        &repo_slug,
        &ref_name,
        &branch.branch_name,
    )
    .await?;

    store
        .update_branch_workspace_status(branch_id, &store::WorkspaceStatus::Running)
        .map_err(|e| e.to_string())?;

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
                store.delete_workdir(&wd.id).map_err(|e| e.to_string())?;
            }
        }
        store::BranchType::Remote => {
            if let Some(ref ws_name) = branch.workspace_name {
                let in_use_elsewhere = store
                    .list_branches_for_project(&branch.project_id)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .any(|b| {
                        b.id != branch.id
                            && b.branch_type == store::BranchType::Remote
                            && b.workspace_name.as_deref() == Some(ws_name.as_str())
                    });
                if in_use_elsewhere {
                    return Ok(());
                }
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

pub(crate) fn infer_workspace_name(branch_name: &str) -> String {
    const WORKSPACE_NAME_MAX_LENGTH: usize = 32;
    let safe = branch_name
        .replace('/', "-")
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if safe.is_empty() {
        return "stg-feature".to_string();
    }
    let mut full = format!("stg-{safe}");
    if full.len() > WORKSPACE_NAME_MAX_LENGTH {
        full = full[..WORKSPACE_NAME_MAX_LENGTH]
            .trim_end_matches('-')
            .to_string();
    }
    full
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

/// Core worktree creation + DB linkage logic shared by [`setup_worktree`] and
/// [`setup_worktree_sync`].
///
/// Acquires the per-branch worktree lock, creates (or reuses) a git worktree,
/// sets upstream tracking for PR branches, persists any branch-name changes,
/// and links the workdir in the database.
///
/// Returns the worktree path as a string.
pub(crate) fn create_and_link_worktree(
    store: &Arc<Store>,
    branch: &store::Branch,
    repo_path: &std::path::Path,
    desired_worktree_path: &std::path::Path,
) -> Result<String, String> {
    // Serialize worktree creation per branch so that concurrent callers
    // (frontend `setup_worktree` vs backend `setup_worktree_sync`) do not race.
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

    // When a stale worktree from another project holds the branch name, pick
    // a unique local branch name so we don't collide.
    let mut local_branch_name = branch.branch_name.clone();
    let existing_worktree =
        find_existing_worktree_for_branch(repo_path, &branch.branch_name, project_root_ref)?;
    if let ExistingWorktree::Stale = &existing_worktree {
        local_branch_name = unique_branch_name(repo_path, &branch.branch_name)?;
        log::info!(
            "Branch '{}' is checked out in a stale worktree; using '{}' instead",
            branch.branch_name,
            local_branch_name,
        );
    }

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
        let remote_ref = format!("origin/{}", branch.branch_name);
        let pr_head_sha;
        let start_point = if let Some(pr_num) = branch.pr_number {
            pr_head_sha = git::fetch_pr_head_sha(repo_path, pr_num).map_err(|e| e.to_string())?;
            &pr_head_sha
        } else if git::remote_branch_exists(repo_path, &branch.branch_name)
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
            Ok(path) => path,
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
                    path
                } else if git::branch_exists(repo_path, &local_branch_name)
                    .map_err(|e| e.to_string())?
                {
                    log::warn!(
                        "Branch '{}' already exists after create attempt; retrying with existing branch",
                        local_branch_name
                    );
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

    // For PR branches created from a SHA, git won't have set upstream tracking
    // automatically. Set it to origin/<branch_name> if the remote branch exists
    // (same-repo PRs). Fork PRs won't have a matching remote branch, so this
    // is a best-effort no-op for them.
    if branch.pr_number.is_some() {
        let _ = git::set_upstream_to_origin(repo_path, &local_branch_name, &branch.branch_name);
    }

    // If we picked a different local branch name, persist it.
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
    branch_id: String,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    // Idempotent fast-path: if this branch already has a workdir, reuse it.
    if let Some(existing) = store
        .get_workdir_for_branch(&branch.id)
        .map_err(|e| e.to_string())?
    {
        return Ok(to_branch_with_workdir(branch, Some(existing.path)));
    }

    // Ensure we have a local clone, then fetch the specific refs we need.
    let repo_slug = resolve_branch_repo_slug(&store, &project, &branch)?;
    let repo_path = git::ensure_local_clone(&repo_slug).map_err(|e| e.to_string())?;
    fetch_for_worktree_with_offline_fallback(
        &repo_path,
        &repo_slug,
        &branch.branch_name,
        &branch.base_branch,
    )?;
    let desired_worktree_path =
        git::project_worktree_path_for(&branch.project_id, &repo_slug, &branch.branch_name)
            .map_err(|e| e.to_string())?;

    let worktree_str =
        create_and_link_worktree(&store, &branch, &repo_path, &desired_worktree_path)?;

    Ok(to_branch_with_workdir(branch, Some(worktree_str)))
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
    // Delegate to the existing setup_worktree command for worktree creation.
    let result = setup_worktree(store.clone(), branch_id.clone()).await?;

    spawn_prerun_actions(
        get_store(&store)?,
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
    let repo_path = git::ensure_local_clone(&target_repo.github_repo).map_err(|e| e.to_string())?;
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

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    // Track whether this is the first workspace start (Starting → Running)
    // vs a restart (Stopped → Running). We only trigger auto-review on the
    // first start so that existing branches don't get a spurious review
    // every time the workspace restarts.
    let is_first_start = branch.workspace_status == Some(store::WorkspaceStatus::Starting);

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let ws_name = branch
        .workspace_name
        .as_deref()
        .ok_or("Branch has no workspace name")?;
    let repo_subpath = resolve_branch_workspace_subpath(&store, &branch)?;
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

    // Secondary repo setup in an already-running shared workspace.
    if let Some(repo_subpath) = repo_subpath.as_deref() {
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
                    repo_subpath,
                    &repo_slug,
                    &ref_name,
                    &branch.branch_name,
                )
                .await?;
                store
                    .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Running)
                    .map_err(|e| e.to_string())?;

                // Trigger auto-review for the newly cloned secondary repo
                // if this is the first start for this branch.
                if is_first_start {
                    let store_bg = Arc::clone(&store);
                    let app_handle_bg = app_handle.clone();
                    let branch_id_bg = branch_id.clone();
                    tauri::async_runtime::spawn(async move {
                        crate::maybe_trigger_auto_review_for_new_repo(
                            &store_bg,
                            &app_handle_bg,
                            &branch_id_bg,
                            None,
                        )
                        .await;
                    });
                }

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
            let has_remote_branch = run_workspace_git_async(
                ws_name,
                repo_subpath.as_deref(),
                &["fetch", "origin", &branch.branch_name],
            )
            .await
            .is_ok();

            let remote_ref = format!("origin/{}", branch.branch_name);
            let checkout_result = if has_remote_branch {
                run_workspace_git_async(
                    ws_name,
                    repo_subpath.as_deref(),
                    &["checkout", "-B", &branch.branch_name, &remote_ref],
                )
                .await
            } else {
                run_workspace_git_async(
                    ws_name,
                    repo_subpath.as_deref(),
                    &["checkout", "-b", &branch.branch_name],
                )
                .await
            };

            if let Err(e) = checkout_result {
                log::warn!(
                    "failed to create branch '{}' in workspace '{}': {e}",
                    branch.branch_name,
                    ws_name
                );
            }

            // If this is the first workspace start for a new branch, check
            // whether the branch already has commits (e.g. from an existing
            // PR) and kick off an automatic code review.
            if is_first_start {
                let store_bg = Arc::clone(&store);
                let app_handle_bg = app_handle.clone();
                let branch_id_bg = branch_id.clone();
                tauri::async_runtime::spawn(async move {
                    crate::maybe_trigger_auto_review_for_new_repo(
                        &store_bg,
                        &app_handle_bg,
                        &branch_id_bg,
                        None,
                    )
                    .await;
                });
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

    store
        .update_branch_workspace_status(&branch_id, &new_status)
        .map_err(|e| e.to_string())?;

    Ok(PollWorkspaceResult {
        status: new_status.as_str().to_string(),
        workstation_id: cached_workstation_id(ws_name),
    })
}

/// Map a numeric `CommandType` (from the blox orchestrator proto) to a stable
/// string identifier that the frontend can map to a display label.
///
/// Proto enum values:
///   0 = COMMAND_TYPE_UNSPECIFIED
///   1 = COMMAND_TYPE_CHECKOUT
///   2 = COMMAND_TYPE_EXECUTE_PROCESS
///   3 = COMMAND_TYPE_PROJECT_BOOTSTRAP
///   4 = COMMAND_TYPE_PROVISION_WORKSPACE
pub(crate) fn bootstrap_command_type_name(command_type: u32) -> &'static str {
    match command_type {
        1 => "checkout",
        2 => "execute_process",
        3 => "project_bootstrap",
        4 => "provision_workspace",
        _ => "unknown",
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
    let completed = bootstrap.iter().filter(|c| c.status == 3).count();

    // All done — workspace will transition to running, no need to emit.
    if completed == total {
        return;
    }

    // Pick the first currently-running command (status 2).  If none are
    // running yet, fall back to the first non-completed command.
    // Safety: `bootstrap` is non-empty (checked above) and `completed < total`,
    // so at least one command has status != 3 (completed).
    let current_cmd = bootstrap
        .iter()
        .find(|c| c.status == 2)
        .or_else(|| bootstrap.iter().find(|c| c.status != 3))
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

                store
                    .update_branch_workspace_status(branch_id, &new_status)
                    .ok();

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
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    executor: tauri::State<'_, Arc<ActionExecutor>>,
    registry: tauri::State<'_, Arc<ActionRegistry>>,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;

    // Stop any running actions for this branch before cleanup.
    crate::actions::commands::stop_actions_for_branches(&executor, &registry, &[&branch_id]);

    // Get the branch
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    tauri::async_runtime::spawn_blocking({
        let store = Arc::clone(&store);
        let branch = branch.clone();
        move || cleanup_branch_resources(&store, &branch)
    })
    .await
    .map_err(|e| format!("Failed to clean up branch resources: {e}"))??;

    // Delete the branch record (cascades to commits, notes, reviews)
    store.delete_branch(&branch_id).map_err(|e| e.to_string())
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
/// A failed transaction puts the worktree back.
pub(crate) async fn move_branch_impl(
    store: &Arc<Store>,
    executor: &ActionExecutor,
    registry: &ActionRegistry,
    branch_id: &str,
    target_project_id: &str,
) -> Result<BranchWithWorkdir, String> {
    let BranchMovePlan {
        source_project,
        github_repo,
        mv,
    } = plan_branch_move(store, branch_id, target_project_id)?;

    crate::actions::commands::stop_actions_for_branches(executor, registry, &[branch_id]);

    let repo_path = crate::paths::repos_dir()
        .map(|d| d.join(&github_repo))
        .ok_or("Cannot determine clone path")?;
    let workdir_move = mv.workdir.clone();

    // A branch whose setup never finished has nothing on disk to relocate.
    let mut moved_on_disk = false;
    if let Some(wd) = &workdir_move {
        let old_path = PathBuf::from(&wd.old_path);
        let new_path = PathBuf::from(&wd.new_path);
        if old_path.exists() {
            let repo_path = repo_path.clone();
            tauri::async_runtime::spawn_blocking(move || {
                git::move_worktree(&repo_path, &old_path, &new_path)
            })
            .await
            .map_err(|e| format!("Failed to move worktree: {e}"))?
            .map_err(|e| format!("Failed to move worktree: {e}"))?;
            moved_on_disk = true;
        }
    }

    if let Err(e) = store.move_branch_to_project(&mv) {
        // Disk and database have to agree, so undo the half that landed.
        if let (true, Some(wd)) = (moved_on_disk, &workdir_move) {
            if let Err(revert) =
                git::move_worktree(&repo_path, Path::new(&wd.new_path), Path::new(&wd.old_path))
            {
                log::warn!(
                    "Failed to move worktree for branch {branch_id} back to {}: {revert}",
                    wd.old_path
                );
            }
        }
        return Err(e.to_string());
    }

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
            if let Some(wd) = store
                .get_workdir_for_branch(&branch.id)
                .map_err(|e| e.to_string())?
            {
                let status = std::process::Command::new("git")
                    .args(["-C", &wd.path, "branch", "-m", new_name])
                    .status()
                    .map_err(|e| format!("Failed to run git rename: {e}"))?;
                if !status.success() {
                    return Err("Failed to rename local git branch".to_string());
                }
            }
        }
        store::BranchType::Remote => {
            if let Some(ws_name) = &branch.workspace_name {
                let repo_subpath = resolve_branch_workspace_subpath(&store, &branch)?;
                if let Err(e) = run_workspace_git_async(
                    ws_name,
                    repo_subpath.as_deref(),
                    &["branch", "-m", new_name],
                )
                .await
                {
                    log::warn!("Failed to rename branch in workspace '{ws_name}': {e}");
                }
            }
        }
    }

    store
        .update_branch_name(&branch.id, new_name)
        .map_err(|e| e.to_string())?;
    if let Some(repo_id) = &branch.project_repo_id {
        store
            .update_project_repo_branch_name(&branch.project_id, repo_id, new_name)
            .map_err(|e| e.to_string())?;
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
/// Like [`setup_worktree`] but callable without Tauri state (e.g. from the
/// MCP server). Handles cloning with optional progress reporting, then
/// delegates to [`create_and_link_worktree`] for worktree creation and DB
/// linkage.
pub(crate) fn setup_worktree_sync(
    store: &Arc<Store>,
    branch_id: &str,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<String, String> {
    let emit_progress = |phase: &str, detail: Option<String>| {
        if let Some(handle) = app_handle {
            emit_setup_progress(handle, branch_id, phase, detail);
        }
    };

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

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
    let repo_path = if let Some(handle) = app_handle {
        let handle = handle.clone();
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
    } else {
        crate::git::ensure_local_clone(&repo_slug).map_err(|e| e.to_string())?
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
    let worktree_str =
        create_and_link_worktree(store, &branch, &repo_path, &desired_worktree_path)?;

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

        let metadata = ActionMetadata {
            action_id: action.id.clone(),
            action_name: action.name.clone(),
            auto_commit: action.auto_commit,
        };

        // execute_and_wait runs the action and waits for it to finish,
        // regardless of success or failure (task requirement)
        match executor
            .execute_and_wait(action.command, working_dir.clone(), metadata, listener)
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
    use crate::test_utils::TempGitRepo;

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
