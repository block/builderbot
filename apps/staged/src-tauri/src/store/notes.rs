//! Note CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::Note;
use super::{now_timestamp, Store, StoreChange, StoreError};

impl Store {
    pub fn create_note(&self, note: &Note) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        Self::insert_note(&conn, note)?;
        self.publish(StoreChange::Notes {
            branch_id: Some(note.branch_id.clone()),
            project_id: None,
        });
        Ok(())
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
        Self::insert_note(&conn, note)?;
        self.publish(StoreChange::Notes {
            branch_id: Some(note.branch_id.clone()),
            project_id: None,
        });
        Ok(())
    }

    fn insert_note(conn: &rusqlite::Connection, note: &Note) -> Result<(), StoreError> {
        conn.execute(
            "INSERT INTO notes (id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, parent_project_note_id)
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
                note.suggested_next_commit_step,
                note.suggested_next_note_step,
                note.parent_project_note_id,
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
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, parent_project_note_id
             FROM notes WHERE id = ?1",
            params![id],
            Self::row_to_note,
        )
        .optional()
        .map_err(Into::into)
    }

    /// List the notes visible in a branch's UI timeline (and the `#`-autocomplete
    /// items derived from it). Child notes are excluded: that is a *UI-visibility*
    /// rule — they are shown under their parent project note instead. Agent branch
    /// history wants every note, so it uses [`Store::list_all_notes_for_branch`].
    pub fn list_notes_for_branch(&self, branch_id: &str) -> Result<Vec<Note>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, parent_project_note_id
             FROM notes WHERE branch_id = ?1 AND parent_project_note_id IS NULL
             ORDER BY COALESCE(completed_at, created_at) DESC, created_at DESC",
        )?;
        let rows = stmt.query_map(params![branch_id], Self::row_to_note)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// List every note on a branch, including child notes. This is the branch-history
    /// path: agents need to read the child notes their parent project note references,
    /// even though the UI timeline hides them (see [`Store::list_notes_for_branch`]).
    pub fn list_all_notes_for_branch(&self, branch_id: &str) -> Result<Vec<Note>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, parent_project_note_id
             FROM notes WHERE branch_id = ?1
             ORDER BY COALESCE(completed_at, created_at) DESC, created_at DESC",
        )?;
        let rows = stmt.query_map(params![branch_id], Self::row_to_note)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// List the child notes aggregated under a given project note. This is the
    /// dedicated path the parent project-note view uses to fetch its children;
    /// they are excluded from `list_notes_for_branch` for UI visibility, but still
    /// appear in agent branch history via `list_all_notes_for_branch`.
    pub fn list_child_notes(&self, parent_project_note_id: &str) -> Result<Vec<Note>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, parent_project_note_id
             FROM notes WHERE parent_project_note_id = ?1
             ORDER BY COALESCE(completed_at, created_at) DESC, created_at DESC",
        )?;
        let rows = stmt.query_map(params![parent_project_note_id], Self::row_to_note)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Find a note linked to a given session (regardless of content).
    pub fn get_note_by_session(&self, session_id: &str) -> Result<Option<Note>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, parent_project_note_id
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
            "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, parent_project_note_id
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
                 FROM notes WHERE id = ?1",
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
            "UPDATE notes SET title = ?1, content = ?2, updated_at = ?3, completed_at = COALESCE(completed_at, ?4), suggested_next_commit_step = ?5, suggested_next_note_step = ?6 WHERE id = ?7",
            params![title, content, now, now, suggested_next_commit_step, suggested_next_note_step, id],
        )?;
        self.publish_with(|| StoreChange::Notes {
            branch_id: Self::lookup_id(&conn, "SELECT branch_id FROM notes WHERE id = ?1", id),
            project_id: None,
        });
        Ok(())
    }

    pub fn update_note_content(&self, id: &str, content: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        conn.execute(
            "UPDATE notes SET content = ?1, updated_at = ?2, completed_at = COALESCE(completed_at, ?3) WHERE id = ?4",
            params![content, now, now, id],
        )?;
        self.publish_with(|| StoreChange::Notes {
            branch_id: Self::lookup_id(&conn, "SELECT branch_id FROM notes WHERE id = ?1", id),
            project_id: None,
        });
        Ok(())
    }

    pub fn mark_note_completed(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        conn.execute(
            "UPDATE notes SET completed_at = COALESCE(completed_at, ?1) WHERE id = ?2",
            params![now, id],
        )?;
        self.publish_with(|| StoreChange::Notes {
            branch_id: Self::lookup_id(&conn, "SELECT branch_id FROM notes WHERE id = ?1", id),
            project_id: None,
        });
        Ok(())
    }

    pub fn delete_note(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        // Resolved before the row disappears, published only if the delete lands.
        let branch_id = Self::lookup_id(&conn, "SELECT branch_id FROM notes WHERE id = ?1", id);
        conn.execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        if branch_id.is_some() {
            self.publish(StoreChange::Notes {
                branch_id,
                project_id: None,
            });
        }
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
            completed_at: row.get(7)?,
            suggested_next_commit_step: row.get(8)?,
            suggested_next_note_step: row.get(9)?,
            parent_project_note_id: row.get(10)?,
        })
    }
}
