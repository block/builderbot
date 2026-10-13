//! One read per tick of the session pane's poll.
//!
//! While a session runs, the pane used to issue four requests every 500 ms:
//! the session row, the follow-up queue, and the visible and metadata
//! transcript increments. [`Store::poll_session`] answers all four from one
//! lock and one read transaction, so the response is a single consistent
//! snapshot. The runner writes an assistant row's final text before it flips
//! the session's status, so a snapshot whose status is terminal always holds
//! that text too.

use serde::{Deserialize, Serialize};

use super::models::{QueuedSessionMessage, Session, SessionMessage};
use super::{Store, StoreError};

/// The poll's cursors, as computed by the client from what it has loaded
/// (`sessionPollRequest` in `messagePoll.ts`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPollRequest {
    /// Visible rows with `id >= since_id`: inclusive, because the tail may
    /// still be growing. `0` returns the whole visible transcript.
    pub since_id: i64,
    /// Visible rows re-read by id: an assistant row still being extended
    /// behind later tool rows.
    #[serde(default)]
    pub refetch_ids: Vec<i64>,
    /// Metadata rows with `id > metadata_since_id`, through the transcript
    /// metadata filter.
    pub metadata_since_id: i64,
    /// Unsettled tool rows re-read by id, bypassing that filter so the caller
    /// sees them settle.
    #[serde(default)]
    pub metadata_refetch_ids: Vec<i64>,
}

/// What the four standalone reads (`get_session`,
/// `list_queued_session_messages`, `get_session_messages_since`,
/// `get_session_acp_metadata_messages_since`) would return for the same
/// request, rows projected for the UI exactly as they are.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPoll {
    pub session: Option<Session>,
    pub queued_messages: Vec<QueuedSessionMessage>,
    pub messages: Vec<SessionMessage>,
    pub acp_metadata_messages: Vec<SessionMessage>,
}

impl Store {
    pub fn poll_session(
        &self,
        session_id: &str,
        request: &SessionPollRequest,
    ) -> Result<SessionPoll, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        // The lock already excludes this process's writers; the read
        // transaction extends the snapshot to any other process on the file.
        let tx = conn.transaction()?;
        let poll = SessionPoll {
            session: Self::get_session_with_conn(&tx, session_id)?,
            queued_messages: Self::list_queued_session_messages_with_conn(&tx, session_id)?,
            messages: Self::get_session_messages_since_with_conn(
                &tx,
                session_id,
                request.since_id,
                &request.refetch_ids,
            )?,
            acp_metadata_messages: Self::get_session_acp_metadata_messages_since_with_conn(
                &tx,
                session_id,
                request.metadata_since_id,
                &request.metadata_refetch_ids,
            )?,
        };
        tx.commit()?;
        Ok(poll)
    }
}
