//! Project note CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::ProjectNote;
use super::{now_timestamp, Store, StoreError};

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
        let now = now_timestamp();
        conn.execute(
            "UPDATE project_notes
             SET title = ?1, content = ?2, updated_at = ?3, completed_at = COALESCE(completed_at, ?4), suggested_next_commit_step = ?5, suggested_next_note_step = ?6
             WHERE id = ?7",
            params![title, content, now, now, suggested_next_commit_step, suggested_next_note_step, id],
        )?;
        Ok(())
    }

    pub fn mark_project_note_completed(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        conn.execute(
            "UPDATE project_notes SET completed_at = COALESCE(completed_at, ?1) WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn delete_project_note(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM project_notes WHERE id = ?1", params![id])?;
        Ok(())
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

    /// Return project notes with session status resolved from the sessions table.
    /// Filters out empty stubs whose session was cancelled (nothing to show).
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
        // Remove empty stubs from cancelled/errored sessions — no content to display.
        notes.retain(|n| {
            let is_empty = n.title.trim().is_empty() && n.content.trim().is_empty();
            let is_terminal = matches!(
                n.session_status.as_deref(),
                Some("cancelled") | Some("error")
            );
            !(is_empty && is_terminal)
        });
        Ok(notes)
    }
}
