//! Tauri commands for project tags and tag groups.
//!
//! Each is a thin, synchronous wrapper over its `Store` method
//! (`store/tags.rs`, `store/project_tags.rs`), which validates, enforces the
//! single-choice and dense-position rules, and publishes `tags-changed`.
//! `web_server::dispatch` mirrors every command here for the web transport.

use std::sync::{Arc, Mutex};

use crate::get_store;
use crate::store::{DeleteTagGroupMode, NewTag, Store, Tag, TagColor, TagGroup, TagState};

#[tauri::command(rename_all = "camelCase")]
pub async fn get_tag_state(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<TagState, String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || store.get_tag_state().map_err(|e| e.to_string()))
        .await
        .map_err(|e| format!("get_tag_state task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn create_tag_group(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    name: String,
    tags: Vec<NewTag>,
) -> Result<TagGroup, String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .create_tag_group(&name, &tags)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("create_tag_group task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn rename_tag_group(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    group_id: String,
    name: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .rename_tag_group(&group_id, &name)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("rename_tag_group task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_icon_tag_group(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    group_id: Option<String>,
) -> Result<(), String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .set_icon_tag_group(group_id.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("set_icon_tag_group task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_tag_group_sidebar(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    group_id: String,
    shown: bool,
) -> Result<(), String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .set_tag_group_sidebar(&group_id, shown)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("set_tag_group_sidebar task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn reorder_tag_groups(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    group_ids: Vec<String>,
) -> Result<(), String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .reorder_tag_groups(&group_ids)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("reorder_tag_groups task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_tag_group(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    group_id: String,
    mode: DeleteTagGroupMode,
) -> Result<(), String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .delete_tag_group(&group_id, mode)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("delete_tag_group task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn create_tag(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    name: String,
    group_id: Option<String>,
    color: Option<TagColor>,
    icon: Option<String>,
    assign_project_id: Option<String>,
) -> Result<Tag, String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .create_tag(
                &NewTag { name, color, icon },
                group_id.as_deref(),
                assign_project_id.as_deref(),
            )
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("create_tag task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_tag(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    tag_id: String,
    name: String,
    color: TagColor,
    icon: Option<String>,
) -> Result<(), String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .update_tag(&tag_id, &name, color, icon.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("update_tag task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn move_tag(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    tag_id: String,
    group_id: Option<String>,
    index: usize,
) -> Result<(), String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .move_tag(&tag_id, group_id.as_deref(), index)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("move_tag task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_tag(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    tag_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store.delete_tag(&tag_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("delete_tag task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_project_group_tag(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    group_id: String,
    tag_id: Option<String>,
) -> Result<(), String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .set_project_group_tag(&project_id, &group_id, tag_id.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("set_project_group_tag task failed: {e}"))?
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_project_tag_assigned(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    tag_id: String,
    assigned: bool,
) -> Result<(), String> {
    let store = get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .set_project_tag_assigned(&project_id, &tag_id, assigned)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("set_project_tag_assigned task failed: {e}"))?
}
