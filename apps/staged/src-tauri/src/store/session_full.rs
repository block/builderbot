//! The full read behind the session stream.
//!
//! An open chat is sent what the pane's poll used to fetch: the session row,
//! the follow-up queue, visible transcript rows and the ACP metadata rows
//! the transcript reads. [`Store::read_session_full`] is `poll_session` with
//! two of its four parts optional, because the stream sends the session row
//! and the queue only when one of them was written. It goes through the same
//! four reads under one lock and one read transaction, so the rows carry the
//! projection the poll gives them: an in-flight tool row whole, a settled
//! one with its large bodies as markers.
//!
//! The cursor is the poll's own ([`SessionPollRequest`]). A snapshot reads
//! from the cursor a client sent. A delta reads rows by id alone
//! ([`SessionPollRequest::by_ids`]), which is the same request with nothing
//! past either cursor: visible rows through the visibility filter, metadata
//! rows around the transcript's metadata filter, so a tool row is still read
//! once it has settled.

use serde::Serialize;

use super::models::{QueuedSessionMessage, Session, SessionMessage};
use super::session_poll::SessionPollRequest;
use super::{Store, StoreError};

/// The most ids one statement is given. A cursor's lists come from a client
/// and a delta's from however many rows a burst wrote, and SQLite caps the
/// parameters of a statement.
const ID_CHUNK: usize = 500;

/// What an open chat holds of a session, or the part of it that changed.
/// The four parts are [`super::SessionPoll`]'s; the first two are left out
/// when they were not read.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFull {
    /// The whole row. `None` when it was not asked for, and when the session
    /// has no row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<Session>,
    /// The whole follow-up queue, in its order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued_messages: Option<Vec<QueuedSessionMessage>>,
    /// Visible rows, in id order.
    pub messages: Vec<SessionMessage>,
    /// ACP metadata rows, in id order.
    pub acp_metadata_messages: Vec<SessionMessage>,
}

impl SessionFull {
    /// Whether there is nothing in it to send.
    pub fn is_empty(&self) -> bool {
        self.session.is_none()
            && self.queued_messages.is_none()
            && self.messages.is_empty()
            && self.acp_metadata_messages.is_empty()
    }
}

impl SessionPollRequest {
    /// The request for these rows and nothing else: no row is past either
    /// cursor, so each list is read by id. The metadata rows are then read
    /// around the transcript's metadata filter, as a poll's
    /// `metadata_refetch_ids` are.
    pub fn by_ids(rows: Vec<i64>, metadata_rows: Vec<i64>) -> Self {
        Self {
            since_id: i64::MAX,
            refetch_ids: rows,
            metadata_since_id: i64::MAX,
            metadata_refetch_ids: metadata_rows,
        }
    }
}

impl Store {
    /// What [`Store::poll_session`] returns for `request`, with the session
    /// row read only when `session` is set and the queue only when `queue`
    /// is.
    pub fn read_session_full(
        &self,
        session_id: &str,
        request: &SessionPollRequest,
        session: bool,
        queue: bool,
    ) -> Result<SessionFull, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        // As in `poll_session`: the lock excludes this process's writers, and
        // the read transaction extends the snapshot to any other process.
        let tx = conn.transaction()?;
        let full = SessionFull {
            session: match session {
                true => Self::get_session_with_conn(&tx, session_id)?,
                false => None,
            },
            queued_messages: match queue {
                true => Some(Self::list_queued_session_messages_with_conn(
                    &tx, session_id,
                )?),
                false => None,
            },
            messages: in_chunks(request.since_id, &request.refetch_ids, |since_id, ids| {
                Self::get_session_messages_since_with_conn(&tx, session_id, since_id, ids)
            })?,
            acp_metadata_messages: in_chunks(
                request.metadata_since_id,
                &request.metadata_refetch_ids,
                |since_id, ids| {
                    Self::get_session_acp_metadata_messages_since_with_conn(
                        &tx, session_id, since_id, ids,
                    )
                },
            )?,
        };
        tx.commit()?;
        Ok(full)
    }
}

/// Run a "past the cursor, or one of these ids" read with at most
/// [`ID_CHUNK`] ids a statement. The first statement carries the cursor and
/// the rest read by id alone; a row both reach is kept once.
fn in_chunks(
    since_id: i64,
    ids: &[i64],
    read: impl Fn(i64, &[i64]) -> Result<Vec<SessionMessage>, StoreError>,
) -> Result<Vec<SessionMessage>, StoreError> {
    if ids.len() <= ID_CHUNK {
        return read(since_id, ids);
    }
    let mut rows = Vec::new();
    for (index, chunk) in ids.chunks(ID_CHUNK).enumerate() {
        let since_id = if index == 0 { since_id } else { i64::MAX };
        rows.extend(read(since_id, chunk)?);
    }
    rows.sort_by_key(|row| row.id);
    rows.dedup_by_key(|row| row.id);
    Ok(rows)
}
