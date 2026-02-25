use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::menu::*;
use tauri::Manager;
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;

/// Holds the sidecar child process so we can kill it on quit.
struct Sidecar(Mutex<Option<CommandChild>>);

/// Tracks whether a window was just destroyed, so we can distinguish
/// "last window closed" from "user quit" in ExitRequested.
static WINDOW_CLOSED: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn get_platform() -> String {
    std::env::consts::OS.to_string()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_decorum::init())
        .manage(Sidecar(Mutex::new(None)))
        .setup(|app| {
            let win = app.get_webview_window("main").unwrap();

            // macOS traffic light positioning
            #[cfg(target_os = "macos")]
            {
                use tauri_plugin_decorum::WebviewWindowExt;
                win.set_traffic_lights_inset(15.0, 18.0).ok();
            }

            // Build application menu
            let app_handle = app.handle();
            let menu = build_menu(app_handle)?;
            app.set_menu(menu)?;

            // Spawn Go server sidecar
            let port = std::env::var("PENPAL_PORT").unwrap_or_else(|_| "8080".to_string());
            let go_port = std::env::var("PENPAL_GO_PORT").unwrap_or_else(|_| "8081".to_string());
            let sidecar = app.shell().sidecar("penpal-server")
                .expect("failed to locate penpal-server sidecar");
            let (_rx, child) = sidecar
                .args(["-port", &port, "-go-port", &go_port])
                .spawn()
                .expect("failed to spawn penpal-server sidecar");

            // Store the child so we can kill it on quit
            *app.state::<Sidecar>().0.lock().unwrap() = Some(child);

            // Wait for server to be ready
            let addr = format!("127.0.0.1:{}", port);
            for _ in 0..50 {
                if std::net::TcpStream::connect(&addr).is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            // Handle custom menu events — dispatch to the focused window
            app.on_menu_event(move |app_handle, event| {
                if event.id().as_ref() == "quit" {
                    app_handle.exit(0);
                    return;
                }

                // Handle new_window — doesn't need an existing window
                if event.id().as_ref() == "new_window" {
                    let label = format!("win-{}", std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis());
                    let _ = tauri::WebviewWindowBuilder::new(
                        app_handle,
                        &label,
                        tauri::WebviewUrl::App("/".into()),
                    )
                    .title("Penpal")
                    .inner_size(1200.0, 800.0)
                    .build();
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
                    _ => {}
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_platform])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::Destroyed,
                ..
            } = &event
            {
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
                    let _ = tauri::WebviewWindowBuilder::new(
                        app_handle,
                        "main",
                        tauri::WebviewUrl::App("/".into()),
                    )
                    .title("Penpal")
                    .inner_size(1200.0, 800.0)
                    .build();
                }
            }
            if let tauri::RunEvent::Exit = event {
                if let Some(child) = app_handle.state::<Sidecar>().0.lock().unwrap().take() {
                    let _ = child.kill();
                }
            }
        });
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
