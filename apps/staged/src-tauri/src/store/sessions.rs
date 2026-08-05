//! Session CRUD operations.

use rusqlite::{params, OptionalExtension};

use super::models::{
    AcpConfigSelection, CompletionReason, PipelineExecution, Session, SessionStatus,
};
use super::{now_timestamp, Store, StoreError};

impl Store {
    pub fn create_session(&self, session: &Session) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let pipeline_json = session
            .pipeline
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StoreError(format!("Failed to serialize pipeline: {e}")))?;
        let acp_config_selection_json =
            serialize_acp_config_selection(session.acp_config_selection.as_ref())?;
        conn.execute(
            "INSERT INTO sessions (id, prompt, status, working_dir, provider, agent_id, error_message, completion_reason, created_at, updated_at, owner_pid, pipeline, acp_config_selection, acp_title)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                pipeline_json,
                acp_config_selection_json,
                session.acp_title,
            ],
        )?;
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, prompt, status, working_dir, provider, agent_id, error_message, completion_reason, created_at, updated_at, owner_pid, pipeline, acp_config_selection, acp_title
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
    ///
    /// Unlike the other status writers, `error_message` is persisted for
    /// `Cancelled` as well as `Error`: a run cancelled from outside (e.g. a
    /// Pikchr sub-session hitting its tool-call timeout) can carry an
    /// explanation of why it ended. Other statuses clear the message.
    pub fn transition_from_running(
        &self,
        id: &str,
        new_status: SessionStatus,
        error_message: Option<&str>,
        completion_reason: Option<&CompletionReason>,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let error_msg = if matches!(new_status, SessionStatus::Error | SessionStatus::Cancelled) {
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

    /// Atomically claim a queued session for execution by this process.
    ///
    /// Returns `true` if the row was updated, `false` if the session was no
    /// longer queued, for example because it was cancelled after the queue was
    /// snapshotted.
    pub fn transition_queued_to_running(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE sessions SET status = 'running', error_message = NULL, completion_reason = NULL, updated_at = ?1, owner_pid = ?2
             WHERE id = ?3 AND status = 'queued'",
            params![now_timestamp(), std::process::id(), id],
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

    /// Restamp the queued artifact stub linked to a session when it starts running.
    ///
    /// `sessions.created_at` is intentionally left unchanged so branch queues
    /// still drain FIFO by enqueue time. Empty failed artifacts use their own
    /// `created_at` as the timeline fallback, so this records when queued work
    /// was actually claimed.
    pub fn mark_session_artifact_started(&self, session_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        conn.execute(
            "UPDATE commits
             SET created_at = ?1, updated_at = ?1
             WHERE session_id = ?2 AND sha IS NULL",
            params![now, session_id],
        )?;
        conn.execute(
            "UPDATE notes
             SET created_at = ?1, updated_at = ?1
             WHERE session_id = ?2 AND completed_at IS NULL AND content = ''",
            params![now, session_id],
        )?;
        conn.execute(
            "UPDATE reviews
             SET created_at = ?1, updated_at = ?1
             WHERE session_id = ?2 AND completed_at IS NULL",
            params![now, session_id],
        )?;
        Ok(())
    }

    pub fn get_running_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, prompt, status, working_dir, provider, agent_id, error_message, completion_reason, created_at, updated_at, owner_pid, pipeline, acp_config_selection, acp_title
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

    /// Look up just the provider for a session (lightweight single-column query).
    pub fn get_session_provider(&self, id: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT provider FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map(|opt| opt.flatten())
        .map_err(Into::into)
    }

    /// Backfill the provider on a session that was created without one.
    ///
    /// This is called by `start_session` when the caller didn't specify a
    /// provider and the system resolved one before launching the local agent.
    pub fn set_session_provider(&self, id: &str, provider: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET provider = ?1, updated_at = ?2 WHERE id = ?3",
            params![provider, now_timestamp(), id],
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
            "SELECT s.id, s.prompt, s.status, s.working_dir, s.provider, s.agent_id, s.error_message, s.completion_reason, s.created_at, s.updated_at, s.owner_pid, s.pipeline, s.acp_config_selection, s.acp_title
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
    ///
    /// Auto-reviews (`is_auto = 1`) are excluded because they run in the
    /// background and should never block user-initiated sessions.
    pub fn has_running_session_for_branch(&self, branch_id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions s
             WHERE s.status = 'running'
               AND (
                   EXISTS (SELECT 1 FROM commits c WHERE c.session_id = s.id AND c.branch_id = ?1)
                   OR EXISTS (SELECT 1 FROM notes n WHERE n.session_id = s.id AND n.branch_id = ?1)
                   OR EXISTS (SELECT 1 FROM reviews r WHERE r.session_id = s.id AND r.branch_id = ?1 AND r.is_auto = 0)
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

    /// Resolve the project that owns a session.
    ///
    /// Checks project_notes first (project-note sessions), then falls back to
    /// resolving via the branch (branch-level sessions have their branch linked
    /// to a project). Used by the recovery path (`recover_orphaned_sessions`)
    /// which has no caller context to pipe through.
    pub fn get_project_id_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let direct: Option<String> = conn
            .query_row(
                "SELECT project_id FROM project_notes WHERE session_id = ?1 LIMIT 1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?;
        if direct.is_some() {
            return Ok(direct);
        }
        conn.query_row(
            "SELECT b.project_id FROM branches b
             INNER JOIN (
                 SELECT branch_id FROM commits WHERE session_id = ?1
                 UNION ALL
                 SELECT branch_id FROM notes WHERE session_id = ?1
                 UNION ALL
                 SELECT branch_id FROM reviews WHERE session_id = ?1
             ) a ON a.branch_id = b.id
             LIMIT 1",
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

    /// Update the selected ACP config values for a session.
    pub fn set_session_acp_config_selection(
        &self,
        id: &str,
        selection: Option<&AcpConfigSelection>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let json = serialize_acp_config_selection(selection)?;
        conn.execute(
            "UPDATE sessions SET acp_config_selection = ?1, updated_at = ?2 WHERE id = ?3",
            params![json, now_timestamp(), id],
        )?;
        Ok(())
    }

    /// Update the ACP-provided session title.
    ///
    /// `None` clears the column (the agent explicitly retracted the title).
    pub fn update_session_acp_title(
        &self,
        id: &str,
        title: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET acp_title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title, now_timestamp(), id],
        )?;
        Ok(())
    }

    /// Update the pipeline execution state for a session.
    pub fn update_session_pipeline(
        &self,
        id: &str,
        pipeline: &PipelineExecution,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let json = serde_json::to_string(pipeline)
            .map_err(|e| StoreError(format!("Failed to serialize pipeline: {e}")))?;
        conn.execute(
            "UPDATE sessions SET pipeline = ?1, updated_at = ?2 WHERE id = ?3",
            params![json, now_timestamp(), id],
        )?;
        Ok(())
    }

    fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
        let status_str: String = row.get(2)?;
        let reason_str: Option<String> = row.get(7)?;
        let pipeline_json: Option<String> = row.get(11)?;
        let acp_config_selection_json: Option<String> = row.get(12)?;
        let pipeline = pipeline_json.as_deref().and_then(|s| {
            serde_json::from_str(s)
                .map_err(|e| log::warn!("Failed to deserialize pipeline JSON: {e}"))
                .ok()
        });
        let acp_config_selection = acp_config_selection_json.as_deref().and_then(|s| {
            serde_json::from_str(s)
                .map_err(|e| log::warn!("Failed to deserialize ACP config selection JSON: {e}"))
                .ok()
        });
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
            pipeline,
            acp_config_selection,
            acp_title: row.get(13)?,
        })
    }
}

fn serialize_acp_config_selection(
    selection: Option<&AcpConfigSelection>,
) -> Result<Option<String>, StoreError> {
    selection
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| StoreError(format!("Failed to serialize ACP config selection: {e}")))
}
