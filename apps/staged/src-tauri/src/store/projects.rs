//! Project CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::Project;
use super::{now_timestamp, Store, StoreChange, StoreError};

impl Store {
    pub fn create_project(&self, project: &Project) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, github_repo, location, subpath, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                project.id,
                project.name,
                project.github_repo,
                project.location.as_str(),
                project.subpath,
                project.created_at,
                project.updated_at,
            ],
        )?;
        self.publish(StoreChange::Project {
            project_id: Some(project.id.clone()),
        });
        Ok(())
    }

    pub fn get_project(&self, id: &str) -> Result<Option<Project>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, github_repo, location, subpath, created_at, updated_at FROM projects WHERE id = ?1",
            params![id],
            |row| {
                let location_str: String = row.get(3)?;
                Ok(Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    github_repo: row.get(2)?,
                    location: location_str.parse().unwrap_or(super::models::ProjectLocation::Local),
                    subpath: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_project_by_repo(&self, github_repo: &str) -> Result<Option<Project>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, github_repo, location, subpath, created_at, updated_at FROM projects WHERE github_repo = ?1",
            params![github_repo],
            |row| {
                let location_str: String = row.get(3)?;
                Ok(Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    github_repo: row.get(2)?,
                    location: location_str.parse().unwrap_or(super::models::ProjectLocation::Local),
                    subpath: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_project_by_repo_and_subpath(
        &self,
        github_repo: &str,
        subpath: Option<&str>,
    ) -> Result<Option<Project>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, github_repo, location, subpath, created_at, updated_at FROM projects WHERE github_repo = ?1 AND subpath IS ?2",
            params![github_repo, subpath],
            |row| {
                let location_str: String = row.get(3)?;
                Ok(Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    github_repo: row.get(2)?,
                    location: location_str.parse().unwrap_or(super::models::ProjectLocation::Local),
                    subpath: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, github_repo, location, subpath, created_at, updated_at FROM projects ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let location_str: String = row.get(3)?;
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                github_repo: row.get(2)?,
                location: location_str
                    .parse()
                    .unwrap_or(super::models::ProjectLocation::Local),
                subpath: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_project(
        &self,
        id: &str,
        name: &str,
        github_repo: Option<&str>,
        location: &super::models::ProjectLocation,
        subpath: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE projects SET name = ?1, github_repo = ?2, location = ?3, subpath = ?4, updated_at = ?5 WHERE id = ?6",
            params![name, github_repo, location.as_str(), subpath, now_timestamp(), id],
        )?;
        self.publish(StoreChange::Project {
            project_id: Some(id.to_string()),
        });
        Ok(())
    }

    pub fn delete_project(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        self.publish(StoreChange::Project {
            project_id: Some(id.to_string()),
        });
        Ok(())
    }
}
