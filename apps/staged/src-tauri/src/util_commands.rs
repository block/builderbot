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
    icon: Option<String>,
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
    ("ghostty", "com.mitchellh.ghostty"),
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
    // IDEs
    ("xcode", "com.apple.dt.Xcode"),
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
    let path = Path::new(&file_path);
    if !path.exists() {
        return Err(format!("File does not exist: {file_path}"));
    }
    if !path.is_file() {
        return Err(format!("Not a file: {file_path}"));
    }
    let metadata = path
        .metadata()
        .map_err(|e| format!("Failed to read file metadata: {e}"))?;
    if metadata.len() > 307_200 {
        return Err("File too large (>300 KB)".to_string());
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
/// On macOS, uses mdfind to detect which apps are installed, then extracts
/// their icons in parallel using threads. On other platforms, returns an
/// empty list.
#[tauri::command]
pub async fn get_available_openers() -> Result<Vec<OpenerApp>, String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        use std::thread;

        // Find which apps are installed by running mdfind in parallel.
        let mdfind_handles: Vec<_> = KNOWN_OPENERS
            .iter()
            .map(|(id, bundle_id)| {
                let id = *id;
                let bundle_id = *bundle_id;
                thread::spawn(move || {
                    let output = Command::new("mdfind")
                        .arg(format!("kMDItemCFBundleIdentifier == '{bundle_id}'"))
                        .output()
                        .ok()?;
                    if !output.status.success() {
                        return None;
                    }
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let first_line = stdout.trim().lines().next().unwrap_or("").to_string();
                    if first_line.is_empty() {
                        None
                    } else {
                        Some((id, first_line))
                    }
                })
            })
            .collect();

        let mut installed: Vec<(&str, String)> = Vec::new();
        for handle in mdfind_handles {
            if let Ok(Some((id, path))) = handle.join() {
                installed.push((id, path));
            }
        }

        // Extract icons in parallel using threads.
        let handles: Vec<_> = installed
            .into_iter()
            .map(|(id, app_path)| {
                let id = id.to_string();
                thread::spawn(move || {
                    let icon = extract_app_icon(&app_path);
                    OpenerApp {
                        name: prettify_app_name(&id),
                        id,
                        icon,
                    }
                })
            })
            .collect();

        let mut available = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.join() {
                Ok(app) => available.push(app),
                Err(_) => {} // Thread panicked — skip this app
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

/// Extract an app icon as a base64-encoded PNG data URI.
///
/// Reads the icon filename from Info.plist, resolves the .icns file,
/// converts to a 32×32 PNG via `sips`, and base64-encodes the result.
/// Returns `None` if any step fails.
#[cfg(target_os = "macos")]
fn extract_app_icon(app_path: &str) -> Option<String> {
    use std::process::Command;

    // 1. Read CFBundleIconFile from Info.plist
    let output = Command::new("defaults")
        .arg("read")
        .arg(format!("{app_path}/Contents/Info"))
        .arg("CFBundleIconFile")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let mut icon_name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if icon_name.is_empty() {
        return None;
    }

    // 2. Append .icns if missing
    if !icon_name.ends_with(".icns") {
        icon_name.push_str(".icns");
    }

    let icns_path = format!("{app_path}/Contents/Resources/{icon_name}");
    if !Path::new(&icns_path).exists() {
        return None;
    }

    // 3. Convert to 32×32 PNG via sips into a temp file
    let tmp_file = tempfile::Builder::new()
        .prefix("staged-icon-")
        .suffix(".png")
        .tempfile()
        .ok()?;
    let tmp_png_str = tmp_file.path().to_string_lossy().to_string();

    let sips = Command::new("sips")
        .args([
            "-s",
            "format",
            "png",
            "-z",
            "32",
            "32",
            &icns_path,
            "--out",
            &tmp_png_str,
        ])
        .output()
        .ok()?;

    if !sips.status.success() {
        return None;
    }

    // 4. Read and base64-encode the PNG
    let png_bytes = std::fs::read(tmp_file.path()).ok()?;

    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Some(format!("data:image/png;base64,{encoded}"))
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
        "xcode" => "Xcode",
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
