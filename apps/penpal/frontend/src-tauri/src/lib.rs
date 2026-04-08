#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::menu::*;
use tauri::Manager;
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;

/// Holds the sidecar child process so we can kill it on quit.
struct Sidecar(Mutex<Option<CommandChild>>);

/// Holds the server port so the run callback can reach the Go server.
struct ServerPort(Mutex<String>);

/// Tracks whether a window was just destroyed, so we can distinguish
/// "last window closed" from "user quit" in ExitRequested.
#[cfg(target_os = "macos")]
static WINDOW_CLOSED: AtomicBool = AtomicBool::new(false);

// E-PENPAL-SESSION-FILE, E-PENPAL-GEO-TRACK: session persistence types.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct WindowGeometry {
    label: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    #[serde(default, rename = "activePath")]
    active_path: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct SessionState {
    version: u32,
    windows: Vec<WindowGeometry>,
}

/// In-memory geometry registry, updated on move/resize events.
struct GeoRegistry(Mutex<HashMap<String, WindowGeometry>>);

fn session_file_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    std::path::Path::new(&home).join(".config/penpal/window-state.json")
}

fn load_session() -> Option<SessionState> {
    let path = session_file_path();
    let data = std::fs::read_to_string(&path).ok()?;
    let session: SessionState = serde_json::from_str(&data).ok()?;
    if session.version != 1 { return None; }
    if session.windows.is_empty() { return None; }
    Some(session)
}

fn save_session(windows: &[WindowGeometry]) {
    let path = session_file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let session = SessionState { version: 1, windows: windows.to_vec() };
    if let Ok(data) = serde_json::to_string_pretty(&session) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &data).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

// E-PENPAL-PROGRAMMATIC-WINDOWS: shared helper for all window creation.
fn create_penpal_window(
    app: &tauri::AppHandle,
    label: &str,
    url: &str,
    x: Option<i32>,
    y: Option<i32>,
    width: f64,
    height: f64,
) -> Option<tauri::WebviewWindow> {
    let mut builder = tauri::WebviewWindowBuilder::new(
        app,
        label,
        tauri::WebviewUrl::App(url.into()),
    )
    .title("Penpal")
    .inner_size(width, height)
    .min_inner_size(800.0, 600.0);

    #[cfg(target_os = "macos")]
    {
        builder = builder
            .hidden_title(true)
            .title_bar_style(tauri::TitleBarStyle::Overlay);
    }

    if let (Some(x), Some(y)) = (x, y) {
        builder = builder.position(x as f64, y as f64);
    }

    let win = builder.build().ok()?;

    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_decorum::WebviewWindowExt;
        win.set_traffic_lights_inset(15.0, 18.0).ok();
    }

    // Register initial geometry
    if let Some(geo_reg) = app.try_state::<GeoRegistry>() {
        if let Ok(mut map) = geo_reg.0.lock() {
            let pos = win.outer_position().unwrap_or(tauri::PhysicalPosition { x: 0, y: 0 });
            let size = win.outer_size().unwrap_or(tauri::PhysicalSize { width: width as u32, height: height as u32 });
            map.insert(label.to_string(), WindowGeometry {
                label: label.to_string(),
                x: pos.x,
                y: pos.y,
                width: size.width,
                height: size.height,
                active_path: url.to_string(),
            });
        }
    }

    Some(win)
}

#[tauri::command]
fn get_platform() -> String {
    std::env::consts::OS.to_string()
}

#[tauri::command]
fn update_active_path(window: tauri::Window, path: String, geo: tauri::State<'_, GeoRegistry>) {
    if let Ok(mut map) = geo.0.lock() {
        if let Some(entry) = map.get_mut(window.label()) {
            entry.active_path = path;
        }
    }
}

fn ready_probe_request(addr: &str) -> String {
    format!("GET /api/ready HTTP/1.0\r\nHost: {}\r\n\r\n", addr)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn file_open_request(addr: &str, body: &str) -> String {
    format!(
        "POST /api/open HTTP/1.0\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        addr,
        body.len(),
        body
    )
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_decorum::init())
        .manage(Sidecar(Mutex::new(None)))
        .manage(ServerPort(Mutex::new(String::new())))
        .manage(GeoRegistry(Mutex::new(HashMap::new())))
        .setup(|app| {
            // E-PENPAL-PROGRAMMATIC-WINDOWS: create windows from saved session or default.
            let session = load_session();
            if let Some(ref s) = session {
                for wg in &s.windows {
                    let url = if wg.active_path.is_empty() { "/" } else { &wg.active_path };
                    create_penpal_window(
                        app.handle(),
                        &wg.label,
                        url,
                        Some(wg.x),
                        Some(wg.y),
                        wg.width as f64,
                        wg.height as f64,
                    );
                }
            }
            if app.webview_windows().is_empty() {
                create_penpal_window(app.handle(), "main", "/", None, None, 1200.0, 800.0);
            }

            // Build application menu
            let app_handle = app.handle();
            let menu = build_menu(app_handle)?;
            app.set_menu(menu)?;

            // E-PENPAL-TAURI: spawn Go server sidecar, poll /api/ready, manage lifecycle.
            let port = std::env::var("PENPAL_PORT").unwrap_or_else(|_| "8080".to_string());
            let sidecar = app.shell().sidecar("penpal-server")
                .expect("failed to locate penpal-server sidecar");
            let (_rx, child) = sidecar
                .args(["-port", &port])
                .spawn()
                .expect("failed to spawn penpal-server sidecar");

            // Store the child so we can kill it on quit
            *app.state::<Sidecar>().0.lock().unwrap() = Some(child);

            // Store the port for the run callback (file open events)
            *app.state::<ServerPort>().0.lock().unwrap() = port.clone();

            // Wait for server to be fully ready (projects discovered and files scanned).
            // The /api/ready endpoint blocks until initialization is complete.
            let addr = format!("127.0.0.1:{}", port);
            for _ in 0..300 {
                if let Ok(mut stream) = std::net::TcpStream::connect(&addr) {
                    use std::io::{Read, Write};
                    let req = ready_probe_request(&addr);
                    if stream.write_all(req.as_bytes()).is_ok() {
                        // Set a generous timeout — initialization may take a while
                        stream.set_read_timeout(Some(std::time::Duration::from_secs(30))).ok();
                        let mut buf = [0u8; 256];
                        if let Ok(n) = stream.read(&mut buf) {
                            let resp = String::from_utf8_lossy(&buf[..n]);
                            if resp.contains("200") {
                                break;
                            }
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            // Handle custom menu events — dispatch to the focused window
            app.on_menu_event(move |app_handle, event| {
                if event.id().as_ref() == "quit" {
                    app_handle.exit(0);
                    return;
                }

                // E-PENPAL-NEW-WINDOW: Cmd+N creates a new window.
                if event.id().as_ref() == "new_window" {
                    let label = format!("win-{}", std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis());
                    create_penpal_window(app_handle, &label, "/", None, None, 1200.0, 800.0);
                    return;
                }

                // All other events require a window to dispatch to
                let focused = app_handle.webview_windows().into_values()
                    .find(|w| w.is_focused().unwrap_or(false))
                    .or_else(|| app_handle.webview_windows().into_values().next());
                let Some(win) = focused else { return };

                match event.id().as_ref() {
                    "reload" => {
                        let _ = win.eval("window.location.reload()");
                    }
                    "devtools" => {
                        if win.is_devtools_open() {
                            win.close_devtools();
                        } else {
                            win.open_devtools();
                        }
                    }
                    "close_tab" => {
                        let _ = win.eval("window.dispatchEvent(new CustomEvent('menu-close-tab'))");
                    }
                    "new_tab" => {
                        let _ = win.eval("window.dispatchEvent(new CustomEvent('menu-new-tab'))");
                    }
                    "find" => {
                        let _ = win.eval("window.dispatchEvent(new CustomEvent('menu-find'))");
                    }
                    "prev_tab" => {
                        let _ = win.eval("window.dispatchEvent(new CustomEvent('menu-prev-tab'))");
                    }
                    "next_tab" => {
                        let _ = win.eval("window.dispatchEvent(new CustomEvent('menu-next-tab'))");
                    }
                    "go_back" => {
                        let _ = win.eval("window.dispatchEvent(new CustomEvent('menu-go-back'))");
                    }
                    "go_forward" => {
                        let _ = win.eval("window.dispatchEvent(new CustomEvent('menu-go-forward'))");
                    }
                    "install_tools" => {
                        let _ = win.eval("window.dispatchEvent(new CustomEvent('menu-install-tools'))");
                    }
                    _ => {}
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_platform, update_active_path])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // E-PENPAL-GEO-TRACK: update geometry on move/resize.
            if let tauri::RunEvent::WindowEvent {
                ref label,
                event: ref win_event,
                ..
            } = event
            {
                match win_event {
                    tauri::WindowEvent::Moved(pos) => {
                        if let Ok(mut map) = app_handle.state::<GeoRegistry>().0.lock() {
                            if let Some(entry) = map.get_mut(label) {
                                entry.x = pos.x;
                                entry.y = pos.y;
                            } else if let Some(win) = app_handle.get_webview_window(label) {
                                let size = win.outer_size().unwrap_or(tauri::PhysicalSize { width: 1200, height: 800 });
                                map.insert(label.to_string(), WindowGeometry {
                                    label: label.to_string(),
                                    x: pos.x,
                                    y: pos.y,
                                    width: size.width,
                                    height: size.height,
                                    active_path: String::new(),
                                });
                            }
                        }
                    }
                    tauri::WindowEvent::Resized(size) => {
                        if let Ok(mut map) = app_handle.state::<GeoRegistry>().0.lock() {
                            if let Some(entry) = map.get_mut(label) {
                                entry.width = size.width;
                                entry.height = size.height;
                            } else if let Some(win) = app_handle.get_webview_window(label) {
                                let pos = win.outer_position().unwrap_or(tauri::PhysicalPosition { x: 0, y: 0 });
                                map.insert(label.to_string(), WindowGeometry {
                                    label: label.to_string(),
                                    x: pos.x,
                                    y: pos.y,
                                    width: size.width,
                                    height: size.height,
                                    active_path: String::new(),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            if let tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::Destroyed,
                label,
                ..
            } = &event
            {
                // Remove from geometry registry so closed windows aren't persisted.
                // On non-macOS, the last window close triggers Exit immediately after
                // Destroyed, so save the session while the registry still has this entry.
                if let Ok(mut map) = app_handle.state::<GeoRegistry>().0.lock() {
                    if map.len() == 1 && map.contains_key(label) {
                        let windows: Vec<WindowGeometry> = map.values().cloned().collect();
                        save_session(&windows);
                    }
                    map.remove(label);
                }
                #[cfg(target_os = "macos")]
                if app_handle.webview_windows().is_empty() {
                    WINDOW_CLOSED.store(true, Ordering::SeqCst);
                }
            }
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::ExitRequested { api, .. } = &event {
                // If a window just closed, keep the app alive in the dock.
                // Otherwise it's a real quit (Cmd+Q, dock quit) — let it exit.
                if WINDOW_CLOSED.swap(false, Ordering::SeqCst) {
                    api.prevent_exit();
                }
            }
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if app_handle.webview_windows().is_empty() {
                    create_penpal_window(app_handle, "main", "/", None, None, 1200.0, 800.0);
                }
            }
            // E-PENPAL-FILE-HANDLER-EVENT: handle macOS file open events (Finder "Open With", `open -a`).
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &event {
                // Ensure a window exists to display the file
                if app_handle.webview_windows().is_empty() {
                    create_penpal_window(app_handle, "main", "/", None, None, 1200.0, 800.0);
                }

                // E-PENPAL-FILE-HANDLER-EVENT: dispatch HTTP on a background thread to avoid blocking the main thread.
                let port = app_handle.state::<ServerPort>().0.lock().unwrap().clone();
                if !port.is_empty() {
                    let addr = format!("127.0.0.1:{}", port);
                    let paths: Vec<String> = urls.iter()
                        .filter_map(|url| url.to_file_path().ok())
                        .map(|p| p.to_string_lossy().to_string())
                        .collect();
                    std::thread::spawn(move || {
                        for path_str in paths {
                            let body = serde_json::json!({"path": path_str}).to_string();
                            if let Ok(mut stream) = std::net::TcpStream::connect(&addr) {
                                use std::io::Write;
                                let req = file_open_request(&addr, &body);
                                let _ = stream.write_all(req.as_bytes());
                            }
                        }
                    });
                }
            }
            // E-PENPAL-SESSION-FILE: flush geometry to session file on quit.
            if let tauri::RunEvent::Exit = event {
                if let Ok(map) = app_handle.state::<GeoRegistry>().0.lock() {
                    let windows: Vec<WindowGeometry> = map.values().cloned().collect();
                    if !windows.is_empty() {
                        save_session(&windows);
                    }
                    // When the map is empty, the Destroyed handler already saved
                    // the session with the last window's geometry, so we preserve
                    // that file instead of deleting it.
                }
                if let Some(child) = app_handle.state::<Sidecar>().0.lock().unwrap().take() {
                    let _ = child.kill();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{file_open_request, ready_probe_request};

    #[test]
    fn ready_probe_request_targets_ready_endpoint() {
        // E-PENPAL-TAURI: verifies desktop shell readiness probe targets /api/ready.
        let req = ready_probe_request("127.0.0.1:8080");
        assert!(req.starts_with("GET /api/ready HTTP/1.0\r\n"));
        assert!(req.contains("Host: 127.0.0.1:8080\r\n"));
    }

    #[test]
    fn open_request_targets_api_open_with_json_body() {
        // E-PENPAL-FILE-HANDLER-EVENT: verifies desktop file-open dispatch targets /api/open.
        let body = r#"{"path":"notes.md"}"#;
        let req = file_open_request("127.0.0.1:8080", body);
        assert!(req.starts_with("POST /api/open HTTP/1.0\r\n"));
        assert!(req.contains("Content-Type: application/json\r\n"));
        assert!(req.contains(&format!("Content-Length: {}\r\n", body.len())));
        assert!(req.ends_with(body));
    }
}

fn build_menu(app: &tauri::AppHandle) -> Result<Menu<tauri::Wry>, tauri::Error> {
    let menu = Menu::new(app)?;

    // App menu (macOS)
    #[cfg(target_os = "macos")]
    {
        let app_menu = Submenu::with_items(
            app,
            "Penpal",
            true,
            &[
                &PredefinedMenuItem::about(app, Some("About Penpal"), None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::services(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &MenuItem::with_id(app, "install_tools", "Manage Command Line Tools\u{2026}", true, None::<&str>)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::show_all(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &MenuItem::with_id(app, "quit", "Quit Penpal", true, Some("CmdOrCtrl+Q"))?,
            ],
        )?;
        menu.append(&app_menu)?;
    }

    // Edit menu
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
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "find", "Find...", true, Some("CmdOrCtrl+F"))?,
        ],
    )?;
    menu.append(&edit_menu)?;

    // View menu
    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &MenuItem::with_id(app, "go_back", "Go Back", true, Some("CmdOrCtrl+["))?,
            &MenuItem::with_id(app, "go_forward", "Go Forward", true, Some("CmdOrCtrl+]"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "reload", "Reload", true, Some("CmdOrCtrl+R"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                "devtools",
                "Toggle Developer Tools",
                true,
                Some("CmdOrCtrl+Alt+I"),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;
    menu.append(&view_menu)?;

    // Window menu
    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "new_window", "New Window", true, Some("CmdOrCtrl+N"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "new_tab", "New Tab", true, Some("CmdOrCtrl+T"))?,
            &MenuItem::with_id(app, "close_tab", "Close Tab", true, Some("CmdOrCtrl+W"))?,
            &MenuItem::with_id(app, "prev_tab", "Show Previous Tab", true, Some("CmdOrCtrl+Shift+["))?,
            &MenuItem::with_id(app, "next_tab", "Show Next Tab", true, Some("CmdOrCtrl+Shift+]"))?,
        ],
    )?;
    menu.append(&window_menu)?;

    Ok(menu)
}
