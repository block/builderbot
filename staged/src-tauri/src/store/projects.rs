//! Project CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::Project;
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_project(&self, project: &Project) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO projects (id, repo_path, subpath, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                project.id,
                project.repo_path,
                project.subpath,
                project.created_at,
                project.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_project(&self, id: &str) -> Result<Option<Project>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, repo_path, subpath, created_at, updated_at FROM projects WHERE id = ?1",
            params![id],
            |row| {
                Ok(Project {
                    id: row.get(0)?,
                    repo_path: row.get(1)?,
                    subpath: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_project_by_repo(&self, repo_path: &str) -> Result<Option<Project>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, repo_path, subpath, created_at, updated_at FROM projects WHERE repo_path = ?1",
            params![repo_path],
            |row| {
                Ok(Project {
                    id: row.get(0)?,
                    repo_path: row.get(1)?,
                    subpath: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, repo_path, subpath, created_at, updated_at FROM projects ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                repo_path: row.get(1)?,
                subpath: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_project(&self, id: &str, subpath: Option<&str>) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE projects SET subpath = ?1, updated_at = ?2 WHERE id = ?3",
            params![subpath, now_timestamp(), id],
        )?;
        Ok(())
    }

    pub fn delete_project(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(())
    }
}
