pub mod diff;
mod refresh;
mod themes;
mod watcher;

use diff::{
    Comment, DiffId, Edit, GitHubAuthStatus, GitRef, NewComment, NewEdit, PRFetchResult,
    PullRequest, RepoInfo, Review,
};
use refresh::RefreshController;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, State};

// =============================================================================
// Helpers
// =============================================================================

/// Open a repository from an optional path (defaults to current directory).
fn open_repo_from_path(repo_path: Option<&str>) -> Result<git2::Repository, String> {
    let path = repo_path
        .map(std::path::Path::new)
        .unwrap_or_else(|| std::path::Path::new("."));
    diff::open_repo(path).map_err(|e| e.0)
}

/// Resolve a ref to a full SHA for use as a stable storage key.
/// WORKDIR is kept as-is (represents working tree).
/// Full SHAs (40 hex chars) are kept as-is - they're already stable.
/// All other refs are resolved to their full SHA.
fn resolve_for_storage(repo: &git2::Repository, ref_str: &str) -> Result<String, String> {
    if ref_str == diff::WORKDIR {
        return Ok(diff::WORKDIR.to_string());
    }

    // If it's already a full SHA, use it directly.
    // This handles cases where the SHA might be from a fetched PR ref
    // that isn't reachable from local branches.
    if is_full_sha(ref_str) {
        return Ok(ref_str.to_string());
    }

    let obj = repo
        .revparse_single(ref_str)
        .map_err(|e| format!("Cannot resolve '{}': {}", ref_str, e))?;

    Ok(obj.id().to_string())
}

/// Check if a string is a full 40-character SHA.
fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Create a DiffId with resolved SHAs for stable storage.
fn make_diff_id(repo_path: Option<&str>, base: &str, head: &str) -> Result<DiffId, String> {
    let repo = open_repo_from_path(repo_path)?;
    let resolved_base = resolve_for_storage(&repo, base)?;
    let resolved_head = resolve_for_storage(&repo, head)?;
    Ok(DiffId::new(resolved_base, resolved_head))
}

// =============================================================================
// Diff Commands
// =============================================================================

/// Get the full diff between two refs.
///
/// If `use_merge_base` is true, diffs from the merge-base instead of base directly.
#[tauri::command]
fn get_diff(
    repo_path: Option<String>,
    base: String,
    head: String,
    use_merge_base: Option<bool>,
) -> Result<Vec<diff::FileDiff>, String> {
    let repo = open_repo_from_path(repo_path.as_deref())?;
    diff::compute_diff(&repo, &base, &head, use_merge_base.unwrap_or(false)).map_err(|e| e.0)
}

/// Get list of refs (branches, tags, special) with type info for autocomplete.
#[tauri::command]
fn get_refs(repo_path: Option<String>) -> Result<Vec<GitRef>, String> {
    let repo = open_repo_from_path(repo_path.as_deref())?;
    diff::get_refs(&repo).map_err(|e| e.0)
}

/// Resolve a ref to its short SHA for display/validation.
#[tauri::command]
fn resolve_ref(repo_path: Option<String>, ref_str: String) -> Result<String, String> {
    let repo = open_repo_from_path(repo_path.as_deref())?;
    diff::resolve_ref(&repo, &ref_str).map_err(|e| e.0)
}

// =============================================================================
// Git Commands
// =============================================================================

/// Get basic repository info (path and branch name).
#[tauri::command]
fn get_repo_info(repo_path: Option<String>) -> Result<RepoInfo, String> {
    let repo = open_repo_from_path(repo_path.as_deref())?;
    diff::get_repo_info(&repo).map_err(|e| e.0)
}

/// Get the last commit message (for amend UI).
#[tauri::command]
fn get_last_commit_message(repo_path: Option<String>) -> Result<Option<String>, String> {
    let repo = open_repo_from_path(repo_path.as_deref())?;
    diff::last_commit_message(&repo).map_err(|e| e.0)
}

/// Create a commit with the specified files and message.
///
/// Returns the short SHA of the new commit.
#[tauri::command]
fn create_commit(
    repo_path: Option<String>,
    paths: Vec<String>,
    message: String,
) -> Result<String, String> {
    let repo = open_repo_from_path(repo_path.as_deref())?;
    diff::create_commit(&repo, &paths, &message).map_err(|e| e.0)
}

// =============================================================================
// GitHub Commands
// =============================================================================

/// Check if the user is authenticated with GitHub CLI.
#[tauri::command]
fn check_github_auth() -> GitHubAuthStatus {
    diff::check_github_auth()
}

/// List open pull requests for the current repository.
///
/// Returns PRs from GitHub API, using cache when available.
/// Pass `force_refresh: true` to bypass cache.
#[tauri::command]
async fn list_pull_requests(
    repo_path: Option<String>,
    force_refresh: Option<bool>,
) -> Result<Vec<PullRequest>, String> {
    // Get GitHub token first
    let token = diff::github::get_github_token().map_err(|e| e.0)?;

    // Open repo and find GitHub remote
    let repo = open_repo_from_path(repo_path.as_deref())?;
    let gh_repo = diff::get_github_remote(&repo).ok_or_else(|| {
        "No GitHub remote found. This repository is not hosted on GitHub.".to_string()
    })?;

    // Fetch PRs (with caching)
    diff::list_pull_requests(&gh_repo, &token, force_refresh.unwrap_or(false))
        .await
        .map_err(|e| e.0)
}

/// Fetch a PR branch from the remote and set up locally.
///
/// This is idempotent - if the branch already exists, it will be updated.
/// Returns both the merge-base SHA and head SHA for stable diff identification.
#[tauri::command]
fn fetch_pr_branch(
    repo_path: Option<String>,
    base_ref: String,
    pr_number: u32,
) -> Result<PRFetchResult, String> {
    let repo = open_repo_from_path(repo_path.as_deref())?;
    diff::fetch_pr_branch(&repo, &base_ref, pr_number).map_err(|e| e.0)
}

// =============================================================================
// Review Commands
// =============================================================================

#[tauri::command]
fn get_review(base: String, head: String, repo_path: Option<String>) -> Result<Review, String> {
    let store = diff::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(repo_path.as_deref(), &base, &head)?;
    store.get_or_create(&id).map_err(|e| e.0)
}

#[tauri::command]
fn add_comment(
    base: String,
    head: String,
    comment: NewComment,
    repo_path: Option<String>,
) -> Result<Comment, String> {
    let store = diff::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(repo_path.as_deref(), &base, &head)?;
    let comment = Comment::new(comment.path, comment.span, comment.content);
    store.add_comment(&id, &comment).map_err(|e| e.0)?;
    Ok(comment)
}

#[tauri::command]
fn update_comment(comment_id: String, content: String) -> Result<(), String> {
    let store = diff::get_store().map_err(|e| e.0)?;
    store.update_comment(&comment_id, &content).map_err(|e| e.0)
}

#[tauri::command]
fn delete_comment(comment_id: String) -> Result<(), String> {
    let store = diff::get_store().map_err(|e| e.0)?;
    store.delete_comment(&comment_id).map_err(|e| e.0)
}

#[tauri::command]
fn mark_reviewed(base: String, head: String, path: String) -> Result<(), String> {
    let store = diff::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(None, &base, &head)?;
    store.mark_reviewed(&id, &path).map_err(|e| e.0)
}

#[tauri::command]
fn unmark_reviewed(base: String, head: String, path: String) -> Result<(), String> {
    let store = diff::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(None, &base, &head)?;
    store.unmark_reviewed(&id, &path).map_err(|e| e.0)
}

#[tauri::command]
fn record_edit(base: String, head: String, edit: NewEdit) -> Result<Edit, String> {
    let store = diff::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(None, &base, &head)?;
    let edit = Edit::new(edit.path, edit.diff);
    store.add_edit(&id, &edit).map_err(|e| e.0)?;
    Ok(edit)
}

#[tauri::command]
fn export_review_markdown(base: String, head: String) -> Result<String, String> {
    let store = diff::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(None, &base, &head)?;
    let review = store.get_or_create(&id).map_err(|e| e.0)?;
    Ok(diff::export_markdown(&review))
}

#[tauri::command]
fn clear_review(base: String, head: String) -> Result<(), String> {
    let store = diff::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(None, &base, &head)?;
    store.delete(&id).map_err(|e| e.0)
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
    use std::path::Path;

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

/// State container for the refresh controller.
/// Wrapped in Option because it's created during setup with the AppHandle.
struct RefreshControllerState(Mutex<Option<RefreshController>>);

#[tauri::command]
async fn start_watching(
    repo_path: String,
    state: State<'_, RefreshControllerState>,
) -> Result<(), String> {
    let controller = state.0.lock().unwrap();
    if let Some(ref ctrl) = *controller {
        ctrl.start(PathBuf::from(repo_path))
    } else {
        Err("Refresh controller not initialized".to_string())
    }
}

#[tauri::command]
fn stop_watching(state: State<RefreshControllerState>) -> Result<(), String> {
    let controller = state.0.lock().unwrap();
    if let Some(ref ctrl) = *controller {
        ctrl.stop();
        Ok(())
    } else {
        Err("Refresh controller not initialized".to_string())
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
        .manage(RefreshControllerState(Mutex::new(None)))
        .setup(|app| {
            // Initialize the review store with app data directory
            diff::init_store(app.handle()).map_err(|e| e.0)?;

            // Initialize the refresh controller with the app handle
            let controller = RefreshController::new(app.handle().clone());
            let state: State<RefreshControllerState> = app.state();
            *state.0.lock().unwrap() = Some(controller);

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
            // Diff commands
            get_diff,
            get_refs,
            resolve_ref,
            // Git commands
            get_repo_info,
            get_last_commit_message,
            create_commit,
            // GitHub commands
            check_github_auth,
            list_pull_requests,
            fetch_pr_branch,
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
            // Theme commands
            get_custom_themes,
            read_custom_theme,
            get_themes_dir,
            open_themes_dir,
            validate_theme,
            install_theme,
            read_json_file,
            // Watcher commands
            start_watching,
            stop_watching,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
