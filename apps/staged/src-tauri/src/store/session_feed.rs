//! In-memory feed of session changes, for pushing session updates to the UI.
//!
//! Store methods that write a session's messages, its row or its follow-up
//! queue record a *mark* here: which session, which rows, which parts. A mark
//! carries ids and flags, never content. A consumer drains the marks and reads
//! the changed rows back from SQLite, so what it ships is always the latest
//! committed state through the projections the reads already use.
//!
//! This sits beside the change feed ([`super::StoreChange`]) rather than in
//! it. A set of marks coalesces by construction (ten updates to one row
//! between drains are one entry) and cannot overflow or lag, so it has no
//! equivalent of the change feed's "refetch everything" response, and a slow
//! consumer here cannot trigger that response there.
//!
//! The feed sees this process's writes only. Another Staged process on the
//! same database writes through its own store and marks nothing here;
//! [`Store::data_version`] is how a consumer learns that one has committed.
//!
//! The feed starts inactive and stays so until a consumer has someone to
//! deliver to. While it is inactive a mark costs one atomic load, and nothing
//! is recorded.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, Params};
use tokio::sync::Notify;

use super::{Store, StoreError};

/// What changed in one session since the feed was last drained.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionDirty {
    /// Transcript rows that were inserted, or whose content was rewritten.
    pub rows: BTreeSet<i64>,
    /// Rows whose ACP metadata was inserted or updated. Inserts are marked
    /// only for the hidden kinds the transcript reads (see
    /// `TRANSCRIPT_HIDDEN_EVENT_KINDS` in `messages.rs`).
    pub metadata_rows: BTreeSet<i64>,
    /// The `sessions` row was inserted, or a field of it that the UI's
    /// `Session` carries was written: status, owner, title, pipeline, working
    /// directory, provider, branch and the rest.
    pub session: bool,
    /// The follow-up queue changed.
    pub queue: bool,
}

/// The dirty marks of every session written since the last drain, plus a
/// wake-up for the task that drains them.
#[derive(Debug, Default)]
pub struct SessionFeed {
    /// Read without the lock on every session write, so an idle feed costs
    /// the writers nothing. Only ever changed under `dirty`'s lock.
    active: AtomicBool,
    dirty: Mutex<HashMap<String, SessionDirty>>,
    wake: Notify,
}

impl SessionFeed {
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Start or stop recording marks. Stopping discards the marks not yet
    /// drained: nobody is left to deliver them to, and whoever activates the
    /// feed again starts from a fresh read, not from these.
    ///
    /// A consumer that activates the feed and then reads a session misses no
    /// write: the store marks while it still holds the connection, so a write
    /// either committed before that read or sees the feed active.
    pub fn set_active(&self, active: bool) {
        let mut dirty = self.lock_dirty();
        self.active.store(active, Ordering::Release);
        if !active {
            dirty.clear();
        }
    }

    /// Take every mark recorded since the last drain.
    pub fn drain(&self) -> HashMap<String, SessionDirty> {
        std::mem::take(&mut *self.lock_dirty())
    }

    /// Wait until a mark has been recorded since the last time this returned.
    /// A mark that lands before the call is not lost: the wake-up is kept
    /// until someone takes it. Meant for a single draining task.
    pub async fn marked(&self) {
        self.wake.notified().await;
    }

    pub(super) fn mark_row(&self, session_id: &str, message_id: i64) {
        self.mark(session_id, |dirty| {
            dirty.rows.insert(message_id);
        });
    }

    pub(super) fn mark_metadata_row(&self, session_id: &str, message_id: i64) {
        self.mark(session_id, |dirty| {
            dirty.metadata_rows.insert(message_id);
        });
    }

    pub(super) fn mark_session(&self, session_id: &str) {
        self.mark(session_id, |dirty| dirty.session = true);
    }

    pub(super) fn mark_queue(&self, session_id: &str) {
        self.mark(session_id, |dirty| dirty.queue = true);
    }

    fn mark(&self, session_id: &str, apply: impl FnOnce(&mut SessionDirty)) {
        if !self.is_active() {
            return;
        }
        {
            let mut dirty = self.lock_dirty();
            // Checked again under the lock, so a mark racing a deactivation
            // cannot land after the clear and outlive it.
            if !self.is_active() {
                return;
            }
            match dirty.get_mut(session_id) {
                Some(entry) => apply(entry),
                None => apply(dirty.entry(session_id.to_owned()).or_default()),
            }
        }
        self.wake.notify_one();
    }

    /// A mark must never fail the write that made it, so a poisoned lock is
    /// taken as it is: the map holds ids and flags, and a mark that panicked
    /// half way leaves it usable.
    fn lock_dirty(&self) -> MutexGuard<'_, HashMap<String, SessionDirty>> {
        self.dirty.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Marks for the store's session writers. Each is a no-op when no feed is
/// attached, and a writer calls it after its statement has run (after the
/// commit, for a transaction) and before it lets go of the connection.
impl Store {
    pub(super) fn mark_message_row(&self, session_id: &str, message_id: i64) {
        if let Some(feed) = &self.session_feed {
            feed.mark_row(session_id, message_id);
        }
    }

    pub(super) fn mark_metadata_row(&self, session_id: &str, message_id: i64) {
        if let Some(feed) = &self.session_feed {
            feed.mark_metadata_row(session_id, message_id);
        }
    }

    /// Mark the session row when `rows_changed` says an `UPDATE` of it
    /// landed, and return whether it did, which is what the guarded status
    /// writers report to their callers.
    pub(super) fn mark_session_written(&self, session_id: &str, rows_changed: usize) -> bool {
        let written = rows_changed > 0;
        if written {
            if let Some(feed) = &self.session_feed {
                feed.mark_session(session_id);
            }
        }
        written
    }

    /// [`Self::mark_session_written`] for a statement that wrote several
    /// session rows: the ids [`updated_session_ids`] collected.
    pub(super) fn mark_sessions_written(&self, session_ids: &[String]) {
        if let Some(feed) = &self.session_feed {
            for session_id in session_ids {
                feed.mark_session(session_id);
            }
        }
    }

    pub(super) fn mark_session_queue(&self, session_id: &str) {
        if let Some(feed) = &self.session_feed {
            feed.mark_queue(session_id);
        }
    }
}

impl Store {
    /// SQLite's `PRAGMA data_version` for the store's connection: a number
    /// that stays the same across this connection's own writes and differs
    /// once any other connection has committed to the database, an in-place
    /// `UPDATE` included. The store holds the process's one read-write
    /// connection, so a different value means another process wrote, though
    /// not what it wrote.
    ///
    /// Only two values from one store compare: the number means nothing
    /// across connections.
    pub fn data_version(&self) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        let version = conn.query_row("PRAGMA data_version", [], |row| row.get(0))?;
        Ok(version)
    }
}

/// Run an `UPDATE sessions … RETURNING id` and collect the ids of the rows
/// it wrote, for a writer that selects its sessions by something other than
/// their id. The caller marks them once its transaction has committed
/// ([`Store::mark_sessions_written`]).
pub(super) fn updated_session_ids(
    conn: &Connection,
    sql: &str,
    params: impl Params,
) -> Result<Vec<String>, StoreError> {
    let mut statement = conn.prepare(sql)?;
    let ids = statement
        .query_map(params, |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(ids)
}
