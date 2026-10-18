//! Session CRUD operations.

use rusqlite::{params, Connection, OptionalExtension};

use super::messages::VISIBLE_ASSISTANT_FILTER;
use super::models::{
    AcpConfigSelection, CompletionReason, PipelineExecution, Session, SessionStatus,
};
use super::{now_timestamp, Store, StoreChange, StoreError};

/// The `sessions` columns [`Store::row_to_session`] reads, in its order.
/// Every query that maps rows through it selects this list, so a new column
/// is added here once rather than in each query.
const SESSION_COLUMNS: &str = "id, prompt, status, working_dir, provider, agent_id,
    error_message, completion_reason, provider_metadata, rate_limit_resets_at, rate_limit_kind,
    created_at, updated_at, owner_pid, pipeline, acp_config_selection, acp_title, branch_id,
    completion_effects_at, parent_session_id, pre_head_sha, blox_command_id, started_by";

/// An in-flight session spawned by another session: the branch it works on
/// and the title the branch timeline would give it. `status` is `"queued"` or
/// `"running"`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionInfo {
    pub session_id: String,
    /// One of `"commit"`, `"note"`, `"review"`.
    pub session_type: String,
    /// `"queued"` or `"running"` — the only two statuses the query returns.
    pub status: String,
    /// The child's title as the branch timeline would show it, resolved by
    /// the same rules ([`super::pending_commit_subject`],
    /// [`super::review_title`], a note's stored title): the start of the prompt
    /// until the agent pushes an ACP title. `None` only for a review with no
    /// stored title and no ACP title yet.
    pub title: Option<String>,
    pub branch_id: String,
}

impl Store {
    pub fn create_session(&self, session: &Session) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let provider_metadata_json = session
            .provider_metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StoreError(format!("Failed to serialize provider metadata: {e}")))?;
        let pipeline_json = session
            .pipeline
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StoreError(format!("Failed to serialize pipeline: {e}")))?;
        let acp_config_selection_json =
            serialize_acp_config_selection(session.acp_config_selection.as_ref())?;
        conn.execute(
            "INSERT INTO sessions (id, prompt, status, working_dir, provider, agent_id, error_message, completion_reason, provider_metadata, rate_limit_resets_at, rate_limit_kind, created_at, updated_at, owner_pid, pipeline, acp_config_selection, acp_title, branch_id, completion_effects_at, parent_session_id, pre_head_sha, blox_command_id, started_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            params![
                session.id,
                session.prompt,
                session.status.as_str(),
                session.working_dir,
                session.provider,
                session.agent_id,
                session.error_message,
                session.completion_reason.as_ref().map(|r| r.as_str()),
                provider_metadata_json,
                session.rate_limit_resets_at,
                session.rate_limit_kind,
                session.created_at,
                session.updated_at,
                session.owner_pid,
                pipeline_json,
                acp_config_selection_json,
                session.acp_title,
                session.branch_id,
                session.completion_effects_at,
                session.parent_session_id,
                session.pre_head_sha,
                session.blox_command_id,
                session.started_by,
            ],
        )?;
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        Self::get_session_with_conn(&conn, id)
    }

    /// [`Self::get_session`] on a connection the caller already holds, so
    /// `poll_session` can read it under the same lock as the transcript.
    pub(super) fn get_session_with_conn(
        conn: &Connection,
        id: &str,
    ) -> Result<Option<Session>, StoreError> {
        conn.query_row(
            &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id = ?1"),
            params![id],
            Self::row_to_session,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Update session status and optionally set an error message and completion reason.
    /// The error_message is only written when status is Error.
    ///
    /// Every status writer here clears `owner_pid` when the new status is
    /// `Disconnected`: a disconnected session has no Staged process behind it,
    /// so no instance's orphan recovery may claim it as its own.
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
            "UPDATE sessions SET status = ?1, error_message = ?2, completion_reason = ?3, rate_limit_resets_at = NULL, rate_limit_kind = NULL, updated_at = ?4,
                 owner_pid = CASE WHEN ?1 = 'disconnected' THEN NULL ELSE owner_pid END
             WHERE id = ?5",
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
    ///
    /// Completing also drops the `staged.rateLimitAutoRecovery` fragment: a
    /// successful completion ends the rate-limit episode, and bookkeeping that
    /// survived the auto-resume claim would otherwise accumulate `attempt` and
    /// keep `first_limited_at` across separate limit hits — an unattended
    /// multi-window run would give up on the retry cap or the 24h window even
    /// though every intervening resume made real progress. Manual resumes
    /// already reset it (`transition_to_running` wipes provider metadata);
    /// this makes the auto-resume path match.
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
        let provider_metadata_expr = if new_status == SessionStatus::Completed {
            "CASE WHEN json_valid(provider_metadata) THEN json_remove(provider_metadata, '$.staged.rateLimitAutoRecovery') ELSE provider_metadata END"
        } else {
            "provider_metadata"
        };
        let rows = conn.execute(
            &format!(
                "UPDATE sessions SET status = ?1, error_message = ?2, completion_reason = ?3, provider_metadata = {provider_metadata_expr}, rate_limit_resets_at = NULL, rate_limit_kind = NULL, updated_at = ?4,
                     owner_pid = CASE WHEN ?1 = 'disconnected' THEN NULL ELSE owner_pid END
                 WHERE id = ?5 AND status = 'running'"
            ),
            params![new_status.as_str(), error_msg, completion_reason.map(|r| r.as_str()), now_timestamp(), id],
        )?;
        Ok(rows > 0)
    }

    /// [`Self::transition_from_running`] for exactly the `running` row the
    /// caller read: the write also requires `updated_at` to still be
    /// `observed_updated_at`.
    ///
    /// Every write that moves a row into or out of `running` restamps
    /// `updated_at`, so a row that finished and was resumed between the
    /// caller's read and this write — the same id, `running` again, but a new
    /// turn — fails the predicate and is left alone. That is what lets the
    /// stranded-session sweep act on a snapshot: it ends the incarnation it
    /// saw, never the one that replaced it.
    pub fn transition_from_running_observed_at(
        &self,
        id: &str,
        observed_updated_at: i64,
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
            "UPDATE sessions SET status = ?1, error_message = ?2, completion_reason = ?3, rate_limit_resets_at = NULL, rate_limit_kind = NULL, updated_at = ?4,
                 owner_pid = CASE WHEN ?1 = 'disconnected' THEN NULL ELSE owner_pid END
             WHERE id = ?5 AND status = 'running' AND updated_at = ?6",
            params![new_status.as_str(), error_msg, completion_reason.map(|r| r.as_str()), now_timestamp(), id, observed_updated_at],
        )?;
        Ok(rows > 0)
    }

    /// Pause a running session after the provider reports a retryable rate or usage limit.
    pub fn transition_running_to_rate_limited(
        &self,
        id: &str,
        error_message: Option<&str>,
        rate_limit_resets_at: Option<&str>,
        rate_limit_kind: Option<&str>,
    ) -> Result<bool, StoreError> {
        self.transition_running_to_rate_limited_with_provider_metadata(
            id,
            error_message,
            rate_limit_resets_at,
            rate_limit_kind,
            None,
        )
    }

    /// [`transition_running_to_rate_limited`](Self::transition_running_to_rate_limited)
    /// that also merges `provider_metadata` into the row in the same guarded
    /// write.
    ///
    /// The rate-limit scheduler lists `paused` rows and builds retry
    /// bookkeeping for any that carry none, so the pause path has to land the
    /// bookkeeping no later than the status flip: written afterwards, a tick in
    /// between sees a bare paused row, writes attempt 1 itself, and the pause
    /// path then bumps that to attempt 2. Merging here means the row is never
    /// observable as paused without its bookkeeping, and a lost race (a Stop
    /// that cancelled the row first) writes nothing at all.
    pub fn transition_running_to_rate_limited_with_provider_metadata(
        &self,
        id: &str,
        error_message: Option<&str>,
        rate_limit_resets_at: Option<&str>,
        rate_limit_kind: Option<&str>,
        provider_metadata: Option<&serde_json::Value>,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let merged_json = match provider_metadata {
            Some(patch) => {
                let existing_json: Option<String> = conn
                    .query_row(
                        "SELECT provider_metadata FROM sessions WHERE id = ?1",
                        params![id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten();
                let mut merged = existing_json
                    .as_deref()
                    .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                merge_json(&mut merged, patch.clone());
                Some(serde_json::to_string(&merged).map_err(|e| {
                    StoreError(format!("Failed to serialize provider metadata: {e}"))
                })?)
            }
            None => None,
        };
        let rows = conn.execute(
            "UPDATE sessions SET status = 'paused', error_message = ?1, completion_reason = 'rate_limited', rate_limit_resets_at = ?2, rate_limit_kind = ?3, provider_metadata = COALESCE(?4, provider_metadata), updated_at = ?5
             WHERE id = ?6 AND status = 'running'",
            params![error_message, rate_limit_resets_at, rate_limit_kind, merged_json, now_timestamp(), id],
        )?;
        Ok(rows > 0)
    }

    /// Transition session status only if the row is still active *and ours*:
    /// `queued`, or `running` under `owner_pid`.
    ///
    /// Returns `true` if the row was updated, `false` if the session already
    /// moved to another state, is running under a different pid, or didn't
    /// exist. This is the app-quit sweep's path, and the guard mirrors the
    /// filter that snapshot uses: the store is shared with any other Staged
    /// instance pointed at the same data dir, queued rows carry no owner and
    /// count as the caller's by convention, and running rows must carry the
    /// caller's pid.
    ///
    /// Checking ownership *here* rather than only in the snapshot is what closes
    /// the gap between the two. A claim landing in that gap
    /// (`transition_queued_to_running`) flips a queued row to `running` and
    /// stamps a pid atomically: another instance's claim now fails this
    /// predicate and its live work is left alone, while our own drain's claim
    /// still matches and is swept — right, because this process is exiting and
    /// taking the session with it.
    pub fn transition_from_owned_active(
        &self,
        id: &str,
        new_status: SessionStatus,
        error_message: Option<&str>,
        completion_reason: Option<&CompletionReason>,
        owner_pid: u32,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let error_msg = if new_status == SessionStatus::Error {
            error_message
        } else {
            None
        };
        let rows = conn.execute(
            "UPDATE sessions SET status = ?1, error_message = ?2, completion_reason = ?3, rate_limit_resets_at = NULL, rate_limit_kind = NULL, updated_at = ?4,
                 owner_pid = CASE WHEN ?1 = 'disconnected' THEN NULL ELSE owner_pid END
             WHERE id = ?5 AND (status = 'queued' OR (status = 'running' AND owner_pid = ?6))",
            params![new_status.as_str(), error_msg, completion_reason.map(|r| r.as_str()), now_timestamp(), id, owner_pid],
        )?;
        Ok(rows > 0)
    }

    /// Take over a `running` row whose owner process is gone, making this
    /// process its owner.
    ///
    /// Returns `true` if the row was taken, `false` if it is no longer
    /// `running` under `previous_owner` — it finished, or another instance
    /// sweeping the same data dir took it first.
    pub fn adopt_running_session(
        &self,
        id: &str,
        previous_owner: Option<u32>,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE sessions SET owner_pid = ?1
             WHERE id = ?2 AND status = 'running' AND owner_pid IS ?3",
            params![std::process::id(), id, previous_owner],
        )?;
        Ok(rows > 0)
    }

    /// Atomically transition a session from `Queued` to the given status.
    ///
    /// Returns `true` if the row was updated, `false` if the session was
    /// no longer queued (e.g. it was already picked up by the drain loop).
    ///
    /// Like [`Self::transition_from_running`], `error_message` is kept for
    /// `Cancelled` as well as `Error`: a queued row the branch drain skips
    /// (its dependency failed) records why it never ran.
    pub fn transition_from_queued(
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
            "UPDATE sessions SET status = ?1, error_message = ?2, completion_reason = ?3, rate_limit_resets_at = NULL, rate_limit_kind = NULL, updated_at = ?4
             WHERE id = ?5 AND status = 'queued'",
            params![new_status.as_str(), error_msg, completion_reason.map(|r| r.as_str()), now_timestamp(), id],
        )?;
        Ok(rows > 0)
    }

    /// Settle a `disconnected` row, if it is still exactly the row the
    /// caller read: `status = 'disconnected'` and `updated_at` still
    /// `observed_updated_at`.
    ///
    /// The reconcile service's write for an outcome it reached without
    /// reattaching (the workstation is gone, a Stop it could not deliver).
    /// Every other writer of a disconnected row (a Stop's fallback, a claim,
    /// a branch teardown) restamps `updated_at`, so a reconcile that lost the
    /// row to one of them fails this predicate and leaves that write alone,
    /// and a second reconcile of the same snapshot is a no-op.
    ///
    /// Like [`Self::transition_from_running`], `error_message` is kept for
    /// `Cancelled` as well as `Error`.
    pub fn transition_from_disconnected_observed_at(
        &self,
        id: &str,
        observed_updated_at: i64,
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
             WHERE id = ?5 AND status = 'disconnected' AND updated_at = ?6",
            params![new_status.as_str(), error_msg, completion_reason.map(|r| r.as_str()), now_timestamp(), id, observed_updated_at],
        )?;
        Ok(rows > 0)
    }

    /// Claim a `disconnected` row for this process, as `running`, to follow
    /// its remote turn: the same compare-and-swap on the row the caller read
    /// as [`Self::transition_from_disconnected_observed_at`], stamping
    /// `owner_pid` like every other move into `running`.
    ///
    /// The caller registers in the session registry *before* this, as every
    /// owner of a `running` row does, and from here on finishes the row with
    /// the ordinary `transition_from_running` writes.
    pub fn claim_disconnected_session(
        &self,
        id: &str,
        observed_updated_at: i64,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE sessions SET status = 'running', error_message = NULL, completion_reason = NULL, updated_at = ?1, owner_pid = ?2
             WHERE id = ?3 AND status = 'disconnected' AND updated_at = ?4",
            params![now_timestamp(), std::process::id(), id, observed_updated_at],
        )?;
        Ok(rows > 0)
    }

    /// Atomically claim a queued session for execution by this process.
    ///
    /// Returns `true` if the row was updated, `false` if the session was no
    /// longer queued, for example because it was cancelled after the queue was
    /// snapshotted. Like every new turn, it forgets a pinned Blox process
    /// (see [`Self::transition_to_running_clearing_agent_id`]).
    pub fn transition_queued_to_running(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE sessions SET status = 'running', error_message = NULL, completion_reason = NULL, provider_metadata = NULL, rate_limit_resets_at = NULL, rate_limit_kind = NULL, updated_at = ?1, owner_pid = ?2, blox_command_id = NULL
             WHERE id = ?3 AND status = 'queued'",
            params![now_timestamp(), std::process::id(), id],
        )?;
        Ok(rows > 0)
    }

    /// Atomically transition a session to `Running`, but only if it is NOT
    /// already running. Returns `true` if the row was updated, `false` if
    /// the session was already running (or didn't exist). A `disconnected`
    /// row counts as running — its remote turn may still be going — so it is
    /// refused too.
    ///
    /// This is the safe entry point for `resume_session` — it prevents two
    /// concurrent resume calls from both succeeding.
    pub fn transition_to_running(&self, id: &str) -> Result<bool, StoreError> {
        self.transition_to_running_clearing_agent_id(id, None)
    }

    /// Atomically transition a session to `Running`, clearing a caller-owned
    /// temporary `agent_id` marker at the same time.
    ///
    /// A new turn also forgets the Blox process the last one was pinned to
    /// (`blox_command_id`, see [`Self::set_session_blox_command_id`]): it may
    /// run on a different process, and a pin to the old one would make a
    /// later reconcile read that process instead.
    ///
    /// Real ACP session ids are preserved. The optional clear is for rows that
    /// were visible as `running` before an ACP session existed; if a user stops
    /// and resumes such a row before the original startup finishes, clearing
    /// that marker makes the original startup's later compare-and-swap fail.
    pub fn transition_to_running_clearing_agent_id(
        &self,
        id: &str,
        agent_id_to_clear: Option<&str>,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE sessions
             SET status = 'running',
                 error_message = NULL,
                 completion_reason = NULL,
                 provider_metadata = NULL,
                 rate_limit_resets_at = NULL,
                 rate_limit_kind = NULL,
                 updated_at = ?1,
                 owner_pid = ?2,
                 agent_id = CASE
                     WHEN ?3 IS NOT NULL AND agent_id = ?3 THEN NULL
                     ELSE agent_id
                 END,
                 blox_command_id = NULL
             WHERE id = ?4 AND status NOT IN ('running', 'disconnected')",
            params![now_timestamp(), std::process::id(), agent_id_to_clear, id],
        )?;
        Ok(rows > 0)
    }

    /// Put a branch session the last app quit ended back on its branch queue.
    ///
    /// The compare-and-swap takes only a row the quit left terminal —
    /// `cancelled` (the shutdown sweep, or a session thread finishing its own
    /// cancel) or `error` (dead-owner recovery after a crash) with
    /// `completion_reason = 'app_quit'` — and only a branch-scoped one: a
    /// project session is never queued. The owner, error and reason are
    /// cleared so the row reads as freshly enqueued; `created_at` is left
    /// alone on purpose, because the branch drain orders by it and a re-queued
    /// row must keep the place it had before the quit. Returns `true` if the
    /// row was moved, `false` if it was already resumed, re-queued or deleted.
    pub fn transition_app_quit_to_queued(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE sessions
             SET status = 'queued', owner_pid = NULL, error_message = NULL,
                 completion_reason = NULL, updated_at = ?1
             WHERE id = ?2
               AND status IN ('cancelled', 'error')
               AND completion_reason = 'app_quit'
               AND branch_id IS NOT NULL",
            params![now_timestamp(), id],
        )?;
        Ok(rows > 0)
    }

    /// Atomically claim a rate-limited paused session for auto-resume,
    /// transitioning it to `Running` while replacing provider metadata.
    ///
    /// Auto rate-limit recovery uses this to preserve only its retry bookkeeping
    /// across a retry attempt while dropping stale ACP error metadata that would
    /// otherwise affect the next completion classification.
    ///
    /// The claim only matches rows still in the paused/rate-limited state, so a
    /// manual resume (or any other transition) that lands between the
    /// scheduler's scan and this update wins the race: this returns `false`
    /// instead of yanking the session back to running.
    pub fn transition_to_running_with_provider_metadata(
        &self,
        id: &str,
        provider_metadata: Option<&serde_json::Value>,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let provider_metadata_json = provider_metadata
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StoreError(format!("Failed to serialize provider metadata: {e}")))?;
        let rows = conn.execute(
            "UPDATE sessions SET status = 'running', error_message = NULL, completion_reason = NULL, provider_metadata = ?1, rate_limit_resets_at = NULL, rate_limit_kind = NULL, updated_at = ?2, owner_pid = ?3
             WHERE id = ?4 AND status = 'paused' AND completion_reason = 'rate_limited'",
            params![provider_metadata_json, now_timestamp(), std::process::id(), id],
        )?;
        Ok(rows > 0)
    }

    /// Sessions an app quit (or a crash, recovered at the next launch) ended
    /// after `updated_after`, oldest-created first.
    ///
    /// The launch-resume candidates: every row with `completion_reason =
    /// 'app_quit'` that is still terminal. `updated_after` is the marker of
    /// the last launch that offered them, so rows the user already dismissed
    /// are not offered again; a row the user resumes by hand and loses to a
    /// later quit carries a newer `updated_at` and is offered afresh.
    pub fn get_app_quit_sessions_since(
        &self,
        updated_after: i64,
    ) -> Result<Vec<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS}
             FROM sessions
             WHERE completion_reason = 'app_quit'
               AND status IN ('cancelled', 'error')
               AND updated_at > ?1
             ORDER BY created_at ASC, id ASC"
        ))?;
        let sessions = stmt
            .query_map(params![updated_after], Self::row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
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
        let mut stmt = conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS}
             FROM sessions WHERE status = 'running'"
        ))?;
        let sessions = stmt
            .query_map([], Self::row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    /// How many sessions ended with exactly this `error_message`.
    ///
    /// Backs the Doctor's session-queue readout: the stranded-session sweep
    /// records one fixed message on every row it ends, so its count is the
    /// number of times the queue needed rescuing.
    pub fn count_sessions_with_error_message(&self, message: &str) -> Result<usize, StoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE error_message = ?1",
            [message],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    /// Every `disconnected` row: Blox sessions whose turn may still be
    /// running on a workstation with no Staged process attached. The
    /// reconcile service's work list.
    pub fn get_disconnected_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS}
             FROM sessions WHERE status = 'disconnected'
             ORDER BY created_at ASC"
        ))?;
        let sessions = stmt
            .query_map([], Self::row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    /// Get all running, queued and disconnected sessions, oldest first.
    ///
    /// Backs the `get_active_sessions` busy-state snapshot command, which
    /// clients hydrate from on load or reconnect instead of relying solely on
    /// accumulated `session-status-changed` events. A `disconnected` row is
    /// in it because it still holds its branch: its remote turn may be
    /// running, and no client can tell that from a terminal row.
    pub fn get_active_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS}
             FROM sessions WHERE status IN ('running', 'queued', 'disconnected')
             ORDER BY created_at ASC"
        ))?;
        let sessions = stmt
            .query_map([], Self::row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    /// List the in-flight sessions spawned by `parent_session_id` — queued,
    /// running, or disconnected with a remote turn possibly still going —
    /// oldest first, projected to the repo they are working in.
    ///
    /// Children are found through their artifact (commit / note / review)
    /// rather than `sessions.branch_id`. Both are stamped on the queued row
    /// now, but the artifact join is what resolves the child's kind, so it is
    /// also the branch link — one join, one source of truth for the badge. A
    /// `child_note` (a note carrying `parent_project_note_id`) reports as a
    /// plain `"note"` here — the badge only needs to say what kind of work is
    /// in flight and where. There is deliberately no visibility filter: a
    /// hidden child note is as much a dependency of the parent as a visible
    /// one.
    ///
    /// Queued children are included. The branch queue runs one session per
    /// branch at a time, so an agent that fires several `start_repo_session`
    /// calls at one repo has all but one of them waiting — a queued child is
    /// still work the parent depends on, and dropping it made those
    /// dependencies invisible. `status` tells the caller which is which.
    ///
    /// Each child yields exactly one row even if it somehow acquires several
    /// artifacts: callers key UI elements by `session_id`, so a duplicate
    /// here would throw in Svelte's keyed `{#each}`.
    ///
    /// `created_at` is only millisecond-precision, and an agent can fire
    /// several `start_repo_session` calls inside one millisecond, so `rowid`
    /// breaks the tie — otherwise badges for concurrent children could swap
    /// positions from one refresh to the next.
    pub fn list_active_child_sessions(
        &self,
        parent_session_id: &str,
    ) -> Result<Vec<ChildSessionInfo>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.id,
                    CASE WHEN c.session_id IS NOT NULL THEN 'commit'
                         WHEN n.session_id IS NOT NULL THEN 'note'
                         ELSE 'review' END AS session_type,
                    s.status,
                    s.prompt,
                    s.acp_title,
                    n.title AS note_title,
                    r.title AS review_title,
                    b.id AS branch_id
             FROM sessions s
             LEFT JOIN commits c ON c.session_id = s.id
             LEFT JOIN notes n ON n.session_id = s.id
             LEFT JOIN reviews r ON r.session_id = s.id
             JOIN branches b ON b.id = COALESCE(c.branch_id, n.branch_id, r.branch_id)
             WHERE s.parent_session_id = ?1 AND s.status IN ('queued', 'running', 'disconnected')
             GROUP BY s.id
             ORDER BY s.created_at ASC, s.rowid ASC",
        )?;
        let children = stmt
            .query_map(params![parent_session_id], |row| {
                let session_type: String = row.get(1)?;
                let status: String = row.get(2)?;
                let prompt: String = row.get(3)?;
                let acp_title: Option<String> = row.get(4)?;
                let note_title: Option<String> = row.get(5)?;
                let review_title: Option<String> = row.get(6)?;
                // The same rule per kind as the branch timeline's row for the
                // same session, so a badge and the branch card agree.
                let title = match session_type.as_str() {
                    "commit" => Some(super::pending_commit_subject(
                        acp_title.as_deref(),
                        Some(prompt.as_str()),
                    )),
                    "note" => note_title,
                    _ => super::review_title(
                        review_title,
                        Some(status.as_str()),
                        acp_title.as_deref(),
                    ),
                };
                Ok(ChildSessionInfo {
                    session_id: row.get(0)?,
                    session_type,
                    status,
                    title,
                    branch_id: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(children)
    }

    /// Sessions parked by a provider rate limit, oldest pause first.
    ///
    /// Backs the auto-recovery scheduler's scan.
    pub fn get_rate_limited_paused_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS}
             FROM sessions
             WHERE status = 'paused' AND completion_reason = 'rate_limited'
             ORDER BY updated_at ASC"
        ))?;
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

    /// Record the worktree HEAD a run started from (`None` when the run has no
    /// commit base to track), so the base survives the runner.
    ///
    /// Leaves `updated_at` alone: this is bookkeeping, not a lifecycle change,
    /// and the stranded-session sweep's compare-and-swap on `updated_at` must
    /// not lose to it.
    pub fn set_session_pre_head_sha(
        &self,
        id: &str,
        pre_head_sha: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET pre_head_sha = ?1 WHERE id = ?2",
            params![pre_head_sha, id],
        )?;
        Ok(())
    }

    /// Pin the Blox process a `disconnected` row's turn ran on, by its
    /// command id (`None` to forget it), so every later read and attach for
    /// that turn finds the same process rather than whatever now runs the
    /// session's argv. Set by the reconcile service the first time it finds
    /// the process; cleared when a new turn starts
    /// ([`Self::transition_to_running_clearing_agent_id`]).
    ///
    /// Leaves `updated_at` alone, like [`Self::set_session_pre_head_sha`]: the
    /// reconcile's own compare-and-swap is on it.
    pub fn set_session_blox_command_id(
        &self,
        id: &str,
        blox_command_id: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET blox_command_id = ?1 WHERE id = ?2",
            params![blox_command_id, id],
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

    /// Update a queued session's working directory, prompt, and owner PID once
    /// its prompt context exists.
    ///
    /// Used by the branch queue drain after it has claimed a `queued` row.
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

    /// Update an immediate branch-start row only if it is still the row the
    /// original starter inserted.
    ///
    /// Immediate branch starts insert a `running` row before context exists and
    /// stamp `agent_id` with a temporary startup marker. A Stop followed by
    /// Resume can legitimately put the row back in `running`; the marker makes
    /// that new turn distinct so the abandoned starter cannot overwrite its
    /// prompt or launch a second runner.
    pub fn prepare_immediate_branch_session(
        &self,
        id: &str,
        expected_agent_id: &str,
        working_dir: &str,
        prompt: &str,
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE sessions
             SET working_dir = ?1, prompt = ?2, owner_pid = ?3, updated_at = ?4
             WHERE id = ?5 AND status = 'running' AND agent_id = ?6",
            params![
                working_dir,
                prompt,
                std::process::id(),
                now_timestamp(),
                id,
                expected_agent_id,
            ],
        )?;
        Ok(rows > 0)
    }

    /// Get all queued sessions for a branch, ordered by creation time (oldest first).
    ///
    /// `sessions.branch_id` is the scheduling key for every branch-scoped
    /// session (commit, note, review and pipeline alike), so this is a
    /// single-table read; the artifact tables are not consulted.
    pub fn get_queued_sessions_for_branch(
        &self,
        branch_id: &str,
    ) -> Result<Vec<Session>, StoreError> {
        self.get_sessions_for_branch_where(branch_id, "status = 'queued'")
    }

    /// Get all running sessions for a branch, oldest first.
    pub fn get_running_sessions_for_branch(
        &self,
        branch_id: &str,
    ) -> Result<Vec<Session>, StoreError> {
        self.get_sessions_for_branch_where(branch_id, "status = 'running'")
    }

    /// Get every session occupying a branch — `running`, or `disconnected`
    /// with its remote turn possibly still going — oldest first.
    ///
    /// The branch queue's view of what is in flight: a disconnected session
    /// blocks new branch work exactly as a running one does.
    pub fn get_busy_sessions_for_branch(
        &self,
        branch_id: &str,
    ) -> Result<Vec<Session>, StoreError> {
        self.get_sessions_for_branch_where(branch_id, "status IN ('running', 'disconnected')")
    }

    fn get_sessions_for_branch_where(
        &self,
        branch_id: &str,
        status_clause: &'static str,
    ) -> Result<Vec<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS}
             FROM sessions
             WHERE {status_clause} AND branch_id = ?1
             ORDER BY created_at ASC",
        ))?;
        let sessions = stmt
            .query_map(params![branch_id], Self::row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    /// Every branch with at least one queued session.
    ///
    /// The startup kick's read: a branch whose queue was left parked — by a
    /// previous run, or by a client that never got to its own drain — is
    /// moved along without anyone connected.
    pub fn branch_ids_with_queued_sessions(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT branch_id FROM sessions
             WHERE status = 'queued' AND branch_id IS NOT NULL
             ORDER BY branch_id",
        )?;
        let branch_ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(branch_ids)
    }

    /// Check whether a branch already has a running session.
    ///
    /// Any branch-scoped session counts, so a push in flight blocks new
    /// branch work the same way a commit does. A `disconnected` session counts
    /// as running: its remote turn may still be working in the branch's
    /// checkout, so new work must not start under it.
    pub fn has_running_session_for_branch(&self, branch_id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions
             WHERE status IN ('running', 'disconnected') AND branch_id = ?1",
            params![branch_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// The project's own sessions in flight, oldest first.
    ///
    /// Project-note sessions have no `branch_id` and reach the project only
    /// through `project_notes.session_id`, so the branch queries above cannot
    /// see them. `queued` counts along with `running` and `disconnected`: a
    /// deferred push ("Push after current work") waits for all three, so work
    /// the project agent has lined up still runs before the push. The ids are
    /// returned, not a count, so the drain can log what is holding a push.
    pub fn project_session_ids_in_flight(
        &self,
        project_id: &str,
    ) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.id FROM sessions s
             INNER JOIN project_notes pn ON pn.session_id = s.id
             WHERE pn.project_id = ?1 AND s.status IN ('running', 'disconnected', 'queued')
             ORDER BY s.created_at ASC",
        )?;
        let ids = stmt
            .query_map(params![project_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    /// The branch's `completed` sessions whose row was last written at or
    /// after `since` (ms), oldest first.
    ///
    /// A pre-filter, not a completion time. `updated_at` is set by the status
    /// write that completes a session, but also by later writes to the row:
    /// `project_transfer` and `branch_move` rewrite every session on a branch
    /// (`working_dir`, `agent_id`) and bump it, and so do the ACP title and
    /// config-selection setters. It is therefore never *earlier* than the
    /// session's completion, so filtering on it cannot miss a session that
    /// completed since `since` — it can only return extra, older ones. The
    /// deferred push narrows the result by the pipeline's own
    /// [`PipelineExecution::finished_at`], which nothing rewrites.
    pub fn get_completed_sessions_for_branch_since(
        &self,
        branch_id: &str,
        since: i64,
    ) -> Result<Vec<Session>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS}
             FROM sessions
             WHERE status = 'completed' AND branch_id = ?1 AND updated_at >= ?2
             ORDER BY created_at ASC",
        ))?;
        let sessions = stmt
            .query_map(params![branch_id, since], Self::row_to_session)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    /// Ids of the branch's sessions in a terminal state (`completed`, `error`
    /// or `cancelled`), other than `except_session_id`, whose row was last
    /// written at or after `since` (ms); oldest-created first.
    ///
    /// The deferred push asks this with a completed push's own
    /// [`PipelineExecution::finished_at`] as `since`, to learn whether
    /// anything else on the branch ended after that push — an agent session
    /// that committed, say — whose work the push cannot have carried. Like
    /// [`Self::get_completed_sessions_for_branch_since`] it reads
    /// `updated_at`, which later writes to the row only ever move *later*,
    /// and it keeps a tie (`>=`): both make it report "something ended after
    /// the push" when nothing did, which costs a redundant push, never a
    /// missing one. Every terminal status counts, since a session cancelled
    /// or failed mid-run may still have committed.
    pub fn branch_session_ids_ended_since(
        &self,
        branch_id: &str,
        since: i64,
        except_session_id: &str,
    ) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id FROM sessions
             WHERE branch_id = ?1
               AND status IN ('completed', 'error', 'cancelled')
               AND updated_at >= ?2
               AND id != ?3
             ORDER BY created_at ASC",
        )?;
        let ids = stmt
            .query_map(params![branch_id, since, except_session_id], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    /// Resolve the branch that owns a session.
    ///
    /// Project-note sessions and Pikchr child sessions do not belong to a
    /// branch and therefore return `None`.
    pub fn get_branch_id_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let branch_id: Option<Option<String>> = conn
            .query_row(
                "SELECT branch_id FROM sessions WHERE id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(branch_id.flatten())
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
             INNER JOIN sessions s ON s.branch_id = b.id
             WHERE s.id = ?1
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

    /// Record that the user has read this session's chat up to now.
    ///
    /// Writes only when the chat has a reply newer than both the last read and
    /// the note the session wrote, so reopening a chat with nothing new, or
    /// one whose replies all predate its note, neither writes nor reloads
    /// anything. Skipping the latter is safe: the chat button ignores the read
    /// time until a reply passes the note, and that reply still reads as
    /// unread against the older read time.
    /// Unlike other session writes this publishes a notes change: the read
    /// state shows on the chat button of the note the session wrote.
    pub fn mark_session_chat_read(&self, session_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            &format!(
                "UPDATE sessions SET chat_read_at = ?2
                 WHERE id = ?1
                   AND (SELECT MAX(created_at) FROM session_messages
                        WHERE session_id = ?1 AND {VISIBLE_ASSISTANT_FILTER})
                       > MAX(
                           COALESCE(chat_read_at, 0),
                           COALESCE(
                             (SELECT updated_at FROM notes WHERE session_id = ?1),
                             (SELECT updated_at FROM project_notes WHERE session_id = ?1),
                             0
                           )
                         )"
            ),
            params![session_id, now_timestamp()],
        )?;
        if changed == 0 {
            return Ok(());
        }
        let branch_id = Self::lookup_id(
            &conn,
            "SELECT branch_id FROM notes WHERE session_id = ?1",
            session_id,
        );
        let project_id = Self::lookup_id(
            &conn,
            "SELECT project_id FROM project_notes WHERE session_id = ?1",
            session_id,
        );
        if branch_id.is_some() || project_id.is_some() {
            self.publish(StoreChange::Notes {
                branch_id,
                project_id,
            });
        }
        Ok(())
    }

    /// Record that this session's pipeline (pr/push) completion outcome events
    /// have been delivered, so a later resume of the same session doesn't
    /// re-fire them against its stale pipeline and transcript.
    ///
    /// One-shot: the completion hook refuses to run once this is set.
    pub fn mark_completion_effects_ran(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let now = now_timestamp();
        conn.execute(
            "UPDATE sessions SET completion_effects_at = ?1, updated_at = ?1 WHERE id = ?2",
            params![now, id],
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

    /// Merge protocol/provider metadata into the session's JSON metadata bag.
    ///
    /// Object values are merged recursively so separate ACP events can fill in
    /// `promptError`, `promptResponse`, and `usageUpdate` independently.
    pub fn merge_session_provider_metadata(
        &self,
        id: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let existing_json: Option<String> = conn
            .query_row(
                "SELECT provider_metadata FROM sessions WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();

        let mut merged = existing_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        merge_json(&mut merged, metadata.clone());
        let merged_json = serde_json::to_string(&merged)
            .map_err(|e| StoreError(format!("Failed to serialize provider metadata: {e}")))?;

        conn.execute(
            "UPDATE sessions SET provider_metadata = ?1, updated_at = ?2 WHERE id = ?3",
            params![merged_json, now_timestamp(), id],
        )?;
        Ok(())
    }

    fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
        let status_str: String = row.get(2)?;
        let reason_str: Option<String> = row.get(7)?;
        let provider_metadata_json: Option<String> = row.get(8)?;
        let provider_metadata = provider_metadata_json.as_deref().and_then(|s| {
            serde_json::from_str(s)
                .map_err(|e| log::warn!("Failed to deserialize provider metadata JSON: {e}"))
                .ok()
        });
        let pipeline_json: Option<String> = row.get(14)?;
        let acp_config_selection_json: Option<String> = row.get(15)?;
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
            provider_metadata,
            rate_limit_resets_at: row.get(9)?,
            rate_limit_kind: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
            owner_pid: row.get(13)?,
            pipeline,
            acp_config_selection,
            acp_title: row.get(16)?,
            branch_id: row.get(17)?,
            completion_effects_at: row.get(18)?,
            parent_session_id: row.get(19)?,
            pre_head_sha: row.get(20)?,
            blox_command_id: row.get(21)?,
            started_by: row.get(22)?,
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

fn merge_json(target: &mut serde_json::Value, patch: serde_json::Value) {
    match (target, patch) {
        (serde_json::Value::Object(target), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                match target.get_mut(&key) {
                    Some(existing) => merge_json(existing, value),
                    None => {
                        target.insert(key, value);
                    }
                }
            }
        }
        (target, patch) => {
            *target = patch;
        }
    }
}
