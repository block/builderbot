mod git;

use git::{GitError, GitStatus};

#[tauri::command]
fn get_git_status(path: Option<String>) -> Result<GitStatus, String> {
    git::get_status(path.as_deref()).map_err(|e| e.message)
}

#[tauri::command]
fn open_repository(path: String) -> Result<GitStatus, String> {
    git::get_status(Some(&path)).map_err(|e| e.message)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
        .invoke_handler(tauri::generate_handler![get_git_status, open_repository])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
