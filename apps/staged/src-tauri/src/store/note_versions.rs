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
//!
//! A move is not a revision, but it does rewrite the note: references to
//! media it copied are re-pointed at the copies, and a branch note's title
//! may gain a suffix to stay unique where it lands. The note's versions are
//! rewritten the same way, in the move's transaction
//! ([`Store::carry_move_into_note_versions`]), so that what differs between
//! a note and its previous version is only ever what a revision changed.
//! References a version holds to media the note no longer embeds are not
//! part of the move and stay as written.

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use super::models::NoteVersion;
use super::note_transfer::MediaRetarget;
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

    /// Rewrite `note_id`'s versions as a move rewrote the note, on the
    /// move's transaction.
    ///
    /// `retarget` is the media rewrite that was applied to the note's body
    /// (see [`MediaRetarget`]); each version's body is put through it. A
    /// version that carries `title_before`, the title the note had going in,
    /// takes `title_after`, the one it lands with: the same heading renamed
    /// by the move, where a version with any other title was retitled by a
    /// revision and still shows as one.
    pub(super) fn carry_move_into_note_versions(
        conn: &Connection,
        note_id: &str,
        title_before: &str,
        title_after: &str,
        retarget: MediaRetarget<'_>,
    ) -> Result<(), StoreError> {
        if title_before != title_after {
            conn.execute(
                "UPDATE note_versions SET title = ?1 WHERE note_id = ?2 AND title = ?3",
                params![title_after, note_id, title_before],
            )?;
        }
        let bodies: Vec<(String, String)> = conn
            .prepare("SELECT id, content FROM note_versions WHERE note_id = ?1")?
            .query_map(params![note_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (version_id, content) in bodies {
            if let Some(rewritten) = retarget(note_id, &content) {
                conn.execute(
                    "UPDATE note_versions SET content = ?1 WHERE id = ?2",
                    params![rewritten, version_id],
                )?;
            }
        }
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
        let conn = self.read_conn();
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
