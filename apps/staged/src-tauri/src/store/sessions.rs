//! Session CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::{CompletionReason, Session, SessionStatus};
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_session(&self, session: &Session) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions (id, prompt, status, working_dir, provider, agent_id, error_message, completion_reason, created_at, updated_at, owner_pid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                session.id,
                session.prompt,
                session.status.as_str(),
                session.working_dir,
                session.provider,
                session.agent_id,
                session.error_message,
                session.completion_reason.as_ref().map(|r| r.as_str()),
                session.created_at,
                session.updated_at,
                session.owner_pid,
            ],
        )?;
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, prompt, status, working_dir, provider, agent_id, error_message, completion_reason, created_at, updated_at, owner_pid
             FROM sessions WHERE id = ?1",
            params![id],
            Self::row_to_session,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Update session status and optionally set an error message and completion reason.
    /// The error_message is only written when status is Error.
    pub fn update_session_status(
        &self,
        id: &str,
        status: SessionStatus,
        error_message: Option<&str>,
        completion_reason: Option<&CompletionReason>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let error_msg = if status == SessionStatus::Error {
            error_message
        } else {
            None
        };
        conn.execute(
            "UPDATE sessions SET status = ?1, error_message = ?2, completion_reason = ?3, updated_at = ?4 WHERE id = ?5",
            params![status.as_str(), error_msg, completion_reason.map(|r| r.as_str()), now_timestamp(), id],
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
        completion_reason: Option<&CompletionReason>,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let error_msg = if new_status == SessionStatus::Error {
            error_message
        } else {
            None
        };
        let rows = conn.execute(
            "UPDATE sessions SET status = ?1, error_message = ?2, completion_reason = ?3, updated_at = ?4
             WHERE id = ?5 AND status = 'running'",
            params![new_status.as_str(), error_msg, completion_reason.map(|r| r.as_str()), now_timestamp(), id],
        )?;
        Ok(rows > 0)
    }

    /// Transition session status only if it is currently `queued` or `running`.
    ///
    /// Returns `true` if the row was updated, `false` if the session already
    /// moved to another state or didn't exist. This is the safe path for
    /// cancelling work that may still be in the queue.
    pub fn transition_from_active(
        &self,
        id: &str,
        new_status: SessionStatus,
        error_message: Option<&str>,
        completion_reason: Option<&CompletionReason>,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let error_msg = if new_status == SessionStatus::Error {
            error_message
        } else {
            None
        };
        let rows = conn.execute(
            "UPDATE sessions SET status = ?1, error_message = ?2, completion_reason = ?3, updated_at = ?4
             WHERE id = ?5 AND status IN ('queued', 'running')",
            params![new_status.as_str(), error_msg, completion_reason.map(|r| r.as_str()), now_timestamp(), id],
        )?;
        Ok(rows > 0)
    }

    /// Atomically transition a session from `Queued` to the given status.
    ///
    /// Returns `true` if the row was updated, `false` if the session was
    /// no longer queued (e.g. it was already picked up by the drain loop).
    pub fn transition_from_queued(
        &self,
        id: &str,
        new_status: SessionStatus,
        error_message: Option<&str>,
        completion_reason: Option<&CompletionReason>,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let error_msg = if new_status == SessionStatus::Error {
            error_message
        } else {
            None
        };
        let rows = conn.execute(
            "UPDATE sessions SET status = ?1, error_message = ?2, completion_reason = ?3, updated_at = ?4
             WHERE id = ?5 AND status = 'queued'",
            params![new_status.as_str(), error_msg, completion_reason.map(|r| r.as_str()), now_timestamp(), id],
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
            "UPDATE sessions SET status = 'running', error_message = NULL, completion_reason = NULL, updated_at = ?1, owner_pid = ?2
             WHERE id = ?3 AND status != 'running'",
            params![now_timestamp(), std::process::id(), id],
        )?;
        Ok(rows > 0)
    }

    pub fn get_running_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, prompt, status, working_dir, provider, agent_id, error_message, completion_reason, created_at, updated_at, owner_pid
             FROM sessions WHERE status = 'running'",
        )?;
        let sessions = stmt
            .query_map([], Self::row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
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

    /// Update a queued session's working directory, prompt, and owner PID
    /// when it is being drained (started for real).
    pub fn prepare_queued_session(
        &self,
        id: &str,
        working_dir: &str,
        prompt: &str,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET working_dir = ?1, prompt = ?2, owner_pid = ?3, updated_at = ?4 WHERE id = ?5",
            params![working_dir, prompt, std::process::id(), now_timestamp(), id],
        )?;
        Ok(())
    }

    /// Get all queued sessions for a branch, ordered by creation time (oldest first).
    ///
    /// Sessions are linked to branches through artifacts (commits, notes, reviews).
    /// This query joins across all three artifact tables to find every queued session
    /// belonging to the given branch.
    pub fn get_queued_sessions_for_branch(
        &self,
        branch_id: &str,
    ) -> Result<Vec<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.prompt, s.status, s.working_dir, s.provider, s.agent_id, s.error_message, s.completion_reason, s.created_at, s.updated_at, s.owner_pid
             FROM sessions s
             WHERE s.status = 'queued'
               AND (
                   EXISTS (SELECT 1 FROM commits c WHERE c.session_id = s.id AND c.branch_id = ?1)
                   OR EXISTS (SELECT 1 FROM notes n WHERE n.session_id = s.id AND n.branch_id = ?1)
                   OR EXISTS (SELECT 1 FROM reviews r WHERE r.session_id = s.id AND r.branch_id = ?1)
               )
             ORDER BY s.created_at ASC",
        )?;
        let sessions = stmt
            .query_map(params![branch_id], Self::row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    /// Check whether a branch already has a running session.
    pub fn has_running_session_for_branch(&self, branch_id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions s
             WHERE s.status = 'running'
               AND (
                   EXISTS (SELECT 1 FROM commits c WHERE c.session_id = s.id AND c.branch_id = ?1)
                   OR EXISTS (SELECT 1 FROM notes n WHERE n.session_id = s.id AND n.branch_id = ?1)
                   OR EXISTS (SELECT 1 FROM reviews r WHERE r.session_id = s.id AND r.branch_id = ?1)
               )",
            params![branch_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Resolve the branch that owns a session through its linked artifact.
    ///
    /// Project-note sessions do not belong to a branch and therefore return `None`.
    /// This assumes all branch-linked artifacts for a session point at the same
    /// branch; if a session somehow links artifacts across multiple branches,
    /// the first row returned by SQLite wins.
    pub fn get_branch_id_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT branch_id FROM (
                 SELECT branch_id FROM commits WHERE session_id = ?1
                 UNION ALL
                 SELECT branch_id FROM notes WHERE session_id = ?1
                 UNION ALL
                 SELECT branch_id FROM reviews WHERE session_id = ?1
             ) LIMIT 1",
            params![session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn delete_session(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
        let status_str: String = row.get(2)?;
        let reason_str: Option<String> = row.get(7)?;
        Ok(Session {
            id: row.get(0)?,
            prompt: row.get(1)?,
            status: SessionStatus::parse(&status_str).unwrap_or(SessionStatus::Error),
            working_dir: row.get(3)?,
            provider: row.get(4)?,
            agent_id: row.get(5)?,
            error_message: row.get(6)?,
            completion_reason: reason_str.as_deref().and_then(CompletionReason::parse),
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
            owner_pid: row.get(10)?,
        })
    }
}
