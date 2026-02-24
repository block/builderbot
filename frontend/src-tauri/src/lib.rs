use tauri::menu::*;
use tauri::Manager;
use tauri_plugin_shell::ShellExt;

#[tauri::command]
fn get_platform() -> String {
    std::env::consts::OS.to_string()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_decorum::init())
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
            let sidecar = app.shell().sidecar("penpal-server")
                .expect("failed to locate penpal-server sidecar");
            let (_rx, _child) = sidecar
                .args(["-port", "8080"])
                .spawn()
                .expect("failed to spawn penpal-server sidecar");

            // Wait for server to be ready
            for _ in 0..50 {
                if std::net::TcpStream::connect("127.0.0.1:8080").is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            // Handle custom menu events
            let win_clone = win.clone();
            app.on_menu_event(move |_app, event| {
                match event.id().as_ref() {
                    "reload" => {
                        let _ = win_clone.eval("window.location.reload()");
                    }
                    "devtools" => {
                        if win_clone.is_devtools_open() {
                            win_clone.close_devtools();
                        } else {
                            win_clone.open_devtools();
                        }
                    }
                    _ => {}
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_platform])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
                &PredefinedMenuItem::quit(app, None)?,
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
        ],
    )?;
    menu.append(&edit_menu)?;

    // View menu
    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[
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
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    menu.append(&window_menu)?;

    Ok(menu)
}
