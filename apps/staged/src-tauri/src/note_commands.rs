//! Note commands — note CRUD operations for branches and projects.

use crate::store::Store;
use crate::NoteTimelineItem;
use std::sync::{Arc, Mutex};

/// Create a standalone note (no session) for a branch.
#[tauri::command(rename_all = "camelCase")]
pub fn create_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    title: String,
    content: String,
) -> Result<NoteTimelineItem, String> {
    let store = crate::get_store(&store)?;
    let note = crate::store::models::Note::new(&branch_id, &title, &content);
    store.create_note(&note).map_err(|e| e.to_string())?;
    Ok(NoteTimelineItem {
        id: note.id,
        title: note.title,
        content: note.content,
        session_id: None,
        session_status: None,
        completion_reason: None,
        created_at: note.created_at,
        updated_at: note.updated_at,
        completed_at: note.completed_at,
        suggested_next_commit_step: None,
        suggested_next_note_step: None,
    })
}

/// Delete a note and optionally its linked session.
#[tauri::command(rename_all = "camelCase")]
pub fn delete_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;
    // Look up the note first so we can find its session
    let note = store
        .get_note(&note_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Note not found: {note_id}"))?;

    store.delete_note(&note_id).map_err(|e| e.to_string())?;

    if delete_session.unwrap_or(false) {
        if let Some(sid) = note.session_id {
            let _ = store.delete_session(&sid);
        }
    }
    Ok(())
}

// =============================================================================
// Project note commands
// =============================================================================

#[tauri::command]
pub fn create_project_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    title: String,
    content: String,
) -> Result<crate::store::ProjectNote, String> {
    let store = crate::get_store(&store)?;
    let note = crate::store::ProjectNote::new(&project_id, &title, &content);
    store
        .create_project_note(&note)
        .map_err(|e| e.to_string())?;
    Ok(note)
}

#[tauri::command]
pub fn list_project_notes(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
) -> Result<Vec<crate::store::ProjectNote>, String> {
    crate::get_store(&store)?
        .list_project_notes_with_status(&project_id)
        .map_err(|e| e.to_string())
}

/// Get a single project note by its linked session ID, with resolved session status.
#[tauri::command(rename_all = "camelCase")]
pub fn get_project_note_by_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    session_id: String,
) -> Result<Option<crate::store::ProjectNote>, String> {
    crate::get_store(&store)?
        .get_project_note_by_session_with_status(&session_id)
        .map_err(|e| e.to_string())
}

/// Delete a project note and its linked session (if any).
#[tauri::command(rename_all = "camelCase")]
pub fn delete_project_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;
    let session_id = store
        .delete_project_note(&note_id)
        .map_err(|e| e.to_string())?;

    if let Some(sid) = session_id {
        let _ = store.delete_session(&sid);
    }
    Ok(())
}
