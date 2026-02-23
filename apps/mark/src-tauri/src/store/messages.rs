//! Session message CRUD operations.

use rusqlite::params;

use super::models::{MessageRole, SessionMessage};
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn add_session_message(
        &self,
        session_id: &str,
        role: MessageRole,
        content: &str,
    ) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO session_messages (session_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![session_id, role.as_str(), content, now_timestamp()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_session_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionMessage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, role, content, created_at
             FROM session_messages WHERE session_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            let role_str: String = row.get(2)?;
            Ok(SessionMessage {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: MessageRole::parse(&role_str).unwrap_or(MessageRole::User),
                content: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Update the content of an existing message (used for streaming updates).
    pub fn update_message_content(&self, id: i64, content: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE session_messages SET content = ?1 WHERE id = ?2",
            params![content, id],
        )?;
        Ok(())
    }

    /// Get messages with id >= since_id (inclusive — re-fetches the last known
    /// message so the caller picks up streaming content updates).
    pub fn get_session_messages_since(
        &self,
        session_id: &str,
        since_id: i64,
    ) -> Result<Vec<SessionMessage>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, role, content, created_at
             FROM session_messages WHERE session_id = ?1 AND id >= ?2 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![session_id, since_id], |row| {
            let role_str: String = row.get(2)?;
            Ok(SessionMessage {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: MessageRole::parse(&role_str).unwrap_or(MessageRole::User),
                content: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}
