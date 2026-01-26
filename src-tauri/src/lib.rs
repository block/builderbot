//! Tauri commands for the Staged diff viewer.
//!
//! This module provides the bridge between the frontend and the git/github modules.

pub mod ai;
pub mod git;
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
        }
    };

    Ok(DiffId::new(resolve(&spec.base)?, resolve(&spec.head)?))
}

// =============================================================================
// File Browsing Commands
// =============================================================================

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

use ai::{ChangesetAnalysis, ChangesetSummary, SmartDiffResult};

/// Check if an AI CLI tool is available.
#[tauri::command(rename_all = "camelCase")]
fn check_ai_available() -> Result<String, String> {
    match ai::find_ai_tool() {
        Some(tool) => Ok(tool.name().to_string()),
        None => Err("No AI CLI found. Install goose or claude.".to_string()),
    }
}

/// Analyze a diff using AI.
///
/// This is the main AI entry point - handles file listing, content loading,
/// and AI analysis in one call. Frontend just provides the diff spec.
#[tauri::command(rename_all = "camelCase")]
async fn analyze_diff(
    repo_path: Option<String>,
    spec: DiffSpec,
) -> Result<ChangesetAnalysis, String> {
    let path = get_repo_path(repo_path.as_deref()).to_path_buf();

    // Run on blocking thread pool since this does file I/O and spawns a subprocess
    tokio::task::spawn_blocking(move || ai::analyze_diff(&path, &spec))
        .await
        .map_err(|e| format!("Task join error: {}", e))?
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

// =============================================================================
// Menu System
// =============================================================================

/// Build the application menu bar.
fn build_menu(app: &AppHandle) -> Result<Menu<Wry>, Box<dyn std::error::Error>> {
    let menu = Menu::new(app)?;

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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
