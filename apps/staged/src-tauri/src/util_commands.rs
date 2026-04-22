//! Utility commands — miscellaneous helpers for the frontend.

use crate::blox;
use serde::Serialize;
use std::path::Path;

/// An application that can open directories.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenerApp {
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

/// Open a URL in the user's default browser.
#[tauri::command]
pub fn open_url(app_handle: tauri::AppHandle, url: String) -> Result<(), String> {
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
pub fn is_sq_available() -> bool {
    blox::is_sq_available()
}

/// Read a text file from an absolute path.
///
/// Used by the frontend to read file contents from paths provided by
/// Tauri's native drag-and-drop events (which give file paths, not
/// File objects like browser drag events).
#[tauri::command(rename_all = "camelCase")]
pub fn read_text_file(file_path: String) -> Result<String, String> {
    const MAX_SIZE: u64 = 1_048_576; // 1 MB

    let path = Path::new(&file_path);
    if !path.exists() {
        return Err(format!("File does not exist: {file_path}"));
    }
    if !path.is_file() {
        return Err(format!("Not a file: {file_path}"));
    }
    let metadata =
        std::fs::metadata(path).map_err(|e| format!("Failed to read file metadata: {e}"))?;
    if metadata.len() > MAX_SIZE {
        return Err("File too large (>1 MB)".to_string());
    }
    std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {e}"))
}

/// Return the absolute path for the shared preferences store file.
#[tauri::command]
pub fn preferences_store_path() -> Result<String, String> {
    crate::preferences_store_path_buf()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "Cannot determine preferences store path".to_string())
}

/// Check whether the user is authenticated with Blox.
///
/// Returns Ok(()) if authenticated, or an error string if not.
/// The frontend can call this before starting a workspace to give
/// an immediate, actionable error instead of a mysterious hang.
#[tauri::command]
pub async fn check_blox_auth() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(blox::check_auth)
        .await
        .map_err(|e| format!("Failed to run blox auth check: {e}"))?
        .map_err(|e| e.to_string())
}

// =============================================================================
// Open In commands
// =============================================================================

/// Get available opener applications.
///
/// On macOS, uses mdfind to detect which apps are installed.
/// On other platforms, returns an empty list.
#[tauri::command]
pub async fn get_available_openers() -> Result<Vec<OpenerApp>, String> {
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
pub async fn open_in_app(path: String, app_id: String) -> Result<(), String> {
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
