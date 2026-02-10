//! Commit CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::Commit;
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_commit(&self, commit: &Commit) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO commits (id, branch_id, sha, session_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                commit.id,
                commit.branch_id,
                commit.sha,
                commit.session_id,
                commit.created_at,
                commit.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_commit(&self, id: &str) -> Result<Option<Commit>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, branch_id, sha, session_id, created_at, updated_at
             FROM commits WHERE id = ?1",
            params![id],
            Self::row_to_commit,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Look up a commit record by its git SHA on a branch.
    pub fn get_commit_by_sha(
        &self,
        branch_id: &str,
        sha: &str,
    ) -> Result<Option<Commit>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, branch_id, sha, session_id, created_at, updated_at
             FROM commits WHERE branch_id = ?1 AND sha = ?2",
            params![branch_id, sha],
            Self::row_to_commit,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_commits_for_branch(&self, branch_id: &str) -> Result<Vec<Commit>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, branch_id, sha, session_id, created_at, updated_at
             FROM commits WHERE branch_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![branch_id], Self::row_to_commit)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Set the SHA once the git commit has landed.
    pub fn update_commit_sha(&self, id: &str, sha: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE commits SET sha = ?1, updated_at = ?2 WHERE id = ?3",
            params![sha, now_timestamp(), id],
        )?;
        Ok(())
    }

    /// Find a pending commit (sha IS NULL) linked to a given session.
    pub fn get_pending_commit_by_session(
        &self,
        session_id: &str,
    ) -> Result<Option<Commit>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, branch_id, sha, session_id, created_at, updated_at
             FROM commits WHERE session_id = ?1 AND sha IS NULL",
            params![session_id],
            Self::row_to_commit,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn delete_commit(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM commits WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn row_to_commit(row: &rusqlite::Row) -> rusqlite::Result<Commit> {
        Ok(Commit {
            id: row.get(0)?,
            branch_id: row.get(1)?,
            sha: row.get(2)?,
            session_id: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    }
}
