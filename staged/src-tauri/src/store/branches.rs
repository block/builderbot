//! Branch CRUD operations.

use rusqlite::{params, Connection, OptionalExtension};

use super::models::Branch;
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_branch(&self, branch: &Branch) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO branches (id, project_id, branch_name, base_branch, pr_number, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                branch.id,
                branch.project_id,
                branch.branch_name,
                branch.base_branch,
                branch.pr_number.map(|n| n as i64),
                branch.created_at,
                branch.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_branch(&self, id: &str) -> Result<Option<Branch>, StoreError> {
        let conn = self.conn.lock().unwrap();
        Self::row_to_branch_query(&conn, "SELECT id, project_id, branch_name, base_branch, pr_number, created_at, updated_at FROM branches WHERE id = ?1", params![id])
    }

    pub fn list_branches_for_project(&self, project_id: &str) -> Result<Vec<Branch>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, branch_name, base_branch, pr_number, created_at, updated_at FROM branches WHERE project_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![project_id], Self::row_to_branch)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_branch_base(&self, id: &str, base_branch: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE branches SET base_branch = ?1, updated_at = ?2 WHERE id = ?3",
            params![base_branch, now_timestamp(), id],
        )?;
        Ok(())
    }

    pub fn delete_branch(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM branches WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn row_to_branch(row: &rusqlite::Row) -> rusqlite::Result<Branch> {
        let pr_number: Option<i64> = row.get(4)?;
        Ok(Branch {
            id: row.get(0)?,
            project_id: row.get(1)?,
            branch_name: row.get(2)?,
            base_branch: row.get(3)?,
            pr_number: pr_number.map(|n| n as u64),
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    }

    fn row_to_branch_query(
        conn: &Connection,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Option<Branch>, StoreError> {
        conn.query_row(sql, params, Self::row_to_branch)
            .optional()
            .map_err(Into::into)
    }
}
