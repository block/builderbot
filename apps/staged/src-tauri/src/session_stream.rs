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
//! | `full`    | not served yet                                               |
//!
//! Rules a consumer can rely on:
//!
//! - **Snapshot, then deltas, on one stream.** A session added to the
//!   interest gets a snapshot frame, and every later frame for it is a
//!   delta. Both come from the hub task through the client's one sink, so a
//!   snapshot is never overtaken by an older delta.
//! - **Deltas are state.** A delta holds the current session fields and the
//!   current version of each changed row. The client upserts rows by id and
//!   trims to its window; applying a delta twice changes nothing.
//! - **Sequence and generation.** Frames are numbered per client from 1
//!   within a generation, which the client picks. A gap means a lost frame;
//!   the client then declares its interest again under a new generation and
//!   gets snapshots. `check_session_stream` lets it compare its last number
//!   with the hub's when no frame has arrived for a while.
//! - **Cadence.** A change waits 30 ms for the rest of its burst, and one
//!   session sends one client at most one delta per 300 ms. Marks coalesce
//!   while they wait, so a later frame is larger, never a backlog.
//! - **Backpressure.** A client whose queue is full is skipped and tried
//!   again; what it is owed keeps coalescing and nothing is lost. It delays
//!   that client only.
//! - **Paused.** A paused client keeps its interest and is sent nothing. The
//!   interest call that ends the pause gets snapshots.
//! - **A session with no row** (deleted, or not written yet) is not an
//!   error: nothing is sent for it.
//!
//! With no client subscribed, or only paused ones, the feed is switched off,
//! so a session write records nothing and the hub task stays parked.
//!
//! This sits beside the change feed (`store_events.rs`), which still leaves
//! sessions out. Polling (`poll_session` and the standalone reads) is
//! untouched and remains what the UI uses until it consumes this stream.

pub mod commands;
mod frames;
mod hub;
#[cfg(test)]
mod tests;
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
