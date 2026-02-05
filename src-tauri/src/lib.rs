//! Tauri commands for the Staged diff viewer.
//!
//! This module provides the bridge between the frontend and the git/github modules.
//! Supports CLI arguments: `staged [path]` opens the app with the specified directory.

pub mod ai;
pub mod git;
pub mod project;
mod recent_repos;
pub mod review;
pub mod store;
mod themes;
mod watcher;

use ai::analysis::ChangesetAnalysis;
use ai::{SessionManager, SessionStatus};
use git::{
    DiffId, DiffSpec, File, FileDiff, FileDiffSummary, GitHubAuthStatus, GitHubSyncResult, GitRef,
    PullRequest,
};
use review::{Comment, Edit, NewComment, NewEdit, Review};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{now_timestamp, SessionFull, Store};
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

/// Search for pull requests on GitHub using a query string.
#[tauri::command(rename_all = "camelCase")]
async fn search_pull_requests(
    repo_path: Option<String>,
    query: String,
) -> Result<Vec<PullRequest>, String> {
    let path = repo_path
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    // Run on blocking thread pool to avoid blocking the UI
    tokio::task::spawn_blocking(move || {
        git::search_pull_requests(&path, &query).map_err(|e| e.to_string())
    })
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

use ai::AcpProviderInfo;

/// Discover available ACP providers on the system.
/// Returns a list of providers that are installed and working.
#[tauri::command]
async fn discover_acp_providers() -> Vec<AcpProviderInfo> {
    // Run blocking shell operations on a separate thread to avoid blocking the event loop
    tokio::task::spawn_blocking(ai::discover_acp_providers)
        .await
        .unwrap_or_default()
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
///
/// The provider parameter specifies which ACP provider to use (e.g., "goose" or "claude").
/// If not specified, defaults to the first available provider.
#[tauri::command(rename_all = "camelCase")]
async fn analyze_diff(
    repo_path: Option<String>,
    spec: DiffSpec,
    provider: Option<String>,
) -> Result<ChangesetAnalysis, String> {
    let path = get_repo_path(repo_path.as_deref()).to_path_buf();

    // analyze_diff is now async (uses ACP)
    ai::analysis::analyze_diff(&path, &spec, provider.as_deref()).await
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

/// Send a prompt to the AI agent with real-time streaming events.
///
/// Similar to send_agent_prompt but emits Tauri events during execution:
/// - "session-update": SessionNotification from the ACP SDK (streaming chunks, tool calls)
/// - "session-complete": Finalized transcript when done
/// - "session-error": Error information if the session fails
///
/// Returns the same response as send_agent_prompt for compatibility.
#[tauri::command(rename_all = "camelCase")]
async fn send_agent_prompt_streaming(
    app_handle: AppHandle,
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

    // Legacy path: no internal session ID, use ACP session ID or "legacy" as fallback
    let internal_id = session_id.as_deref().unwrap_or("legacy");
    let result = ai::run_acp_prompt_streaming(
        &agent,
        &path,
        &prompt,
        session_id.as_deref(),
        internal_id,
        app_handle,
    )
    .await?;

    Ok(AgentPromptResponse {
        response: result.response,
        session_id: result.session_id,
    })
}

// =============================================================================
// Chat Session Commands (new architecture)
// =============================================================================

/// Create a new session.
/// Returns the session ID.
#[tauri::command(rename_all = "camelCase")]
async fn create_session(
    state: State<'_, Arc<SessionManager>>,
    working_dir: String,
    agent_id: Option<String>,
) -> Result<String, String> {
    state
        .create_session(PathBuf::from(working_dir), agent_id.as_deref())
        .await
}

/// Get full session with all messages.
#[tauri::command(rename_all = "camelCase")]
fn get_session(
    state: State<'_, Arc<Store>>,
    session_id: String,
) -> Result<Option<SessionFull>, String> {
    state
        .get_session_full(&session_id)
        .map_err(|e| e.to_string())
}

/// Get session status (idle, processing, error).
#[tauri::command(rename_all = "camelCase")]
async fn get_session_status(
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
) -> Result<SessionStatus, String> {
    state.get_session_status(&session_id).await
}

/// Send a prompt to a session.
/// Streams response via events, persists to database on completion.
#[tauri::command(rename_all = "camelCase")]
async fn send_prompt(
    state: State<'_, Arc<SessionManager>>,
    session_id: String,
    prompt: String,
) -> Result<(), String> {
    state.send_prompt(&session_id, prompt).await
}

/// Update session title.
#[tauri::command(rename_all = "camelCase")]
fn update_session_title(
    state: State<'_, Arc<Store>>,
    session_id: String,
    title: String,
) -> Result<(), String> {
    state
        .update_session_title(&session_id, &title)
        .map_err(|e| e.to_string())
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
// Legacy Artifact Commands (DiffSpec-based, used by AgentPanel/Sidebar)
// =============================================================================

/// Simple artifact shape expected by the frontend (review.ts).
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LegacyArtifact {
    id: String,
    title: String,
    content: String,
    created_at: String,
}

/// Derive a deterministic project ID from a DiffSpec for legacy artifact storage.
fn diff_spec_project_id(repo: &Path, spec: &DiffSpec) -> Result<String, String> {
    let diff_id = make_diff_id(repo, spec)?;
    Ok(format!("diff:{}..{}", diff_id.before, diff_id.after))
}

/// Ensure a project exists for the given DiffSpec, creating one if needed.
fn ensure_diff_project(store: &Store, repo: &Path, spec: &DiffSpec) -> Result<String, String> {
    let diff_id = make_diff_id(repo, spec)?;
    let project_id = format!("diff:{}..{}", diff_id.before, diff_id.after);
    if store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .is_none()
    {
        let project = Project {
            id: project_id.clone(),
            name: format!("{}..{}", diff_id.before, diff_id.after),
            created_at: now_timestamp(),
            updated_at: now_timestamp(),
        };
        store.create_project(&project).map_err(|e| e.to_string())?;
    }
    Ok(project_id)
}

#[tauri::command(rename_all = "camelCase")]
fn get_artifacts(
    state: State<'_, Arc<Store>>,
    repo_path: Option<String>,
    spec: DiffSpec,
) -> Result<Vec<LegacyArtifact>, String> {
    let repo = get_repo_path(repo_path.as_deref());
    let project_id = diff_spec_project_id(repo, &spec)?;

    // If no project exists yet, just return empty
    if state
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .is_none()
    {
        return Ok(vec![]);
    }

    let artifacts = state
        .list_artifacts(&project_id)
        .map_err(|e| e.to_string())?;
    Ok(artifacts
        .into_iter()
        .filter_map(|a| {
            if let ArtifactData::Markdown { content } = a.data {
                Some(LegacyArtifact {
                    id: a.id,
                    title: a.title,
                    content,
                    created_at: a.created_at.to_string(),
                })
            } else {
                None
            }
        })
        .collect())
}

#[tauri::command(rename_all = "camelCase")]
fn save_artifact(
    state: State<'_, Arc<Store>>,
    repo_path: Option<String>,
    spec: DiffSpec,
    artifact: LegacyArtifact,
) -> Result<(), String> {
    let repo = get_repo_path(repo_path.as_deref());
    let project_id = ensure_diff_project(&state, repo, &spec)?;
    let now = now_timestamp();

    let store_artifact = ProjectArtifact {
        id: artifact.id,
        project_id,
        title: artifact.title,
        data: ArtifactData::Markdown {
            content: artifact.content,
        },
        created_at: now,
        updated_at: now,
        parent_artifact_id: None,
        status: ArtifactStatus::Complete,
        error_message: None,
        session_id: None,
    };

    state
        .create_artifact(&store_artifact)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Project Commands (artifact-centric model)
// =============================================================================

use project::{Artifact as ProjectArtifact, ArtifactData, ArtifactStatus, Project};

/// Create a new project.
#[tauri::command(rename_all = "camelCase")]
fn create_project(state: State<'_, Arc<Store>>, name: String) -> Result<Project, String> {
    let project = Project::new(name);
    state.create_project(&project).map_err(|e| e.to_string())?;
    Ok(project)
}

/// Get a project by ID.
#[tauri::command(rename_all = "camelCase")]
fn get_project(
    state: State<'_, Arc<Store>>,
    project_id: String,
) -> Result<Option<Project>, String> {
    state.get_project(&project_id).map_err(|e| e.to_string())
}

/// List all projects.
#[tauri::command(rename_all = "camelCase")]
fn list_projects(state: State<'_, Arc<Store>>) -> Result<Vec<Project>, String> {
    state.list_projects().map_err(|e| e.to_string())
}

/// Update a project's name.
#[tauri::command(rename_all = "camelCase")]
fn update_project(
    state: State<'_, Arc<Store>>,
    project_id: String,
    name: String,
) -> Result<(), String> {
    state
        .update_project(&project_id, &name)
        .map_err(|e| e.to_string())
}

/// Delete a project and all its artifacts.
#[tauri::command(rename_all = "camelCase")]
fn delete_project(state: State<'_, Arc<Store>>, project_id: String) -> Result<(), String> {
    state.delete_project(&project_id).map_err(|e| e.to_string())
}

/// Create a new artifact.
#[tauri::command(rename_all = "camelCase")]
fn create_artifact(
    state: State<'_, Arc<Store>>,
    project_id: String,
    title: String,
    data: ArtifactData,
) -> Result<ProjectArtifact, String> {
    let now = now_timestamp();
    let artifact = ProjectArtifact {
        id: uuid::Uuid::new_v4().to_string(),
        project_id,
        title,
        created_at: now,
        updated_at: now,
        parent_artifact_id: None,
        data,
        status: ArtifactStatus::Complete,
        error_message: None,
        session_id: None,
    };
    state
        .create_artifact(&artifact)
        .map_err(|e| e.to_string())?;
    Ok(artifact)
}

/// Get an artifact by ID.
#[tauri::command(rename_all = "camelCase")]
fn get_artifact(
    state: State<'_, Arc<Store>>,
    artifact_id: String,
) -> Result<Option<ProjectArtifact>, String> {
    state.get_artifact(&artifact_id).map_err(|e| e.to_string())
}

/// List artifacts in a project.
#[tauri::command(rename_all = "camelCase")]
fn list_artifacts(
    state: State<'_, Arc<Store>>,
    project_id: String,
) -> Result<Vec<ProjectArtifact>, String> {
    state.list_artifacts(&project_id).map_err(|e| e.to_string())
}

/// Update an artifact.
#[tauri::command(rename_all = "camelCase")]
fn update_artifact(
    state: State<'_, Arc<Store>>,
    artifact_id: String,
    title: Option<String>,
    data: Option<ArtifactData>,
) -> Result<(), String> {
    state
        .update_artifact(&artifact_id, title.as_deref(), data.as_ref())
        .map_err(|e| e.to_string())
}

/// Delete an artifact from a project.
#[tauri::command(rename_all = "camelCase")]
fn delete_artifact(state: State<'_, Arc<Store>>, artifact_id: String) -> Result<(), String> {
    state
        .delete_artifact(&artifact_id)
        .map_err(|e| e.to_string())
}

/// Add context links to an artifact (which artifacts were used as input).
#[tauri::command(rename_all = "camelCase")]
fn add_artifact_context(
    state: State<'_, Arc<Store>>,
    artifact_id: String,
    context_artifact_ids: Vec<String>,
) -> Result<(), String> {
    for context_id in context_artifact_ids {
        state
            .add_context(&artifact_id, &context_id)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Get the artifacts that were used as context when creating an artifact.
#[tauri::command(rename_all = "camelCase")]
fn get_artifact_context(
    state: State<'_, Arc<Store>>,
    artifact_id: String,
) -> Result<Vec<String>, String> {
    state
        .get_context_artifacts(&artifact_id)
        .map_err(|e| e.to_string())
}

/// System prompt for artifact generation.
/// Instructs the AI that only its final message becomes the artifact.
const ARTIFACT_SYSTEM_PROMPT: &str = r#"You are an AI assistant helping create research documents, plans, and analysis artifacts.

IMPORTANT: Only your FINAL message will become the artifact. Any intermediate reasoning, tool calls, or exploratory work you do will NOT be shown to the user. The artifact must be completely self-contained.

Guidelines for your final response:
- Write in well-structured Markdown
- Use clear headings (##, ###) to organize content
- Include code blocks with language tags when showing code
- Be thorough but concise
- The document should stand alone without needing the conversation context

"#;

/// Generate a new artifact using AI.
///
/// Creates a placeholder artifact immediately and runs AI generation in the background.
/// Emits events as the artifact is updated:
/// - `artifact-updated`: When the artifact content/status changes
#[tauri::command(rename_all = "camelCase")]
async fn generate_artifact(
    app_handle: AppHandle,
    state: State<'_, Arc<Store>>,
    project_id: String,
    prompt: String,
    context_artifact_ids: Vec<String>,
) -> Result<ProjectArtifact, String> {
    // Create a placeholder title from the prompt
    let placeholder_title = if prompt.len() > 50 {
        format!("{}...", &prompt[..47])
    } else {
        prompt.clone()
    };

    // Create the artifact in "generating" state
    let artifact = ProjectArtifact::new_generating(&project_id, &placeholder_title);

    // Save to database
    state
        .create_artifact(&artifact)
        .map_err(|e| e.to_string())?;

    // Add context links
    for context_id in &context_artifact_ids {
        state
            .add_context(&artifact.id, context_id)
            .map_err(|e| e.to_string())?;
    }

    // Clone what we need for the background task
    let artifact_for_task = artifact.clone();
    let store_clone = state.inner().clone();

    // Spawn background task to run AI generation
    tauri::async_runtime::spawn(async move {
        run_artifact_generation(
            app_handle,
            store_clone,
            artifact_for_task,
            prompt,
            context_artifact_ids,
        )
        .await;
    });

    Ok(artifact)
}

/// Background task to run AI generation and update the artifact.
async fn run_artifact_generation(
    app_handle: AppHandle,
    store: Arc<Store>,
    artifact: ProjectArtifact,
    prompt: String,
    context_artifact_ids: Vec<String>,
) {
    // Find an AI agent
    let agent = match ai::find_acp_agent() {
        Some(a) => a,
        None => {
            // Update artifact with error
            let _ = store.update_artifact_status(
                &artifact.id,
                ArtifactStatus::Error,
                Some("No AI agent found. Install Goose: https://github.com/block/goose"),
                None,
                None,
            );
            let _ = emit_artifact_updated(&app_handle, &artifact.id);
            return;
        }
    };

    // Use current directory as working dir (artifacts aren't repo-specific)
    let working_dir = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            let _ = store.update_artifact_status(
                &artifact.id,
                ArtifactStatus::Error,
                Some(&format!("Failed to get working directory: {}", e)),
                None,
                None,
            );
            let _ = emit_artifact_updated(&app_handle, &artifact.id);
            return;
        }
    };

    // Create a session for this artifact generation
    let session_id = store::generate_session_id();
    let now = store::now_timestamp();
    let session = store::Session {
        id: session_id.clone(),
        working_dir: working_dir.to_string_lossy().to_string(),
        agent_id: agent.name().to_string(),
        title: Some(format!("Artifact: {}", artifact.title)),
        created_at: now,
        updated_at: now,
    };

    if let Err(e) = store.create_session(&session) {
        log::error!("Failed to create session for artifact: {}", e);
        // Continue without session - artifact will still work, just no session view
    } else {
        // Link session to artifact
        let _ = store.set_artifact_session(&artifact.id, &session_id);
    }

    // Build the full prompt with context
    let mut full_prompt = String::from(ARTIFACT_SYSTEM_PROMPT);

    // Add context artifacts if any
    if !context_artifact_ids.is_empty() {
        full_prompt
            .push_str("\n## Context\n\nThe following artifacts have been provided as context:\n\n");

        for artifact_id in &context_artifact_ids {
            if let Ok(Some(ctx_artifact)) = store.get_artifact(artifact_id) {
                full_prompt.push_str(&format!("### {}\n\n", ctx_artifact.title));
                if let ArtifactData::Markdown { content } = &ctx_artifact.data {
                    full_prompt.push_str(content);
                    full_prompt.push_str("\n\n---\n\n");
                }
            }
        }
    }

    // Add the user's request
    full_prompt.push_str("## Request\n\n");
    full_prompt.push_str(&prompt);
    full_prompt.push_str("\n\nPlease create a comprehensive artifact addressing this request. Remember: only your final message becomes the artifact, so make it complete and self-contained.");

    // Store the user message in the session
    let _ = store.add_message(&session_id, store::MessageRole::User, &full_prompt);

    // Call the AI with streaming (emits session-update events)
    match ai::run_acp_prompt_streaming(
        &agent,
        &working_dir,
        &full_prompt,
        None,
        &session_id,
        app_handle.clone(),
    )
    .await
    {
        Ok(result) => {
            // Store the assistant response in the session
            let _ = store.add_assistant_turn(&session_id, &result.segments);

            // Extract a title from the response
            let title = extract_title_from_markdown(&result.response, &prompt);
            let data = ArtifactData::Markdown {
                content: result.response.clone(),
            };

            // Update artifact with success
            let _ = store.update_artifact_status(
                &artifact.id,
                ArtifactStatus::Complete,
                None,
                Some(&title),
                Some(&data),
            );
        }
        Err(e) => {
            // Update artifact with error
            let _ = store.update_artifact_status(
                &artifact.id,
                ArtifactStatus::Error,
                Some(&format!("AI generation failed: {}", e)),
                None,
                None,
            );
        }
    }

    let _ = emit_artifact_updated(&app_handle, &artifact.id);
}

/// Emit an artifact-updated event to the frontend.
fn emit_artifact_updated(app_handle: &AppHandle, artifact_id: &str) -> Result<(), String> {
    app_handle
        .emit("artifact-updated", artifact_id)
        .map_err(|e| e.to_string())
}

/// Extract a title from markdown content.
/// Looks for the first # heading, or falls back to first line or prompt.
fn extract_title_from_markdown(content: &str, fallback_prompt: &str) -> String {
    // Look for first heading
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            return heading.trim().to_string();
        }
    }

    // Fall back to first non-empty line
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let title = trimmed.trim_start_matches('#').trim();
            if title.len() > 60 {
                return format!("{}...", &title[..57]);
            }
            return title.to_string();
        }
    }

    // Fall back to prompt
    let prompt_title = fallback_prompt.trim();
    if prompt_title.len() > 60 {
        format!("{}...", &prompt_title[..57])
    } else {
        prompt_title.to_string()
    }
}

// =============================================================================
// Branch Commands (git-integrated workflow)
// =============================================================================

use store::{Branch, BranchNote, BranchSession};

/// Commit info for frontend display.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitInfo {
    sha: String,
    short_sha: String,
    subject: String,
    author: String,
    timestamp: i64,
}

impl From<git::CommitInfo> for CommitInfo {
    fn from(c: git::CommitInfo) -> Self {
        Self {
            sha: c.sha,
            short_sha: c.short_sha,
            subject: c.subject,
            author: c.author,
            timestamp: c.timestamp,
        }
    }
}

/// Create a new branch with a worktree.
/// If base_branch is not provided, uses the detected default branch (e.g., origin/main).
#[tauri::command(rename_all = "camelCase")]
fn create_branch(
    state: State<'_, Arc<Store>>,
    repo_path: String,
    branch_name: String,
    base_branch: Option<String>,
) -> Result<Branch, String> {
    let repo = Path::new(&repo_path);

    // Use provided base branch or detect the default
    let base_branch = match base_branch {
        Some(b) if !b.is_empty() => b,
        _ => git::detect_default_branch(repo).map_err(|e| e.to_string())?,
    };

    // Create the worktree (this will fail atomically if branch already exists)
    let worktree_path = git::create_worktree(repo, &branch_name, &base_branch).map_err(|e| {
        // Provide user-friendly error for common case
        let msg = e.to_string();
        if msg.contains("already exists") {
            format!("Branch '{}' already exists", branch_name)
        } else {
            msg
        }
    })?;

    // Create the branch record
    let branch = Branch::new(
        &repo_path,
        &branch_name,
        worktree_path.to_string_lossy().to_string(),
        &base_branch,
    );

    // If DB insert fails, clean up the worktree
    if let Err(e) = state.create_branch(&branch) {
        let _ = git::remove_worktree(repo, &worktree_path); // Best-effort cleanup
        return Err(e.to_string());
    }

    Ok(branch)
}

/// List git branches (local and remote) for base branch selection.
#[tauri::command(rename_all = "camelCase")]
fn list_git_branches(repo_path: String) -> Result<Vec<git::BranchRef>, String> {
    let repo = Path::new(&repo_path);
    git::list_branches(repo).map_err(|e| e.to_string())
}

/// Detect the default branch for a repository.
#[tauri::command(rename_all = "camelCase")]
fn detect_default_branch(repo_path: String) -> Result<String, String> {
    let repo = Path::new(&repo_path);
    git::detect_default_branch(repo).map_err(|e| e.to_string())
}

/// Get a branch by ID.
#[tauri::command(rename_all = "camelCase")]
fn get_branch(state: State<'_, Arc<Store>>, branch_id: String) -> Result<Option<Branch>, String> {
    state.get_branch(&branch_id).map_err(|e| e.to_string())
}

/// List all branches.
#[tauri::command(rename_all = "camelCase")]
fn list_branches(state: State<'_, Arc<Store>>) -> Result<Vec<Branch>, String> {
    state.list_branches().map_err(|e| e.to_string())
}

/// List branches for a specific repository.
#[tauri::command(rename_all = "camelCase")]
fn list_branches_for_repo(
    state: State<'_, Arc<Store>>,
    repo_path: String,
) -> Result<Vec<Branch>, String> {
    state
        .list_branches_for_repo(&repo_path)
        .map_err(|e| e.to_string())
}

/// Update a branch's base branch.
#[tauri::command(rename_all = "camelCase")]
fn update_branch_base(
    state: State<'_, Arc<Store>>,
    branch_id: String,
    base_branch: String,
) -> Result<(), String> {
    state
        .update_branch_base(&branch_id, &base_branch)
        .map_err(|e| e.to_string())
}

/// Delete a branch and its worktree.
#[tauri::command(rename_all = "camelCase")]
fn delete_branch(state: State<'_, Arc<Store>>, branch_id: String) -> Result<(), String> {
    // Get the branch first
    let branch = state
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch '{}' not found", branch_id))?;

    // Remove the worktree
    let repo = Path::new(&branch.repo_path);
    let worktree = Path::new(&branch.worktree_path);
    if worktree.exists() {
        git::remove_worktree(repo, worktree).map_err(|e| e.to_string())?;
    }

    // Delete from database
    state.delete_branch(&branch_id).map_err(|e| e.to_string())?;

    Ok(())
}

/// Get commits for a branch since it diverged from base.
#[tauri::command(rename_all = "camelCase")]
fn get_branch_commits(
    state: State<'_, Arc<Store>>,
    branch_id: String,
) -> Result<Vec<CommitInfo>, String> {
    let branch = state
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch '{}' not found", branch_id))?;

    let worktree = Path::new(&branch.worktree_path);
    let commits =
        git::get_commits_since_base(worktree, &branch.base_branch).map_err(|e| e.to_string())?;

    Ok(commits.into_iter().map(Into::into).collect())
}

/// Get sessions for a branch.
#[tauri::command(rename_all = "camelCase")]
fn list_branch_sessions(
    state: State<'_, Arc<Store>>,
    branch_id: String,
) -> Result<Vec<BranchSession>, String> {
    state
        .list_branch_sessions(&branch_id)
        .map_err(|e| e.to_string())
}

/// Get the session associated with a specific commit.
#[tauri::command(rename_all = "camelCase")]
fn get_session_for_commit(
    state: State<'_, Arc<Store>>,
    branch_id: String,
    commit_sha: String,
) -> Result<Option<BranchSession>, String> {
    state
        .get_session_for_commit(&branch_id, &commit_sha)
        .map_err(|e| e.to_string())
}

/// Get the currently running session for a branch (if any).
#[tauri::command(rename_all = "camelCase")]
fn get_running_session(
    state: State<'_, Arc<Store>>,
    branch_id: String,
) -> Result<Option<BranchSession>, String> {
    state
        .get_running_session(&branch_id)
        .map_err(|e| e.to_string())
}

/// Response from starting a branch session
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StartBranchSessionResponse {
    branch_session_id: String,
    ai_session_id: String,
}

/// Start a new session on a branch.
/// Creates an AI session, then a branch_session record linking to it, and sends the prompt.
///
/// - `user_prompt`: The user's original prompt (stored for display in the UI)
/// - `full_prompt`: The full prompt with context to send to the AI agent
#[tauri::command(rename_all = "camelCase")]
async fn start_branch_session(
    state: State<'_, Arc<Store>>,
    session_manager: State<'_, Arc<SessionManager>>,
    branch_id: String,
    user_prompt: String,
    full_prompt: String,
) -> Result<StartBranchSessionResponse, String> {
    // Get the branch to find the worktree path
    let branch = state
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch '{}' not found", branch_id))?;

    // Check if there's already a running session
    if let Some(running) = state
        .get_running_session(&branch_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "Branch already has a running session: {}",
            running.id
        ));
    }

    // Create an AI session in the worktree directory FIRST
    // This way we have the ai_session_id to store in the branch session
    let worktree_path = std::path::PathBuf::from(&branch.worktree_path);
    let ai_session_id = session_manager
        .create_session(worktree_path, None)
        .await
        .map_err(|e| format!("Failed to create AI session: {}", e))?;

    // Create the branch session record with the AI session ID
    // Store the user's original prompt for display purposes
    let branch_session = BranchSession::new_running(&branch_id, &ai_session_id, &user_prompt);
    state
        .create_branch_session(&branch_session)
        .map_err(|e| format!("Failed to create branch session: {}", e))?;

    // Send the full prompt (with context) to the AI
    if let Err(e) = session_manager
        .send_prompt(&ai_session_id, full_prompt)
        .await
    {
        // Clean up on failure
        let _ = state.delete_branch_session(&branch_session.id);
        return Err(format!("Failed to send prompt: {}", e));
    }

    Ok(StartBranchSessionResponse {
        branch_session_id: branch_session.id,
        ai_session_id,
    })
}

/// Mark a branch session as completed with a commit SHA.
#[tauri::command(rename_all = "camelCase")]
fn complete_branch_session(
    state: State<'_, Arc<Store>>,
    branch_session_id: String,
    commit_sha: String,
) -> Result<(), String> {
    state
        .update_branch_session_completed(&branch_session_id, &commit_sha)
        .map_err(|e| e.to_string())
}

/// Mark a branch session as failed with an error message.
#[tauri::command(rename_all = "camelCase")]
fn fail_branch_session(
    state: State<'_, Arc<Store>>,
    branch_session_id: String,
    error_message: String,
) -> Result<(), String> {
    state
        .update_branch_session_error(&branch_session_id, &error_message)
        .map_err(|e| e.to_string())
}

/// Cancel a running branch session (deletes the record).
/// Used to recover from stuck sessions.
#[tauri::command(rename_all = "camelCase")]
fn cancel_branch_session(
    state: State<'_, Arc<Store>>,
    branch_session_id: String,
) -> Result<(), String> {
    state
        .delete_branch_session(&branch_session_id)
        .map_err(|e| e.to_string())
}

/// Recover orphaned sessions for a branch.
/// If there's a "running" session but no live AI session, check if commits were made
/// and mark the session as completed or errored accordingly.
#[tauri::command(rename_all = "camelCase")]
fn recover_orphaned_session(
    state: State<'_, Arc<Store>>,
    branch_id: String,
) -> Result<Option<BranchSession>, String> {
    // Check if there's a running session
    let running = state
        .get_running_session(&branch_id)
        .map_err(|e| e.to_string())?;

    let Some(session) = running else {
        return Ok(None);
    };

    // Get the branch to check for commits
    let branch = state
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch '{}' not found", branch_id))?;

    // Get HEAD commit in the worktree
    let worktree_path = std::path::Path::new(&branch.worktree_path);
    let head_sha = git::get_head_sha(worktree_path).map_err(|e| e.to_string())?;

    // Get commits since base to see if there are any new ones
    let commits = git::get_commits_since_base(worktree_path, &branch.base_branch)
        .map_err(|e| e.to_string())?;

    if !commits.is_empty() {
        // There are commits - mark session as completed with the HEAD commit
        state
            .update_branch_session_completed(&session.id, &head_sha)
            .map_err(|e| e.to_string())?;

        // Return the updated session
        state
            .get_branch_session(&session.id)
            .map_err(|e| e.to_string())
    } else {
        // No commits - mark as error (session ran but produced nothing)
        state
            .update_branch_session_error(&session.id, "Session ended without creating a commit")
            .map_err(|e| e.to_string())?;

        state
            .get_branch_session(&session.id)
            .map_err(|e| e.to_string())
    }
}

/// Get a branch session by its AI session ID.
/// Used by the frontend to look up branch sessions when AI session status changes.
#[tauri::command(rename_all = "camelCase")]
fn get_branch_session_by_ai_session(
    state: State<'_, Arc<Store>>,
    ai_session_id: String,
) -> Result<Option<BranchSession>, String> {
    state
        .get_branch_session_by_ai_session(&ai_session_id)
        .map_err(|e| e.to_string())
}

/// Get the HEAD commit SHA for a branch's worktree.
#[tauri::command(rename_all = "camelCase")]
fn get_branch_head(state: State<'_, Arc<Store>>, branch_id: String) -> Result<String, String> {
    let branch = state
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch '{}' not found", branch_id))?;

    let worktree = Path::new(&branch.worktree_path);
    git::get_head_sha(worktree).map_err(|e| e.to_string())
}

// =============================================================================
// Branch Note Commands
// =============================================================================

/// Response from starting a branch note generation.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StartBranchNoteResponse {
    branch_note_id: String,
    ai_session_id: String,
}

/// Start generating a new note on a branch.
/// Creates an AI session, then a branch_note record, and sends the prompt.
#[tauri::command(rename_all = "camelCase")]
async fn start_branch_note(
    state: State<'_, Arc<Store>>,
    session_manager: State<'_, Arc<SessionManager>>,
    branch_id: String,
    title: String,
    prompt: String,
) -> Result<StartBranchNoteResponse, String> {
    // Get the branch to find the worktree path
    let branch = state
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch '{}' not found", branch_id))?;

    // Check if there's already a generating note
    if let Some(generating) = state
        .get_generating_note(&branch_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "Branch already has a note being generated: {}",
            generating.id
        ));
    }

    // Create an AI session in the worktree directory
    let worktree_path = std::path::PathBuf::from(&branch.worktree_path);
    let ai_session_id = session_manager
        .create_session(worktree_path, None)
        .await
        .map_err(|e| format!("Failed to create AI session: {}", e))?;

    // Create the branch note record
    let branch_note = BranchNote::new_generating(&branch_id, &ai_session_id, &title, &prompt);
    state
        .create_branch_note(&branch_note)
        .map_err(|e| format!("Failed to create branch note: {}", e))?;

    // Send the prompt (this runs async in background)
    if let Err(e) = session_manager.send_prompt(&ai_session_id, prompt).await {
        // Clean up on failure
        let _ = state.delete_branch_note(&branch_note.id);
        return Err(format!("Failed to send prompt: {}", e));
    }

    Ok(StartBranchNoteResponse {
        branch_note_id: branch_note.id,
        ai_session_id,
    })
}

/// List all notes for a branch.
#[tauri::command(rename_all = "camelCase")]
fn list_branch_notes(
    state: State<'_, Arc<Store>>,
    branch_id: String,
) -> Result<Vec<BranchNote>, String> {
    state
        .list_branch_notes(&branch_id)
        .map_err(|e| e.to_string())
}

/// Get a branch note by ID.
#[tauri::command(rename_all = "camelCase")]
fn get_branch_note(
    state: State<'_, Arc<Store>>,
    note_id: String,
) -> Result<Option<BranchNote>, String> {
    state.get_branch_note(&note_id).map_err(|e| e.to_string())
}

/// Get the currently generating note for a branch (if any).
#[tauri::command(rename_all = "camelCase")]
fn get_generating_note(
    state: State<'_, Arc<Store>>,
    branch_id: String,
) -> Result<Option<BranchNote>, String> {
    state
        .get_generating_note(&branch_id)
        .map_err(|e| e.to_string())
}

/// Get a branch note by its AI session ID.
#[tauri::command(rename_all = "camelCase")]
fn get_branch_note_by_ai_session(
    state: State<'_, Arc<Store>>,
    ai_session_id: String,
) -> Result<Option<BranchNote>, String> {
    state
        .get_branch_note_by_ai_session(&ai_session_id)
        .map_err(|e| e.to_string())
}

/// Mark a branch note as completed with content.
#[tauri::command(rename_all = "camelCase")]
fn complete_branch_note(
    state: State<'_, Arc<Store>>,
    note_id: String,
    content: String,
) -> Result<(), String> {
    state
        .update_branch_note_completed(&note_id, &content)
        .map_err(|e| e.to_string())
}

/// Mark a branch note as failed with an error message.
#[tauri::command(rename_all = "camelCase")]
fn fail_branch_note(
    state: State<'_, Arc<Store>>,
    note_id: String,
    error_message: String,
) -> Result<(), String> {
    state
        .update_branch_note_error(&note_id, &error_message)
        .map_err(|e| e.to_string())
}

/// Delete a branch note.
#[tauri::command(rename_all = "camelCase")]
fn delete_branch_note(state: State<'_, Arc<Store>>, note_id: String) -> Result<(), String> {
    state
        .delete_branch_note(&note_id)
        .map_err(|e| e.to_string())
}

/// Recover an orphaned note for a branch.
/// If there's a "generating" note but the AI session is idle, extracts the final
/// message content and marks the note as complete.
#[tauri::command(rename_all = "camelCase")]
fn recover_orphaned_note(
    state: State<'_, Arc<Store>>,
    branch_id: String,
) -> Result<Option<BranchNote>, String> {
    // Check if there's a generating note
    let generating = state
        .get_generating_note(&branch_id)
        .map_err(|e| e.to_string())?;

    let Some(note) = generating else {
        return Ok(None);
    };

    // Get the AI session to extract the final message
    let Some(ai_session_id) = &note.ai_session_id else {
        // No AI session - mark as error
        state
            .update_branch_note_error(&note.id, "No AI session associated with note")
            .map_err(|e| e.to_string())?;
        return state.get_branch_note(&note.id).map_err(|e| e.to_string());
    };

    // Get the session messages
    let session = state
        .get_session_full(ai_session_id)
        .map_err(|e| e.to_string())?;

    let Some(session) = session else {
        state
            .update_branch_note_error(&note.id, "AI session not found")
            .map_err(|e| e.to_string())?;
        return state.get_branch_note(&note.id).map_err(|e| e.to_string());
    };

    // Find the last assistant message and extract text content
    let content = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == store::MessageRole::Assistant)
        .map(|m| extract_text_from_assistant_content(&m.content))
        .unwrap_or_default();

    if content.is_empty() {
        state
            .update_branch_note_error(&note.id, "AI session produced no content")
            .map_err(|e| e.to_string())?;
    } else {
        state
            .update_branch_note_completed(&note.id, &content)
            .map_err(|e| e.to_string())?;
    }

    state.get_branch_note(&note.id).map_err(|e| e.to_string())
}

/// Extract text content from an assistant message (which is JSON-encoded segments).
fn extract_text_from_assistant_content(content: &str) -> String {
    // Assistant content is stored as JSON array of segments
    // Each segment is either { "type": "text", "text": "..." } or { "type": "toolCall", ... }
    let segments: Vec<serde_json::Value> = serde_json::from_str(content).unwrap_or_default();

    segments
        .iter()
        .filter_map(|seg| {
            if seg.get("type")?.as_str()? == "text" {
                seg.get("text")?.as_str().map(String::from)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("")
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

/// Open a URL in the default browser.
#[tauri::command]
fn open_url(url: &str) -> Result<(), String> {
    open::that(url).map_err(|e| e.to_string())
}

// =============================================================================
// Open In... Commands
// =============================================================================

/// An application that can open a directory.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct OpenerApp {
    /// Unique identifier (e.g., "vscode", "terminal", "warp")
    id: String,
    /// Display name (e.g., "VS Code", "Terminal")
    name: String,
}

/// Known apps we can detect and open directories in.
/// Each entry: (id, display_name, macOS_bundle_id, open_command_args)
#[cfg(target_os = "macos")]
const KNOWN_OPENERS: &[(&str, &str, &str)] = &[
    ("terminal", "Terminal", "com.apple.Terminal"),
    ("warp", "Warp", "dev.warp.Warp-Stable"),
    ("iterm", "iTerm", "com.googlecode.iterm2"),
    ("ghostty", "Ghostty", "com.mitchellh.ghostty"),
    ("vscode", "VS Code", "com.microsoft.VSCode"),
    ("cursor", "Cursor", "com.todesktop.230313mzl4w4u92"),
    ("windsurf", "Windsurf", "com.codeium.windsurf"),
    (
        "github-desktop",
        "GitHub Desktop",
        "com.github.GitHubClient",
    ),
    ("finder", "Finder", "com.apple.finder"),
];

/// Get the list of supported apps that are currently installed.
/// Uses macOS `mdfind` to check for bundle IDs.
#[tauri::command]
fn get_available_openers() -> Vec<OpenerApp> {
    #[cfg(target_os = "macos")]
    {
        KNOWN_OPENERS
            .iter()
            .filter(|(_, _, bundle_id)| {
                std::process::Command::new("mdfind")
                    .args(["kMDItemCFBundleIdentifier", "=", bundle_id])
                    .output()
                    .map(|o| {
                        o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().is_empty()
                    })
                    .unwrap_or(false)
            })
            .map(|(id, name, _)| OpenerApp {
                id: id.to_string(),
                name: name.to_string(),
            })
            .collect()
    }

    #[cfg(not(target_os = "macos"))]
    {
        vec![]
    }
}

/// Open a directory path in a specific application.
#[tauri::command(rename_all = "camelCase")]
fn open_in_app(path: String, app_id: String) -> Result<(), String> {
    let dir = Path::new(&path);
    if !dir.exists() {
        return Err(format!("Directory does not exist: {}", path));
    }

    #[cfg(target_os = "macos")]
    {
        let bundle_id = KNOWN_OPENERS
            .iter()
            .find(|(id, _, _)| *id == app_id)
            .map(|(_, _, bid)| *bid)
            .ok_or_else(|| format!("Unknown app: {}", app_id))?;

        let output = std::process::Command::new("open")
            .args(["-b", bundle_id, &path])
            .output()
            .map_err(|e| format!("Failed to launch app: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("Failed to open in {}: {}", app_id, stderr.trim()))
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (dir, app_id);
        Err("Open in app is only supported on macOS".to_string())
    }
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

            // Initialize the unified store (sessions, projects, artifacts)
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("Cannot get app data dir: {}", e))?;
            let db_path = app_data_dir.join("data.db");
            let store =
                Arc::new(Store::open(db_path).map_err(|e| format!("Failed to open store: {}", e))?);
            app.manage(store.clone());

            // Initialize the session manager
            let session_manager = Arc::new(SessionManager::new(app.handle().clone(), store));
            app.manage(session_manager);

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
                app.handle().plugin(tauri_plugin_mcp_bridge::init())?;
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
            search_pull_requests,
            fetch_pr,
            sync_review_to_github,
            invalidate_pr_cache,
            // AI commands (analysis)
            check_ai_available,
            discover_acp_providers,
            analyze_diff,
            send_agent_prompt,
            send_agent_prompt_streaming,
            // Session commands
            create_session,
            get_session,
            get_session_status,
            send_prompt,
            update_session_title,
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
            // Legacy artifact commands (DiffSpec-based, used by AgentPanel/Sidebar)
            get_artifacts,
            save_artifact,
            // Project commands (artifact-centric model)
            create_project,
            get_project,
            list_projects,
            update_project,
            delete_project,
            create_artifact,
            get_artifact,
            list_artifacts,
            update_artifact,
            delete_artifact,
            add_artifact_context,
            get_artifact_context,
            generate_artifact,
            // Branch commands (git-integrated workflow)
            create_branch,
            get_branch,
            list_branches,
            list_branches_for_repo,
            list_git_branches,
            detect_default_branch,
            delete_branch,
            update_branch_base,
            get_branch_commits,
            list_branch_sessions,
            get_session_for_commit,
            get_running_session,
            start_branch_session,
            complete_branch_session,
            fail_branch_session,
            cancel_branch_session,
            recover_orphaned_session,
            get_branch_session_by_ai_session,
            get_branch_head,
            // Branch note commands
            start_branch_note,
            list_branch_notes,
            get_branch_note,
            get_generating_note,
            get_branch_note_by_ai_session,
            complete_branch_note,
            fail_branch_note,
            delete_branch_note,
            recover_orphaned_note,
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
            open_url,
            // Open in... commands
            get_available_openers,
            open_in_app,
            // CLI commands
            get_initial_path,
            install_cli,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
