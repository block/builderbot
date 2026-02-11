//! Project action CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::{ActionType, ProjectAction};
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_project_action(&self, action: &ProjectAction) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO project_actions (id, project_id, name, command, action_type, sort_order, auto_commit, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                action.id,
                action.project_id,
                action.name,
                action.command,
                action.action_type.as_str(),
                action.sort_order,
                action.auto_commit as i32,
                action.created_at,
                action.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_project_action(&self, id: &str) -> Result<Option<ProjectAction>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, name, command, action_type, sort_order, auto_commit, created_at, updated_at
             FROM project_actions WHERE id = ?1",
            params![id],
            Self::row_to_action,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_project_actions(&self, project_id: &str) -> Result<Vec<ProjectAction>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, name, command, action_type, sort_order, auto_commit, created_at, updated_at
             FROM project_actions WHERE project_id = ?1 ORDER BY sort_order ASC",
        )?;
        let rows = stmt.query_map(params![project_id], Self::row_to_action)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_project_action(&self, action: &ProjectAction) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE project_actions SET name = ?1, command = ?2, action_type = ?3, sort_order = ?4, auto_commit = ?5, updated_at = ?6 WHERE id = ?7",
            params![
                action.name,
                action.command,
                action.action_type.as_str(),
                action.sort_order,
                action.auto_commit as i32,
                now_timestamp(),
                action.id,
            ],
        )?;
        Ok(())
    }

    pub fn delete_project_action(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM project_actions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn reorder_project_actions(&self, action_ids: &[String]) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        for (i, id) in action_ids.iter().enumerate() {
            conn.execute(
                "UPDATE project_actions SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
                params![i as i32, now, id],
            )?;
        }
        Ok(())
    }

    fn row_to_action(row: &rusqlite::Row) -> rusqlite::Result<ProjectAction> {
        let action_type_str: String = row.get(4)?;
        let auto_commit: i32 = row.get(6)?;
        Ok(ProjectAction {
            id: row.get(0)?,
            project_id: row.get(1)?,
            name: row.get(2)?,
            command: row.get(3)?,
            action_type: ActionType::parse(&action_type_str).unwrap_or(ActionType::Run),
            sort_order: row.get(5)?,
            auto_commit: auto_commit != 0,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    }
}
