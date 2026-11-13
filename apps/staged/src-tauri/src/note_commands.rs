//! Note commands — note CRUD operations for branches and projects.

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
        session_error_message: resolved.error_message,
        rate_limit_resets_at: resolved.rate_limit_resets_at,
        rate_limit_kind: resolved.rate_limit_kind,
        created_at: note.created_at,
        updated_at: note.updated_at,
        completed_at: note.completed_at,
        suggested_next_commit_step: note.suggested_next_commit_step,
        suggested_next_note_step: note.suggested_next_note_step,
        subtype: note.subtype,
        last_chat_message_at,
        chat_read_at,
        read_at: note.read_at,
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
        session_error_message: None,
        rate_limit_resets_at: None,
        rate_limit_kind: None,
        created_at: note.created_at,
        updated_at: note.updated_at,
        completed_at: note.completed_at,
        suggested_next_commit_step: None,
        suggested_next_note_step: None,
        subtype: note.subtype,
        last_chat_message_at: None,
        chat_read_at: None,
        read_at: note.read_at,
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
    create_standalone_note(&store, &branch_id, &title, &content, subtype)
}

/// The body of [`create_note`], shared with the web dispatcher's arm.
///
/// A note the user just wrote starts read; a dropped file or saved action
/// output is new to them, so it starts unread like a session's note.
pub(crate) fn create_standalone_note(
    store: &Store,
    branch_id: &str,
    title: &str,
    content: &str,
    subtype: Option<String>,
) -> Result<NoteTimelineItem, String> {
    let mut note = Note::new_standalone(branch_id, title, content);
    note.subtype = subtype;
    note.set_initial_read(note.is_written());
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
/// Media the edit removed is deliberately not collected here. The editor can
/// still bring it back: undo after a save that left the dialog open (the
/// transfer menu saves first), or a cut image pasted into another note. Either
/// would then reference a row this save had deleted. Collection happens when
/// a note is deleted ([`delete_note_impl`]), for the media it still embeds;
/// a row an edit dropped for good is left behind, the cheaper failure.
pub(crate) fn update_note_impl(
    store: &Store,
    note_id: &str,
    title: &str,
    content: &str,
) -> Result<NoteTimelineItem, String> {
    let note = store
        .update_written_note(note_id, title, content)
        .map_err(|e| e.to_string())?;
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
    crate::note_media::cleanup_unreferenced(
        store,
        crate::note_media::refs::media_ids(&note.content),
    );

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

/// Mark a branch note read or unread, returning the row as it now reads.
///
/// The note viewer marks a note read when it opens one; the viewer's toggle
/// and the timeline context menu set either state.
#[tauri::command(rename_all = "camelCase")]
pub fn set_note_read(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
    read: bool,
) -> Result<Note, String> {
    crate::get_store(&store)?
        .set_note_read(&note_id, read)
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

/// Mark a project note read or unread, returning the row as it now reads; the
/// project-note counterpart of [`set_note_read`].
#[tauri::command(rename_all = "camelCase")]
pub fn set_project_note_read(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_note_id: String,
    read: bool,
) -> Result<crate::store::ProjectNote, String> {
    crate::get_store(&store)?
        .set_project_note_read(&project_note_id, read)
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
    use crate::store::models::{Branch, Project};
    use crate::store::{Image, NOTE_MEDIA_SESSION_ID};

    fn embed(image: &Image) -> String {
        format!("![{}](staged-media://{}.png)", image.filename, image.id)
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

    /// A save must not collect the media it dropped: the editor's undo history
    /// and clipboard can still bring the reference back (see
    /// [`update_note_impl`]).
    #[test]
    fn saving_a_written_note_keeps_the_media_the_edit_removed() {
        let f = Fixture::new();
        // Media a written note owns, the way a copy lands it in its new scope.
        let image = store_media_file(
            &f.store,
            Some(&f.branch.id),
            &f.project.id,
            Some(NOTE_MEDIA_SESSION_ID),
            "shot.png",
            PNG,
            true,
        )
        .unwrap();
        let with_image = format!("Before\n\n{}", embed(&image));
        let mut note = Note::new_standalone(&f.branch.id, "Shots", &with_image)
            .with_subtype(Note::SUBTYPE_WRITTEN);
        f.store.create_note_with_unique_title(&mut note).unwrap();

        // The transfer menu saves the deletion but leaves the dialog open...
        let saved = update_note_impl(&f.store, &note.id, "Shots", "Before").unwrap();
        assert_eq!(saved.content, "Before");
        assert_kept(&f, &image);

        // ...so undo can restore the reference, or it can be pasted elsewhere.
        update_note_impl(&f.store, &note.id, "Shots", &with_image).unwrap();
        let mut pasted = Note::new_standalone(&f.branch.id, "Pasted", &embed(&image))
            .with_subtype(Note::SUBTYPE_WRITTEN);
        f.store.create_note_with_unique_title(&mut pasted).unwrap();
        update_note_impl(&f.store, &note.id, "Shots", "Before").unwrap();
        assert_kept(&f, &image);
    }

    /// Deleting a note checks all the media it embedded in one pass over the
    /// note bodies and one over the chat attachments, not one per image.
    #[test]
    fn deleting_a_note_checks_its_media_in_one_pass() {
        use crate::store::images::{take_reference_passes, ReferencePasses};

        let f = Fixture::new();
        let images: Vec<Image> = (0..8)
            .map(|n| {
                store_media_file(
                    &f.store,
                    Some(&f.branch.id),
                    &f.project.id,
                    Some(NOTE_MEDIA_SESSION_ID),
                    &format!("shot-{n}.png"),
                    PNG,
                    true,
                )
                .unwrap()
            })
            .collect();
        let body = images.iter().map(embed).collect::<Vec<_>>().join("\n\n");
        let mut note =
            Note::new_standalone(&f.branch.id, "Shots", &body).with_subtype(Note::SUBTYPE_WRITTEN);
        f.store.create_note_with_unique_title(&mut note).unwrap();
        // Another note still embeds the first image.
        let mut other = Note::new_standalone(&f.branch.id, "Other", &embed(&images[0]))
            .with_subtype(Note::SUBTYPE_WRITTEN);
        f.store.create_note_with_unique_title(&mut other).unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app");

        take_reference_passes();
        delete_note_impl(&f.store, app.handle(), &note.id, false).unwrap();
        assert_eq!(
            take_reference_passes(),
            ReferencePasses { notes: 1, chat: 1 }
        );

        assert_kept(&f, &images[0]);
        for image in &images[1..] {
            assert!(f.store.get_image(&image.id).unwrap().is_none());
            assert!(
                !image_file_path(&image.project_id, &image.id, &image.filename)
                    .unwrap()
                    .exists()
            );
        }
    }

    #[test]
    fn standalone_notes_start_read_only_when_the_user_wrote_them() {
        let store = Store::in_memory().unwrap();
        let project = Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();

        let written = create_standalone_note(
            &store,
            &branch.id,
            "Mine",
            "body",
            Some(Note::SUBTYPE_WRITTEN.to_string()),
        )
        .unwrap();
        assert!(written.read_at.is_some());
        assert_eq!(written.read_at, written.completed_at);

        // A dropped file is new to the user, like a session's note.
        let dropped = create_standalone_note(&store, &branch.id, "notes.md", "body", None).unwrap();
        assert_eq!(dropped.read_at, None);

        let stored_read_at = |id: &str| store.get_note(id).unwrap().unwrap().read_at;
        assert_eq!(stored_read_at(&written.id), written.read_at);
        assert_eq!(stored_read_at(&dropped.id), None);
    }
}
