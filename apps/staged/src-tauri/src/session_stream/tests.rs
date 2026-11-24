//! The hub, driven through closure-like sinks over an in-memory store, on a
//! paused clock.
//!
//! The clock only moves when every task is idle and no blocking read is
//! outstanding, so a test that waits a millisecond has let the hub finish
//! whatever it was woken for.

mod cadence;
mod clients;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

use crate::store::{MessageRole, Session, SessionFeed, SessionSummary, Store};

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
    task: tokio::task::JoinHandle<()>,
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
        let feed = Arc::new(SessionFeed::default());
        let store = Arc::new(Store::in_memory().unwrap().with_session_feed(feed.clone()));
        let hub = Arc::new(SessionStreamHub::new(feed.clone()));
        let task = tokio::spawn(hub::run(hub.clone(), {
            let store = store.clone();
            move || readable.then(|| store.clone())
        }));
        Self {
            hub,
            store,
            feed,
            task,
        }
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

fn row_ids(frame: &SessionStreamFrame) -> Vec<i64> {
    summary(frame).rows.iter().map(|row| row.id).collect()
}

/// The one frame in `frames`.
#[track_caller]
fn only(frames: Vec<SessionStreamFrame>) -> SessionStreamFrame {
    assert_eq!(frames.len(), 1, "expected one frame, got {frames:?}");
    frames.into_iter().next().unwrap()
}
