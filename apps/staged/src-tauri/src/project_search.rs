//! Project search: the title index the landing grid and the sidebar match
//! their query against.
//!
//! Matching stays in the frontend so every keystroke is synchronous; this
//! only hands it what it can't already see. Branch names, PR numbers and repo
//! labels are in the frontend's project and branch lists, so the index holds
//! the rest: note, project-note and review titles
//! ([`Store::list_project_search_items`]). The frontend loads it lazily and
//! refetches on the store change feed.

use std::sync::{Arc, Mutex};

use crate::store::{ProjectSearchItem, Store};

/// A read over every note, project-note and review title, run on the
/// blocking pool: as a synchronous command it held the main thread, and so
/// the window, for the whole read and for any wait on the store before it.
///
/// As for every command in `OFFLOADED_COMMANDS`, only the store handle
/// lookup may run inline: an `async fn` command runs on a runtime worker,
/// where a store lock wait would stall timers and event sockets instead.
#[tauri::command]
pub async fn get_project_search_index(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<ProjectSearchItem>, String> {
    let store = crate::get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store.list_project_search_items().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("get_project_search_index task failed: {e}"))?
}
