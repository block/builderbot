//! Project CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::{Project, ProjectLocation};
use super::{now_timestamp, Store, StoreChange, StoreError};

// `projects.status_override` (0032) is frozen: tags replaced it, and only the
// one-shot legacy import (`tag_import.rs`) still reads it.
const PROJECT_COLUMNS: &str = "id, name, github_repo, location, subpath, created_at, updated_at";

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    let location_str: String = row.get(3)?;
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        github_repo: row.get(2)?,
        location: location_str.parse().unwrap_or(ProjectLocation::Local),
        subpath: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

impl Store {
    pub fn create_project(&self, project: &Project) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            &format!(
                "INSERT INTO projects ({PROJECT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
            ),
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
            &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE id = ?1"),
            params![id],
            project_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn get_project_by_repo(&self, github_repo: &str) -> Result<Option<Project>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE github_repo = ?1"),
            params![github_repo],
            project_from_row,
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
            &format!(
                "SELECT {PROJECT_COLUMNS} FROM projects WHERE github_repo = ?1 AND subpath IS ?2"
            ),
            params![github_repo, subpath],
            project_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            // Newest first. The rowid tiebreak keeps the order stable when two
            // projects share a created_at millisecond, giving the later insert
            // precedence so a freshly created project always lands at the top.
            &format!("SELECT {PROJECT_COLUMNS} FROM projects ORDER BY created_at DESC, rowid DESC"),
        )?;
        let rows = stmt.query_map([], project_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_project(
        &self,
        id: &str,
        name: &str,
        github_repo: Option<&str>,
        location: &ProjectLocation,
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

    /// Rename a project. Only the name column moves: the primary-repo
    /// columns are rewritten by `update_project` and `location` by a transfer,
    /// so a rename racing either must not carry a stale snapshot of them back
    /// over the row. Nothing derived from the name at creation time (branch
    /// names, worktrees, workspaces) is touched.
    pub fn rename_project(&self, id: &str, name: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let updated = conn.execute(
            "UPDATE projects SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name, now_timestamp(), id],
        )?;
        if updated == 0 {
            return Err(StoreError(format!("Project not found: {id}")));
        }
        self.publish(StoreChange::Project {
            project_id: Some(id.to_string()),
        });
        Ok(())
    }

    /// Delete a project. Its `project_tags` rows go with it (FK cascade), so
    /// this publishes [`StoreChange::Tags`] as well.
    pub fn delete_project(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        self.publish(StoreChange::Project {
            project_id: Some(id.to_string()),
        });
        self.publish(StoreChange::Tags);
        Ok(())
    }
}
