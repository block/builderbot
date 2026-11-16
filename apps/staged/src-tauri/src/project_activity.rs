//! Project activity: when each project last saw work, for sorting projects
//! by "Last updated".
//!
//! Read-only and derived ([`Store::list_project_activity`]): the latest note,
//! project-note, review or commit timestamp per project. Sorting stays in the
//! frontend, which already holds each project's creation time and uses it as
//! the floor. The frontend loads this lazily, only while a sort uses it, and
//! refetches on the store change feed.

use std::sync::{Arc, Mutex};

use crate::store::{ProjectActivity, Store};

/// A read over the timestamps of every note, project note, review and commit,
/// run on the blocking pool so a wait on the store never holds the window.
///
/// As for every command in `OFFLOADED_COMMANDS`, only the store handle
/// lookup may run inline: an `async fn` command runs on a runtime worker,
/// where a store lock wait would stall timers and event sockets instead.
#[tauri::command]
pub async fn get_project_activity(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<ProjectActivity>, String> {
    let store = crate::get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store.list_project_activity().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("get_project_activity task failed: {e}"))?
}
