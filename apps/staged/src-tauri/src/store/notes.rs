//! Note CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::{
    suggested_next_steps_from_storage, suggested_next_steps_legacy_commit_step,
    suggested_next_steps_legacy_note_step, suggested_next_steps_to_storage, Note,
    SuggestedNextStep,
};
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_note(&self, note: &Note) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        Self::insert_note(&conn, note)
    }

    /// Like [`Store::create_note`], but if the requested title collides with
    /// an existing note on the same branch, append ` (2)`, ` (3)`, … to make
    /// it unique (filesystem "Untitled (2).txt" convention). Lookup and
    /// insert run under one connection lock so concurrent saves can't pick
    /// the same suffix.
    ///
    /// Empty titles are left untouched — those are session stubs that get
    /// their title filled in later by the runner.
    pub fn create_note_with_unique_title(&self, note: &mut Note) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        if !note.title.is_empty() {
            note.title = Self::resolve_unique_note_title(&conn, &note.branch_id, &note.title)?;
        }
        Self::insert_note(&conn, note)
    }

    fn insert_note(conn: &rusqlite::Connection, note: &Note) -> Result<(), StoreError> {
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
            "INSERT INTO notes (id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, suggested_next_steps)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                note.id,
                note.branch_id,
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

    fn resolve_unique_note_title(
        conn: &rusqlite::Connection,
        branch_id: &str,
        base: &str,
    ) -> Result<String, StoreError> {
        let mut stmt = conn.prepare("SELECT title FROM notes WHERE branch_id = ?1")?;
        let titles: Vec<String> = stmt
            .query_map(params![branch_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;

        if !titles.iter().any(|t| t == base) {
            return Ok(base.to_string());
        }

        let prefix = format!("{base} (");
        let max_n = titles
            .iter()
            .filter_map(|t| {
                let rest = t.strip_prefix(&prefix)?;
                let num = rest.strip_suffix(')')?;
                let n = num.parse::<u32>().ok()?;
                (n >= 2).then_some(n)
            })
            .max()
            .unwrap_or(1);

        Ok(format!("{base} ({})", max_n + 1))
    }

    pub fn get_note(&self, id: &str) -> Result<Option<Note>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, suggested_next_steps
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
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, suggested_next_steps
             FROM notes WHERE branch_id = ?1
             ORDER BY COALESCE(completed_at, created_at) DESC, created_at DESC",
        )?;
        let rows = stmt.query_map(params![branch_id], Self::row_to_note)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Find a note linked to a given session (regardless of content).
    pub fn get_note_by_session(&self, session_id: &str) -> Result<Option<Note>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, suggested_next_steps
             FROM notes WHERE session_id = ?1",
            params![session_id],
            Self::row_to_note,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Find an empty note (content = '') linked to a given session.
    pub fn get_empty_note_by_session(&self, session_id: &str) -> Result<Option<Note>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, suggested_next_steps
             FROM notes WHERE session_id = ?1 AND content = ''",
            params![session_id],
            Self::row_to_note,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Update a note's title, content, and optional suggested next steps.
    pub fn update_note_title_and_content(
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
                 FROM notes WHERE id = ?1",
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
            "UPDATE notes SET title = ?1, content = ?2, updated_at = ?3, completed_at = COALESCE(completed_at, ?4), suggested_next_commit_step = ?5, suggested_next_note_step = ?6, suggested_next_steps = ?7 WHERE id = ?8",
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

    pub fn update_note_content(&self, id: &str, content: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        conn.execute(
            "UPDATE notes SET content = ?1, updated_at = ?2, completed_at = COALESCE(completed_at, ?3) WHERE id = ?4",
            params![content, now, now, id],
        )?;
        Ok(())
    }

    pub fn mark_note_completed(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        conn.execute(
            "UPDATE notes SET completed_at = COALESCE(completed_at, ?1) WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn delete_note(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn row_to_note(row: &rusqlite::Row) -> rusqlite::Result<Note> {
        let suggested_next_commit_step: Option<String> = row.get(8)?;
        let suggested_next_note_step: Option<String> = row.get(9)?;
        let suggested_next_steps_json: Option<String> = row.get(10)?;
        let suggested_next_steps = suggested_next_steps_from_storage(
            suggested_next_steps_json,
            suggested_next_commit_step.clone(),
            suggested_next_note_step.clone(),
        );
        Ok(Note {
            id: row.get(0)?,
            branch_id: row.get(1)?,
            session_id: row.get(2)?,
            title: row.get(3)?,
            content: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
            completed_at: row.get(7)?,
            suggested_next_commit_step,
            suggested_next_note_step,
            suggested_next_steps,
        })
    }
}
