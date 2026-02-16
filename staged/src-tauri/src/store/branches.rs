//! Branch CRUD operations.

use rusqlite::{params, Connection, OptionalExtension};

use super::models::{Branch, BranchType, WorkspaceStatus};
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_branch(&self, branch: &Branch) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO branches (id, project_id, project_repo_id, branch_name, base_branch, pr_number,
                branch_type, workspace_name, workspace_status,
                pr_state, pr_checks_status, pr_review_decision, pr_mergeable, pr_draft,
                pr_url, pr_updated_at, pr_fetched_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                branch.id,
                branch.project_id,
                branch.project_repo_id,
                branch.branch_name,
                branch.base_branch,
                branch.pr_number.map(|n| n as i64),
                branch.branch_type.as_str(),
                branch.workspace_name,
                branch.workspace_status.as_ref().map(|s| s.as_str()),
                branch.pr_state,
                branch.pr_checks_status,
                branch.pr_review_decision,
                branch.pr_mergeable.map(|b| if b { 1 } else { 0 }),
                branch.pr_draft.map(|b| if b { 1 } else { 0 }),
                branch.pr_url,
                branch.pr_updated_at,
                branch.pr_fetched_at,
                branch.created_at,
                branch.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_branch(&self, id: &str) -> Result<Option<Branch>, StoreError> {
        let conn = self.conn.lock().unwrap();
        Self::row_to_branch_query(
            &conn,
            "SELECT id, project_id, project_repo_id, branch_name, base_branch, pr_number,
                    branch_type, workspace_name, workspace_status,
                    pr_state, pr_checks_status, pr_review_decision, pr_mergeable, pr_draft,
                    pr_url, pr_updated_at, pr_fetched_at,
                    created_at, updated_at
             FROM branches WHERE id = ?1",
            params![id],
        )
    }

    pub fn list_branches_for_project(&self, project_id: &str) -> Result<Vec<Branch>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, project_repo_id, branch_name, base_branch, pr_number,
                    branch_type, workspace_name, workspace_status,
                    pr_state, pr_checks_status, pr_review_decision, pr_mergeable, pr_draft,
                    pr_url, pr_updated_at, pr_fetched_at,
                    created_at, updated_at
             FROM branches WHERE project_id = ?1 ORDER BY created_at ASC",
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

    pub fn update_branch_name(&self, id: &str, branch_name: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE branches SET branch_name = ?1, updated_at = ?2 WHERE id = ?3",
            params![branch_name, now_timestamp(), id],
        )?;
        Ok(())
    }

    /// Update the workspace status for a remote branch.
    pub fn update_branch_workspace_status(
        &self,
        id: &str,
        status: &WorkspaceStatus,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE branches SET workspace_status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), now_timestamp(), id],
        )?;
        Ok(())
    }

    /// Update the PR number for a branch.
    pub fn update_branch_pr_number(
        &self,
        id: &str,
        pr_number: Option<u64>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE branches SET pr_number = ?1, updated_at = ?2 WHERE id = ?3",
            params![pr_number.map(|n| n as i64), now_timestamp(), id],
        )?;
        Ok(())
    }

    /// Update PR status fields for a branch.
    pub fn update_branch_pr_status(
        &self,
        id: &str,
        pr_state: Option<String>,
        pr_checks_status: Option<String>,
        pr_review_decision: Option<String>,
        pr_mergeable: Option<bool>,
        pr_draft: Option<bool>,
        pr_url: Option<String>,
        pr_updated_at: Option<i64>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        conn.execute(
            "UPDATE branches SET
                pr_state = ?1,
                pr_checks_status = ?2,
                pr_review_decision = ?3,
                pr_mergeable = ?4,
                pr_draft = ?5,
                pr_url = ?6,
                pr_updated_at = ?7,
                pr_fetched_at = ?8,
                updated_at = ?9
             WHERE id = ?10",
            params![
                pr_state,
                pr_checks_status,
                pr_review_decision,
                pr_mergeable.map(|b| if b { 1 } else { 0 }),
                pr_draft.map(|b| if b { 1 } else { 0 }),
                pr_url,
                pr_updated_at,
                now,
                now,
                id
            ],
        )?;
        Ok(())
    }

    pub fn delete_branch(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM branches WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn row_to_branch(row: &rusqlite::Row) -> rusqlite::Result<Branch> {
        let pr_number: Option<i64> = row.get(5)?;
        let branch_type_str: String = row.get(6)?;
        let workspace_status_str: Option<String> = row.get(8)?;
        let pr_mergeable: Option<i64> = row.get(12)?;
        let pr_draft: Option<i64> = row.get(13)?;
        Ok(Branch {
            id: row.get(0)?,
            project_id: row.get(1)?,
            project_repo_id: row.get(2)?,
            branch_name: row.get(3)?,
            base_branch: row.get(4)?,
            pr_number: pr_number.map(|n| n as u64),
            branch_type: branch_type_str.parse().unwrap_or(BranchType::Local),
            workspace_name: row.get(7)?,
            workspace_status: workspace_status_str.and_then(|s| s.parse().ok()),
            pr_state: row.get(9)?,
            pr_checks_status: row.get(10)?,
            pr_review_decision: row.get(11)?,
            pr_mergeable: pr_mergeable.map(|b| b != 0),
            pr_draft: pr_draft.map(|b| b != 0),
            pr_url: row.get(14)?,
            pr_updated_at: row.get(15)?,
            pr_fetched_at: row.get(16)?,
            created_at: row.get(17)?,
            updated_at: row.get(18)?,
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
