use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager};

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

// In-memory cache: workspace name → numeric workstation ID.
// Populated by `poll_workspace_status` and `start_workspace` when `blox ws info`
// returns an ID; read by `to_branch_with_workdir` when serializing for the frontend.
fn workstation_id_cache() -> &'static Mutex<HashMap<String, u64>> {
    static CACHE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_workstation_id(workspace_name: &str) -> Option<u64> {
    workstation_id_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(workspace_name).copied())
}

fn get_store(store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized — please reset from the startup prompt".into())
}

fn to_branch_with_workdir(
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
        setup_complete: branch.setup_complete,
        worktree_path: workdir_path,
        created_at: branch.created_at,
        updated_at: branch.updated_at,
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

pub(crate) fn run_workspace_git(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    git_args: &[&str],
) -> Result<String, blox::BloxError> {
    let mut owned = Vec::<String>::new();
    owned.push("git".to_string());
    if let Some(subpath) = repo_subpath.map(str::trim).filter(|s| !s.is_empty()) {
        let resolved = resolve_workspace_repo_path(workspace_name, subpath)?;
        owned.push("-C".to_string());
        owned.push(resolved);
    }
    owned.extend(git_args.iter().map(|arg| (*arg).to_string()));
    let borrowed = owned.iter().map(String::as_str).collect::<Vec<_>>();
    blox::ws_exec(workspace_name, &borrowed)
}

pub(crate) fn run_workspace_git_bytes(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    git_args: &[&str],
) -> Result<Vec<u8>, blox::BloxError> {
    let mut owned = Vec::<String>::new();
    owned.push("git".to_string());
    if let Some(subpath) = repo_subpath.map(str::trim).filter(|s| !s.is_empty()) {
        let resolved = resolve_workspace_repo_path(workspace_name, subpath)?;
        owned.push("-C".to_string());
        owned.push(resolved);
    }
    owned.extend(git_args.iter().map(|arg| (*arg).to_string()));
    let borrowed = owned.iter().map(String::as_str).collect::<Vec<_>>();
    blox::ws_exec_bytes(workspace_name, &borrowed)
}

async fn run_blox_blocking<T, F>(op: F) -> Result<T, blox::BloxError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, blox::BloxError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(op)
        .await
        .map_err(|e| blox::BloxError::CommandFailed(format!("blox task failed: {e}")))?
}

async fn ws_exec_async(workspace_name: &str, args: &[&str]) -> Result<String, blox::BloxError> {
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

async fn run_workspace_git_async(
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
    let workspace_path = match &repo.subpath {
        Some(subpath) => {
            let validated = validate_workspace_subpath(subpath)?;
            format!("home:{clone_dir}/{validated}")
        }
        None => format!("home:{clone_dir}"),
    };
    Ok(Some(workspace_path))
}

fn normalize_branch_ref(branch: &str) -> String {
    branch.strip_prefix("origin/").unwrap_or(branch).to_string()
}

/// Clone a repo into an already-running workspace, fetch the base branch,
/// and create the feature branch.
///
/// This is used both by `start_workspace` (secondary repo in a shared
/// workspace) and by `add_project_repo` (adding a repo to a remote project
/// whose workspace is already running).
async fn clone_repo_into_workspace(
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

fn find_existing_worktree_for_branch(
    repo_path: &Path,
    branch_name: &str,
) -> Result<Option<PathBuf>, String> {
    Ok(git::list_worktrees(repo_path)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find_map(|(path, wt_branch)| match wt_branch.as_deref() {
            Some(name) if name == branch_name => Some(path),
            _ => None,
        }))
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

fn create_worktree_for_existing_branch_with_fallback(
    repo_path: &Path,
    branch_name: &str,
    desired_worktree_path: &Path,
) -> Result<PathBuf, String> {
    match git::create_worktree_for_existing_branch_at_path(
        repo_path,
        branch_name,
        desired_worktree_path,
    ) {
        Ok(path) => Ok(path),
        Err(err) => {
            if let Some(path) = find_existing_worktree_for_branch(repo_path, branch_name)? {
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
) -> Result<PathBuf, String> {
    match git::create_worktree_at_path(repo_path, branch_name, base_branch, desired_worktree_path) {
        Ok(path) => Ok(path),
        Err(err) => {
            if let Some(path) = find_existing_worktree_for_branch(repo_path, branch_name)? {
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

fn is_blox_onboarding_precondition_error(err: &blox::BloxError) -> bool {
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
    match branch.branch_type {
        store::BranchType::Local => {
            let project = store
                .get_project(&branch.project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

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

pub(crate) fn infer_branch_name(project_name: &str) -> String {
    let branch = project_name
        .to_lowercase()
        .replace([' ', '_'], "-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '/' || *c == '.')
        .collect::<String>()
        .replace(['.', '/'], "-")
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if branch.is_empty() {
        "feature".to_string()
    } else {
        branch
    }
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
        return "mrk-feature".to_string();
    }
    let mut full = format!("mrk-{safe}");
    if full.len() > WORKSPACE_NAME_MAX_LENGTH {
        full = full[..WORKSPACE_NAME_MAX_LENGTH]
            .trim_end_matches('-')
            .to_string();
    }
    full
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

    let mut result = Vec::with_capacity(branches.len());
    for branch in branches {
        let workdir = store
            .get_workdir_for_branch(&branch.id)
            .map_err(|e| e.to_string())?;

        let bw = to_branch_with_workdir(branch, workdir.map(|w| w.path));
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
    git::fetch_for_worktree(
        &repo_path,
        &repo_slug,
        &branch.branch_name,
        &branch.base_branch,
    )
    .map_err(|e| e.to_string())?;
    let desired_worktree_path =
        git::project_worktree_path_for(&branch.project_id, &repo_slug, &branch.branch_name)
            .map_err(|e| e.to_string())?;

    // Reuse any existing worktree for this branch; otherwise create one.
    let existing_worktree_path =
        find_existing_worktree_for_branch(&repo_path, &branch.branch_name)?;

    let worktree_path = if let Some(path) = existing_worktree_path {
        path
    } else if git::branch_exists(&repo_path, &branch.branch_name).map_err(|e| e.to_string())? {
        create_worktree_for_existing_branch_with_fallback(
            &repo_path,
            &branch.branch_name,
            &desired_worktree_path,
        )
        .map_err(|e| e.to_string())?
    } else {
        // If the branch exists on the remote (e.g. from an existing PR),
        // start the new local branch from the remote tracking ref so it
        // includes the PR's commits. Otherwise fall back to base_branch
        // for genuinely new branches.
        let remote_ref = format!("origin/{}", branch.branch_name);
        let start_point = if git::remote_branch_exists(&repo_path, &branch.branch_name)
            .map_err(|e| e.to_string())?
        {
            &remote_ref
        } else {
            &branch.base_branch
        };
        match create_worktree_with_fallback(
            &repo_path,
            &branch.branch_name,
            start_point,
            &desired_worktree_path,
        ) {
            Ok(path) => path,
            Err(create_err) => {
                if let Some(path) =
                    find_existing_worktree_for_branch(&repo_path, &branch.branch_name)?
                {
                    log::warn!(
                        "Reusing existing worktree '{}' for branch '{}' after create failure",
                        path.display(),
                        branch.branch_name
                    );
                    path
                } else if git::branch_exists(&repo_path, &branch.branch_name)
                    .map_err(|e| e.to_string())?
                {
                    // Handle races/stale refs where the branch appears between our
                    // pre-check and `git worktree add -b ...`.
                    log::warn!(
                        "Branch '{}' already exists after create attempt; retrying with existing branch in repo '{}'",
                        branch.branch_name,
                        repo_slug
                    );
                    create_worktree_for_existing_branch_with_fallback(
                        &repo_path,
                        &branch.branch_name,
                        &desired_worktree_path,
                    )
                    .map_err(|e| e.to_string())?
                } else {
                    return Err(create_err);
                }
            }
        }
    };

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

    Ok(to_branch_with_workdir(branch, Some(worktree_str)))
}

/// Like [`setup_worktree`], but also runs prerun actions after the worktree is
/// ready.  Used by the frontend retry path so that a failed initial setup
/// (which skips prerun actions) can be fully recovered by the user.
#[tauri::command(rename_all = "camelCase")]
pub async fn setup_worktree_and_run_prerun(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: AppHandle,
    branch_id: String,
) -> Result<BranchWithWorkdir, String> {
    // Delegate to the existing setup_worktree command for worktree creation.
    let result = setup_worktree(store.clone(), branch_id.clone()).await?;

    let store = get_store(&store)?;

    // Atomically claim setup ownership — only run prerun actions if we win.
    match store.mark_branch_setup_complete(&branch_id) {
        Ok(true) => {
            let executor = app_handle.state::<Arc<ActionExecutor>>();
            let act_registry = app_handle.state::<Arc<ActionRegistry>>();
            match run_prerun_actions_for_branch(
                &store,
                &app_handle,
                &branch_id,
                &executor,
                &act_registry,
            )
            .await
            {
                Ok(count) => {
                    log::info!("[setup_worktree_and_run_prerun] ran {count} prerun actions");
                }
                Err(e) => {
                    log::warn!("[setup_worktree_and_run_prerun] prerun actions failed: {e}");
                }
            }
        }
        Ok(false) => {
            log::info!(
                "[setup_worktree_and_run_prerun] branch {} already setup complete, skipping prerun",
                branch_id
            );
        }
        Err(e) => {
            log::warn!("[setup_worktree_and_run_prerun] failed to mark setup complete: {e}");
        }
    }

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
        Some(source) => format!("sq blox ws start {} {}", ws_name, source),
        None => format!("sq blox ws start {}", ws_name),
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
        move || blox::ws_start(&ws_name, source.as_deref())
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
fn map_blox_status_to_workspace_status(
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

/// Poll workspace statuses for multiple branches in a single `sq blox ws list` call.
///
/// Returns a map from branch ID to `PollWorkspaceResult`. Branches that cannot be
/// resolved (e.g. missing workspace name, not found in store) are silently omitted
/// from the result rather than failing the entire batch.
/// Best-effort label for a bootstrap command type.
///
/// These numeric types are undocumented in the blox-cli crate, so we map the
/// ones we've observed and fall back to a generic label for anything new.
/// Display order is chosen so that cloning (type 1) sorts last — it tends to
/// be the longest step for most users.
fn bootstrap_phase_label(command_type: u32) -> &'static str {
    match command_type {
        4 => "Starting services…",
        3 => "Running setup…",
        1 => "Cloning repository…",
        _ => "Setting up…",
    }
}

/// Sort key for bootstrap command types so that cloning (type 1) is displayed
/// as the final step.  Lower values sort first.
fn bootstrap_sort_order(command_type: u32) -> u32 {
    match command_type {
        4 => 0, // services first
        3 => 1, // setup second
        1 => 2, // cloning last
        _ => 1, // unknown types slot in the middle
    }
}

/// Derive workspace setup progress from bootstrap commands and emit events
/// for all branches sharing the workspace.
fn emit_workspace_setup_progress(
    app_handle: &AppHandle,
    store: &Arc<Store>,
    ws_name: &str,
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

    // Sort bootstrap commands by display order to assign step numbers.
    let mut sorted: Vec<&blox::WorkspaceCommand> = bootstrap.clone();
    sorted.sort_by_key(|c| (bootstrap_sort_order(c.command_type), c.command_id.clone()));

    // Find the step number of the first non-completed command in display order.
    let (step_index, current_cmd) = sorted
        .iter()
        .enumerate()
        .find(|(_, c)| c.status != 3)
        .unwrap_or((completed, sorted.last().unwrap()));

    let phase = bootstrap_phase_label(current_cmd.command_type);
    let detail = Some(format!(
        "Step {} of {} · {}",
        step_index + 1,
        total,
        current_cmd.command_id
    ));

    // Collect branch IDs that share this workspace.
    let peer_branch_ids: Vec<String> = branch_ids
        .iter()
        .filter(|bid| {
            store
                .get_branch(bid)
                .ok()
                .flatten()
                .and_then(|b| b.workspace_name)
                .as_deref()
                == Some(ws_name)
        })
        .cloned()
        .collect();

    for bid in peer_branch_ids {
        let _ = app_handle.emit(
            "workspace-setup-progress",
            WorktreeSetupProgress {
                branch_id: bid,
                phase: phase.to_string(),
                detail: detail.clone(),
            },
        );
    }
}

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

    // Collect workspace names that are currently in "starting" state so we can
    // fetch their bootstrap command progress.
    let mut starting_workspaces: Vec<String> = Vec::new();

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
                if new_status == store::WorkspaceStatus::Starting
                    && !starting_workspaces.contains(&ws_name.to_string())
                {
                    starting_workspaces.push(ws_name.to_string());
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
                    if !starting_workspaces.contains(&ws_name.to_string()) {
                        starting_workspaces.push(ws_name.to_string());
                    }
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
        let branch_ids_owned: Vec<String> = branch_ids.clone();
        let store_clone = store.clone();
        let app_handle_clone = app_handle.clone();

        // Fetch commands for each starting workspace in a blocking task to
        // avoid holding up the response.
        tauri::async_runtime::spawn_blocking(move || {
            for ws_name in starting_workspaces {
                match blox::ws_commands(&ws_name) {
                    Ok(cmds) => {
                        emit_workspace_setup_progress(
                            &app_handle_clone,
                            &store_clone,
                            &ws_name,
                            &branch_ids_owned,
                            &cmds,
                        );
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
/// This replicates the core logic from `branches::setup_worktree` without
/// requiring Tauri state, so it can be called from the MCP server.
pub(crate) fn setup_worktree_sync(
    store: &Arc<Store>,
    branch_id: &str,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<String, String> {
    let emit_progress = |phase: &str, detail: Option<String>| {
        if let Some(handle) = app_handle {
            let _ = handle.emit(
                "worktree-setup-progress",
                WorktreeSetupProgress {
                    branch_id: branch_id.to_string(),
                    phase: phase.to_string(),
                    detail,
                },
            );
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
                    let _ = handle.emit(
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
    crate::git::fetch_for_worktree(
        &repo_path,
        &repo_slug,
        &branch.branch_name,
        &branch.base_branch,
    )
    .map_err(|e| e.to_string())?;
    let desired_worktree_path =
        crate::git::project_worktree_path_for(&branch.project_id, &repo_slug, &branch.branch_name)
            .map_err(|e| e.to_string())?;

    emit_progress("creating_worktree", None);
    // Reuse any existing worktree for this branch; otherwise create one.
    let existing_worktree_path = crate::git::list_worktrees(&repo_path)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find_map(|(path, wt_branch)| match wt_branch.as_deref() {
            Some(name) if name == branch.branch_name => Some(path),
            _ => None,
        });

    let worktree_path = if let Some(path) = existing_worktree_path {
        path
    } else if crate::git::branch_exists(&repo_path, &branch.branch_name)
        .map_err(|e| e.to_string())?
    {
        crate::git::create_worktree_for_existing_branch_at_path(
            &repo_path,
            &branch.branch_name,
            &desired_worktree_path,
        )
        .map_err(|e| e.to_string())?
    } else {
        // If the branch exists on the remote (e.g. from an existing PR),
        // start the new local branch from the remote tracking ref so it
        // includes the PR's commits. Otherwise fall back to base_branch
        // for genuinely new branches.
        let remote_ref = format!("origin/{}", branch.branch_name);
        let start_point = if crate::git::remote_branch_exists(&repo_path, &branch.branch_name)
            .map_err(|e| e.to_string())?
        {
            &remote_ref
        } else {
            &branch.base_branch
        };
        match crate::git::create_worktree_at_path(
            &repo_path,
            &branch.branch_name,
            start_point,
            &desired_worktree_path,
        ) {
            Ok(path) => path,
            Err(create_err) => {
                if crate::git::branch_exists(&repo_path, &branch.branch_name)
                    .map_err(|e| e.to_string())?
                {
                    log::warn!(
                        "[project_mcp] Branch '{}' already exists after create attempt; retrying with existing branch",
                        branch.branch_name
                    );
                    crate::git::create_worktree_for_existing_branch_at_path(
                        &repo_path,
                        &branch.branch_name,
                        &desired_worktree_path,
                    )
                    .map_err(|e| e.to_string())?
                } else {
                    return Err(create_err.to_string());
                }
            }
        }
    };

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
            let workdir = crate::store::Workdir::new(&branch.project_id, &worktree_str)
                .with_branch(&branch.id);
            store.create_workdir(&workdir).map_err(|e| e.to_string())?;
        }
    }

    Ok(worktree_str)
}

/// Run detect_actions (if needed) and all prerun actions for a branch.
///
/// This replicates the core logic from `actions::commands::run_prerun_actions`
/// without requiring Tauri state.
pub(crate) async fn run_prerun_actions_for_branch(
    store: &Arc<Store>,
    app_handle: &AppHandle,
    branch_id: &str,
    executor: &Arc<ActionExecutor>,
    act_registry: &Arc<ActionRegistry>,
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

    // If actions haven't been detected yet for this repo+subpath, detect now
    if !context.has_detected_actions {
        log::info!(
            "[project_mcp] detecting actions for repo {} (subpath: {:?})",
            github_repo,
            subpath
        );
        store
            .set_action_context_detecting(&context.id, true)
            .map_err(|e| format!("Failed to set detection status: {e}"))?;

        let _ = app_handle.emit(
            "repo-actions-detection",
            serde_json::json!({
                "githubRepo": github_repo,
                "subpath": subpath,
                "detecting": true,
            }),
        );

        // Run detection (may call out to AI)
        let detected = crate::actions::commands::detect_actions_for_repo_context(
            &github_repo,
            subpath.as_deref(),
        )
        .await
        .unwrap_or_default();

        // Persist detected actions (skip duplicates)
        let existing_actions = store
            .list_repo_actions(&context.id)
            .map_err(|e| format!("Failed to list actions: {e}"))?;
        let mut existing_commands: std::collections::HashSet<String> =
            existing_actions.iter().map(|a| a.command.clone()).collect();
        let mut next_sort_order = existing_actions
            .iter()
            .map(|a| a.sort_order)
            .max()
            .unwrap_or(-1)
            + 1;

        for suggestion in detected {
            if existing_commands.contains(&suggestion.command) {
                continue;
            }
            existing_commands.insert(suggestion.command.clone());
            let action = crate::store::RepoAction::new(
                context.id.clone(),
                suggestion.name,
                suggestion.command,
                suggestion.action_type,
                next_sort_order,
            )
            .with_auto_commit(suggestion.auto_commit);
            store
                .create_repo_action(&action)
                .map_err(|e| format!("Failed to create detected action: {e}"))?;
            next_sort_order += 1;
        }

        store
            .mark_action_context_detected(&context.id)
            .map_err(|e| format!("Failed to update detection status: {e}"))?;

        let _ = app_handle.emit(
            "repo-actions-detection",
            serde_json::json!({
                "githubRepo": github_repo,
                "subpath": subpath,
                "detecting": false,
            }),
        );
    }

    // Get all prerun actions for this context
    let actions = store
        .list_repo_actions(&context.id)
        .map_err(|e| format!("Failed to list actions: {e}"))?;
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
                log::info!(
                    "[project_mcp] prerun action '{}' completed for branch {}",
                    action.id,
                    branch_id
                );
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
