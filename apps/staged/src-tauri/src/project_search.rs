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

#[tauri::command]
pub fn get_project_search_index(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<ProjectSearchItem>, String> {
    crate::get_store(&store)?
        .list_project_search_items()
        .map_err(|e| e.to_string())
}
