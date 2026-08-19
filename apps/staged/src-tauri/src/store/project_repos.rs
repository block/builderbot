//! Project repository CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::ProjectRepo;
use super::{now_timestamp, Store, StoreChange, StoreError};

impl Store {
    pub fn create_project_repo(&self, repo: &ProjectRepo) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        if let Err(e) = conn.execute(
            "INSERT INTO project_repos (id, project_id, github_repo, branch_name, subpath, is_primary, reason, head_repo, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                repo.id,
                repo.project_id,
                repo.github_repo,
                repo.branch_name,
                repo.subpath,
                if repo.is_primary { 1 } else { 0 },
                repo.reason,
                repo.head_repo,
                repo.created_at,
                repo.updated_at,
            ],
        ) {
            if is_duplicate_project_repo_error(&e) {
                let subpath_hint = repo
                    .subpath
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| format!(" at subpath '{s}'"))
                    .unwrap_or_default();
                return Err(StoreError(format!(
                    "Repository '{}'{} is already attached to this project.",
                    repo.github_repo, subpath_hint
                )));
            }
            return Err(e.into());
        }
        self.publish(StoreChange::Project {
            project_id: Some(repo.project_id.clone()),
        });
        Ok(())
    }

    pub fn get_project_repo(&self, id: &str) -> Result<Option<ProjectRepo>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, github_repo, branch_name, subpath, is_primary, reason, head_repo, created_at, updated_at
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
            "SELECT id, project_id, github_repo, branch_name, subpath, is_primary, reason, head_repo, created_at, updated_at
             FROM project_repos WHERE project_id = ?1
             ORDER BY is_primary DESC, created_at ASC",
        )?;
        let rows = stmt.query_map(params![project_id], Self::row_to_project_repo)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_primary_project_repo(
        &self,
        project_id: &str,
    ) -> Result<Option<ProjectRepo>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, github_repo, branch_name, subpath, is_primary, reason, head_repo, created_at, updated_at
             FROM project_repos WHERE project_id = ?1 AND is_primary = 1
             ORDER BY created_at ASC LIMIT 1",
            params![project_id],
            Self::row_to_project_repo,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn update_project_repo_branch_name(
        &self,
        project_id: &str,
        repo_id: &str,
        branch_name: &str,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE project_repos SET branch_name = ?1, updated_at = ?2 WHERE id = ?3 AND project_id = ?4",
            params![branch_name, now_timestamp(), repo_id, project_id],
        )?;
        self.publish(StoreChange::Project {
            project_id: Some(project_id.to_string()),
        });
        Ok(())
    }

    pub fn set_primary_project_repo(
        &self,
        project_id: &str,
        repo_id: &str,
    ) -> Result<(), StoreError> {
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
        self.publish(StoreChange::Project {
            project_id: Some(project_id.to_string()),
        });
        Ok(())
    }

    pub fn clear_project_repo_reason(&self, repo_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE project_repos SET reason = NULL, updated_at = ?1 WHERE id = ?2",
            params![now_timestamp(), repo_id],
        )?;
        self.publish_with(|| StoreChange::Project {
            project_id: Self::lookup_id(
                &conn,
                "SELECT project_id FROM project_repos WHERE id = ?1",
                repo_id,
            ),
        });
        Ok(())
    }

    pub fn delete_project_repo(&self, repo_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        // Resolved before the row disappears, published only if the delete lands.
        let project_id = Self::lookup_id(
            &conn,
            "SELECT project_id FROM project_repos WHERE id = ?1",
            repo_id,
        );
        conn.execute("DELETE FROM project_repos WHERE id = ?1", params![repo_id])?;
        self.publish(StoreChange::Project { project_id });
        Ok(())
    }

    fn row_to_project_repo(row: &rusqlite::Row) -> rusqlite::Result<ProjectRepo> {
        let is_primary_i64: i64 = row.get(5)?;
        Ok(ProjectRepo {
            id: row.get(0)?,
            project_id: row.get(1)?,
            github_repo: row.get(2)?,
            branch_name: row.get(3)?,
            subpath: row.get(4)?,
            is_primary: is_primary_i64 == 1,
            reason: row.get(6)?,
            head_repo: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        })
    }
}

fn is_duplicate_project_repo_error(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(_, Some(msg)) => {
            msg.contains("idx_project_repos_unique")
                || (msg.contains("UNIQUE constraint failed")
                    && msg.contains("project_repos.project_id")
                    && msg.contains("project_repos.github_repo"))
        }
        _ => false,
    }
}
