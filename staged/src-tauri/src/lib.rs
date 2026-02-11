//! Staged — clean rewrite.
//!
//! Tauri commands for the new frontend, built incrementally.
//! See `src-archive/lib.rs` for the previous implementation.

pub mod agent;
pub mod blox;
pub mod git;
mod recent_repos;
pub mod session_commands;
pub mod session_runner;
pub mod store;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use store::Store;
use tauri::Manager;

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
    pub branch_type: String,
    pub workspace_name: Option<String>,
    pub workspace_status: Option<String>,
    pub agent: Option<String>,
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

#[tauri::command]
fn create_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    repo_path: String,
    subpath: Option<String>,
) -> Result<store::Project, String> {
    let store = get_store(&store)?;

    // Validate that the path is a git repo
    let path = Path::new(&repo_path);
    if !path.join(".git").exists() && !path.is_dir() {
        return Err(format!("Not a git repository: {}", repo_path));
    }

    // Check for duplicate
    if let Some(existing) = store
        .get_project_by_repo(&repo_path)
        .map_err(|e| e.to_string())?
    {
        return Ok(existing);
    }

    let mut project = store::Project::new(&repo_path);
    if let Some(sub) = subpath {
        project = project.with_subpath(sub);
    }
    store.create_project(&project).map_err(|e| e.to_string())?;
    Ok(project)
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
        branch_type: branch.branch_type.as_str().to_string(),
        workspace_name: branch.workspace_name,
        workspace_status: branch.workspace_status.map(|s| s.as_str().to_string()),
        agent: branch.agent,
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
    let branches = store
        .list_branches_for_project(&project_id)
        .map_err(|e| e.to_string())?;

    let mut result = Vec::with_capacity(branches.len());
    for branch in branches {
        let workdir = store
            .get_workdir_for_branch(&branch.id)
            .map_err(|e| e.to_string())?;

        result.push(to_branch_with_workdir(branch, workdir.map(|w| w.path)));
    }
    Ok(result)
}

#[tauri::command]
fn create_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    branch_name: String,
    base_branch: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;

    // Get the project to find its repo path
    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", project_id))?;

    let repo_path = Path::new(&project.repo_path);

    // Detect default branch if none specified
    let effective_base = match base_branch {
        Some(b) => b,
        None => git::detect_default_branch(repo_path).map_err(|e| e.to_string())?,
    };

    // Create git branch + worktree
    let worktree_path = git::create_worktree(repo_path, &branch_name, &effective_base)
        .map_err(|e| e.to_string())?;

    let worktree_str = worktree_path
        .to_str()
        .ok_or("Invalid worktree path")?
        .to_string();

    // Create branch record
    let branch = store::Branch::new(&project_id, &branch_name, &effective_base);
    store.create_branch(&branch).map_err(|e| e.to_string())?;

    // Create workdir record assigned to this branch
    let workdir = store::Workdir::new(&project_id, &worktree_str).with_branch(&branch.id);
    store.create_workdir(&workdir).map_err(|e| e.to_string())?;

    Ok(to_branch_with_workdir(branch, Some(worktree_str)))
}

/// Create a remote branch backed by a Blox workspace.
///
/// This creates the branch record with type=remote, starts a Blox workspace
/// via `blox ws start`, and stores the workspace metadata on the branch.
/// No local worktree or workdir is created.
#[tauri::command(rename_all = "camelCase")]
async fn create_remote_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    branch_name: String,
    base_branch: Option<String>,
    workspace_name: String,
    agent: Option<String>,
    source: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;

    // Get the project to find its repo path
    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", project_id))?;

    let repo_path = Path::new(&project.repo_path);

    // Detect default branch if none specified
    let effective_base = match base_branch {
        Some(b) => b,
        None => git::detect_default_branch(repo_path).map_err(|e| e.to_string())?,
    };

    let effective_agent = agent.unwrap_or_else(|| "goose".to_string());

    // Create the branch record (starts in Starting status)
    let branch = store::Branch::new_remote(
        &project_id,
        &branch_name,
        &effective_base,
        &workspace_name,
        &effective_agent,
    );
    store.create_branch(&branch).map_err(|e| e.to_string())?;

    // Start the Blox workspace
    match blox::ws_start(&workspace_name, source.as_deref()) {
        Ok(_) => {
            // Update status to Running
            store
                .update_branch_workspace_status(&branch.id, &store::WorkspaceStatus::Running)
                .map_err(|e| e.to_string())?;

            // Re-fetch to get updated status
            let updated = store
                .get_branch(&branch.id)
                .map_err(|e| e.to_string())?
                .ok_or("Branch disappeared after creation")?;

            Ok(to_branch_with_workdir(updated, None))
        }
        Err(e) => {
            // Update status to Error but keep the branch record
            let _ =
                store.update_branch_workspace_status(&branch.id, &store::WorkspaceStatus::Error);

            Err(format!("Failed to start workspace: {e}"))
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
        .ok_or_else(|| format!("Branch not found: {}", branch_id))?;

    let ws_name = branch
        .workspace_name
        .ok_or("Branch is not a remote workspace branch")?;

    blox::ws_info(&ws_name)
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
        .ok_or_else(|| format!("Branch not found: {}", branch_id))?;

    match branch.branch_type {
        store::BranchType::Local => {
            // Get the project for the repo path
            let project = store
                .get_project(&branch.project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

            // Get the workdir (if any) so we can remove the worktree
            let workdir = store
                .get_workdir_for_branch(&branch_id)
                .map_err(|e| e.to_string())?;

            if let Some(ref wd) = workdir {
                let repo_path = Path::new(&project.repo_path);
                let worktree_path = Path::new(&wd.path);
                git::remove_worktree(repo_path, worktree_path).map_err(|e| e.to_string())?;
                store.delete_workdir(&wd.id).map_err(|e| e.to_string())?;
            }
        }
        store::BranchType::Remote => {
            // Delete the Blox workspace
            if let Some(ref ws_name) = branch.workspace_name {
                // Best-effort: log but don't fail if workspace is already gone
                if let Err(e) = blox::ws_delete(ws_name) {
                    log::warn!("Failed to delete workspace {}: {}", ws_name, e);
                }
            }
        }
    }

    // Delete the branch record (cascades to commits, notes, reviews)
    store.delete_branch(&branch_id).map_err(|e| e.to_string())
}

// =============================================================================
// Timeline commands
// =============================================================================

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
        .ok_or_else(|| format!("Note not found: {}", note_id))?;

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
        .ok_or_else(|| format!("Commit not found: {}", commit_id))?;

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
        .ok_or_else(|| format!("No worktree for branch: {}", branch_id))?;

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
        .ok_or_else(|| format!("Branch not found: {}", branch_id))?;

    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| e.to_string())?;

    // Get commits from git (the source of truth for commit data)
    let mut commits = Vec::new();
    if let Some(ref wd) = workdir {
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
        .ok_or_else(|| format!("Branch not found: {}", branch_id))?;

    let workdir = store
        .get_workdir_for_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {}", branch_id))?;

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
        store::ReviewScope::parse(&scope).ok_or_else(|| format!("Invalid scope: {}", scope))?;

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

#[tauri::command]
fn list_git_branches(repo_path: String) -> Result<Vec<git::BranchRef>, String> {
    let path = Path::new(&repo_path);
    git::list_branches(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn detect_default_branch_cmd(repo_path: String) -> Result<String, String> {
    let path = Path::new(&repo_path);
    git::detect_default_branch(path).map_err(|e| e.to_string())
}

// =============================================================================
// Utilities
// =============================================================================

/// Open a URL in the user's default browser.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    open::that(&url).map_err(|e| format!("Failed to open URL: {e}"))
}

// =============================================================================
// Tauri App Setup
// =============================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
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
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("Cannot get app data dir: {e}"))?;

            // Ensure the app data directory exists on first launch.
            std::fs::create_dir_all(&app_data_dir)
                .map_err(|e| format!("Cannot create app data dir: {e}"))?;

            let db_path = app_data_dir.join("data.db");

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
            list_branches_for_project,
            create_branch,
            create_remote_branch,
            delete_branch,
            get_workspace_info,
            get_branch_timeline,
            delete_note,
            delete_commit,
            delete_pending_commit,
            list_git_branches,
            detect_default_branch_cmd,
            open_url,
            session_commands::discover_acp_providers,
            session_commands::get_session,
            session_commands::get_session_messages,
            session_commands::get_session_messages_since,
            session_commands::start_session,
            session_commands::resume_session,
            session_commands::cancel_session,
            session_commands::delete_session,
            session_commands::start_branch_session,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
