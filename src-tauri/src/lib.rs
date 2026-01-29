//! Tauri commands for the Staged diff viewer.
//!
//! This module provides the bridge between the frontend and the git/github modules.
//! Supports CLI arguments: `staged [path]` opens the app with the specified directory.

pub mod ai;
pub mod git;
mod recent_repos;
pub mod review;
mod themes;
mod watcher;

use git::{
    DiffId, DiffSpec, File, FileDiff, FileDiffSummary, GitHubAuthStatus, GitHubSyncResult, GitRef,
    PullRequest,
};
use review::{Comment, Edit, NewComment, NewEdit, Review};
use std::path::{Path, PathBuf};
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, State, Wry};
use watcher::WatcherHandle;

// =============================================================================
// Helpers
// =============================================================================

/// Get the repo path, defaulting to current directory.
fn get_repo_path(path: Option<&str>) -> &Path {
    path.map(Path::new).unwrap_or(Path::new("."))
}

/// Create a DiffId from a DiffSpec for review storage.
/// Resolves refs to SHAs for stable keys.
fn make_diff_id(repo: &Path, spec: &DiffSpec) -> Result<DiffId, String> {
    let resolve = |r: &GitRef| -> Result<String, String> {
        match r {
            GitRef::WorkingTree => Ok("@".to_string()),
            GitRef::Rev(rev) => git::resolve_ref(repo, rev).map_err(|e| e.to_string()),
            GitRef::MergeBase => {
                // Resolve merge-base to a concrete SHA for stable storage key
                let default_branch = git::detect_default_branch(repo).map_err(|e| e.to_string())?;
                git::merge_base(repo, &default_branch, "HEAD").map_err(|e| e.to_string())
            }
        }
    };

    Ok(DiffId::new(resolve(&spec.base)?, resolve(&spec.head)?))
}

// =============================================================================
// File Browsing Commands
// =============================================================================

/// Entry in a directory listing.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
    is_repo: bool,
}

/// List contents of a directory.
/// Returns directories first (sorted), then files (sorted).
/// For directories, also indicates if they are git repositories.
#[tauri::command(rename_all = "camelCase")]
fn list_directory(path: String) -> Result<Vec<DirEntry>, String> {
    let dir = Path::new(&path);

    if !dir.exists() {
        return Err(format!("Directory does not exist: {}", path));
    }

    if !dir.is_dir() {
        return Err(format!("Not a directory: {}", path));
    }

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    let entries = std::fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {}", e))?;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files/directories
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

    // Sort alphabetically (case-insensitive)
    dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    // Directories first, then files
    dirs.extend(files);
    Ok(dirs)
}

/// Folders to skip during search - system folders unlikely to contain projects.
const SKIP_FOLDERS: &[&str] = &[
    // macOS system
    "Library",
    "Applications",
    "System",
    "Volumes",
    "cores",
    "private",
    // Common non-project folders
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
    // Media/documents unlikely to have repos
    "Movies",
    "Music",
    "Pictures",
    "Photos Library.photoslibrary",
];

/// Common development folder names - search these when at home directory.
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
/// Only returns directories containing a .git folder.
/// When at home directory, only searches inside common dev folders.
/// Returns up to `limit` matches sorted by relevance.
#[tauri::command(rename_all = "camelCase")]
fn search_directories(
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
        return Err(format!("Invalid directory: {}", path));
    }

    let mut results = Vec::new();
    let collect_limit = limit * 3;

    // Check if we're at the home directory
    let home_dir = dirs::home_dir();
    let is_home = home_dir.as_ref().is_some_and(|h| h == dir);

    if is_home {
        // When at home, only search inside common dev folders
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
        // Normal recursive search for non-home directories
        search_repos_recursive(dir, &query_lower, 0, max_depth, &mut results, collect_limit);
    }

    // Sort results by relevance:
    // 1. Exact matches first
    // 2. Then by path depth (shallower = better)
    results.sort_by(|a, b| {
        let a_exact = a.name.to_lowercase() == query_lower;
        let b_exact = b.name.to_lowercase() == query_lower;
        if a_exact != b_exact {
            return b_exact.cmp(&a_exact); // exact matches first
        }

        let a_depth = a.path.matches('/').count();
        let b_depth = b.path.matches('/').count();
        a_depth.cmp(&b_depth) // shallower first
    });
    results.truncate(limit);

    Ok(results)
}

/// Recursive helper for searching git repositories.
/// Only adds directories that contain a .git folder.
/// Returns true if we should stop searching (hit the limit).
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

        // Skip hidden directories
        if name.starts_with('.') {
            continue;
        }

        // Skip system/non-project folders
        if SKIP_FOLDERS.contains(&name.as_str()) {
            continue;
        }

        let entry_path = entry.path();
        if !entry_path.is_dir() {
            continue;
        }

        // Check if this is a git repository
        let is_repo = entry_path.join(".git").exists();

        if is_repo {
            // Only add if name matches query
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
            // Don't recurse into repos (nested repos are rare)
        } else {
            // Not a repo, recurse to find repos inside
            if search_repos_recursive(&entry_path, query, depth + 1, max_depth, results, limit) {
                return true;
            }
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

/// Find git repositories that have been recently active.
///
/// Uses macOS Spotlight to find files modified within the last `hours_ago` hours,
/// then walks up to find the containing git repository.
#[tauri::command(rename_all = "camelCase")]
fn find_recent_repos(
    hours_ago: Option<u32>,
    limit: Option<usize>,
) -> Vec<recent_repos::RecentRepo> {
    recent_repos::find_recent_repos(hours_ago.unwrap_or(24), limit.unwrap_or(10))
}

/// Search for files matching a query in the repository.
///
/// Uses fuzzy matching - returns up to `limit` matches sorted by relevance.
#[tauri::command(rename_all = "camelCase")]
fn search_files(
    repo_path: Option<String>,
    ref_name: String,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<String>, String> {
    let path = get_repo_path(repo_path.as_deref());
    git::search_files(path, &ref_name, &query, limit.unwrap_or(20)).map_err(|e| e.to_string())
}

/// Get the content of a file at a specific ref.
#[tauri::command(rename_all = "camelCase")]
fn get_file_at_ref(
    repo_path: Option<String>,
    ref_name: String,
    path: String,
) -> Result<File, String> {
    let repo = get_repo_path(repo_path.as_deref());
    git::get_file_at_ref(repo, &ref_name, &path).map_err(|e| e.to_string())
}

// =============================================================================
// Git Commands
// =============================================================================

/// Get the absolute path to the repository root.
#[tauri::command(rename_all = "camelCase")]
fn get_repo_root(repo_path: Option<String>) -> Result<String, String> {
    let path = get_repo_path(repo_path.as_deref());
    git::get_repo_root(path).map_err(|e| e.to_string())
}

/// List refs (branches, tags, remotes) for autocomplete.
#[tauri::command(rename_all = "camelCase")]
fn list_refs(repo_path: Option<String>) -> Result<Vec<String>, String> {
    let path = get_repo_path(repo_path.as_deref());
    git::list_refs(path).map_err(|e| e.to_string())
}

/// Resolve a ref to its full SHA. Used for validation.
#[tauri::command(rename_all = "camelCase")]
fn resolve_ref(repo_path: Option<String>, reference: String) -> Result<String, String> {
    let path = get_repo_path(repo_path.as_deref());
    git::resolve_ref(path, &reference).map_err(|e| e.to_string())
}

/// Compute the merge-base between two refs.
/// Returns the SHA of the common ancestor.
#[tauri::command(rename_all = "camelCase")]
fn get_merge_base(repo_path: Option<String>, ref1: String, ref2: String) -> Result<String, String> {
    let path = get_repo_path(repo_path.as_deref());
    git::merge_base(path, &ref1, &ref2).map_err(|e| e.to_string())
}

/// List files changed in a diff (for sidebar).
/// Runs on a blocking thread to avoid freezing the UI on large repos.
#[tauri::command(rename_all = "camelCase")]
async fn list_diff_files(
    repo_path: Option<String>,
    spec: DiffSpec,
) -> Result<Vec<FileDiffSummary>, String> {
    let path = repo_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    tokio::task::spawn_blocking(move || {
        git::list_diff_files(&path, &spec).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Get full diff content for a single file.
#[tauri::command(rename_all = "camelCase")]
fn get_file_diff(
    repo_path: Option<String>,
    spec: DiffSpec,
    file_path: String,
) -> Result<FileDiff, String> {
    let path = get_repo_path(repo_path.as_deref());
    git::get_file_diff(path, &spec, Path::new(&file_path)).map_err(|e| e.to_string())
}

/// Create a commit with the specified files.
/// Returns the short SHA of the new commit.
#[tauri::command(rename_all = "camelCase")]
fn commit(
    repo_path: Option<String>,
    paths: Vec<String>,
    message: String,
) -> Result<String, String> {
    let path = get_repo_path(repo_path.as_deref());
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    git::commit(path, &paths, &message).map_err(|e| e.to_string())
}

// =============================================================================
// GitHub Commands
// =============================================================================

/// Check if GitHub CLI is installed and authenticated.
#[tauri::command]
fn check_github_auth() -> GitHubAuthStatus {
    git::check_github_auth()
}

/// Invalidate the PR list cache, forcing a fresh fetch on next request.
#[tauri::command(rename_all = "camelCase")]
fn invalidate_pr_cache(repo_path: Option<String>) {
    let path = get_repo_path(repo_path.as_deref());
    git::invalidate_pr_cache(path);
}

/// List open pull requests for the repo.
#[tauri::command(rename_all = "camelCase")]
async fn list_pull_requests(repo_path: Option<String>) -> Result<Vec<PullRequest>, String> {
    let path = repo_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    // Run on blocking thread pool to avoid blocking the UI
    tokio::task::spawn_blocking(move || git::list_pull_requests(&path).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Fetch PR refs and compute merge-base.
/// Returns DiffSpec with concrete SHAs.
#[tauri::command(rename_all = "camelCase")]
async fn fetch_pr(
    repo_path: Option<String>,
    base_ref: String,
    pr_number: u64,
) -> Result<DiffSpec, String> {
    // Convert to owned PathBuf for the blocking task
    let path = repo_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    // Run on blocking thread pool to avoid blocking the UI
    tokio::task::spawn_blocking(move || {
        git::fetch_pr(&path, &base_ref, pr_number).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Sync local review comments to a GitHub PR as a pending review.
///
/// This will delete any existing pending review and create a new one
/// with all the local comments. Returns the URL to the pending review.
#[tauri::command(rename_all = "camelCase")]
async fn sync_review_to_github(
    repo_path: Option<String>,
    pr_number: u64,
    spec: DiffSpec,
) -> Result<GitHubSyncResult, String> {
    let path = repo_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    // Get the review with comments
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(&path, &spec)?;
    let review = store.get_or_create(&id).map_err(|e| e.0)?;

    if review.comments.is_empty() {
        return Err("No comments to sync".to_string());
    }

    // Sync to GitHub
    git::sync_review_to_github(&path, pr_number, &review.comments)
        .await
        .map_err(|e| e.to_string())
}

// =============================================================================
// AI Commands
// =============================================================================

use ai::{AcpProviderInfo, ChangesetAnalysis, ChangesetSummary, SmartDiffResult};

/// Discover available ACP providers on the system.
/// Returns a list of providers that are installed and working.
#[tauri::command]
fn discover_acp_providers() -> Vec<AcpProviderInfo> {
    ai::discover_acp_providers()
}

/// Check if an AI agent is available (via ACP).
#[tauri::command(rename_all = "camelCase")]
fn check_ai_available() -> Result<String, String> {
    match ai::find_acp_agent() {
        Some(agent) => Ok(agent.name().to_string()),
        None => Err("No AI agent found. Install Goose: https://github.com/block/goose".to_string()),
    }
}

/// Analyze a diff using AI via ACP.
///
/// This is the main AI entry point - handles file listing, content loading,
/// and AI analysis in one call. Frontend just provides the diff spec.
#[tauri::command(rename_all = "camelCase")]
async fn analyze_diff(
    repo_path: Option<String>,
    spec: DiffSpec,
) -> Result<ChangesetAnalysis, String> {
    let path = get_repo_path(repo_path.as_deref()).to_path_buf();

    // analyze_diff is now async (uses ACP)
    ai::analyze_diff(&path, &spec).await
}

/// Response from send_agent_prompt including session ID for continuity.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentPromptResponse {
    response: String,
    session_id: String,
}

/// Send a prompt to the AI agent and get a response.
///
/// Accepts an optional session_id to resume an existing session. Returns both
/// the response and the session_id for future resumption. Sessions are persisted
/// in the agent's database, so context is maintained across prompts.
///
/// The provider parameter specifies which ACP provider to use (e.g., "goose" or "claude").
/// If not specified, defaults to the first available provider.
#[tauri::command(rename_all = "camelCase")]
async fn send_agent_prompt(
    repo_path: Option<String>,
    prompt: String,
    session_id: Option<String>,
    provider: Option<String>,
) -> Result<AgentPromptResponse, String> {
    let agent = if let Some(provider_id) = provider {
        ai::find_acp_agent_by_id(&provider_id).ok_or_else(|| {
            format!(
                "Provider '{}' not found. Run discover_acp_providers to see available providers.",
                provider_id
            )
        })?
    } else {
        ai::find_acp_agent().ok_or_else(|| {
            "No AI agent found. Install Goose: https://github.com/block/goose".to_string()
        })?
    };

    let path = get_repo_path(repo_path.as_deref()).to_path_buf();

    let result =
        ai::run_acp_prompt_with_session(&agent, &path, &prompt, session_id.as_deref()).await?;

    Ok(AgentPromptResponse {
        response: result.response,
        session_id: result.session_id,
    })
}

// =============================================================================
// AI Analysis Persistence Commands
// =============================================================================

/// Save a changeset summary to the database.
#[tauri::command(rename_all = "camelCase")]
fn save_changeset_summary(
    repo_path: Option<String>,
    spec: DiffSpec,
    summary: ChangesetSummary,
) -> Result<(), String> {
    let path = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(path, &spec)?;
    store.save_changeset_summary(&id, &summary).map_err(|e| e.0)
}

/// Get a saved changeset summary from the database.
#[tauri::command(rename_all = "camelCase")]
fn get_changeset_summary(
    repo_path: Option<String>,
    spec: DiffSpec,
) -> Result<Option<ChangesetSummary>, String> {
    let path = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(path, &spec)?;
    store.get_changeset_summary(&id).map_err(|e| e.0)
}

/// Save a file analysis to the database.
#[tauri::command(rename_all = "camelCase")]
fn save_file_analysis(
    repo_path: Option<String>,
    spec: DiffSpec,
    file_path: String,
    result: SmartDiffResult,
) -> Result<(), String> {
    let path = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(path, &spec)?;
    store
        .save_file_analysis(&id, &file_path, &result)
        .map_err(|e| e.0)
}

/// Get a saved file analysis from the database.
#[tauri::command(rename_all = "camelCase")]
fn get_file_analysis(
    repo_path: Option<String>,
    spec: DiffSpec,
    file_path: String,
) -> Result<Option<SmartDiffResult>, String> {
    let path = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(path, &spec)?;
    store.get_file_analysis(&id, &file_path).map_err(|e| e.0)
}

/// Get all saved file analyses for a diff.
#[tauri::command(rename_all = "camelCase")]
fn get_all_file_analyses(
    repo_path: Option<String>,
    spec: DiffSpec,
) -> Result<Vec<(String, SmartDiffResult)>, String> {
    let path = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(path, &spec)?;
    store.get_all_file_analyses(&id).map_err(|e| e.0)
}

/// Delete all AI analyses for a diff (used when refreshing).
#[tauri::command(rename_all = "camelCase")]
fn delete_all_analyses(repo_path: Option<String>, spec: DiffSpec) -> Result<(), String> {
    let path = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(path, &spec)?;
    store.delete_all_analyses(&id).map_err(|e| e.0)
}

// =============================================================================
// Review Commands
// =============================================================================

#[tauri::command(rename_all = "camelCase")]
fn get_review(repo_path: Option<String>, spec: DiffSpec) -> Result<Review, String> {
    let path = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(path, &spec)?;
    store.get_or_create(&id).map_err(|e| e.0)
}

#[tauri::command(rename_all = "camelCase")]
fn add_comment(
    repo_path: Option<String>,
    spec: DiffSpec,
    comment: NewComment,
) -> Result<Comment, String> {
    let path = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(path, &spec)?;
    let comment = Comment::new(comment.path, comment.span, comment.content);
    store.add_comment(&id, &comment).map_err(|e| e.0)?;
    Ok(comment)
}

#[tauri::command(rename_all = "camelCase")]
fn update_comment(comment_id: String, content: String) -> Result<(), String> {
    let store = review::get_store().map_err(|e| e.0)?;
    store.update_comment(&comment_id, &content).map_err(|e| e.0)
}

#[tauri::command(rename_all = "camelCase")]
fn delete_comment(comment_id: String) -> Result<(), String> {
    let store = review::get_store().map_err(|e| e.0)?;
    store.delete_comment(&comment_id).map_err(|e| e.0)
}

#[tauri::command(rename_all = "camelCase")]
fn mark_reviewed(repo_path: Option<String>, spec: DiffSpec, path: String) -> Result<(), String> {
    let repo = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(repo, &spec)?;
    store.mark_reviewed(&id, &path).map_err(|e| e.0)
}

#[tauri::command(rename_all = "camelCase")]
fn unmark_reviewed(repo_path: Option<String>, spec: DiffSpec, path: String) -> Result<(), String> {
    let repo = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(repo, &spec)?;
    store.unmark_reviewed(&id, &path).map_err(|e| e.0)
}

#[tauri::command(rename_all = "camelCase")]
fn record_edit(repo_path: Option<String>, spec: DiffSpec, edit: NewEdit) -> Result<Edit, String> {
    let path = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(path, &spec)?;
    let edit = Edit::new(edit.path, edit.diff);
    store.add_edit(&id, &edit).map_err(|e| e.0)?;
    Ok(edit)
}

#[tauri::command(rename_all = "camelCase")]
fn export_review_markdown(repo_path: Option<String>, spec: DiffSpec) -> Result<String, String> {
    let path = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(path, &spec)?;
    let review = store.get_or_create(&id).map_err(|e| e.0)?;
    Ok(review::export_markdown(&review))
}

#[tauri::command(rename_all = "camelCase")]
fn clear_review(repo_path: Option<String>, spec: DiffSpec) -> Result<(), String> {
    let path = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(path, &spec)?;
    store.delete(&id).map_err(|e| e.0)
}

#[tauri::command(rename_all = "camelCase")]
fn add_reference_file(
    repo_path: Option<String>,
    spec: DiffSpec,
    path: String,
) -> Result<(), String> {
    let repo = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(repo, &spec)?;
    store.add_reference_file(&id, &path).map_err(|e| e.0)
}

#[tauri::command(rename_all = "camelCase")]
fn remove_reference_file(
    repo_path: Option<String>,
    spec: DiffSpec,
    path: String,
) -> Result<(), String> {
    let repo = get_repo_path(repo_path.as_deref());
    let store = review::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(repo, &spec)?;
    store.remove_reference_file(&id, &path).map_err(|e| e.0)
}

// =============================================================================
// Theme Commands
// =============================================================================

/// Get list of custom themes from ~/.config/staged/themes/
#[tauri::command]
fn get_custom_themes() -> Vec<themes::CustomTheme> {
    themes::discover_custom_themes()
}

/// Read the full JSON content of a custom theme file.
#[tauri::command]
fn read_custom_theme(path: String) -> Result<String, String> {
    themes::read_theme_file(&path)
}

/// Get the path to the themes directory (creates it if needed).
#[tauri::command]
fn get_themes_dir() -> Result<String, String> {
    themes::ensure_themes_dir().map(|p| p.to_string_lossy().to_string())
}

/// Open the themes directory in the system file manager.
#[tauri::command]
fn open_themes_dir() -> Result<(), String> {
    let dir = themes::ensure_themes_dir()?;
    open::that(&dir).map_err(|e| format!("Failed to open themes directory: {}", e))
}

/// Validate a theme JSON string without installing.
#[tauri::command]
fn validate_theme(content: String) -> themes::ThemeValidation {
    themes::validate_theme(&content)
}

/// Install a theme from JSON content.
#[tauri::command]
fn install_theme(content: String, filename: String) -> Result<themes::CustomTheme, String> {
    themes::install_theme(&content, &filename)
}

/// Read a JSON file from disk (for file picker).
/// Only allows .json files for security.
#[tauri::command]
fn read_json_file(path: String) -> Result<String, String> {
    let path = Path::new(&path);

    // Security: only allow .json files
    if path.extension().and_then(|e| e.to_str()) != Some("json") {
        return Err("Only .json files are allowed".to_string());
    }

    std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))
}

// =============================================================================
// Watcher Commands
// =============================================================================

/// Start watching a repository (idempotent - no-op if already watching).
/// Fire-and-forget: returns immediately, actual setup happens on background thread.
#[tauri::command(rename_all = "camelCase")]
fn watch_repo(repo_path: String, watch_id: u64, state: State<WatcherHandle>) {
    state.watch(PathBuf::from(repo_path), watch_id);
}

/// Stop watching a repository.
/// Fire-and-forget: returns immediately, actual teardown happens on background thread.
#[tauri::command(rename_all = "camelCase")]
fn unwatch_repo(repo_path: String, state: State<WatcherHandle>) {
    state.unwatch(PathBuf::from(repo_path));
}

// =============================================================================
// Window Commands
// =============================================================================

/// Get the current window's label.
#[tauri::command]
fn get_window_label(window: tauri::Window) -> String {
    window.label().to_string()
}

/// Get the initial repository path from CLI arguments.
/// Returns the canonicalized path if a valid directory was provided, otherwise None.
#[tauri::command]
fn get_initial_path() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();

    // Skip the binary name, look for a path argument (not starting with -)
    for arg in args.iter().skip(1) {
        if arg.starts_with('-') {
            continue;
        }

        // Try to canonicalize the path
        let path = std::path::Path::new(arg);
        if let Ok(canonical) = path.canonicalize() {
            if canonical.is_dir() {
                return canonical.to_str().map(|s| s.to_string());
            }
        }
    }
    None
}

/// Install the CLI command to /usr/local/bin using a helper script with sudo.
/// Returns Ok(path) on success, Err(message) on failure.
#[tauri::command]
fn install_cli() -> Result<String, String> {
    let install_path = Path::new("/usr/local/bin/staged");
    install_cli_to(install_path, true)
}

fn install_cli_to(install_path: &Path, use_admin: bool) -> Result<String, String> {
    let cli_script = include_str!("../../bin/staged");

    // Write script to a temp file first
    let temp_path = std::env::temp_dir().join("staged-cli-install");
    std::fs::write(&temp_path, cli_script)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;

    if use_admin {
        #[cfg(target_os = "macos")]
        {
            let script = format!(
                r#"do shell script "cp '{}' '{}' && chmod +x '{}'" with administrator privileges"#,
                temp_path.display(),
                install_path.display(),
                install_path.display()
            );

            let output = std::process::Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .output()
                .map_err(|e| format!("Failed to run installer: {}", e))?;

            let _ = std::fs::remove_file(&temp_path);

            if output.status.success() {
                Ok(install_path.display().to_string())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("User canceled") || stderr.contains("(-128)") {
                    Err("Installation cancelled".to_string())
                } else {
                    Err(format!("Installation failed: {}", stderr))
                }
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = std::fs::remove_file(&temp_path);
            Err(
                "CLI installation is only supported on macOS. Copy bin/staged to your PATH manually."
                    .to_string(),
            )
        }
    } else {
        // Non-admin install: direct copy (for testing or user-writable paths)
        std::fs::copy(&temp_path, install_path)
            .map_err(|e| format!("Failed to copy CLI script: {}", e))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(install_path)
                .map_err(|e| format!("Failed to get permissions: {}", e))?
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(install_path, perms)
                .map_err(|e| format!("Failed to set permissions: {}", e))?;
        }

        let _ = std::fs::remove_file(&temp_path);
        Ok(install_path.display().to_string())
    }
}

#[cfg(test)]
mod install_cli_tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_install_cli_writes_executable_script() {
        let temp_dir = tempdir().unwrap();
        let install_path = temp_dir.path().join("staged");

        let result = install_cli_to(&install_path, false);
        assert!(result.is_ok(), "install_cli_to failed: {:?}", result);
        assert!(install_path.exists(), "CLI script was not created");

        let content = fs::read_to_string(&install_path).unwrap();
        assert!(content.contains("#!/bin/bash"), "Script missing shebang");
        assert!(
            content.contains("staged.app"),
            "Script missing app reference"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_install_cli_sets_executable_permission() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempdir().unwrap();
        let install_path = temp_dir.path().join("staged");

        install_cli_to(&install_path, false).unwrap();

        let perms = fs::metadata(&install_path).unwrap().permissions();
        let mode = perms.mode();
        assert!(mode & 0o111 != 0, "Script is not executable: {:o}", mode);
    }

    #[test]
    fn test_install_cli_returns_install_path() {
        let temp_dir = tempdir().unwrap();
        let install_path = temp_dir.path().join("staged");

        let result = install_cli_to(&install_path, false).unwrap();
        assert_eq!(result, install_path.display().to_string());
    }

    #[test]
    fn test_install_cli_fails_on_invalid_path() {
        let install_path = Path::new("/nonexistent/directory/staged");

        let result = install_cli_to(install_path, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to copy"));
    }
}

// =============================================================================
// Menu System
// =============================================================================

/// Build the application menu bar.
fn build_menu(app: &AppHandle) -> Result<Menu<Wry>, Box<dyn std::error::Error>> {
    let menu = Menu::new(app)?;

    // macOS app menu (required for Cmd+Q, Cmd+H, etc.)
    #[cfg(target_os = "macos")]
    let app_menu = {
        Submenu::with_items(
            app,
            "Staged",
            true,
            &[
                &PredefinedMenuItem::about(app, Some("About Staged"), None)?,
                &PredefinedMenuItem::separator(app)?,
                &MenuItem::with_id(
                    app,
                    "install-cli",
                    "Install CLI Command...",
                    true,
                    None::<&str>,
                )?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::services(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::show_all(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, None)?,
            ],
        )?
    };

    #[cfg(not(target_os = "macos"))]
    let app_menu = {
        Submenu::with_items(
            app,
            "Staged",
            true,
            &[
                &MenuItem::with_id(
                    app,
                    "install-cli",
                    "Install CLI Command...",
                    true,
                    None::<&str>,
                )?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::quit(app, None)?,
            ],
        )?
    };

    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &MenuItem::with_id(app, "open-folder", "New Tab...", true, Some("CmdOrCtrl+T"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "close-tab", "Close Tab", true, Some("CmdOrCtrl+W"))?,
            &MenuItem::with_id(
                app,
                "close-window",
                "Close Window",
                true,
                Some("CmdOrCtrl+Shift+W"),
            )?,
        ],
    )?;

    // Edit menu is required for standard text editing shortcuts (Cmd+A, Cmd+C, etc.)
    // to work in input fields. Without this, the shortcuts get swallowed at the
    // native menu level and never reach the webview.
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    menu.append(&app_menu)?;
    menu.append(&file_menu)?;
    menu.append(&edit_menu)?;
    Ok(menu)
}

/// Handle menu events.
fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id.as_ref() {
        "open-folder" => {
            let _ = app.emit("menu:open-folder", ());
        }
        "close-tab" => {
            let _ = app.emit("menu:close-tab", ());
        }
        "close-window" => {
            let _ = app.emit("menu:close-window", ());
        }
        "install-cli" => {
            let _ = app.emit("menu:install-cli", ());
        }
        _ => {}
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
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .setup(|app| {
            // Initialize the review store with app data directory
            review::init_store(app.handle()).map_err(|e| e.0)?;

            // Initialize the watcher handle (spawns background thread)
            let watcher = WatcherHandle::new(app.handle().clone());
            app.manage(watcher);

            // Build and set the menu
            let menu = build_menu(app.handle()).map_err(|e| e.to_string())?;
            app.set_menu(menu).map_err(|e| e.to_string())?;

            // Register menu event handler
            app.on_menu_event(move |app, event| {
                handle_menu_event(app, event);
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
            // File browsing commands
            list_directory,
            search_directories,
            get_home_dir,
            find_recent_repos,
            search_files,
            get_file_at_ref,
            // Git commands
            get_repo_root,
            list_refs,
            resolve_ref,
            get_merge_base,
            list_diff_files,
            get_file_diff,
            commit,
            // GitHub commands
            check_github_auth,
            list_pull_requests,
            fetch_pr,
            sync_review_to_github,
            invalidate_pr_cache,
            // AI commands
            analyze_diff,
            check_ai_available,
            discover_acp_providers,
            send_agent_prompt,
            // AI persistence commands
            save_changeset_summary,
            get_changeset_summary,
            save_file_analysis,
            get_file_analysis,
            get_all_file_analyses,
            delete_all_analyses,
            // Review commands
            get_review,
            add_comment,
            update_comment,
            delete_comment,
            mark_reviewed,
            unmark_reviewed,
            record_edit,
            export_review_markdown,
            clear_review,
            add_reference_file,
            remove_reference_file,
            // Theme commands
            get_custom_themes,
            read_custom_theme,
            get_themes_dir,
            open_themes_dir,
            validate_theme,
            install_theme,
            read_json_file,
            // Watcher commands
            watch_repo,
            unwatch_repo,
            // Window commands
            get_window_label,
            // CLI commands
            get_initial_path,
            install_cli,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
