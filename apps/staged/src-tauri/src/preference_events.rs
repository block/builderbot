//! Forward the store plugin's preference notifications to browser clients.
//! Desktop windows already receive these events directly from the plugin.

use std::path::PathBuf;
use tauri::Listener;
use tokio::sync::broadcast;

use crate::web_server::WebEvent;

pub fn forward_to_web<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    path: PathBuf,
    tx: broadcast::Sender<WebEvent>,
) {
    app.listen("store://change", move |event| {
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
            return;
        };
        if payload.get("path").and_then(|value| value.as_str()) != path.to_str() {
            return;
        }
        // Preserve the plugin payload, including `exists` for deleted keys.
        // Do not emit back to Tauri: that would recursively trigger this listener.
        let _ = tx.send(WebEvent {
            event_name: "store://change".to_string(),
            payload: serde_json::json!({ "event": "store://change", "payload": payload })
                .to_string(),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use tauri::test::{mock_builder, mock_context, noop_assets};
    use tauri_plugin_store::StoreBuilder;

    #[test]
    fn forwards_real_preference_writes_and_deletions_but_not_other_stores() {
        let app = mock_builder()
            .plugin(tauri_plugin_store::Builder::new().build())
            .build(mock_context(noop_assets()))
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let store = StoreBuilder::new(&app, &path)
            .disable_auto_save()
            .build()
            .unwrap();
        let other = StoreBuilder::new(&app, dir.path().join("other.json"))
            .disable_auto_save()
            .build()
            .unwrap();
        let (tx, mut rx) = broadcast::channel(8);
        forward_to_web(app.handle(), path.clone(), tx);

        other.set("project-status-options", json!([]));
        assert!(rx.try_recv().is_err());

        for value in [Some(json!([{ "id": "custom" }])), Some(json!([])), None] {
            match &value {
                Some(value) => store.set("project-status-options", value.clone()),
                None => {
                    store.delete("project-status-options");
                }
            }
            let event = rx.try_recv().unwrap();
            assert_eq!(event.event_name, "store://change");
            let message: Value = serde_json::from_str(&event.payload).unwrap();
            assert_eq!(message["event"], "store://change");
            assert_eq!(message["payload"]["path"], json!(path));
            assert_eq!(message["payload"]["key"], "project-status-options");
            assert_eq!(message["payload"]["exists"], value.is_some());
            assert_eq!(message["payload"]["value"], json!(value));
            assert!(rx.try_recv().is_err());
        }
    }
}
