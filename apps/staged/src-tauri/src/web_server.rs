//! Web server module — stubbed out.
//!
//! The full web server implementation (Axum HTTPS/WebSocket server for
//! browser-based access) is intentionally excluded from this merge.
//! Only the public API surface used by the rest of the crate is preserved
//! here so that `emit_to_all` broadcasting and the startup plumbing compile.
//!
//! TODO(web): restore full web server implementation from the `mobile-web` branch.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

/// A serialized event for WebSocket broadcast.
#[derive(Clone, Debug)]
pub struct WebEvent {
    pub event_name: String,
    pub payload: String,
}

/// State shared between the Axum web server and the Tauri app.
#[derive(Clone)]
pub struct WebAppState {
    pub app_handle: tauri::AppHandle,
    pub event_tx: broadcast::Sender<WebEvent>,
    pub auth_token: String,
    pub sessions: Arc<Mutex<HashSet<String>>>,
}

/// Emit an event both to Tauri (for the desktop window) and to the web
/// broadcast channel (for browser clients over WebSocket).
///
/// When no web server is running the broadcast send is a no-op.
pub fn emit_to_all<S: serde::Serialize + Clone>(
    app_handle: &tauri::AppHandle,
    event_name: &str,
    payload: S,
) {
    use tauri::{Emitter, Manager};
    let _ = app_handle.emit(event_name, payload.clone());
    if let Some(tx) = app_handle.try_state::<broadcast::Sender<WebEvent>>() {
        if let Ok(json) = serde_json::to_string(&serde_json::json!({
            "event": event_name,
            "payload": payload,
        })) {
            let _ = tx.send(WebEvent {
                event_name: event_name.to_string(),
                payload: json,
            });
        }
    }
}

/// Generate a random 256-bit hex-encoded authentication token.
pub fn generate_token() -> String {
    use rand::Rng;
    let bytes: [u8; 32] = rand::rng().random();
    hex::encode(bytes)
}

/// Start the web server. Intentionally stubbed — logs a warning and returns.
///
/// TODO(web): restore full web server implementation from the `mobile-web` branch.
pub fn start(_state: WebAppState) {
    log::warn!("Web server requested but this build has the web server stubbed out");
}
