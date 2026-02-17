//! Project repository CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::ProjectRepo;
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_project_repo(&self, repo: &ProjectRepo) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO project_repos (id, project_id, github_repo, subpath, is_primary, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                repo.id,
                repo.project_id,
                repo.github_repo,
                repo.subpath,
                if repo.is_primary { 1 } else { 0 },
                repo.created_at,
                repo.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_project_repo(&self, id: &str) -> Result<Option<ProjectRepo>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, github_repo, subpath, is_primary, created_at, updated_at
             FROM project_repos WHERE id = ?1",
            params![id],
            Self::row_to_project_repo,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_project_repos(&self, project_id: &str) -> Result<Vec<ProjectRepo>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, github_repo, subpath, is_primary, created_at, updated_at
             FROM project_repos WHERE project_id = ?1
             ORDER BY is_primary DESC, created_at ASC",
        )?;
        let rows = stmt.query_map(params![project_id], Self::row_to_project_repo)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_primary_project_repo(&self, project_id: &str) -> Result<Option<ProjectRepo>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, github_repo, subpath, is_primary, created_at, updated_at
             FROM project_repos WHERE project_id = ?1 AND is_primary = 1
             ORDER BY created_at ASC LIMIT 1",
            params![project_id],
            Self::row_to_project_repo,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn set_primary_project_repo(&self, project_id: &str, repo_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        conn.execute(
            "UPDATE project_repos SET is_primary = 0, updated_at = ?1 WHERE project_id = ?2",
            params![now, project_id],
        )?;
        conn.execute(
            "UPDATE project_repos SET is_primary = 1, updated_at = ?1 WHERE id = ?2 AND project_id = ?3",
            params![now, repo_id, project_id],
        )?;
        Ok(())
    }

    pub fn delete_project_repo(&self, repo_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM project_repos WHERE id = ?1", params![repo_id])?;
        Ok(())
    }

    fn row_to_project_repo(row: &rusqlite::Row) -> rusqlite::Result<ProjectRepo> {
        let is_primary_i64: i64 = row.get(4)?;
        Ok(ProjectRepo {
            id: row.get(0)?,
            project_id: row.get(1)?,
            github_repo: row.get(2)?,
            subpath: row.get(3)?,
            is_primary: is_primary_i64 == 1,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    }
}

