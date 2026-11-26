//! The hub, driven through closure-like sinks over an in-memory store, on a
//! paused clock.
//!
//! The clock only moves when every task is idle and no blocking read is
//! outstanding, so a test that waits a millisecond has let the hub finish
//! whatever it was woken for.

mod cadence;
mod clients;
mod foreign;
mod full;
mod full_cadence;
mod full_foreign;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

use crate::store::{
    MessageRole, Session, SessionFeed, SessionFull, SessionPollRequest, SessionSummary, Store,
};

use super::hub::{self, GATHER, SUMMARY_INTERVAL};
use super::{
    FrameSink, SessionInterest, SessionStreamFrame, SessionStreamFrameKind, SessionStreamHub,
    SessionStreamInterest, SessionStreamLevel, SinkError,
};

/// The summary window the tests ask for unless they say otherwise.
const WINDOW: u32 = 3;

/// A hub task over an in-memory store that shares its feed.
struct Harness {
    hub: Arc<SessionStreamHub>,
    store: Arc<Store>,
    feed: Arc<SessionFeed>,
    /// How many rounds the hub has gone to the store for: a round of reads,
    /// or a read of the data version alone.
    rounds: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
    /// The database file's directory, for a harness that has one.
    _dir: Option<tempfile::TempDir>,
}

impl Harness {
    fn new() -> Self {
        Self::build(true)
    }

    /// A hub that is never given a store to read, as before the store opens.
    fn without_store() -> Self {
        Self::build(false)
    }

    fn build(readable: bool) -> Self {
        Self::over(Store::in_memory().unwrap(), readable, None)
    }

    /// A hub over a database file, and a second store on the same file with
    /// no feed: what another Staged process is to this one.
    fn beside_another_instance() -> (Self, Store) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let store = Store::new(&path).unwrap();
        let other = Store::new(&path).unwrap();
        (Self::over(store, true, Some(dir)), other)
    }

    fn over(store: Store, readable: bool, dir: Option<tempfile::TempDir>) -> Self {
        let feed = Arc::new(SessionFeed::default());
        let store = Arc::new(store.with_session_feed(feed.clone()));
        let hub = Arc::new(SessionStreamHub::new(feed.clone()));
        let rounds = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(hub::run(hub.clone(), {
            let store = store.clone();
            let rounds = rounds.clone();
            move || {
                rounds.fetch_add(1, Ordering::SeqCst);
                readable.then(|| store.clone())
            }
        }));
        Self {
            hub,
            store,
            feed,
            rounds,
            task,
            _dir: dir,
        }
    }

    fn rounds(&self) -> usize {
        self.rounds.load(Ordering::SeqCst)
    }

    /// A running session in `/tmp`.
    fn session(&self) -> Session {
        let session = Session::new_running("work", Path::new("/tmp"));
        self.store.create_session(&session).unwrap();
        session
    }

    fn say(&self, session: &Session, role: MessageRole, content: &str) -> i64 {
        self.store
            .add_session_message(&session.id, role, content)
            .unwrap()
    }

    /// Open a stream for `client_id` on a recording sink.
    fn client(&self, client_id: &str) -> Arc<Recorder> {
        let recorder = Arc::new(Recorder::default());
        self.hub.connect(client_id, recorder.clone());
        recorder
    }

    /// Declare `sessions`, some of them at the full level, for `client_id`
    /// in `generation`.
    fn want_chats(&self, client_id: &str, generation: u64, sessions: Vec<SessionInterest>) {
        self.hub
            .set_interest(client_id, chats(generation, sessions))
            .unwrap();
    }

    /// Declare `sessions` for `client_id` in `generation`, unpaused, with
    /// the default window.
    fn want(&self, client_id: &str, generation: u64, sessions: &[(&Session, SessionStreamLevel)]) {
        self.hub
            .set_interest(client_id, interest(generation, sessions))
            .unwrap();
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn interest(generation: u64, sessions: &[(&Session, SessionStreamLevel)]) -> SessionStreamInterest {
    SessionStreamInterest {
        generation,
        paused: false,
        summary_window: WINDOW,
        sessions: sessions
            .iter()
            .map(|(session, level)| SessionInterest {
                session_id: session.id.clone(),
                level: *level,
                cursor: None,
            })
            .collect(),
    }
}

/// A sink that keeps what it is sent, with when. It has room for
/// everything until a test says otherwise.
struct Recorder {
    frames: Mutex<Vec<(Instant, SessionStreamFrame)>>,
    room: AtomicUsize,
    closed: AtomicBool,
}

impl Default for Recorder {
    fn default() -> Self {
        Self {
            frames: Mutex::default(),
            room: AtomicUsize::new(usize::MAX),
            closed: AtomicBool::new(false),
        }
    }
}

impl Recorder {
    /// The frames received since the last call.
    fn take(&self) -> Vec<SessionStreamFrame> {
        self.take_timed()
            .into_iter()
            .map(|(_, frame)| frame)
            .collect()
    }

    fn take_timed(&self) -> Vec<(Instant, SessionStreamFrame)> {
        std::mem::take(&mut self.frames.lock().unwrap())
    }

    fn set_room(&self, room: usize) {
        self.room.store(room, Ordering::SeqCst);
    }

    /// From now on the client is gone.
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

impl FrameSink for Recorder {
    fn room(&self) -> usize {
        self.room.load(Ordering::SeqCst)
    }

    fn send(&self, frame: SessionStreamFrame) -> Result<(), SinkError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(SinkError::Closed);
        }
        self.frames.lock().unwrap().push((Instant::now(), frame));
        Ok(())
    }
}

/// Let the hub finish what it was woken for.
async fn settle() {
    advance(Duration::from_millis(1)).await;
}

async fn advance(duration: Duration) {
    tokio::time::sleep(duration).await;
}

/// Long enough after a frame that the cadence no longer holds the next.
async fn past_the_interval() {
    advance(SUMMARY_INTERVAL).await;
}

/// Long enough after a write, once the interval has passed, for its delta.
async fn past_the_gather() {
    advance(GATHER + Duration::from_millis(2)).await;
}

/// A frame as (sequence number, kind, session id).
type Shape = (u64, SessionStreamFrameKind, String);

fn shape(frames: &[SessionStreamFrame]) -> Vec<Shape> {
    frames
        .iter()
        .map(|frame| (frame.seq, frame.kind, frame.session_id.clone()))
        .collect()
}

fn snapshot(seq: u64, session: &Session) -> Shape {
    (seq, SessionStreamFrameKind::Snapshot, session.id.clone())
}

fn delta(seq: u64, session: &Session) -> Shape {
    (seq, SessionStreamFrameKind::Delta, session.id.clone())
}

fn summary(frame: &SessionStreamFrame) -> &SessionSummary {
    frame.summary.as_ref().expect("a frame carries a summary")
}

/// A session at the full level, with the cursor of what the client holds.
fn chat(session: &Session, cursor: Option<SessionPollRequest>) -> SessionInterest {
    SessionInterest {
        session_id: session.id.clone(),
        level: SessionStreamLevel::Full,
        cursor,
    }
}

/// The cursor of a client that holds nothing of a session.
fn from_the_start() -> Option<SessionPollRequest> {
    Some(SessionPollRequest::default())
}

/// An unpaused interest in exactly `sessions`.
fn chats(generation: u64, sessions: Vec<SessionInterest>) -> SessionStreamInterest {
    SessionStreamInterest {
        generation,
        paused: false,
        summary_window: WINDOW,
        sessions,
    }
}

fn full(frame: &SessionStreamFrame) -> &SessionFull {
    frame.full.as_ref().expect("a frame carries a full part")
}

/// The ids of a full frame's visible rows and of its metadata rows.
fn full_ids(frame: &SessionStreamFrame) -> (Vec<i64>, Vec<i64>) {
    let ids = |rows: &[crate::store::SessionMessage]| rows.iter().map(|row| row.id).collect();
    (
        ids(&full(frame).messages),
        ids(&full(frame).acp_metadata_messages),
    )
}

fn wire(value: &impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(value).unwrap()
}

fn row_ids(frame: &SessionStreamFrame) -> Vec<i64> {
    summary(frame).rows.iter().map(|row| row.id).collect()
}

/// The one frame in `frames`.
#[track_caller]
fn only(frames: Vec<SessionStreamFrame>) -> SessionStreamFrame {
    assert_eq!(frames.len(), 1, "expected one frame, got {frames:?}");
    frames.into_iter().next().unwrap()
}
