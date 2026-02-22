//! Note CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::Note;
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_note(&self, note: &Note) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO notes (id, branch_id, session_id, title, content, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                note.id,
                note.branch_id,
                note.session_id,
                note.title,
                note.content,
                note.created_at,
                note.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_note(&self, id: &str) -> Result<Option<Note>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at
             FROM notes WHERE id = ?1",
            params![id],
            Self::row_to_note,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_notes_for_branch(&self, branch_id: &str) -> Result<Vec<Note>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at
             FROM notes WHERE branch_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![branch_id], Self::row_to_note)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Find an empty note (content = '') linked to a given session.
    pub fn get_empty_note_by_session(&self, session_id: &str) -> Result<Option<Note>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at
             FROM notes WHERE session_id = ?1 AND content = ''",
            params![session_id],
            Self::row_to_note,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Update a note's title and content.
    pub fn update_note_title_and_content(
        &self,
        id: &str,
        title: &str,
        content: &str,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE notes SET title = ?1, content = ?2, updated_at = ?3 WHERE id = ?4",
            params![title, content, now_timestamp(), id],
        )?;
        Ok(())
    }

    pub fn update_note_content(&self, id: &str, content: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE notes SET content = ?1, updated_at = ?2 WHERE id = ?3",
            params![content, now_timestamp(), id],
        )?;
        Ok(())
    }

    pub fn delete_note(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn row_to_note(row: &rusqlite::Row) -> rusqlite::Result<Note> {
        Ok(Note {
            id: row.get(0)?,
            branch_id: row.get(1)?,
            session_id: row.get(2)?,
            title: row.get(3)?,
            content: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    }
}
