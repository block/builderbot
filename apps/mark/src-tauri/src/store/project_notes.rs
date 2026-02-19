//! Project note CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::ProjectNote;
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_project_note(&self, note: &ProjectNote) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO project_notes (id, project_id, session_id, title, content, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                note.id,
                note.project_id,
                note.session_id,
                note.title,
                note.content,
                note.created_at,
                note.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_project_note(&self, id: &str) -> Result<Option<ProjectNote>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, session_id, title, content, created_at, updated_at
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
            "SELECT id, project_id, session_id, title, content, created_at, updated_at
             FROM project_notes WHERE project_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id], Self::row_to_project_note)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Find an empty project note (content = '') linked to a given session.
    pub fn get_empty_project_note_by_session(
        &self,
        session_id: &str,
    ) -> Result<Option<ProjectNote>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, session_id, title, content, created_at, updated_at
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
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE project_notes SET title = ?1, content = ?2, updated_at = ?3 WHERE id = ?4",
            params![title, content, now_timestamp(), id],
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
        })
    }
}
