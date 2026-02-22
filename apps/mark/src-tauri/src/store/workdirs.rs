//! Workdir CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::Workdir;
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_workdir(&self, workdir: &Workdir) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO workdirs (id, project_id, path, branch_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                workdir.id,
                workdir.project_id,
                workdir.path,
                workdir.branch_id,
                workdir.created_at,
                workdir.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_workdir(&self, id: &str) -> Result<Option<Workdir>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, path, branch_id, created_at, updated_at
             FROM workdirs WHERE id = ?1",
            params![id],
            Self::row_to_workdir,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Get the workdir currently assigned to a branch (if any).
    pub fn get_workdir_for_branch(&self, branch_id: &str) -> Result<Option<Workdir>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, path, branch_id, created_at, updated_at
             FROM workdirs WHERE branch_id = ?1",
            params![branch_id],
            Self::row_to_workdir,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Find an available (unoccupied) workdir for a project.
    pub fn find_available_workdir(&self, project_id: &str) -> Result<Option<Workdir>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, path, branch_id, created_at, updated_at
             FROM workdirs WHERE project_id = ?1 AND branch_id IS NULL
             LIMIT 1",
            params![project_id],
            Self::row_to_workdir,
        )
        .optional()
        .map_err(Into::into)
    }

    /// List all workdirs for a project.
    pub fn list_workdirs_for_project(&self, project_id: &str) -> Result<Vec<Workdir>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, path, branch_id, created_at, updated_at
             FROM workdirs WHERE project_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![project_id], Self::row_to_workdir)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Assign a branch to a workdir. Sets `branch_id` on the workdir.
    pub fn assign_workdir(&self, workdir_id: &str, branch_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workdirs SET branch_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![branch_id, now_timestamp(), workdir_id],
        )?;
        Ok(())
    }

    /// Release a workdir (clear its branch assignment).
    pub fn release_workdir(&self, workdir_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workdirs SET branch_id = NULL, updated_at = ?1 WHERE id = ?2",
            params![now_timestamp(), workdir_id],
        )?;
        Ok(())
    }

    pub fn delete_workdir(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM workdirs WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn row_to_workdir(row: &rusqlite::Row) -> rusqlite::Result<Workdir> {
        Ok(Workdir {
            id: row.get(0)?,
            project_id: row.get(1)?,
            path: row.get(2)?,
            branch_id: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    }
}
