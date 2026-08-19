//! Recent repository tracking operations.

use rusqlite::params;

use super::models::RecentRepo;
use super::{now_timestamp, Store, StoreChange, StoreError};

impl Store {
    /// Record that a repository was used, keeping only the most recent 20.
    pub fn record_recent_repo(
        &self,
        github_repo: &str,
        subpath: Option<String>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();

        // Insert or update the recent repo
        conn.execute(
            "INSERT INTO recent_repos (id, github_repo, subpath, last_used_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(github_repo, COALESCE(subpath, ''))
             DO UPDATE SET last_used_at = ?4",
            params![uuid::Uuid::new_v4().to_string(), github_repo, subpath, now],
        )?;

        // Keep only the 20 most recent
        conn.execute(
            "DELETE FROM recent_repos
             WHERE id NOT IN (
                 SELECT id FROM recent_repos
                 ORDER BY last_used_at DESC
                 LIMIT 20
             )",
            [],
        )?;

        self.publish(StoreChange::Repos {
            github_repo: Some(github_repo.to_string()),
        });
        Ok(())
    }

    /// Get the most recent repositories, up to the specified limit.
    pub fn list_recent_repos(&self, limit: usize) -> Result<Vec<RecentRepo>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, github_repo, subpath, last_used_at
             FROM recent_repos
             ORDER BY last_used_at DESC
             LIMIT ?1",
        )?;

        let repos = stmt
            .query_map(params![limit as i64], |row| {
                Ok(RecentRepo {
                    id: row.get(0)?,
                    github_repo: row.get(1)?,
                    subpath: row.get(2)?,
                    last_used_at: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(repos)
    }
}
