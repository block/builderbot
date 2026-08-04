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

    /// Return a map of branch_id → number of finalized commits (sha IS NOT NULL)
    /// for every branch belonging to the given project.
    pub fn count_finalized_commits_by_branch_for_project(
        &self,
        project_id: &str,
    ) -> Result<std::collections::HashMap<String, u64>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.branch_id, COUNT(*)
             FROM commits c
             JOIN branches b ON b.id = c.branch_id
             WHERE b.project_id = ?1 AND c.sha IS NOT NULL
             GROUP BY c.branch_id",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        })?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (branch_id, count) = row?;
            map.insert(branch_id, count);
        }
        Ok(map)
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

    /// Complete a pending commit row with a git SHA.
    ///
    /// If another metadata row on the same branch already owns the target SHA,
    /// the pending row is deleted instead of violating the branch/SHA unique
    /// index. Returns `true` when this row now owns the SHA, or `false` when it
    /// was resolved by removing the duplicate pending row.
    pub fn complete_pending_commit_sha(
        &self,
        id: &str,
        branch_id: &str,
        sha: &str,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();

        let existing_id = conn
            .query_row(
                "SELECT id FROM commits WHERE branch_id = ?1 AND sha = ?2",
                params![branch_id, sha],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        if let Some(existing_id) = existing_id {
            if existing_id == id {
                return Ok(true);
            }
            conn.execute(
                "DELETE FROM commits WHERE id = ?1 AND branch_id = ?2 AND sha IS NULL",
                params![id, branch_id],
            )?;
            return Ok(false);
        }

        let rows = conn.execute(
            "UPDATE commits SET sha = ?1, updated_at = ?2 WHERE id = ?3 AND branch_id = ?4 AND sha IS NULL",
            params![sha, now_timestamp(), id, branch_id],
        )?;
        Ok(rows > 0)
    }

    /// Repoint commit rows at the SHAs a history rewrite (e.g. a rebase) gave
    /// them, carrying each row's reviews along so they don't drop out of the
    /// timeline with their old commit.
    ///
    /// `remaps` is a list of `(row_id, old_sha, new_sha)`, applied in one
    /// transaction. Each pair re-checks the `(branch_id, sha)` unique index
    /// inside the transaction and is skipped on collision, so a row another
    /// writer already attached to `new_sha` keeps it. Returns how many rows
    /// were repointed.
    pub fn remap_commit_shas(
        &self,
        branch_id: &str,
        remaps: &[(&str, &str, &str)],
    ) -> Result<usize, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = now_timestamp();
        let mut remapped = 0;

        for (row_id, old_sha, new_sha) in remaps {
            let owner = tx
                .query_row(
                    "SELECT id FROM commits WHERE branch_id = ?1 AND sha = ?2",
                    params![branch_id, new_sha],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if owner.is_some_and(|owner| owner != *row_id) {
                continue;
            }

            let rows = tx.execute(
                "UPDATE commits SET sha = ?1, updated_at = ?2
                 WHERE id = ?3 AND branch_id = ?4 AND sha = ?5",
                params![new_sha, now, row_id, branch_id, old_sha],
            )?;
            if rows == 0 {
                continue;
            }

            tx.execute(
                "UPDATE reviews SET commit_sha = ?1, updated_at = ?2
                 WHERE branch_id = ?3 AND commit_sha = ?4",
                params![new_sha, now, branch_id, old_sha],
            )?;
            remapped += 1;
        }

        tx.commit()?;
        Ok(remapped)
    }

    /// Delete a linked pending commit row if it has not landed.
    pub fn delete_pending_commit_for_session(&self, session_id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM commits WHERE session_id = ?1 AND sha IS NULL",
            params![session_id],
        )?;
        Ok(rows > 0)
    }

    /// Find any commit linked to a given session (regardless of SHA status).
    pub fn get_commit_by_session(&self, session_id: &str) -> Result<Option<Commit>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, branch_id, sha, session_id, created_at, updated_at
             FROM commits WHERE session_id = ?1",
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
