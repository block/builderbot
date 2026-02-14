//! Staged — clean rewrite.
//!
//! Tauri commands for the new frontend, built incrementally.
//! See `src-archive/lib.rs` for the previous implementation.

pub mod actions;
pub mod agent;
pub mod blox;
pub mod doctor;
pub mod git;
pub mod paths;
mod recent_repos;
pub mod session_commands;
pub mod session_runner;
pub mod store;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use store::Store;
use tauri::{Emitter, Manager};

// =============================================================================
// Managed state
// =============================================================================

/// Holds the database path and optional incompatibility info detected
/// at startup. When `needs_reset` is `Some`, the store has not been
/// created yet — the frontend shows a confirmation dialog and then
/// calls `confirm_reset_store` to proceed.
struct DbState {
    db_path: PathBuf,
    needs_reset: Mutex<Option<StoreIncompatibility>>,
}

/// Structured info about a database incompatibility, passed to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoreIncompatibility {
    /// The app version that last used the database (e.g. "0.1.0").
    db_app_version: String,
    /// The version of this build (e.g. "0.2.0").
    app_version: String,
    /// Whether the user can reset, or must update the app instead.
    /// "needs_reset" = old DB, offer wipe. "too_new" = newer DB, suggest update.
    kind: String,
}

// =============================================================================
// Frontend-facing types (enriched views of store models)
// =============================================================================

/// Branch enriched with its workdir path (if any).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchWithWorkdir {
    pub id: String,
    pub project_id: String,
    pub branch_name: String,
    pub base_branch: String,
    pub pr_number: Option<u64>,
    pub branch_type: store::BranchType,
    pub workspace_name: Option<String>,
    pub workspace_status: Option<store::WorkspaceStatus>,
    pub worktree_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Commit info combining git data with our metadata.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitTimelineItem {
    /// DB id – present for pending commits so they can be deleted by id.
    pub id: Option<String>,
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author: String,
    pub timestamp: i64,
    pub session_id: Option<String>,
    pub session_status: Option<String>,
}

/// Note with session status resolved.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteTimelineItem {
    pub id: String,
    pub title: String,
    pub content: String,
    pub session_id: Option<String>,
    pub session_status: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Review with session status resolved.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewTimelineItem {
    pub id: String,
    pub commit_sha: String,
    pub scope: String,
    pub session_id: Option<String>,
    pub session_status: Option<String>,
    pub comment_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Composite timeline for a branch.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchTimeline {
    pub commits: Vec<CommitTimelineItem>,
    pub notes: Vec<NoteTimelineItem>,
    pub reviews: Vec<ReviewTimelineItem>,
}

// =============================================================================
// Helper — get the store or return a clear error
// =============================================================================

fn get_store(store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized — please reset from the startup prompt".into())
}

// =============================================================================
// File browsing commands
// =============================================================================

/// Entry in a directory listing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
    is_repo: bool,
}

/// List contents of a directory.
/// Returns directories first (sorted), then files (sorted).
/// Hidden files (starting with .) are excluded.
#[tauri::command(rename_all = "camelCase")]
async fn list_directory(path: String) -> Result<Vec<DirEntry>, String> {
    let dir = Path::new(&path);

    if !dir.exists() {
        return Err(format!("Directory does not exist: {path}"));
    }
    if !dir.is_dir() {
        return Err(format!("Not a directory: {path}"));
    }

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    let entries = std::fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {e}"))?;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }

        let entry_path = entry.path();
        let is_dir = entry_path.is_dir();
        let is_repo = is_dir && entry_path.join(".git").exists();

        let item = DirEntry {
            name,
            path: entry_path.to_string_lossy().to_string(),
            is_dir,
            is_repo,
        };

        if is_dir {
            dirs.push(item);
        } else {
            files.push(item);
        }
    }

    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    dirs.extend(files);
    Ok(dirs)
}

/// Folders to skip during recursive search.
const SKIP_FOLDERS: &[&str] = &[
    "Library",
    "Applications",
    "System",
    "Volumes",
    "cores",
    "private",
    "node_modules",
    "target",
    "build",
    "dist",
    "vendor",
    ".git",
    "__pycache__",
    "venv",
    ".venv",
    "env",
    ".cargo",
    ".rustup",
    ".npm",
    ".cache",
    "Caches",
    "Movies",
    "Music",
    "Pictures",
    "Photos Library.photoslibrary",
];

/// Common development folder names — searched first when at home directory.
const DEV_FOLDERS: &[&str] = &[
    "dev",
    "projects",
    "code",
    "repos",
    "src",
    "workspace",
    "work",
    "github",
    "gitlab",
    "Development",
    "Documents",
    "Desktop",
];

/// Search for git repositories matching a query.
/// Only returns directories containing a `.git` folder.
/// When at home directory, only searches inside common dev folders.
#[tauri::command(rename_all = "camelCase")]
async fn search_directories(
    path: String,
    query: String,
    max_depth: Option<u32>,
    limit: Option<usize>,
) -> Result<Vec<DirEntry>, String> {
    let dir = Path::new(&path);
    let max_depth = max_depth.unwrap_or(6);
    let limit = limit.unwrap_or(20);
    let query_lower = query.to_lowercase();

    if !dir.exists() || !dir.is_dir() {
        return Err(format!("Invalid directory: {path}"));
    }

    let mut results = Vec::new();
    let collect_limit = limit * 3; // over-collect for ranking

    let home_dir = dirs::home_dir();
    let is_home = home_dir.as_ref().is_some_and(|h| h == dir);

    if is_home {
        for dev_folder in DEV_FOLDERS {
            let dev_path = dir.join(dev_folder);
            if dev_path.exists() && dev_path.is_dir() {
                search_repos_recursive(
                    &dev_path,
                    &query_lower,
                    0,
                    max_depth,
                    &mut results,
                    collect_limit,
                );
                if results.len() >= collect_limit {
                    break;
                }
            }
        }
    } else {
        search_repos_recursive(dir, &query_lower, 0, max_depth, &mut results, collect_limit);
    }

    // Sort: exact name matches first, then by path depth (shallower = better)
    results.sort_by(|a, b| {
        let a_exact = a.name.to_lowercase() == query_lower;
        let b_exact = b.name.to_lowercase() == query_lower;
        if a_exact != b_exact {
            return b_exact.cmp(&a_exact);
        }
        a.path
            .matches('/')
            .count()
            .cmp(&b.path.matches('/').count())
    });
    results.truncate(limit);

    Ok(results)
}

/// Recursive helper — only adds directories that contain `.git`.
fn search_repos_recursive(
    dir: &Path,
    query: &str,
    depth: u32,
    max_depth: u32,
    results: &mut Vec<DirEntry>,
    limit: usize,
) -> bool {
    if depth > max_depth || results.len() >= limit {
        return results.len() >= limit;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || SKIP_FOLDERS.contains(&name.as_str()) {
            continue;
        }

        let entry_path = entry.path();
        if !entry_path.is_dir() {
            continue;
        }

        let is_repo = entry_path.join(".git").exists();

        if is_repo {
            let name_lower = name.to_lowercase();
            if query.is_empty() || name_lower.starts_with(query) || name_lower.contains(query) {
                results.push(DirEntry {
                    name: name.clone(),
                    path: entry_path.to_string_lossy().to_string(),
                    is_dir: true,
                    is_repo: true,
                });
                if results.len() >= limit {
                    return true;
                }
            }
            // Don't recurse into repos
        } else if search_repos_recursive(&entry_path, query, depth + 1, max_depth, results, limit) {
            return true;
        }
    }

    false
}

/// Get the user's home directory.
#[tauri::command]
fn get_home_dir() -> Result<String, String> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Could not determine home directory".to_string())
}

/// Find git repositories recently active via macOS Spotlight.
#[tauri::command(rename_all = "camelCase")]
async fn find_recent_repos(
    hours_ago: Option<u32>,
    limit: Option<usize>,
) -> Vec<recent_repos::RecentRepo> {
    recent_repos::find_recent_repos(hours_ago.unwrap_or(24), limit.unwrap_or(10))
}

// =============================================================================
// Store status commands
// =============================================================================

/// Returns null if the store is ready, or version info if a reset is needed.
#[tauri::command]
fn get_store_status(db_state: tauri::State<'_, DbState>) -> Option<StoreIncompatibility> {
    db_state.needs_reset.lock().unwrap().clone()
}

/// Delete the old database and create a fresh store.
///
/// Called after the user confirms the reset dialog.
#[tauri::command]
fn confirm_reset_store(
    db_state: tauri::State<'_, DbState>,
    store_slot: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<(), String> {
    store::remove_db_files(&db_state.db_path).map_err(|e| e.to_string())?;

    let s = Store::new(&db_state.db_path).map_err(|e| e.to_string())?;
    *store_slot.lock().unwrap() = Some(Arc::new(s));
    *db_state.needs_reset.lock().unwrap() = None;
    Ok(())
}

// =============================================================================
// Project commands
// =============================================================================

#[tauri::command]
fn list_projects(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<store::Project>, String> {
    get_store(&store)?
        .list_projects()
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn create_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    github_repo: String,
    subpath: Option<String>,
) -> Result<store::Project, String> {
    let store = get_store(&store)?;

    // Check for duplicate (must match both repo and subpath)
    if let Some(existing) = store
        .get_project_by_repo_and_subpath(&github_repo, subpath.as_deref())
        .map_err(|e| e.to_string())?
    {
        return Ok(existing);
    }

    let mut project = store::Project::new(&github_repo);
    if let Some(sub) = subpath {
        project = project.with_subpath(sub);
    }
    store.create_project(&project).map_err(|e| e.to_string())?;

    Ok(project)
}

/// List the authenticated user's GitHub organization memberships.
#[tauri::command]
async fn list_github_orgs() -> Result<Vec<String>, String> {
    git::list_github_orgs().map_err(|e| e.to_string())
}

/// List GitHub repositories for the authenticated user or a specific owner.
#[tauri::command]
async fn list_github_repos(owner: Option<String>) -> Result<Vec<git::GitHubRepo>, String> {
    git::list_github_repos(owner.as_deref()).map_err(|e| e.to_string())
}

/// Search GitHub repositories for the authenticated user or a specific owner.
#[tauri::command]
async fn search_github_repos(
    query: String,
    owner: Option<String>,
) -> Result<Vec<git::GitHubRepo>, String> {
    git::search_github_repos(&query, owner.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    id: String,
) -> Result<(), String> {
    get_store(&store)?
        .delete_project(&id)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Branch commands
// =============================================================================

/// Helper to convert a Branch + optional Workdir into a BranchWithWorkdir.
fn to_branch_with_workdir(
    branch: store::Branch,
    workdir_path: Option<String>,
) -> BranchWithWorkdir {
    BranchWithWorkdir {
        id: branch.id,
        project_id: branch.project_id,
        branch_name: branch.branch_name,
        base_branch: branch.base_branch,
        pr_number: branch.pr_number,
        branch_type: branch.branch_type,
        workspace_name: branch.workspace_name,
        workspace_status: branch.workspace_status,
        worktree_path: workdir_path,
        created_at: branch.created_at,
        updated_at: branch.updated_at,
    }
}

#[tauri::command]
fn list_branches_for_project(
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
fn create_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    branch_name: String,
    base_branch: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;

    // Get the project
    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    // Detect default branch if none specified (via GitHub API, no local clone needed)
    let effective_base = match base_branch {
        Some(b) => b,
        None => {
            git::detect_default_branch_for_repo(&project.github_repo).map_err(|e| e.to_string())?
        }
    };

    // Create branch record only — no git worktree yet
    let branch = store::Branch::new(&project_id, &branch_name, &effective_base);
    store.create_branch(&branch).map_err(|e| e.to_string())?;

    Ok(to_branch_with_workdir(branch, None))
}

/// Create the git worktree for a local branch and record its workdir.
///
/// Separated from `create_branch` so the frontend can dismiss the modal
/// immediately and show a "Creating worktree…" spinner on the branch card
/// while this runs in the background.
#[tauri::command(rename_all = "camelCase")]
async fn setup_worktree(
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

    // Ensure we have a local clone (clones on first use, fetches on subsequent)
    let repo_path = git::ensure_local_clone(&project.github_repo).map_err(|e| e.to_string())?;

    // Create git branch + worktree
    let worktree_path = git::create_worktree(&repo_path, &branch.branch_name, &branch.base_branch)
        .map_err(|e| e.to_string())?;

    let worktree_str = worktree_path
        .to_str()
        .ok_or("Invalid worktree path")?
        .to_string();

    // Create workdir record assigned to this branch
    let workdir = store::Workdir::new(&branch.project_id, &worktree_str).with_branch(&branch.id);
    store.create_workdir(&workdir).map_err(|e| e.to_string())?;

    Ok(to_branch_with_workdir(branch, Some(worktree_str)))
}

/// Import a GitHub PR as a local branch with a worktree.
///
/// Fetches the PR's head ref, creates a local branch at that commit, sets up
/// a git worktree, and records everything in the DB. Returns the branch with
/// `worktree_path` already populated so the frontend doesn't need to call
/// `setup_worktree` separately.
#[tauri::command(rename_all = "camelCase")]
async fn setup_worktree_from_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    pr_number: u64,
    head_ref: String,
    base_ref: String,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;

    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    // Ensure we have a local clone
    let repo_path = git::ensure_local_clone(&project.github_repo).map_err(|e| e.to_string())?;

    // Fetch PR head and create worktree
    let (worktree_path, branch_name, base_branch) =
        git::create_worktree_from_pr(&repo_path, pr_number, &head_ref, &base_ref)
            .map_err(|e| e.to_string())?;

    let worktree_str = worktree_path
        .to_str()
        .ok_or("Invalid worktree path")?
        .to_string();

    // Create branch record with PR number
    let branch = store::Branch::new(&project_id, &branch_name, &base_branch).with_pr(pr_number);
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
async fn create_remote_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    branch_name: String,
    base_branch: Option<String>,
    workspace_name: String,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;

    // Get the project
    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    // Detect default branch if none specified (via GitHub API, no local clone needed)
    let effective_base = match base_branch {
        Some(b) => b,
        None => {
            git::detect_default_branch_for_repo(&project.github_repo).map_err(|e| e.to_string())?
        }
    };

    // Create the branch record (starts in Starting status)
    let branch =
        store::Branch::new_remote(&project_id, &branch_name, &effective_base, &workspace_name);
    store.create_branch(&branch).map_err(|e| e.to_string())?;

    Ok(to_branch_with_workdir(branch, None))
}

/// Start the Blox workspace for a remote branch.
///
/// Separated from `create_remote_branch` so the frontend can dismiss the
/// dialog immediately and show the card in its "Provisioning…" state while
/// this runs in the background.
#[tauri::command(rename_all = "camelCase")]
async fn start_workspace(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;

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

    // Pre-flight: verify the user is authenticated with Blox before starting
    // a workspace. Without this check, `blox ws start` can hang or fail
    // opaquely, leaving the frontend spinning for minutes (issue #99).
    if let Err(e) = blox::check_auth() {
        store
            .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
            .ok();
        return Err(e.to_string());
    }

    // Construct the HTTPS git URL directly from github_repo.
    let ref_name = branch
        .base_branch
        .strip_prefix("origin/")
        .unwrap_or(&branch.base_branch);
    let resolved_source = Some(format!(
        "https://github.com/{}.git?ref={}",
        project.github_repo, ref_name
    ));

    match blox::ws_start(ws_name, resolved_source.as_deref()) {
        Ok(_) => {
            // Create the feature branch inside the workspace so work happens
            // on `branch_name` rather than the detached base ref.
            if let Err(e) = blox::ws_exec(ws_name, &["git", "checkout", "-b", &branch.branch_name])
            {
                log::warn!(
                    "failed to create branch '{}' in workspace '{}': {e}",
                    branch.branch_name,
                    ws_name
                );
            }
            Ok(())
        }
        Err(blox::BloxError::NotAuthenticated) => {
            // Auth errors are definitive — mark as Error so the frontend
            // stops polling and shows an actionable message.
            store
                .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                .ok();
            Err("Not authenticated with Blox. Run: sq login".to_string())
        }
        Err(e) => {
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

/// Get info about a remote branch's Blox workspace.
#[tauri::command(rename_all = "camelCase")]
async fn get_workspace_info(
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

    blox::ws_info(&ws_name).map_err(|e| e.to_string())
}

/// Poll a remote branch's workspace status, update the DB, and return the new status.
///
/// This is the primary mechanism for the frontend to detect when a workspace
/// transitions from `Starting` to `Running` (or `Error`). It queries the
/// Blox CLI, maps the reported status to our `WorkspaceStatus` enum, persists
/// the change, and returns the updated status string.
#[tauri::command(rename_all = "camelCase")]
async fn poll_workspace_status(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<String, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let ws_name = branch
        .workspace_name
        .as_deref()
        .ok_or("Branch is not a remote workspace branch")?;

    let info = match blox::ws_info(ws_name) {
        Ok(info) => info,
        Err(blox::BloxError::NotAuthenticated) => {
            // Auth errors are definitive — stop polling and surface the error.
            store
                .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                .ok();
            return Err("Not authenticated with Blox. Run: sq login".to_string());
        }
        Err(e) => {
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
                return Ok(store::WorkspaceStatus::Starting.as_str().to_string());
            }
            return Err(e.to_string());
        }
    };

    // Map the CLI-reported status to our enum.
    // During initial startup, Blox may briefly report "stopped" before the
    // workspace transitions to "running". If the DB still says Starting,
    // treat a Blox "stopped" as still Starting so we keep polling.
    let new_status = match info.status.as_deref() {
        Some("running") | Some("Running") => store::WorkspaceStatus::Running,
        Some("stopped") | Some("Stopped") => {
            if branch.workspace_status == Some(store::WorkspaceStatus::Starting) {
                store::WorkspaceStatus::Starting
            } else {
                store::WorkspaceStatus::Stopped
            }
        }
        Some("starting") | Some("Starting") | Some("provisioning") | Some("Provisioning") => {
            store::WorkspaceStatus::Starting
        }
        Some("error") | Some("Error") | Some("failed") | Some("Failed") => {
            store::WorkspaceStatus::Error
        }
        // If the CLI returns an unrecognized status, keep it as Starting
        // (optimistic — the workspace may still be booting)
        _ => store::WorkspaceStatus::Starting,
    };

    store
        .update_branch_workspace_status(&branch_id, &new_status)
        .map_err(|e| e.to_string())?;

    Ok(new_status.as_str().to_string())
}

#[tauri::command]
async fn delete_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;

    // Get the branch
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    match branch.branch_type {
        store::BranchType::Local => {
            // Get the project for its github_repo (to derive clone path)
            let project = store
                .get_project(&branch.project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

            let workdir = store
                .get_workdir_for_branch(&branch_id)
                .map_err(|e| e.to_string())?;

            if let Some(ref wd) = workdir {
                let repo_path = project.clone_path().ok_or("Cannot determine clone path")?;
                let worktree_path = Path::new(&wd.path);
                git::remove_worktree(&repo_path, worktree_path).map_err(|e| e.to_string())?;
                store.delete_workdir(&wd.id).map_err(|e| e.to_string())?;
            }
        }
        store::BranchType::Remote => {
            if let Some(ref ws_name) = branch.workspace_name {
                if let Err(e) = blox::ws_delete(ws_name) {
                    log::warn!("Failed to delete workspace {ws_name}: {e}");
                }
            }
        }
    }

    // Delete the branch record (cascades to commits, notes, reviews)
    store.delete_branch(&branch_id).map_err(|e| e.to_string())
}

// =============================================================================
// Project Actions commands
// =============================================================================

#[tauri::command]
fn list_project_actions(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
) -> Result<Vec<store::models::ProjectAction>, String> {
    let store = get_store(&store)?;
    store
        .list_project_actions(&project_id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn create_project_action(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    name: String,
    command: String,
    action_type: String,
    sort_order: i32,
    auto_commit: bool,
) -> Result<store::models::ProjectAction, String> {
    let store = get_store(&store)?;
    let parsed_type = builderbot_actions::ActionType::parse(&action_type)
        .ok_or_else(|| format!("Invalid action type: {action_type}"))?;
    let action =
        store::models::ProjectAction::new(project_id, name, command, parsed_type, sort_order)
            .with_auto_commit(auto_commit);
    store
        .create_project_action(&action)
        .map_err(|e| e.to_string())?;
    Ok(action)
}

#[tauri::command(rename_all = "camelCase")]
fn update_project_action(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    action_id: String,
    name: String,
    command: String,
    action_type: String,
    sort_order: i32,
    auto_commit: bool,
) -> Result<(), String> {
    let store = get_store(&store)?;
    let action = store
        .get_project_action(&action_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Action not found: {action_id}"))?;

    let updated = store::models::ProjectAction {
        id: action.id,
        project_id: action.project_id,
        name,
        command,
        action_type: builderbot_actions::ActionType::parse(&action_type)
            .ok_or_else(|| format!("Invalid action type: {action_type}"))?,
        sort_order,
        auto_commit,
        created_at: action.created_at,
        updated_at: store::now_timestamp(),
    };

    store
        .update_project_action(&updated)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
fn delete_project_action(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    action_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    store
        .delete_project_action(&action_id)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Timeline commands
// =============================================================================

/// Create a standalone note (no session) for a branch.
#[tauri::command(rename_all = "camelCase")]
fn create_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    title: String,
    content: String,
) -> Result<NoteTimelineItem, String> {
    let store = get_store(&store)?;
    let note = store::models::Note::new(&branch_id, &title, &content);
    store.create_note(&note).map_err(|e| e.to_string())?;
    Ok(NoteTimelineItem {
        id: note.id,
        title: note.title,
        content: note.content,
        session_id: None,
        session_status: None,
        created_at: note.created_at,
        updated_at: note.updated_at,
    })
}

/// Delete a note and optionally its linked session.
#[tauri::command(rename_all = "camelCase")]
fn delete_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    let store = get_store(&store)?;
    // Look up the note first so we can find its session
    let note = store
        .get_note(&note_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Note not found: {note_id}"))?;

    store.delete_note(&note_id).map_err(|e| e.to_string())?;

    if delete_session.unwrap_or(false) {
        if let Some(sid) = note.session_id {
            let _ = store.delete_session(&sid);
        }
    }
    Ok(())
}

/// Delete a pending commit (one with no SHA) by its DB id.
/// This does NOT touch git — it only removes the DB record and optionally its session.
#[tauri::command(rename_all = "camelCase")]
fn delete_pending_commit(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    commit_id: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    let store = get_store(&store)?;

    let commit = store
        .get_commit(&commit_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Commit not found: {commit_id}"))?;

    // Safety check: only allow deleting commits that have no SHA (pending/failed)
    if commit.sha.is_some() {
        return Err(
            "Cannot use delete_pending_commit for commits with a SHA. Use delete_commit instead."
                .to_string(),
        );
    }

    store.delete_commit(&commit_id).map_err(|e| e.to_string())?;

    if delete_session.unwrap_or(false) {
        if let Some(sid) = commit.session_id {
            let _ = store.delete_session(&sid);
        }
    }

    Ok(())
}

/// Delete a commit: resets the branch HEAD to the parent commit,
/// removing the git commit, then cleans up the DB record and session.
///
/// Only works for the tip commit (HEAD) of the branch's worktree.
/// Returns an error if the commit is not the current HEAD.
#[tauri::command(rename_all = "camelCase")]
fn delete_commit(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    let store = get_store(&store)?;

    // Get the worktree path for this branch
    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

    let worktree = Path::new(&workdir.path);

    // Verify the commit is the current HEAD
    let head_sha = git::get_head_sha(worktree).map_err(|e| e.to_string())?;
    if !head_sha.starts_with(&commit_sha)
        && !commit_sha.starts_with(&head_sha)
        && head_sha != commit_sha
    {
        return Err(format!(
            "Can only delete the latest commit. {} is not HEAD ({})",
            &commit_sha[..7.min(commit_sha.len())],
            &head_sha[..7.min(head_sha.len())]
        ));
    }

    // Find the parent commit
    let parent = git::get_parent_commit(worktree, &commit_sha)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Cannot delete the initial commit".to_string())?;

    // Reset to parent — this removes the commit from the branch
    git::reset_to_commit(worktree, &parent).map_err(|e| e.to_string())?;

    // Clean up DB record if one exists
    if let Ok(Some(db_commit)) = store.get_commit_by_sha(&branch_id, &commit_sha) {
        let _ = store.delete_commit(&db_commit.id);

        if delete_session.unwrap_or(false) {
            if let Some(sid) = db_commit.session_id {
                let _ = store.delete_session(&sid);
            }
        }
    }

    Ok(())
}

#[tauri::command]
fn get_branch_timeline(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<BranchTimeline, String> {
    let store = get_store(&store)?;

    // Get the branch and its workdir for git operations
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| e.to_string())?;

    // Get commits from git (the source of truth for commit data)
    let mut commits = Vec::new();
    if let Some(ref ws_name) = branch.workspace_name {
        // Remote branch: fetch commits via ws_exec
        let range = format!("{}..HEAD", &branch.base_branch);
        let format_arg = "--format=%H|%h|%s|%an|%ct";
        if let Ok(output) = blox::ws_exec(ws_name, &["git", "log", format_arg, &range]) {
            for line in output.lines() {
                if line.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = line.splitn(5, '|').collect();
                if parts.len() >= 5 {
                    let sha = parts[0].to_string();
                    let our_commit = store.get_commit_by_sha(&branch_id, &sha).unwrap_or(None);
                    let (session_id, session_status) = store.resolve_session_status(
                        our_commit.as_ref().and_then(|c| c.session_id.as_deref()),
                    );

                    commits.push(CommitTimelineItem {
                        id: our_commit.as_ref().map(|c| c.id.clone()),
                        sha,
                        short_sha: parts[1].to_string(),
                        subject: parts[2].to_string(),
                        author: parts[3].to_string(),
                        timestamp: parts[4].parse().unwrap_or(0),
                        session_id,
                        session_status,
                    });
                }
            }
        }
    } else if let Some(ref wd) = workdir {
        // Local branch: fetch commits from the local worktree
        let worktree_path = Path::new(&wd.path);
        if worktree_path.exists() {
            let git_commits =
                git::get_commits_since_base(worktree_path, &branch.base_branch).unwrap_or_default();

            // For each git commit, look up our metadata (session linkage)
            for gc in git_commits {
                let our_commit = store.get_commit_by_sha(&branch_id, &gc.sha).unwrap_or(None);
                let (session_id, session_status) = store.resolve_session_status(
                    our_commit.as_ref().and_then(|c| c.session_id.as_deref()),
                );

                commits.push(CommitTimelineItem {
                    id: our_commit.as_ref().map(|c| c.id.clone()),
                    sha: gc.sha,
                    short_sha: gc.short_sha,
                    subject: gc.subject,
                    author: gc.author,
                    timestamp: gc.timestamp,
                    session_id,
                    session_status,
                });
            }
        }
    }

    // Also include pending commits (sha = None, i.e. session in progress)
    let db_commits = store
        .list_commits_for_branch(&branch_id)
        .map_err(|e| e.to_string())?;
    for dc in db_commits {
        if dc.sha.is_none() {
            let (session_id, session_status) =
                store.resolve_session_status(dc.session_id.as_deref());

            commits.push(CommitTimelineItem {
                id: Some(dc.id.clone()),
                sha: String::new(),
                short_sha: String::new(),
                subject: session_id
                    .as_deref()
                    .and_then(|sid| {
                        store
                            .get_session(sid)
                            .ok()
                            .flatten()
                            .map(|s| s.prompt.clone())
                    })
                    .unwrap_or_else(|| "Pending commit".to_string()),
                author: String::new(),
                timestamp: dc.created_at / 1000, // convert ms to seconds
                session_id,
                session_status,
            });
        }
    }

    // Get notes
    let db_notes = store
        .list_notes_for_branch(&branch_id)
        .map_err(|e| e.to_string())?;
    let notes: Vec<NoteTimelineItem> = db_notes
        .into_iter()
        .map(|n| {
            let (session_id, session_status) =
                store.resolve_session_status(n.session_id.as_deref());
            NoteTimelineItem {
                id: n.id,
                title: n.title,
                content: n.content,
                session_id,
                session_status,
                created_at: n.created_at,
                updated_at: n.updated_at,
            }
        })
        .collect();

    // Get reviews
    let db_reviews = store
        .list_reviews_for_branch(&branch_id)
        .map_err(|e| e.to_string())?;
    let reviews: Vec<ReviewTimelineItem> = db_reviews
        .into_iter()
        .map(|r| {
            let (session_id, session_status) =
                store.resolve_session_status(r.session_id.as_deref());
            let comment_count = r.comments.len();
            ReviewTimelineItem {
                id: r.id,
                commit_sha: r.commit_sha,
                scope: r.scope.as_str().to_string(),
                session_id,
                session_status,
                comment_count,
                created_at: r.created_at,
                updated_at: r.updated_at,
            }
        })
        .collect();

    Ok(BranchTimeline {
        commits,
        notes,
        reviews,
    })
}

// =============================================================================
// Diff commands
// =============================================================================

/// Context needed to compute diffs for a branch.
struct BranchDiffContext {
    worktree_path: String,
    base_branch: String,
}

/// Resolve the worktree path and base branch for a given branch.
fn resolve_branch_context(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<BranchDiffContext, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

    Ok(BranchDiffContext {
        worktree_path: workdir.path,
        base_branch: branch.base_branch,
    })
}

/// Build a DiffSpec for a branch diff.
///
/// - Branch scope with no commit_sha: merge-base(base, tip)..tip
/// - Branch scope with commit_sha: merge-base(base, sha)..sha
/// - Commit scope: sha~1..sha
fn build_diff_spec(
    worktree: &Path,
    base_branch: &str,
    commit_sha: Option<&str>,
    scope: &str,
) -> Result<(git::DiffSpec, String), String> {
    match scope {
        "commit" => {
            let sha = commit_sha.ok_or("commit_sha required for commit scope")?;
            let parent = git::get_parent_commit(worktree, sha)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("No parent commit for {sha}"))?;
            let spec = git::DiffSpec {
                base: git::GitRef::Rev(parent),
                head: git::GitRef::Rev(sha.to_string()),
            };
            Ok((spec, sha.to_string()))
        }
        _ => {
            let resolved_sha = match commit_sha {
                Some(sha) => sha.to_string(),
                None => git::get_head_sha(worktree).map_err(|e| e.to_string())?,
            };
            let spec = git::DiffSpec {
                base: git::GitRef::MergeBaseOf([base_branch.to_string(), resolved_sha.clone()]),
                head: git::GitRef::Rev(resolved_sha.clone()),
            };
            Ok((spec, resolved_sha))
        }
    }
}

/// Response from get_diff_files including the resolved commit SHA.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiffFilesResponse {
    /// The resolved commit SHA (tip for branch scope, or the passed-in SHA).
    commit_sha: String,
    /// Changed files in the diff.
    files: Vec<git::FileDiffSummary>,
}

/// List files changed in a branch or commit diff.
///
/// For branch scope: merge-base(base, tip)..tip
/// For commit scope: parent..sha
///
/// `commit_sha` is optional for branch scope (resolves to current tip).
#[tauri::command(rename_all = "camelCase")]
async fn get_diff_files(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: Option<String>,
    scope: String,
) -> Result<DiffFilesResponse, String> {
    let store = get_store(&store)?;
    let ctx = resolve_branch_context(&store, &branch_id)?;
    let worktree = Path::new(&ctx.worktree_path);

    let (spec, resolved_sha) =
        build_diff_spec(worktree, &ctx.base_branch, commit_sha.as_deref(), &scope)?;

    let files = git::list_diff_files(worktree, &spec).map_err(|e| e.to_string())?;

    Ok(DiffFilesResponse {
        commit_sha: resolved_sha,
        files,
    })
}

/// Get the full diff content for a single file.
#[tauri::command(rename_all = "camelCase")]
async fn get_file_diff(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: String,
    scope: String,
    path: String,
) -> Result<git::FileDiff, String> {
    let store = get_store(&store)?;
    let ctx = resolve_branch_context(&store, &branch_id)?;
    let worktree = Path::new(&ctx.worktree_path);

    let (spec, _) = build_diff_spec(worktree, &ctx.base_branch, Some(&commit_sha), &scope)?;
    let file_path = Path::new(&path);

    git::get_file_diff(worktree, &spec, file_path).map_err(|e| e.to_string())
}

/// Get file content at a specific ref (for reference files).
#[tauri::command(rename_all = "camelCase")]
async fn get_file_at_ref(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    ref_name: String,
    path: String,
) -> Result<git::File, String> {
    let store = get_store(&store)?;
    let ctx = resolve_branch_context(&store, &branch_id)?;
    let worktree = Path::new(&ctx.worktree_path);

    git::get_file_at_ref(worktree, &ref_name, &path).map_err(|e| e.to_string())
}

// =============================================================================
// Review commands
// =============================================================================

/// Get or create a review for a branch + commit + scope.
///
/// This is the "lazy create" entry point — called when the user does
/// their first persistent action (comment, mark reviewed, etc.).
/// If a review already exists for this triple, returns it.
#[tauri::command(rename_all = "camelCase")]
async fn ensure_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    commit_sha: String,
    scope: String,
) -> Result<store::Review, String> {
    let store = get_store(&store)?;
    let review_scope =
        store::ReviewScope::parse(&scope).ok_or_else(|| format!("Invalid scope: {scope}"))?;

    store
        .ensure_review(&branch_id, &commit_sha, review_scope)
        .map_err(|e| e.to_string())
}

/// Get a review by ID with all child data.
#[tauri::command(rename_all = "camelCase")]
async fn get_review(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
) -> Result<Option<store::Review>, String> {
    get_store(&store)?
        .get_review(&review_id)
        .map_err(|e| e.to_string())
}

/// Mark a file as reviewed.
#[tauri::command(rename_all = "camelCase")]
async fn mark_reviewed(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    get_store(&store)?
        .mark_reviewed(&review_id, &path)
        .map_err(|e| e.to_string())
}

/// Unmark a file as reviewed.
#[tauri::command(rename_all = "camelCase")]
async fn unmark_reviewed(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    get_store(&store)?
        .unmark_reviewed(&review_id, &path)
        .map_err(|e| e.to_string())
}

/// Add a comment to a review.
#[tauri::command(rename_all = "camelCase")]
async fn add_comment(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
    span_start: u32,
    span_end: u32,
    content: String,
) -> Result<store::Comment, String> {
    let store = get_store(&store)?;
    let comment = store::Comment::new(&path, git::Span::new(span_start, span_end), &content);
    store
        .add_comment(&review_id, &comment)
        .map_err(|e| e.to_string())?;
    Ok(comment)
}

/// Update a comment's content.
#[tauri::command(rename_all = "camelCase")]
async fn update_comment(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    comment_id: String,
    content: String,
) -> Result<(), String> {
    get_store(&store)?
        .update_comment(&comment_id, &content)
        .map_err(|e| e.to_string())
}

/// Delete a comment.
#[tauri::command(rename_all = "camelCase")]
async fn delete_comment(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    comment_id: String,
) -> Result<(), String> {
    get_store(&store)?
        .delete_comment(&comment_id)
        .map_err(|e| e.to_string())
}

/// Add a reference file to a review.
#[tauri::command(rename_all = "camelCase")]
async fn add_reference_file(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    get_store(&store)?
        .add_reference_file(&review_id, &path)
        .map_err(|e| e.to_string())
}

/// Remove a reference file from a review.
#[tauri::command(rename_all = "camelCase")]
async fn remove_reference_file(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    review_id: String,
    path: String,
) -> Result<(), String> {
    get_store(&store)?
        .remove_reference_file(&review_id, &path)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Git helper commands
// =============================================================================

/// List branches for a repo via GitHub API (no local clone needed).
#[tauri::command(rename_all = "camelCase")]
async fn list_git_branches(github_repo: String) -> Result<Vec<git::BranchRef>, String> {
    git::list_branches_for_repo(&github_repo).map_err(|e| e.to_string())
}

/// Detect default branch via GitHub API (no local clone needed).
#[tauri::command(rename_all = "camelCase")]
async fn detect_default_branch_cmd(github_repo: String) -> Result<String, String> {
    git::detect_default_branch_for_repo(&github_repo).map_err(|e| e.to_string())
}

/// Prune stale remote-tracking refs. With GitHub-repo-based projects,
/// branch listing uses the API directly, so this is a no-op.
#[tauri::command(rename_all = "camelCase")]
async fn prune_remote_refs(github_repo: String) -> Result<(), String> {
    git::prune_remote_for_repo(&github_repo).map_err(|e| e.to_string())
}

/// List open pull requests for a repository (via `-R owner/repo`).
#[tauri::command(rename_all = "camelCase")]
fn list_pull_requests(github_repo: String) -> Result<Vec<git::github::PullRequest>, String> {
    git::list_pull_requests_for_repo(&github_repo).map_err(|e| e.to_string())
}

/// List open issues for a repository (via `-R owner/repo`).
#[tauri::command(rename_all = "camelCase")]
fn list_issues(github_repo: String) -> Result<Vec<git::github::Issue>, String> {
    git::list_issues_for_repo(&github_repo).map_err(|e| e.to_string())
}

// =============================================================================
// PR creation
// =============================================================================

/// Create a pull request for a branch by kicking off an agent session.
///
/// The agent pushes the branch to the remote and creates a PR using `gh`.
/// Returns the session ID so the frontend can track progress. The PR title
/// uses conventional commit styling and the agent figures out changes by
/// comparing the branch's HEAD with when it branched off the parent branch.
#[tauri::command(rename_all = "camelCase")]
fn create_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
) -> Result<String, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

    let mut working_dir = PathBuf::from(&workdir.path);
    if let Some(ref subpath) = project.subpath {
        working_dir = working_dir.join(subpath);
    }

    // Build the PR creation prompt
    let base_branch = branch
        .base_branch
        .strip_prefix("origin/")
        .unwrap_or(&branch.base_branch);

    let prompt = format!(
        r#"<action>
Create a pull request for the current branch.

Steps:
1. First, look at the diff between the current branch and when it branched off of the base branch `{base_branch}` to understand all changes. Use `git log --oneline {base_branch}..HEAD` and `git diff {base_branch}...HEAD --stat` to see what changed.
2. Push the current branch to the remote: `git push -u origin {branch_name}`
3. Create a PR using the GitHub CLI: `gh pr create --base {base_branch} --fill-first`
   - The title MUST use conventional commit style (e.g., "feat: add user authentication", "fix: resolve null pointer in parser", "refactor: extract validation logic")
   - Choose the most appropriate conventional commit type (feat, fix, refactor, docs, style, test, chore, perf, ci, build) based on the actual changes
   - The body should be a concise summary of the changes

IMPORTANT: After creating the PR, you MUST output the PR URL on its own line in this exact format:
PR_URL: https://github.com/...

This is critical - the application parses this to link the PR.
</action>"#,
        base_branch = base_branch,
        branch_name = branch.branch_name,
    );

    // Create the session
    let mut session = store::Session::new_running(&prompt, &working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    session_runner::start_session(
        session_runner::SessionConfig {
            session_id: session.id.clone(),
            prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha: None,
            provider,
            workspace_name: None,
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(session.id)
}

/// Build the GitHub PR URL for a branch from its remote origin and PR number.
///
/// Parses the `origin` remote URL to extract the GitHub owner/repo, then
/// returns `https://github.com/{owner}/{repo}/pull/{pr_number}`.
#[tauri::command(rename_all = "camelCase")]
fn get_pr_url(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    pr_number: u64,
) -> Result<String, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    // project.github_repo is already "owner/repo"
    let parts: Vec<&str> = project.github_repo.splitn(2, '/').collect();
    if parts.len() != 2 {
        return Err(format!(
            "Invalid github_repo format: {}",
            project.github_repo
        ));
    }
    let (owner, repo_name) = (parts[0], parts[1]);

    Ok(format!(
        "https://github.com/{owner}/{repo_name}/pull/{pr_number}"
    ))
}

/// Update the PR number for a branch.
#[tauri::command(rename_all = "camelCase")]
fn update_branch_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    pr_number: Option<u64>,
) -> Result<(), String> {
    get_store(&store)?
        .update_branch_pr_number(&branch_id, pr_number)
        .map_err(|e| e.to_string())
}

/// Check if a branch has commits that haven't been pushed to the remote.
#[tauri::command(rename_all = "camelCase")]
fn has_unpushed_commits(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<bool, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

    git::has_unpushed_commits(Path::new(&workdir.path), &branch.branch_name)
        .map_err(|e| e.to_string())
}

/// Push a branch to its remote by kicking off an agent session.
///
/// The agent runs `git push` and can diagnose and fix pre-push hook
/// failures or other push errors. Returns the session ID so the
/// frontend can track progress (same pattern as `create_pr`).
///
/// For remote branches the session runs inside the Blox workspace.
#[tauri::command(rename_all = "camelCase")]
fn push_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    force: Option<bool>,
) -> Result<String, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let is_remote = branch.branch_type == store::BranchType::Remote;

    let (working_dir, workspace_name) = if is_remote {
        let clone_path = project
            .clone_path()
            .ok_or_else(|| "Cannot determine clone path for remote branch".to_string())?;
        (clone_path, branch.workspace_name.clone())
    } else {
        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut working_dir = PathBuf::from(&workdir.path);
        if let Some(ref subpath) = project.subpath {
            working_dir = working_dir.join(subpath);
        }
        (working_dir, None)
    };

    let force = force.unwrap_or(false);

    let prompt = if force {
        format!(
            r#"<action>
Push the current branch to the remote using force-with-lease.

Run: `git push -u origin {branch_name} --force-with-lease`

If the push fails due to pre-push hook errors, read the error output, fix the underlying issue, and retry the push.

The push must succeed before you finish.
</action>"#,
            branch_name = branch.branch_name,
        )
    } else {
        format!(
            r#"<action>
Push the current branch to the remote.

Run: `git push -u origin {branch_name}`

IMPORTANT: You MUST NOT use --force, --force-with-lease, or any force-push variant. Only a normal push is allowed.

If the push fails due to pre-push hook errors, read the error output, fix the underlying issue, and retry the push.

If the push is rejected because the remote has commits that would be lost (non-fast-forward rejection), do NOT attempt to fix it. Instead, output the following marker on its own line and stop:
PUSH_REJECTED: NON_FAST_FORWARD

For any other failure, diagnose the problem and fix it, then retry the push.

The push must succeed before you finish (unless you output the non-fast-forward marker above).
</action>"#,
            branch_name = branch.branch_name,
        )
    };

    // Create the session
    let mut session = store::Session::new_running(&prompt, &working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    session_runner::start_session(
        session_runner::SessionConfig {
            session_id: session.id.clone(),
            prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha: None,
            provider,
            workspace_name,
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(session.id)
}

// =============================================================================
// Utilities
// =============================================================================

/// Open a URL in the user's default browser.
#[tauri::command]
fn open_url(app_handle: tauri::AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app_handle
        .opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| format!("Failed to open URL: {e}"))
}

/// Check whether the `sq` CLI is available on this system.
///
/// The frontend uses this to decide whether to show the Remote branch option
/// in the new-branch modal.
#[tauri::command]
fn is_sq_available() -> bool {
    blox::is_sq_available()
}

/// Read a text file from an absolute path.
///
/// Used by the frontend to read file contents from paths provided by
/// Tauri's native drag-and-drop events (which give file paths, not
/// File objects like browser drag events).
#[tauri::command(rename_all = "camelCase")]
fn read_text_file(file_path: String) -> Result<String, String> {
    let path = Path::new(&file_path);
    if !path.exists() {
        return Err(format!("File does not exist: {file_path}"));
    }
    if !path.is_file() {
        return Err(format!("Not a file: {file_path}"));
    }
    std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {e}"))
}

/// Check whether the user is authenticated with Blox.
///
/// Returns Ok(()) if authenticated, or an error string if not.
/// The frontend can call this before starting a workspace to give
/// an immediate, actionable error instead of a mysterious hang.
#[tauri::command]
fn check_blox_auth() -> Result<(), String> {
    blox::check_auth().map_err(|e| e.to_string())
}

// =============================================================================
// Open In commands
// =============================================================================

/// An application that can open directories.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenerApp {
    id: String,
    name: String,
}

/// Known applications with their bundle IDs (macOS).
#[cfg(target_os = "macos")]
const KNOWN_OPENERS: &[(&str, &str)] = &[
    // Terminals
    ("terminal", "com.apple.Terminal"),
    ("warp", "dev.warp.Warp-Stable"),
    ("iterm", "com.googlecode.iterm2"),
    ("hyper", "co.zeit.hyper"),
    ("kitty", "net.kovidgoyal.kitty"),
    ("alacritty", "org.alacritty"),
    // Editors
    ("vscode", "com.microsoft.VSCode"),
    ("vscode-insiders", "com.microsoft.VSCodeInsiders"),
    ("cursor", "com.todesktop.230313mzl4w4u92"),
    ("sublime", "com.sublimetext.4"),
    ("atom", "com.github.atom"),
    ("textmate", "com.macromates.TextMate"),
    ("nova", "com.panic.Nova"),
    ("bbedit", "com.barebones.bbedit"),
    ("intellij", "com.jetbrains.intellij"),
    ("webstorm", "com.jetbrains.WebStorm"),
    ("pycharm", "com.jetbrains.pycharm"),
    ("rubymine", "com.jetbrains.rubymine"),
    ("goland", "com.jetbrains.goland"),
    ("fleet", "fleet.app"),
    ("zed", "dev.zed.Zed"),
    // File browsers
    ("finder", "com.apple.finder"),
];

/// Get available opener applications.
///
/// On macOS, uses mdfind to detect which apps are installed.
/// On other platforms, returns an empty list.
#[tauri::command]
async fn get_available_openers() -> Result<Vec<OpenerApp>, String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        let mut available = Vec::new();

        for (id, bundle_id) in KNOWN_OPENERS {
            let output = Command::new("mdfind")
                .arg(format!("kMDItemCFBundleIdentifier == '{bundle_id}'"))
                .output()
                .map_err(|e| format!("Failed to run mdfind: {e}"))?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.trim().is_empty() {
                    available.push(OpenerApp {
                        id: id.to_string(),
                        name: prettify_app_name(id),
                    });
                }
            }
        }

        Ok(available)
    }

    #[cfg(not(target_os = "macos"))]
    {
        // On non-macOS platforms, return empty list
        Ok(Vec::new())
    }
}

/// Convert app ID to a human-readable name.
#[cfg(target_os = "macos")]
fn prettify_app_name(id: &str) -> String {
    match id {
        "vscode" => "VS Code",
        "vscode-insiders" => "VS Code Insiders",
        "cursor" => "Cursor",
        "sublime" => "Sublime Text",
        "atom" => "Atom",
        "textmate" => "TextMate",
        "nova" => "Nova",
        "bbedit" => "BBEdit",
        "intellij" => "IntelliJ IDEA",
        "webstorm" => "WebStorm",
        "pycharm" => "PyCharm",
        "rubymine" => "RubyMine",
        "goland" => "GoLand",
        "fleet" => "Fleet",
        "zed" => "Zed",
        "terminal" => "Terminal",
        "warp" => "Warp",
        "iterm" => "iTerm",
        "hyper" => "Hyper",
        "kitty" => "Kitty",
        "alacritty" => "Alacritty",
        "finder" => "Finder",
        _ => id,
    }
    .to_string()
}

/// Open a directory in a specific application.
///
/// On macOS, uses the `open -b` command with the app's bundle ID.
/// On other platforms, returns an error.
#[tauri::command]
#[allow(unused_variables)]
async fn open_in_app(path: String, app_id: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        // Find the bundle ID for this app
        let bundle_id = KNOWN_OPENERS
            .iter()
            .find(|(id, _)| *id == app_id)
            .map(|(_, bundle)| *bundle)
            .ok_or_else(|| format!("Unknown app ID: {app_id}"))?;

        let status = Command::new("open")
            .arg("-b")
            .arg(bundle_id)
            .arg(&path)
            .status()
            .map_err(|e| format!("Failed to run open command: {e}"))?;

        if status.success() {
            Ok(())
        } else {
            Err(format!("Failed to open {path} in {app_id}"))
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err("Open in app is only supported on macOS".to_string())
    }
}

// =============================================================================
// Tauri App Setup
// =============================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    // Restore everything except visibility — the frontend
                    // calls window.show() after the theme is applied to
                    // avoid a flash of wrong-colored background.
                    tauri_plugin_window_state::StateFlags::all()
                        & !tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            // Build a custom macOS application menu so that the app submenu,
            // "About" item, and "Quit" item use the capitalised product name
            // "Staged" instead of the lowercase Cargo package name "staged".
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};

                let handle = app.handle();
                let pkg_info = handle.package_info();
                let config = handle.config();
                let about_metadata = AboutMetadata {
                    name: Some("Staged".into()),
                    version: Some(pkg_info.version.to_string()),
                    copyright: config.bundle.copyright.clone(),
                    authors: config.bundle.publisher.clone().map(|p| vec![p]),
                    ..Default::default()
                };

                let app_menu = Submenu::with_items(
                    handle,
                    "Staged",
                    true,
                    &[
                        &PredefinedMenuItem::about(
                            handle,
                            Some("About Staged"),
                            Some(about_metadata),
                        )?,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::services(handle, None)?,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::hide(handle, None)?,
                        &PredefinedMenuItem::hide_others(handle, None)?,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::quit(handle, Some("Quit Staged"))?,
                    ],
                )?;

                let file_menu = Submenu::with_items(
                    handle,
                    "File",
                    true,
                    &[&PredefinedMenuItem::close_window(handle, None)?],
                )?;

                let edit_menu = Submenu::with_items(
                    handle,
                    "Edit",
                    true,
                    &[
                        &PredefinedMenuItem::undo(handle, None)?,
                        &PredefinedMenuItem::redo(handle, None)?,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::cut(handle, None)?,
                        &PredefinedMenuItem::copy(handle, None)?,
                        &PredefinedMenuItem::paste(handle, None)?,
                        &PredefinedMenuItem::select_all(handle, None)?,
                    ],
                )?;

                let view_menu = Submenu::with_items(
                    handle,
                    "View",
                    true,
                    &[&PredefinedMenuItem::fullscreen(handle, None)?],
                )?;

                let window_menu = Submenu::with_id_and_items(
                    handle,
                    tauri::menu::WINDOW_SUBMENU_ID,
                    "Window",
                    true,
                    &[
                        &PredefinedMenuItem::minimize(handle, None)?,
                        &PredefinedMenuItem::maximize(handle, None)?,
                        &PredefinedMenuItem::separator(handle)?,
                        &PredefinedMenuItem::close_window(handle, None)?,
                    ],
                )?;

                let doctor_item =
                    MenuItem::with_id(handle, "doctor", "Health Check…", true, None::<&str>)?;

                let help_menu = Submenu::with_id_and_items(
                    handle,
                    tauri::menu::HELP_SUBMENU_ID,
                    "Help",
                    true,
                    &[&doctor_item],
                )?;

                let menu = Menu::with_items(
                    handle,
                    &[
                        &app_menu,
                        &file_menu,
                        &edit_menu,
                        &view_menu,
                        &window_menu,
                        &help_menu,
                    ],
                )?;

                app.set_menu(menu)?;
            }

            let data_dir = crate::paths::data_dir()
                .ok_or_else(|| "Cannot determine data directory".to_string())?;
            std::fs::create_dir_all(&data_dir)
                .map_err(|e| format!("Cannot create data dir: {e}"))?;

            let db_path = data_dir.join("data.db");

            // Migrate from older data directories if the new location is empty.
            // Priority: legacy ~/Library/Application Support/staged/ first,
            // then original Tauri app_data_dir (com.staged.app).
            if !db_path.exists() {
                let legacy_candidates: Vec<PathBuf> = [
                    crate::paths::legacy_data_dir(),
                    app.path().app_data_dir().ok(),
                ]
                .into_iter()
                .flatten()
                .filter(|d| d != &data_dir)
                .collect();

                for old_dir in legacy_candidates {
                    let old_db = old_dir.join("data.db");
                    if old_db.exists() {
                        log::info!(
                            "Migrating data from {} to {}",
                            old_dir.display(),
                            data_dir.display()
                        );
                        // Move the entire directory contents (db, repos, worktrees)
                        crate::paths::migrate_directory_contents(&old_dir, &data_dir);
                        break;
                    }
                }
            }

            // Check compatibility *before* creating the store.
            let compat = store::check_db_compatibility(&db_path)
                .map_err(|e| format!("Cannot check database: {e}"))?;

            let (store_slot, reset_info) = match compat {
                store::DbCompatibility::Ok => {
                    let s =
                        Store::new(&db_path).map_err(|e| format!("Failed to open store: {e}"))?;
                    // Cancel any sessions that were running when the app last closed
                    match s.cancel_orphaned_sessions() {
                        Ok(0) => {}
                        Ok(n) => log::info!("Cancelled {n} orphaned session(s) from previous run"),
                        Err(e) => log::warn!("Failed to cancel orphaned sessions: {e}"),
                    }
                    (Mutex::new(Some(Arc::new(s))), None)
                }
                store::DbCompatibility::NeedsReset { db_app_version } => {
                    let info = StoreIncompatibility {
                        db_app_version: db_app_version.clone(),
                        app_version: store::APP_VERSION.to_string(),
                        kind: "needs_reset".to_string(),
                    };
                    log::warn!(
                        "Database from v{} incompatible with v{}, will prompt user to reset",
                        db_app_version,
                        store::APP_VERSION,
                    );
                    (Mutex::new(None), Some(info))
                }
                store::DbCompatibility::TooNew { db_app_version } => {
                    let info = StoreIncompatibility {
                        db_app_version: db_app_version.clone(),
                        app_version: store::APP_VERSION.to_string(),
                        kind: "too_new".to_string(),
                    };
                    log::warn!(
                        "Database from v{} is newer than this build (v{}), user should update",
                        db_app_version,
                        store::APP_VERSION,
                    );
                    (Mutex::new(None), Some(info))
                }
            };

            app.manage(store_slot);
            app.manage(Arc::new(session_runner::SessionRegistry::new()));
            app.manage(Arc::new(actions::ActionExecutor::new()));
            app.manage(DbState {
                db_path,
                needs_reset: Mutex::new(reset_info),
            });

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id() == "doctor" {
                // Emit an event to the frontend to open the doctor modal.
                if let Err(e) = app.emit("menu:doctor", ()) {
                    log::warn!("Failed to emit menu:doctor event: {e}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_directory,
            search_directories,
            get_home_dir,
            find_recent_repos,
            get_store_status,
            confirm_reset_store,
            list_projects,
            create_project,
            delete_project,
            list_github_orgs,
            list_github_repos,
            search_github_repos,
            list_branches_for_project,
            create_branch,
            setup_worktree,
            setup_worktree_from_pr,
            create_remote_branch,
            start_workspace,
            delete_branch,
            get_workspace_info,
            poll_workspace_status,
            list_project_actions,
            create_project_action,
            update_project_action,
            delete_project_action,
            get_branch_timeline,
            create_note,
            delete_note,
            delete_commit,
            delete_pending_commit,
            list_git_branches,
            detect_default_branch_cmd,
            prune_remote_refs,
            list_pull_requests,
            list_issues,
            create_pr,
            get_pr_url,
            update_branch_pr,
            has_unpushed_commits,
            push_branch,
            open_url,
            is_sq_available,
            read_text_file,
            check_blox_auth,
            get_available_openers,
            open_in_app,
            session_commands::discover_acp_providers,
            session_commands::get_session,
            session_commands::get_session_messages,
            session_commands::get_session_messages_since,
            session_commands::start_session,
            session_commands::resume_session,
            session_commands::cancel_session,
            session_commands::delete_session,
            session_commands::start_branch_session,
            // Actions
            actions::commands::detect_project_actions,
            actions::commands::run_branch_action,
            actions::commands::stop_branch_action,
            actions::commands::get_running_branch_actions,
            actions::commands::get_action_output_buffer,
            actions::commands::clear_action_execution,
            actions::commands::run_prerun_actions,
            // Diff
            get_diff_files,
            get_file_diff,
            get_file_at_ref,
            // Review
            ensure_review,
            get_review,
            mark_reviewed,
            unmark_reviewed,
            add_comment,
            update_comment,
            delete_comment,
            add_reference_file,
            remove_reference_file,
            // Doctor
            doctor::run_doctor,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
