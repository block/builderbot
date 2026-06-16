//! Note commands — note CRUD operations for branches and projects.

use crate::store::models::Note;
use crate::store::Store;
use crate::NoteTimelineItem;
use std::sync::{Arc, Mutex};

/// Convert a stored [`Note`] into a [`NoteTimelineItem`] with its session
/// status resolved. Mirrors the mapping used when building branch timelines so
/// note lookups outside the timeline (e.g. the project-note child view) stay
/// consistent.
fn note_to_timeline_item(store: &Store, note: Note) -> NoteTimelineItem {
    let resolved = store.resolve_session_status(note.session_id.as_deref());
    NoteTimelineItem {
        id: note.id,
        title: note.title,
        content: note.content,
        session_id: resolved.session_id,
        session_status: resolved.status,
        completion_reason: resolved.completion_reason,
        created_at: note.created_at,
        updated_at: note.updated_at,
        completed_at: note.completed_at,
        suggested_next_commit_step: note.suggested_next_commit_step,
        suggested_next_note_step: note.suggested_next_note_step,
    }
}

/// Create a standalone note (no session) for a branch.
#[tauri::command(rename_all = "camelCase")]
pub fn create_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    title: String,
    content: String,
) -> Result<NoteTimelineItem, String> {
    let store = crate::get_store(&store)?;
    let mut note = crate::store::models::Note::new(&branch_id, &title, &content);
    store
        .create_note_with_unique_title(&mut note)
        .map_err(|e| e.to_string())?;
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

/// Get a single branch note by its linked session ID.
///
/// Parallels [`get_project_note_by_session`]; lets the frontend resolve the
/// note a review comment produced (for `NoteModal`) without refetching the
/// whole branch timeline.
#[tauri::command(rename_all = "camelCase")]
pub fn get_branch_note_by_session(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    session_id: String,
) -> Result<Option<crate::store::Note>, String> {
    crate::get_store(&store)?
        .get_note_by_session(&session_id)
        .map_err(|e| e.to_string())
}

/// Fetch a single note by id (no branch filter), with resolved session status.
///
/// Used by the project-note view to open a `#note:<id>` reference — including
/// child notes that live on other repo branches and are therefore absent from
/// any single branch timeline.
#[tauri::command(rename_all = "camelCase")]
pub fn get_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
) -> Result<Option<NoteTimelineItem>, String> {
    let store = crate::get_store(&store)?;
    let note = store.get_note(&note_id).map_err(|e| e.to_string())?;
    Ok(note.map(|n| note_to_timeline_item(&store, n)))
}

/// List the child notes aggregated under a given project note, with resolved
/// session status. Children are hidden from branch timelines, so this is the
/// dedicated path the parent project-note view uses to resolve `#note:<id>`
/// hashtag references to their titles.
#[tauri::command(rename_all = "camelCase")]
pub fn list_child_notes(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    parent_project_note_id: String,
) -> Result<Vec<NoteTimelineItem>, String> {
    let store = crate::get_store(&store)?;
    let notes = store
        .list_child_notes(&parent_project_note_id)
        .map_err(|e| e.to_string())?;
    Ok(notes
        .into_iter()
        .map(|n| note_to_timeline_item(&store, n))
        .collect())
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
