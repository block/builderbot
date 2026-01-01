pub mod diff;
mod refresh;
mod watcher;

use diff::{Comment, DiffId, Edit, GitRef, NewComment, NewEdit, RepoInfo, Review};
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
/// All other refs are resolved to their full SHA.
fn resolve_for_storage(repo: &git2::Repository, ref_str: &str) -> Result<String, String> {
    if ref_str == diff::WORKDIR {
        return Ok(diff::WORKDIR.to_string());
    }

    let obj = repo
        .revparse_single(ref_str)
        .map_err(|e| format!("Cannot resolve '{}': {}", ref_str, e))?;

    Ok(obj.id().to_string())
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
/// Returns all changed files with their content and alignments.
#[tauri::command]
fn get_diff(
    repo_path: Option<String>,
    base: String,
    head: String,
) -> Result<Vec<diff::FileDiff>, String> {
    let repo = open_repo_from_path(repo_path.as_deref())?;
    diff::compute_diff(&repo, &base, &head).map_err(|e| e.0)
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

// =============================================================================
// Review Commands
// =============================================================================

#[tauri::command]
fn get_review(base: String, head: String) -> Result<Review, String> {
    let store = diff::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(None, &base, &head)?;
    store.get_or_create(&id).map_err(|e| e.0)
}

#[tauri::command]
fn add_comment(base: String, head: String, comment: NewComment) -> Result<Comment, String> {
    let store = diff::get_store().map_err(|e| e.0)?;
    let id = make_diff_id(None, &base, &head)?;
    let comment = Comment::new(comment.path, comment.selection, comment.content);
    store.add_comment(&id, &comment).map_err(|e| e.0)?;
    Ok(comment)
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
            // Review commands
            get_review,
            add_comment,
            delete_comment,
            mark_reviewed,
            unmark_reviewed,
            record_edit,
            export_review_markdown,
            clear_review,
            // Watcher commands
            start_watching,
            stop_watching,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
