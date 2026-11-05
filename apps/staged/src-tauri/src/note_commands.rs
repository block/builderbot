//! Note commands — note CRUD operations for branches and projects.

use crate::note_media::refs::media_ids;
use crate::store::models::Note;
use crate::store::Store;
use crate::NoteTimelineItem;
use std::sync::{Arc, Mutex};

/// Convert a stored [`Note`] into a [`NoteTimelineItem`] with its session
/// status resolved. Mirrors the mapping used when building branch timelines so
/// note lookups outside the timeline (e.g. the project-note child view) stay
/// consistent. Shared with the web-server dispatch so both runtimes resolve
/// session status identically.
pub(crate) fn note_to_timeline_item(store: &Store, note: Note) -> NoteTimelineItem {
    let resolved = store.resolve_session_status(note.session_id.as_deref());
    // Best-effort, like the status above: a lookup miss leaves the chat button
    // in its plain state rather than failing the note.
    let (last_chat_message_at, chat_read_at) = note
        .session_id
        .as_deref()
        .and_then(|session_id| store.session_chat_activity(session_id).ok())
        .unwrap_or_default();
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
        subtype: note.subtype,
        last_chat_message_at,
        chat_read_at,
    }
}

/// Build the timeline item for a note that has no session by construction, so
/// there is no session status to resolve. Shared with the web-server dispatch.
pub(crate) fn standalone_note_to_timeline_item(note: Note) -> NoteTimelineItem {
    NoteTimelineItem {
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
        subtype: note.subtype,
        last_chat_message_at: None,
        chat_read_at: None,
    }
}

/// Create a standalone note (no session) for a branch.
///
/// `subtype` is `"written"` when the user authored the note in the editor
/// dialog; the drag-drop and save-action-output paths leave it unset.
#[tauri::command(rename_all = "camelCase")]
pub fn create_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    title: String,
    content: String,
    subtype: Option<String>,
) -> Result<NoteTimelineItem, String> {
    let store = crate::get_store(&store)?;
    let mut note = crate::store::models::Note::new_standalone(&branch_id, &title, &content);
    note.subtype = subtype;
    store
        .create_note_with_unique_title(&mut note)
        .map_err(|e| e.to_string())?;
    Ok(standalone_note_to_timeline_item(note))
}

/// Save an edit to a user-authored ("written") note.
///
/// Rejects notes an agent session produced — their content is owned by that
/// session and would be overwritten on its next turn.
#[tauri::command(rename_all = "camelCase")]
pub fn update_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
    title: String,
    content: String,
) -> Result<NoteTimelineItem, String> {
    let store = crate::get_store(&store)?;
    update_note_impl(&store, &note_id, &title, &content)
}

/// The body of [`update_note`], shared with the web dispatcher's arm.
///
/// A written note can carry media (a copied note brings its own), and the
/// editor can delete it. Nothing else would ever collect a row only this note
/// used, so once the save has committed, every id the old content embedded
/// and the new content doesn't goes to `cleanup_unreferenced`, which keeps
/// anything another note, a chat message or the branch timeline still uses. A
/// rejected save returns before any of that runs.
pub(crate) fn update_note_impl(
    store: &Store,
    note_id: &str,
    title: &str,
    content: &str,
) -> Result<NoteTimelineItem, String> {
    let (note, previous_content) = store
        .update_written_note(note_id, title, content)
        .map_err(|e| e.to_string())?;
    let removed = media_ids(&previous_content)
        .difference(&media_ids(&note.content))
        .cloned()
        .collect();
    crate::note_media::cleanup_unreferenced(store, removed);
    Ok(standalone_note_to_timeline_item(note))
}

/// Delete a note and optionally its linked session.
#[tauri::command(rename_all = "camelCase")]
pub fn delete_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    note_id: String,
    delete_session: Option<bool>,
) -> Result<(), String> {
    delete_note_impl(
        &crate::get_store(&store)?,
        &app_handle,
        &note_id,
        delete_session.unwrap_or(false),
    )
}

/// The body of [`delete_note`], shared with the web dispatcher's arm.
///
/// A note's session may be the branch's busy marker or a barrier in its
/// queue, so the branch is kicked after the delete; the kick is what starts
/// whatever was queued behind it without a client's follow-up.
pub(crate) fn delete_note_impl<R: tauri::Runtime>(
    store: &Arc<Store>,
    app_handle: &tauri::AppHandle<R>,
    note_id: &str,
    delete_session: bool,
) -> Result<(), String> {
    // Look up the note first so we can find its session
    let note = store
        .get_note(note_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Note not found: {note_id}"))?;

    store.delete_note(note_id).map_err(|e| e.to_string())?;
    crate::note_media::cleanup_unreferenced(store, media_ids(&note.content));

    // The session keeps its `branch_id` after the note is gone, so a `queued`
    // row left behind would stay in the branch's queue with nothing to say
    // what it is. Kept sessions are cancelled if queued (see
    // `cancel_queued_session_of_deleted_artifact`); a failed delete is
    // reported, not swallowed — but the branch is kicked either way, since
    // the note itself is gone.
    let session_outcome = match note.session_id.as_deref() {
        Some(sid) if delete_session => store
            .delete_session(sid)
            .map_err(|e| format!("Failed to delete session {sid} of note {note_id}: {e}")),
        Some(sid) => crate::session_commands::cancel_queued_session_of_deleted_artifact(
            store, app_handle, sid,
        ),
        None => Ok(()),
    };
    crate::session_queue::kick(
        app_handle,
        &note.branch_id,
        crate::session_queue::KickSource::ArtifactDeleted,
    );
    session_outcome
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

/// Fetch a single project note by id (no project filter), with resolved
/// session status.
///
/// The `#project-note:<id>` counterpart of [`get_note`]: branch history renders
/// a child note as "of project note #project-note:<id>", so an agent can quote
/// that reference into a note body whose reader is scoped to a different
/// project. This lets such a reference open instead of silently no-opping.
#[tauri::command(rename_all = "camelCase")]
pub fn get_project_note(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_note_id: String,
) -> Result<Option<crate::store::ProjectNote>, String> {
    crate::get_store(&store)?
        .get_project_note_with_status(&project_note_id)
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

/// Delete a project note, its child notes, and every session they owned.
///
/// The store cascade removes the child-note rows, but their sessions may still
/// be running — the note-delete trigger skips those, and a child session that
/// outlived its note would keep running with nowhere to write its result. So
/// each orphaned session (the project note's own plus every child's) is
/// cancelled in the registry and then deleted.
#[tauri::command(rename_all = "camelCase")]
pub fn delete_project_note(
    registry: tauri::State<'_, Arc<crate::session_runner::SessionRegistry>>,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;
    let media_ids = crate::note_media::project_note_media_ids(&store, &note_id)?;
    let orphaned = store
        .delete_project_note(&note_id)
        .map_err(|e| e.to_string())?;
    crate::note_media::cleanup_unreferenced(&store, media_ids);

    for sid in orphaned.all_session_ids() {
        registry.cancel(&sid);
        let _ = store.delete_session(&sid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note_media::files::store_media_file;
    use crate::note_media::tests::{Fixture, PNG};
    use crate::store::images::image_file_path;
    use crate::store::{Image, MessageRole, ProjectNote, NOTE_MEDIA_SESSION_ID};

    /// Media a written note owns, the way a copy lands it in its new scope.
    fn note_media(f: &Fixture, filename: &str) -> Image {
        stored(f, Some(NOTE_MEDIA_SESSION_ID), filename)
    }

    fn stored(f: &Fixture, session_id: Option<&str>, filename: &str) -> Image {
        store_media_file(
            &f.store,
            Some(&f.branch.id),
            &f.project.id,
            session_id,
            filename,
            PNG,
            true,
        )
        .unwrap()
    }

    fn embed(image: &Image) -> String {
        format!("![{}](staged-media://{}.png)", image.filename, image.id)
    }

    fn written_note(f: &Fixture, content: &str) -> Note {
        let mut note = Note::new_standalone(&f.branch.id, "Shots", content)
            .with_subtype(Note::SUBTYPE_WRITTEN);
        f.store.create_note_with_unique_title(&mut note).unwrap();
        note
    }

    fn assert_kept(f: &Fixture, image: &Image) {
        assert!(
            f.store.get_image(&image.id).unwrap().is_some(),
            "{} lost its row",
            image.filename
        );
        let path = image_file_path(&image.project_id, &image.id, &image.filename).unwrap();
        assert!(path.exists(), "{} lost its file", image.filename);
    }

    fn assert_deleted(f: &Fixture, image: &Image) {
        assert!(
            f.store.get_image(&image.id).unwrap().is_none(),
            "{} kept its row",
            image.filename
        );
        let path = image_file_path(&image.project_id, &image.id, &image.filename).unwrap();
        assert!(!path.exists(), "{} kept its file", image.filename);
    }

    #[test]
    fn saving_a_written_note_deletes_the_media_the_edit_removed() {
        let f = Fixture::new();
        let dropped = note_media(&f, "dropped.png");
        let kept = note_media(&f, "kept.png");
        let note = written_note(&f, &format!("{}\n\n{}", embed(&dropped), embed(&kept)));

        let saved = update_note_impl(&f.store, &note.id, "Shots", &embed(&kept)).unwrap();

        assert_eq!(saved.content, embed(&kept));
        assert_deleted(&f, &dropped);
        assert_kept(&f, &kept);
    }

    #[test]
    fn saving_a_written_note_keeps_removed_media_still_used_elsewhere() {
        let f = Fixture::new();
        let in_other_note = note_media(&f, "other-note.png");
        let other = Note::new_standalone(&f.branch.id, "Other", &embed(&in_other_note));
        f.store.create_note(&other).unwrap();
        let in_project_note = note_media(&f, "project-note.png");
        let plan = ProjectNote::new(&f.project.id, "Plan", &embed(&in_project_note));
        f.store.create_project_note(&plan).unwrap();
        let in_chat = stored(&f, Some(&f.session.id), "chat.png");
        f.store
            .add_session_message_with_images(
                &f.session.id,
                MessageRole::User,
                "Screenshot",
                std::slice::from_ref(&in_chat.id),
            )
            .unwrap();
        // Session-less: a branch-timeline attachment the note quoted.
        let on_timeline = stored(&f, None, "timeline.png");
        // The control: proves this save did run the clean-up.
        let only_here = note_media(&f, "only-here.png");
        let shared = [&in_other_note, &in_project_note, &in_chat, &on_timeline];
        let body = [shared.map(embed).join("\n\n"), embed(&only_here)].join("\n\n");
        let note = written_note(&f, &body);

        update_note_impl(&f.store, &note.id, "Shots", "Nothing embedded now").unwrap();

        for image in shared {
            assert_kept(&f, image);
        }
        assert_deleted(&f, &only_here);
    }

    #[test]
    fn an_unchanged_save_deletes_no_media() {
        let f = Fixture::new();
        let image = note_media(&f, "shot.png");
        let content = format!("Before\n\n{}\n\nAfter", embed(&image));
        let note = written_note(&f, &content);

        // The same body (a retitle), then the same media under new prose and
        // alt text: the set of embedded ids is unchanged either way.
        update_note_impl(&f.store, &note.id, "Renamed", &content).unwrap();
        assert_kept(&f, &image);
        let reworded = format!("Reworded\n\n![New alt](staged-media://{}.png)", image.id);
        update_note_impl(&f.store, &note.id, "Renamed", &reworded).unwrap();
        assert_kept(&f, &image);
    }
}
