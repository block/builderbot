//! Session CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::{Session, SessionStatus};
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_session(&self, session: &Session) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions (id, prompt, status, working_dir, provider, agent_id, error_message, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session.id,
                session.prompt,
                session.status.as_str(),
                session.working_dir,
                session.provider,
                session.agent_id,
                session.error_message,
                session.created_at,
                session.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, prompt, status, working_dir, provider, agent_id, error_message, created_at, updated_at
             FROM sessions WHERE id = ?1",
            params![id],
            Self::row_to_session,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Update session status and optionally set an error message.
    /// The error_message is only written when status is Error.
    pub fn update_session_status(
        &self,
        id: &str,
        status: SessionStatus,
        error_message: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let error_msg = if status == SessionStatus::Error {
            error_message
        } else {
            None
        };
        conn.execute(
            "UPDATE sessions SET status = ?1, error_message = ?2, updated_at = ?3 WHERE id = ?4",
            params![status.as_str(), error_msg, now_timestamp(), id],
        )?;
        Ok(())
    }

    /// Transition session status only if it is currently `running`.
    ///
    /// Returns `true` if the row was updated, `false` if the session was
    /// already in a terminal state (cancelled, completed, error) or didn't
    /// exist. This is the safe path for background threads — it prevents
    /// a late-arriving "completed" write from overwriting a "cancelled"
    /// status that was set by a concurrent cancel request.
    pub fn transition_from_running(
        &self,
        id: &str,
        new_status: SessionStatus,
        error_message: Option<&str>,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let error_msg = if new_status == SessionStatus::Error {
            error_message
        } else {
            None
        };
        let rows = conn.execute(
            "UPDATE sessions SET status = ?1, error_message = ?2, updated_at = ?3
             WHERE id = ?4 AND status = 'running'",
            params![new_status.as_str(), error_msg, now_timestamp(), id],
        )?;
        Ok(rows > 0)
    }

    /// Atomically transition a session to `Running`, but only if it is NOT
    /// already running. Returns `true` if the row was updated, `false` if
    /// the session was already running (or didn't exist).
    ///
    /// This is the safe entry point for `resume_session` — it prevents two
    /// concurrent resume calls from both succeeding.
    pub fn transition_to_running(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE sessions SET status = 'running', error_message = NULL, updated_at = ?1
             WHERE id = ?2 AND status != 'running'",
            params![now_timestamp(), id],
        )?;
        Ok(rows > 0)
    }

    /// Mark all running sessions as cancelled (used on app startup to clean
    /// up sessions that were interrupted by the previous app close).
    pub fn cancel_orphaned_sessions(&self) -> Result<u64, StoreError> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "UPDATE sessions SET status = 'cancelled', updated_at = ?1 WHERE status = 'running'",
            params![now_timestamp()],
        )?;
        Ok(count as u64)
    }

    /// Store the ACP session ID returned by the agent after `new_session`.
    /// This is used by `load_session` to resume the conversation on follow-up turns.
    pub fn set_agent_session_id(&self, id: &str, agent_session_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET agent_id = ?1, updated_at = ?2 WHERE id = ?3",
            params![agent_session_id, now_timestamp(), id],
        )?;
        Ok(())
    }

    pub fn delete_session(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
        let status_str: String = row.get(2)?;
        Ok(Session {
            id: row.get(0)?,
            prompt: row.get(1)?,
            status: SessionStatus::parse(&status_str).unwrap_or(SessionStatus::Error),
            working_dir: row.get(3)?,
            provider: row.get(4)?,
            agent_id: row.get(5)?,
            error_message: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    }
}
