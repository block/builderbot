mod git;

use git::{FileDiff, GitStatus};

#[tauri::command]
fn get_git_status(path: Option<String>) -> Result<GitStatus, String> {
    git::get_status(path.as_deref()).map_err(|e| e.message)
}

#[tauri::command]
fn open_repository(path: String) -> Result<GitStatus, String> {
    git::get_status(Some(&path)).map_err(|e| e.message)
}

#[tauri::command]
fn get_file_diff(
    repo_path: Option<String>,
    file_path: String,
    staged: bool,
) -> Result<FileDiff, String> {
    git::get_file_diff(repo_path.as_deref(), &file_path, staged).map_err(|e| e.message)
}

#[tauri::command]
fn get_untracked_file_diff(
    repo_path: Option<String>,
    file_path: String,
) -> Result<FileDiff, String> {
    git::get_untracked_file_diff(repo_path.as_deref(), &file_path).map_err(|e| e.message)
}

#[tauri::command]
fn stage_file(repo_path: Option<String>, file_path: String) -> Result<(), String> {
    git::stage_file(repo_path.as_deref(), &file_path).map_err(|e| e.message)
}

#[tauri::command]
fn unstage_file(repo_path: Option<String>, file_path: String) -> Result<(), String> {
    git::unstage_file(repo_path.as_deref(), &file_path).map_err(|e| e.message)
}

#[tauri::command]
fn discard_file(repo_path: Option<String>, file_path: String) -> Result<(), String> {
    git::discard_file(repo_path.as_deref(), &file_path).map_err(|e| e.message)
}

#[tauri::command]
fn stage_all(repo_path: Option<String>) -> Result<(), String> {
    git::stage_all(repo_path.as_deref()).map_err(|e| e.message)
}

#[tauri::command]
fn unstage_all(repo_path: Option<String>) -> Result<(), String> {
    git::unstage_all(repo_path.as_deref()).map_err(|e| e.message)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
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
            get_git_status,
            open_repository,
            get_file_diff,
            get_untracked_file_diff,
            stage_file,
            unstage_file,
            discard_file,
            stage_all,
            unstage_all
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
