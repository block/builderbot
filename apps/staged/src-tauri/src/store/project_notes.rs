//! Project note CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::ProjectNote;
use super::{now_timestamp, Store, StoreChange, StoreError};

/// Sessions left behind by [`Store::delete_project_note`].
///
/// The store only removes rows; cancelling the processes those sessions are
/// still driving is the command layer's job (it owns the session registry), so
/// the delete reports every session it orphaned.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeletedProjectNoteSessions {
    /// The project note's own session, if it had one.
    pub project_note_session_id: Option<String>,
    /// Sessions belonging to the child notes removed by the cascade.
    pub child_session_ids: Vec<String>,
}

impl DeletedProjectNoteSessions {
    /// Every orphaned session id, children first.
    pub fn all_session_ids(&self) -> Vec<String> {
        self.child_session_ids
            .iter()
            .cloned()
            .chain(self.project_note_session_id.clone())
            .collect()
    }
}

impl Store {
    pub fn create_project_note(&self, note: &ProjectNote) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO project_notes (id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                note.id,
                note.project_id,
                note.session_id,
                note.title,
                note.content,
                note.created_at,
                note.updated_at,
                note.completed_at,
                note.suggested_next_commit_step,
                note.suggested_next_note_step,
            ],
        )?;
        self.publish(StoreChange::Notes {
            branch_id: None,
            project_id: Some(note.project_id.clone()),
        });
        Ok(())
    }

    pub fn get_project_note(&self, id: &str) -> Result<Option<ProjectNote>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step
             FROM project_notes WHERE id = ?1",
            params![id],
            Self::row_to_project_note,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_project_notes(&self, project_id: &str) -> Result<Vec<ProjectNote>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step
             FROM project_notes
             WHERE project_id = ?1
             ORDER BY COALESCE(completed_at, created_at) DESC, created_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id], Self::row_to_project_note)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Find a project note linked to a given session (regardless of content).
    pub fn get_project_note_by_session(
        &self,
        session_id: &str,
    ) -> Result<Option<ProjectNote>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step
             FROM project_notes WHERE session_id = ?1",
            params![session_id],
            Self::row_to_project_note,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Find an empty project note (content = '') linked to a given session.
    pub fn get_empty_project_note_by_session(
        &self,
        session_id: &str,
    ) -> Result<Option<ProjectNote>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step
             FROM project_notes WHERE session_id = ?1 AND content = ''",
            params![session_id],
            Self::row_to_project_note,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn update_project_note_title_and_content(
        &self,
        id: &str,
        title: &str,
        content: &str,
        suggested_next_commit_step: Option<&str>,
        suggested_next_note_step: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        // The session runner re-runs note extraction at the end of every turn for sessions
        // with a linked note, even if the assistant didn't rewrite the note. Without this
        // short-circuit, `updated_at` would advance on every turn, defeating any freshness
        // comparison that relies on it.
        let existing: Option<(String, String, Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT title, content, suggested_next_commit_step, suggested_next_note_step
                 FROM project_notes WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if let Some((cur_title, cur_content, cur_sncs, cur_snns)) = existing {
            if cur_title == title
                && cur_content == content
                && cur_sncs.as_deref() == suggested_next_commit_step
                && cur_snns.as_deref() == suggested_next_note_step
            {
                return Ok(());
            }
        }
        let now = now_timestamp();
        conn.execute(
            "UPDATE project_notes
             SET title = ?1, content = ?2, updated_at = ?3, completed_at = COALESCE(completed_at, ?4), suggested_next_commit_step = ?5, suggested_next_note_step = ?6
             WHERE id = ?7",
            params![title, content, now, now, suggested_next_commit_step, suggested_next_note_step, id],
        )?;
        self.publish_with(|| StoreChange::Notes {
            branch_id: None,
            project_id: Self::lookup_id(
                &conn,
                "SELECT project_id FROM project_notes WHERE id = ?1",
                id,
            ),
        });
        Ok(())
    }

    pub fn mark_project_note_completed(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        conn.execute(
            "UPDATE project_notes SET completed_at = COALESCE(completed_at, ?1) WHERE id = ?2",
            params![now, id],
        )?;
        self.publish_with(|| StoreChange::Notes {
            branch_id: None,
            project_id: Self::lookup_id(
                &conn,
                "SELECT project_id FROM project_notes WHERE id = ?1",
                id,
            ),
        });
        Ok(())
    }

    /// Delete a project note and report the sessions it orphaned, atomically.
    ///
    /// Child notes aggregated under this project note (linked via
    /// `parent_project_note_id`) are deleted in the same transaction. The
    /// `notes` and `project_notes` tables have independent lifecycles with no
    /// FK between them, so this cleanup is enforced here in code. Deleting the
    /// children fires `trg_cleanup_session_after_note_delete`, which cleans up
    /// their sessions — but that trigger deliberately skips sessions that are
    /// still `running`, so the child session ids are returned for the caller to
    /// cancel and remove (see `note_commands::delete_project_note`).
    pub fn delete_project_note(&self, id: &str) -> Result<DeletedProjectNoteSessions, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let child_session_ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT session_id FROM notes
                 WHERE parent_project_note_id = ?1 AND session_id IS NOT NULL",
            )?;
            let rows = stmt.query_map(params![id], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        tx.execute(
            "DELETE FROM notes WHERE parent_project_note_id = ?1",
            params![id],
        )?;
        let deleted: Option<(Option<String>, String)> = tx
            .query_row(
                "DELETE FROM project_notes WHERE id = ?1 RETURNING session_id, project_id",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        tx.commit()?;

        let (project_note_session_id, project_id) = match deleted {
            Some((session_id, project_id)) => (session_id, Some(project_id)),
            None => (None, None),
        };
        if project_id.is_some() || !child_session_ids.is_empty() {
            self.publish(StoreChange::Notes {
                branch_id: None,
                project_id,
            });
        }
        Ok(DeletedProjectNoteSessions {
            project_note_session_id,
            child_session_ids,
        })
    }

    fn row_to_project_note(row: &rusqlite::Row) -> rusqlite::Result<ProjectNote> {
        Ok(ProjectNote {
            id: row.get(0)?,
            project_id: row.get(1)?,
            session_id: row.get(2)?,
            title: row.get(3)?,
            content: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
            completed_at: row.get(7)?,
            suggested_next_commit_step: row.get(8)?,
            suggested_next_note_step: row.get(9)?,
            session_status: None,
            completion_reason: None,
        })
    }

    /// Find a project note by id with session status resolved.
    ///
    /// Unlike [`Self::list_project_notes_with_status`] this takes no project
    /// scope, so it can serve a `#project-note:<id>` reference that points at
    /// another project's note.
    pub fn get_project_note_with_status(
        &self,
        id: &str,
    ) -> Result<Option<ProjectNote>, StoreError> {
        let mut note = match self.get_project_note(id)? {
            Some(n) => n,
            None => return Ok(None),
        };
        let resolved = self.resolve_session_status(note.session_id.as_deref());
        note.session_status = resolved.status;
        note.completion_reason = resolved.completion_reason;
        Ok(Some(note))
    }

    /// Find a project note by session ID with session status resolved.
    pub fn get_project_note_by_session_with_status(
        &self,
        session_id: &str,
    ) -> Result<Option<ProjectNote>, StoreError> {
        let mut note = match self.get_project_note_by_session(session_id)? {
            Some(n) => n,
            None => return Ok(None),
        };
        let resolved = self.resolve_session_status(note.session_id.as_deref());
        note.session_status = resolved.status;
        note.completion_reason = resolved.completion_reason;
        Ok(Some(note))
    }

    /// Return project notes with session status resolved from the sessions table.
    pub fn list_project_notes_with_status(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectNote>, StoreError> {
        let mut notes = self.list_project_notes(project_id)?;
        for note in &mut notes {
            let resolved = self.resolve_session_status(note.session_id.as_deref());
            note.session_status = resolved.status;
            note.completion_reason = resolved.completion_reason;
        }
        Ok(notes)
    }
}
