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
pub fn get_tag_state(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<TagState, String> {
    get_store(&store)?
        .get_tag_state()
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn create_tag_group(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    name: String,
    tags: Vec<NewTag>,
) -> Result<TagGroup, String> {
    get_store(&store)?
        .create_tag_group(&name, &tags)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn rename_tag_group(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    group_id: String,
    name: String,
) -> Result<(), String> {
    get_store(&store)?
        .rename_tag_group(&group_id, &name)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_icon_tag_group(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    group_id: Option<String>,
) -> Result<(), String> {
    get_store(&store)?
        .set_icon_tag_group(group_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_tag_group_sidebar(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    group_id: String,
    shown: bool,
) -> Result<(), String> {
    get_store(&store)?
        .set_tag_group_sidebar(&group_id, shown)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn reorder_tag_groups(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    group_ids: Vec<String>,
) -> Result<(), String> {
    get_store(&store)?
        .reorder_tag_groups(&group_ids)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_tag_group(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    group_id: String,
    mode: DeleteTagGroupMode,
) -> Result<(), String> {
    get_store(&store)?
        .delete_tag_group(&group_id, mode)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn create_tag(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    name: String,
    group_id: Option<String>,
    color: Option<TagColor>,
    icon: Option<String>,
    assign_project_id: Option<String>,
) -> Result<Tag, String> {
    get_store(&store)?
        .create_tag(
            &NewTag { name, color, icon },
            group_id.as_deref(),
            assign_project_id.as_deref(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn update_tag(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    tag_id: String,
    name: String,
    color: TagColor,
    icon: Option<String>,
) -> Result<(), String> {
    get_store(&store)?
        .update_tag(&tag_id, &name, color, icon.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn move_tag(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    tag_id: String,
    group_id: Option<String>,
    index: usize,
) -> Result<(), String> {
    get_store(&store)?
        .move_tag(&tag_id, group_id.as_deref(), index)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_tag(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    tag_id: String,
) -> Result<(), String> {
    get_store(&store)?
        .delete_tag(&tag_id)
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_project_group_tag(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    group_id: String,
    tag_id: Option<String>,
) -> Result<(), String> {
    get_store(&store)?
        .set_project_group_tag(&project_id, &group_id, tag_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_project_tag_assigned(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    tag_id: String,
    assigned: bool,
) -> Result<(), String> {
    get_store(&store)?
        .set_project_tag_assigned(&project_id, &tag_id, assigned)
        .map_err(|e| e.to_string())
}
