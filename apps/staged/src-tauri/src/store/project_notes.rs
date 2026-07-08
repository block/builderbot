//! Project note CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::{
    suggested_next_steps_from_storage, suggested_next_steps_legacy_commit_step,
    suggested_next_steps_legacy_note_step, suggested_next_steps_to_storage, ProjectNote,
    SuggestedNextStep,
};
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_project_note(&self, note: &ProjectNote) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let suggested_next_steps = suggested_next_steps_to_storage(&note.suggested_next_steps)
            .map_err(|e| StoreError(format!("Failed to serialize suggested next steps: {e}")))?;
        let suggested_next_commit_step = note
            .suggested_next_commit_step
            .as_deref()
            .or_else(|| suggested_next_steps_legacy_commit_step(&note.suggested_next_steps));
        let suggested_next_note_step = note
            .suggested_next_note_step
            .as_deref()
            .or_else(|| suggested_next_steps_legacy_note_step(&note.suggested_next_steps));
        conn.execute(
            "INSERT INTO project_notes (id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, suggested_next_steps)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                note.id,
                note.project_id,
                note.session_id,
                note.title,
                note.content,
                note.created_at,
                note.updated_at,
                note.completed_at,
                suggested_next_commit_step,
                suggested_next_note_step,
                suggested_next_steps,
            ],
        )?;
        Ok(())
    }

    pub fn get_project_note(&self, id: &str) -> Result<Option<ProjectNote>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, suggested_next_steps
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
            "SELECT id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, suggested_next_steps
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
            "SELECT id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, suggested_next_steps
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
            "SELECT id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, suggested_next_steps
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
        suggested_next_steps: &[SuggestedNextStep],
    ) -> Result<(), StoreError> {
        let suggested_next_steps_json = suggested_next_steps_to_storage(suggested_next_steps)
            .map_err(|e| StoreError(format!("Failed to serialize suggested next steps: {e}")))?;
        let comparable_next_steps =
            suggested_next_steps_from_storage(suggested_next_steps_json.clone(), None, None);
        let suggested_next_commit_step =
            suggested_next_steps_legacy_commit_step(&comparable_next_steps);
        let suggested_next_note_step =
            suggested_next_steps_legacy_note_step(&comparable_next_steps);

        let conn = self.conn.lock().unwrap();
        // The session runner re-runs note extraction at the end of every turn for sessions
        // with a linked note, even if the assistant didn't rewrite the note. Without this
        // short-circuit, `updated_at` would advance on every turn, defeating any freshness
        // comparison that relies on it.
        let existing: Option<(
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        )> = conn
            .query_row(
                "SELECT title, content, suggested_next_commit_step, suggested_next_note_step, suggested_next_steps
                 FROM project_notes WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        if let Some((cur_title, cur_content, cur_sncs, cur_snns, cur_steps_json)) = existing {
            let cur_steps = suggested_next_steps_from_storage(cur_steps_json, cur_sncs, cur_snns);
            if cur_title == title && cur_content == content && cur_steps == comparable_next_steps {
                return Ok(());
            }
        }
        let now = now_timestamp();
        conn.execute(
            "UPDATE project_notes
             SET title = ?1, content = ?2, updated_at = ?3, completed_at = COALESCE(completed_at, ?4), suggested_next_commit_step = ?5, suggested_next_note_step = ?6, suggested_next_steps = ?7
             WHERE id = ?8",
            params![
                title,
                content,
                now,
                now,
                suggested_next_commit_step,
                suggested_next_note_step,
                suggested_next_steps_json,
                id
            ],
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

    /// Delete a project note and return its session_id (if any) atomically.
    pub fn delete_project_note(&self, id: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let session_id: Option<Option<String>> = conn
            .query_row(
                "DELETE FROM project_notes WHERE id = ?1 RETURNING session_id",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(session_id.flatten())
    }

    fn row_to_project_note(row: &rusqlite::Row) -> rusqlite::Result<ProjectNote> {
        let suggested_next_commit_step: Option<String> = row.get(8)?;
        let suggested_next_note_step: Option<String> = row.get(9)?;
        let suggested_next_steps_json: Option<String> = row.get(10)?;
        let suggested_next_steps = suggested_next_steps_from_storage(
            suggested_next_steps_json,
            suggested_next_commit_step.clone(),
            suggested_next_note_step.clone(),
        );
        Ok(ProjectNote {
            id: row.get(0)?,
            project_id: row.get(1)?,
            session_id: row.get(2)?,
            title: row.get(3)?,
            content: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
            completed_at: row.get(7)?,
            suggested_next_commit_step,
            suggested_next_note_step,
            suggested_next_steps,
            session_status: None,
            completion_reason: None,
        })
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
