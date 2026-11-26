//! Session updates pushed to the UI.
//!
//! Store methods that write a session mark the session feed
//! ([`crate::store::SessionFeed`]): which session, which rows, which parts,
//! and no content. The hub in this module drains those marks a few times a
//! second, reads what changed back from SQLite once, and sends each
//! subscribed client a [`SessionStreamFrame`] holding what that client asked
//! for and nothing else.
//!
//! A *client* is one native window (`tauri-<label>`) or one browser tab (the
//! id it sends on its events socket), the same ids the PR poll scheduler
//! uses. It has one ordered sink: a Tauri channel, or a bounded queue its
//! socket forwards. It says what it wants with `set_session_stream_interest`,
//! which replaces its whole interest: a list of sessions, each at a
//! [`SessionStreamLevel`].
//!
//! | Level     | A frame carries                                              |
//! |-----------|--------------------------------------------------------------|
//! | `title`   | status and title                                             |
//! | `summary` | those, the working directory, the pipeline, the opening      |
//! |           | prompt's id and the transcript rows the subtitle reads       |
//! | `full`    | what `poll_session` returns: the session row, the follow-up  |
//! |           | queue, the visible rows and the ACP metadata rows            |
//!
//! `title` and `summary` are what a row showing the session needs, and a
//! session is held at the higher of the two. `full` is what an open chat
//! needs, in a different shape ([`crate::store::SessionFull`]), and is held
//! beside them: a session wanted both ways gets a snapshot and deltas of
//! each. A frame is about one level and carries `summary` or `full`, never
//! both. `summary.rs` and `full.rs` hold what each is owed and how its
//! frames are built; `hub.rs` holds the clients and the task.
//!
//! Rules a consumer can rely on:
//!
//! - **Snapshot, then deltas, on one stream.** A session added to the
//!   interest gets a snapshot frame, and every later frame for it is a
//!   delta. Both come from the hub task through the client's one sink, so a
//!   snapshot is never overtaken by an older delta.
//! - **Deltas are state.** A delta holds the current session fields and the
//!   current version of each changed row. The client upserts rows by id and
//!   trims to its window; applying a delta twice changes nothing. A full
//!   delta holds only the parts that were written: the session row and the
//!   queue are absent from one that did not change them.
//! - **A full snapshot starts from a cursor.** The client gives, with its
//!   interest, the cursor it would have polled with
//!   ([`crate::store::SessionPollRequest`]), and the snapshot is that poll:
//!   incremental from what the client holds, to be merged and not to
//!   replace. Giving a cursor always asks for a snapshot; an interest that
//!   names a held chat without one leaves its deltas running.
//! - **Sequence and generation.** Frames are numbered per client from 1
//!   within a generation, which the client picks. A gap means a lost frame;
//!   the client then declares its interest again under a new generation and
//!   gets snapshots. `check_session_stream` lets it compare its last number
//!   with the hub's when no frame has arrived for a while.
//! - **Cadence.** A change waits 30 ms for the rest of its burst, and one
//!   session sends one client at most one delta per 300 ms, or per 150 ms
//!   at the full level. Marks coalesce while they wait, so a later frame is
//!   larger, never a backlog. At the full level a tool call still in flight
//!   whose row is over 32 KB is sent at most twice a second.
//! - **A terminal status does not precede the final text.** The session row
//!   and the rows of a full frame are one read, and a frame that carries
//!   the session row holds no owed row back. `full.rs` says what that means
//!   beside the status event, which travels separately.
//! - **Backpressure.** A client whose queue is full is skipped and tried
//!   again; what it is owed keeps coalescing and nothing is lost. It delays
//!   that client only.
//! - **Paused.** A paused client keeps its interest and is sent nothing. The
//!   interest call that ends the pause gets snapshots.
//! - **A session with no row** (deleted, or not written yet) is not an
//!   error: nothing is sent for it.
//! - **Another Staged process on the same database.** Its writes mark
//!   nothing here. While a client is subscribed to a session this process is
//!   not running, the hub reads SQLite's `data_version` once a second, and
//!   when another process has committed it reads those sessions again and
//!   sends a delta of what differs from what the client holds. So a session
//!   running in another instance follows within about a second. See
//!   `watch.rs` for which sessions count and the one gap that is left.
//!   A chat holds too many rows to compare, so at the full level the client
//!   is sent a `resync` frame instead, with no payload, and answers by
//!   giving its cursor again. A chat on a session this process *is* running
//!   has its session row read again, which is how another instance's Stop
//!   reaches it; the version is therefore read while any chat is open.
//!
//! With no client subscribed, or only paused ones, the feed is switched off,
//! so a session write records nothing and the hub task stays parked. The
//! once-a-second read is the only timer the hub runs with nothing owed, and
//! it runs only for the sessions above.
//!
//! The hub task runs for the life of the app. Should it ever end (a panic),
//! the hub drops its clients and refuses every interest from then on, so a
//! client's `check_session_stream` and its next interest both fail and it
//! shows what it can without the stream: a chat pane reads `poll_session`
//! on a timer, a row drops its live subtitle.
//!
//! This sits beside the change feed (`store_events.rs`), which still leaves
//! sessions out. It is the only way a running session's subtitle, title and
//! transcript reach a window that is showing them; `poll_session` remains as
//! the read a chat pane makes when it reopens a session and while the stream
//! is unavailable.

pub mod commands;
mod frames;
mod full;
mod hub;
mod summary;
#[cfg(test)]
mod tests;
mod watch;
mod web;

use std::sync::{Arc, Mutex, PoisonError};

use tauri::Manager;

use crate::store::Store;

pub use frames::{
    SessionInterest, SessionStreamFrame, SessionStreamFrameKind, SessionStreamInterest,
    SessionStreamLevel,
};
pub use hub::{FrameSink, SessionStreamHub, SinkError};
pub(crate) use web::web_queue;

/// Spawn the hub task. Runs for the life of the app, and survives a store
/// reset: it asks for the store on every read, and the replacement store is
/// given the same feed.
pub fn spawn(app_handle: tauri::AppHandle, hub: Arc<SessionStreamHub>) {
    tauri::async_runtime::spawn(hub::run(hub, move || {
        let slot = app_handle.try_state::<Mutex<Option<Arc<Store>>>>()?;
        let store = slot.lock().unwrap_or_else(PoisonError::into_inner).clone();
        store
    }));
}
