//! Superseded note text: what a note or project note said before it was
//! revised.
//!
//! `notes` and `project_notes` hold a note's current title and body only.
//! When one of the writes that replace that text lands
//! ([`Store::update_note_title_and_content`],
//! [`Store::update_project_note_title_and_content`],
//! [`Store::update_written_note`]) the text going out is kept as a
//! [`NoteVersion`], in the same transaction as the write. The current text is
//! never copied, so a note has no versions until it is first revised.
//!
//! One table serves both kinds of note: their ids are UUIDs, so an id names
//! at most one row across the two tables. Rows go when their note does, by
//! trigger (migration 0051), and a move between projects keeps the note's id
//! and so its versions. A copy is a new note and starts with none.

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use super::models::NoteVersion;
use super::{Store, StoreError};

/// How many versions a note keeps. Each revision past this drops the oldest.
pub(super) const MAX_NOTE_VERSIONS: usize = 10;

/// Whether replacing `(outgoing_title, outgoing_content)` with
/// `(title, content)` is a revision worth keeping the outgoing text for.
///
/// An empty outgoing body is a session's stub being filled for the first
/// time, not a version anyone read. Identical text is a republish, or a
/// change to the suggested next steps alone.
pub(super) fn is_revision(
    outgoing_title: &str,
    outgoing_content: &str,
    title: &str,
    content: &str,
) -> bool {
    !outgoing_content.is_empty() && (outgoing_title != title || outgoing_content != content)
}

impl Store {
    /// Keep `(title, content)` as a version of `note_id`, then drop the
    /// note's versions beyond the newest [`MAX_NOTE_VERSIONS`].
    ///
    /// `saved_at` is the `updated_at` of the row being replaced. Call it on
    /// the transaction that carries the replacing `UPDATE`, after
    /// [`is_revision`] says the write is one, so the version exists exactly
    /// when the note's text changed.
    ///
    /// A note's versions are ordered by `saved_at` alone, so it is moved
    /// past the note's latest version when it would not already sort after
    /// it: two revisions in one millisecond, or a clock that stepped back
    /// between them.
    pub(super) fn snapshot_note_version(
        conn: &Connection,
        note_id: &str,
        title: &str,
        content: &str,
        saved_at: i64,
    ) -> Result<(), StoreError> {
        let latest: Option<i64> = conn.query_row(
            "SELECT MAX(saved_at) FROM note_versions WHERE note_id = ?1",
            params![note_id],
            |row| row.get(0),
        )?;
        let saved_at = latest.map_or(saved_at, |latest| saved_at.max(latest.saturating_add(1)));
        conn.execute(
            "INSERT INTO note_versions (id, note_id, title, content, saved_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                Uuid::new_v4().to_string(),
                note_id,
                title,
                content,
                saved_at
            ],
        )?;
        conn.execute(
            "DELETE FROM note_versions
             WHERE note_id = ?1
               AND id NOT IN (
                   SELECT id FROM note_versions
                   WHERE note_id = ?1
                   ORDER BY saved_at DESC
                   LIMIT ?2
               )",
            params![note_id, MAX_NOTE_VERSIONS as i64],
        )?;
        Ok(())
    }

    /// The text `note_id` held before its latest revision, for a branch note
    /// or a project note. `None` for a note that has never been revised
    /// (which includes every note not revised since versions were first
    /// kept) and for an id that names no note.
    pub fn get_previous_note_version(
        &self,
        note_id: &str,
    ) -> Result<Option<NoteVersion>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, note_id, title, content, saved_at
             FROM note_versions
             WHERE note_id = ?1
             ORDER BY saved_at DESC
             LIMIT 1",
            params![note_id],
            row_to_note_version,
        )
        .optional()
        .map_err(Into::into)
    }
}

fn row_to_note_version(row: &rusqlite::Row) -> rusqlite::Result<NoteVersion> {
    Ok(NoteVersion {
        id: row.get(0)?,
        note_id: row.get(1)?,
        title: row.get(2)?,
        content: row.get(3)?,
        saved_at: row.get(4)?,
    })
}
