//! The hub: who is subscribed to what, and the loop that turns marks on the
//! session feed into frames.
//!
//! What a client is owed of one session, and what goes in the frame that
//! pays it, is each level's own business: `summary.rs` for the title and
//! summary levels, `full.rs` for the full level. A session a client holds
//! both ways has an entry in each, served side by side. The hub hands both
//! the marks, asks them when a frame is due, does the reads and the sends,
//! and numbers the frames.
//!
//! All of it is in memory. The hub task is the only reader of the feed and
//! the only sender on any sink, so for one client a frame is always built
//! from a read made after the frame before it, and frames reach the sink in
//! that order. The commands only change the state under its lock and wake
//! the task; they never read the store or send.
//!
//! The feed holds this process's writes only. What the hub does about
//! another process's is in `watch.rs`.

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

use crate::store::{
    SessionDirty, SessionFeed, SessionFull, SessionPollRequest, SessionSummary, Store,
};

use super::frames::{
    SessionStreamFrame, SessionStreamFrameKind, SessionStreamInterest, SessionStreamLevel,
};
use super::watch::FOREIGN_CHECK;
use super::{full, summary};

/// How long a change waits before it is read, so the rest of its burst is
/// read with it.
pub(super) const GATHER: Duration = Duration::from_millis(30);

/// The least time between a frame and the next delta for one session to one
/// client, at the title and summary levels. The full level has its own
/// (`full::FULL_INTERVAL`).
pub(super) const SUMMARY_INTERVAL: Duration = Duration::from_millis(300);

/// How soon a client whose sink had no room is tried again.
pub(super) const SINK_RETRY: Duration = Duration::from_millis(100);

/// How soon a read that failed is tried again.
pub(super) const READ_RETRY: Duration = Duration::from_secs(1);

/// The most rows a summary snapshot carries, whatever window a client asks
/// for.
pub(super) const MAX_SUMMARY_WINDOW: u32 = 200;

/// Why a sink did not take a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkError {
    /// The frame is lost and the client is still there. Its next frame shows
    /// a gap in the sequence, which makes it ask again.
    Dropped,
    /// The client is gone.
    Closed,
}

/// Where one client's frames go: a Tauri channel, or the queue its socket
/// forwards.
pub trait FrameSink: Send + Sync {
    /// How many frames the sink takes right now. Only the hub task sends, so
    /// until it does the room can only grow.
    fn room(&self) -> usize;
    fn send(&self, frame: SessionStreamFrame) -> Result<(), SinkError>;
}

/// The stream's state, shared by the hub task and the commands.
pub struct SessionStreamHub {
    feed: Arc<SessionFeed>,
    state: Mutex<HubState>,
    /// Wakes the task for an interest change, which the feed knows nothing
    /// about.
    poke: Notify,
    /// The hub task has ended. It runs for the life of the app, so this
    /// means it panicked. Nobody would send a frame again, so from then on
    /// the hub holds no client and refuses every interest: a client's health
    /// check finds no stream, its interest fails, and it falls back to
    /// polling instead of showing what it last received for ever.
    stopped: AtomicBool,
}

#[derive(Default)]
struct HubState {
    clients: HashMap<String, Client>,
    /// Hands out connection tokens and tracked-session revisions.
    counter: u64,
    /// When the database's `data_version` is next read. `None` while no
    /// unpaused client tracks a session another process may write.
    watch_at: Option<Instant>,
}

impl HubState {
    fn next_id(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }
}

struct Client {
    sink: Arc<dyn FrameSink>,
    /// Names this attachment of a sink, so a socket that closes late cannot
    /// remove the client its replacement set up.
    connection: u64,
    /// `None` until the first interest call.
    generation: Option<u64>,
    paused: bool,
    summary_window: u32,
    /// The sequence number of the last frame sent in this generation.
    seq: u64,
    /// The sessions wanted at the title or summary level.
    sessions: HashMap<String, summary::Tracked>,
    /// The sessions wanted at the full level. One can be in both maps.
    full: HashMap<String, full::Tracked>,
}

impl Client {
    fn wants_nothing(&self) -> bool {
        self.sessions.is_empty() && self.full.is_empty()
    }

    /// Number the next frame and address it.
    fn frame(
        &mut self,
        client_id: &str,
        session_id: &str,
        kind: SessionStreamFrameKind,
        summary: Option<SessionSummary>,
        full: Option<SessionFull>,
    ) -> Outgoing {
        self.seq += 1;
        Outgoing {
            sink: Arc::clone(&self.sink),
            client_id: client_id.to_owned(),
            connection: self.connection,
            frame: SessionStreamFrame {
                // Set by the interest call that made the entry.
                generation: self.generation.unwrap_or_default(),
                seq: self.seq,
                session_id: session_id.to_owned(),
                kind,
                summary,
                full,
            },
        }
    }
}

/// A frame on its way to a sink.
struct Outgoing {
    sink: Arc<dyn FrameSink>,
    client_id: String,
    connection: u64,
    frame: SessionStreamFrame,
}

/// What the task does next.
struct Plan {
    /// The frames that are due and have room in their sinks, by level.
    summaries: Vec<summary::Delivery>,
    fulls: Vec<full::Delivery>,
    /// The database's `data_version` is due a read.
    check_version: bool,
    /// When the next frame or version read falls due.
    next: Option<Instant>,
}

impl Plan {
    fn has_deliveries(&self) -> bool {
        !self.summaries.is_empty() || !self.fulls.is_empty()
    }

    /// Whether this round takes the database's `data_version`: a check is
    /// due, or it does a read that is tagged with the value. Those are the
    /// reads of a session another process may write, and every read at the
    /// full level, where the session row of a session running here is
    /// watched too. A round of this process's own sessions with no chat
    /// open on them, which is most rounds with one Staged running, does not.
    fn wants_version(&self) -> bool {
        self.check_version
            || self.summaries.iter().any(|delivery| !delivery.ours)
            || self.fulls.iter().any(full::Delivery::reads)
    }
}

/// What one round read.
#[derive(Default)]
struct Round {
    /// The database's `data_version` from before the reads, when the round
    /// took it.
    version: Option<i64>,
    /// By session id.
    summaries: HashMap<String, summary::Loaded>,
    /// In the order of the reads, which each full delivery indexes.
    fulls: Vec<full::Loaded>,
}

impl SessionStreamHub {
    pub fn new(feed: Arc<SessionFeed>) -> Self {
        Self {
            feed,
            state: Mutex::default(),
            poke: Notify::new(),
            stopped: AtomicBool::new(false),
        }
    }

    /// The feed this hub drains. Every store the app opens is given it.
    pub fn feed(&self) -> &Arc<SessionFeed> {
        &self.feed
    }

    /// Give a client its sink, replacing any it had along with the interest
    /// declared through that one. Returns the token to disconnect with.
    pub fn connect(&self, client_id: &str, sink: Arc<dyn FrameSink>) -> u64 {
        let mut state = self.lock();
        let connection = state.next_id();
        if self.stopped.load(Ordering::Acquire) {
            return connection;
        }
        state.clients.insert(
            client_id.to_owned(),
            Client {
                sink,
                connection,
                generation: None,
                paused: false,
                summary_window: 0,
                seq: 0,
                sessions: HashMap::new(),
                full: HashMap::new(),
            },
        );
        self.sync_feed(&state);
        connection
    }

    /// Forget a client. With a token, only if that is still its connection.
    pub fn disconnect(&self, client_id: &str, connection: Option<u64>) {
        let mut state = self.lock();
        let current = state.clients.get(client_id).map(|client| client.connection);
        if current.is_some() && (connection.is_none() || connection == current) {
            state.clients.remove(client_id);
            self.sync_feed(&state);
        }
    }

    /// Replace a client's interest. Sessions it adds get a snapshot; a new
    /// generation, a new window or the end of a pause gives every session
    /// one, and so does a cursor at the full level.
    pub fn set_interest(
        &self,
        client_id: &str,
        interest: SessionStreamInterest,
    ) -> Result<(), String> {
        let mut wanted: BTreeMap<String, SessionStreamLevel> = BTreeMap::new();
        let mut wanted_full: BTreeMap<String, Option<SessionPollRequest>> = BTreeMap::new();
        for session in interest.sessions {
            if session.level == SessionStreamLevel::Full {
                let cursor = wanted_full.entry(session.session_id).or_default();
                *cursor = session.cursor.or(cursor.take());
            } else {
                let level = wanted.entry(session.session_id).or_insert(session.level);
                *level = (*level).max(session.level);
            }
        }
        let now = Instant::now();
        {
            let mut state = self.lock();
            // Read under the lock the task's end takes to clear the clients,
            // so an interest cannot land after that and switch the feed on
            // for nobody.
            if self.stopped.load(Ordering::Acquire) {
                return Err("The session stream has stopped".to_string());
            }
            let HubState {
                clients, counter, ..
            } = &mut *state;
            let client = clients
                .get_mut(client_id)
                .ok_or_else(|| format!("No session stream is open for client {client_id}"))?;
            let window = interest.summary_window.min(MAX_SUMMARY_WINDOW);
            let new_generation = client.generation != Some(interest.generation);
            let resumed = client.paused && !interest.paused;
            if new_generation {
                client.seq = 0;
            }
            // A paused client is owed nothing until it asks again.
            let ready_at = (!interest.paused).then_some(now);
            let mut revision = || {
                *counter += 1;
                *counter
            };
            let restart = new_generation || resumed || client.summary_window != window;
            let mut previous = std::mem::take(&mut client.sessions);
            for (session_id, level) in wanted {
                let kept = if restart {
                    None
                } else {
                    previous.remove(&session_id)
                };
                let tracked = match kept {
                    Some(tracked) if tracked.level() == level => tracked,
                    // Lowered to the title level: the client already holds
                    // more than that level sends.
                    Some(tracked) if tracked.level() > level => tracked.lowered(level, revision()),
                    _ => summary::Tracked::new(level, revision(), ready_at),
                };
                client.sessions.insert(session_id, tracked);
            }
            // The summary window is not the full level's: only a new
            // generation and the end of a pause start it over.
            let restart = new_generation || resumed;
            let mut previous = std::mem::take(&mut client.full);
            for (session_id, cursor) in wanted_full {
                let tracked = match (previous.remove(&session_id), cursor) {
                    // Held already, and the client did not ask to be read
                    // again: carry on with deltas.
                    (Some(tracked), None) if !restart => tracked,
                    // Held already, and read again from what the client
                    // says it holds now.
                    (Some(tracked), Some(cursor)) if !restart => {
                        tracked.renewed(cursor, revision(), ready_at)
                    }
                    // A snapshot is owed, from the cursor given now or else
                    // from the last one given.
                    (kept, cursor) => {
                        let cursor = cursor
                            .or(kept.map(full::Tracked::into_cursor))
                            .unwrap_or_default();
                        full::Tracked::new(cursor, revision(), ready_at)
                    }
                };
                client.full.insert(session_id, tracked);
            }
            if interest.paused {
                client
                    .sessions
                    .values_mut()
                    .for_each(summary::Tracked::pause);
                client.full.values_mut().for_each(full::Tracked::pause);
            }
            client.generation = Some(interest.generation);
            client.paused = interest.paused;
            client.summary_window = window;
            self.sync_feed(&state);
        }
        self.poke.notify_one();
        Ok(())
    }

    /// Whether the last frame sent to the client in `generation` is the one
    /// it says it last received. `false` also when the hub has no such
    /// stream, and for a frame that is still on its way.
    pub fn check(&self, client_id: &str, generation: u64, last_seq: u64) -> bool {
        self.lock()
            .clients
            .get(client_id)
            .is_some_and(|client| client.generation == Some(generation) && client.seq == last_seq)
    }

    /// The feed records marks only while someone would be sent them, so with
    /// no client, or only paused ones, a session write costs what it did
    /// before the stream existed. Called under the state lock, so two
    /// changes cannot settle the feed in the wrong order.
    fn sync_feed(&self, state: &HubState) {
        let wanted = state
            .clients
            .values()
            .any(|client| !client.paused && !client.wants_nothing());
        if self.feed.is_active() != wanted {
            self.feed.set_active(wanted);
        }
    }

    /// The hub task has ended: forget every client and switch the feed off,
    /// so writers stop recording marks nobody will drain.
    fn stop(&self) {
        let mut state = self.lock();
        self.stopped.store(true, Ordering::Release);
        state.clients.clear();
        state.watch_at = None;
        self.sync_feed(&state);
    }

    /// The hub must outlive a panic on a thread that held its state: the
    /// state is ids, flags and instants, usable as it stands.
    fn lock(&self) -> MutexGuard<'_, HubState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hand the drained marks to the clients that want them, and take out
    /// every frame that is due and has room in its sink. Also says whether
    /// the database's `data_version` is due a read, and when the next thing
    /// falls due.
    fn plan(&self, marks: &HashMap<String, SessionDirty>, now: Instant) -> Plan {
        /// One client's entry for a session, at one level or the other.
        enum Due<'a> {
            Summary(&'a mut summary::Tracked),
            Full(&'a mut full::Tracked),
        }
        let mut state = self.lock();
        let HubState {
            clients, watch_at, ..
        } = &mut *state;
        let mut plan = Plan {
            summaries: Vec::new(),
            fulls: Vec::new(),
            check_version: false,
            next: None,
        };
        let mut watching = false;
        for (client_id, client) in clients.iter_mut() {
            if client.paused {
                continue;
            }
            // At the full level every session is watched, whoever runs it.
            watching |= !client.full.is_empty()
                || client.sessions.values().any(|tracked| !tracked.is_ours());
            for (session_id, dirty) in marks {
                if let Some(tracked) = client.sessions.get_mut(session_id) {
                    tracked.mark(dirty, now);
                }
                if let Some(tracked) = client.full.get_mut(session_id) {
                    tracked.mark(dirty, now);
                }
            }
            let summaries = client
                .sessions
                .iter_mut()
                .map(|(id, tracked)| (tracked.due_at(), id, Due::Summary(tracked)));
            let fulls = client
                .full
                .iter_mut()
                .map(|(id, tracked)| (tracked.due_at(), id, Due::Full(tracked)));
            let mut due = Vec::new();
            for (at, session_id, entry) in summaries.chain(fulls) {
                match at {
                    Some(at) if at <= now => due.push((at, session_id, entry)),
                    Some(at) => earliest(&mut plan.next, at),
                    None => {}
                }
            }
            // Longest owed first, so a sink short of room starves nobody.
            due.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
            let mut room = client.sink.room();
            for (_, session_id, entry) in due {
                if room == 0 {
                    // What is owed stays owed and keeps coalescing.
                    earliest(&mut plan.next, now + SINK_RETRY);
                    break;
                }
                room -= 1;
                match entry {
                    Due::Summary(tracked) => plan.summaries.push(tracked.take(
                        client_id,
                        session_id,
                        client.summary_window,
                    )),
                    Due::Full(tracked) => plan.fulls.push(tracked.take(client_id, session_id, now)),
                }
            }
        }
        // The first read of the version comes a whole interval after the
        // watch starts: the read that made a session watched took the value
        // itself.
        plan.check_version = match *watch_at {
            _ if !watching => {
                *watch_at = None;
                false
            }
            Some(at) if at <= now => {
                *watch_at = Some(now + FOREIGN_CHECK);
                true
            }
            Some(at) => {
                earliest(&mut plan.next, at);
                false
            }
            None => {
                *watch_at = Some(now + FOREIGN_CHECK);
                earliest(&mut plan.next, now + FOREIGN_CHECK);
                false
            }
        };
        plan
    }

    /// Build the planned frames from what was read and send them.
    ///
    /// When the round took the database's `data_version`, every session
    /// another process may have written since its own last read is then put
    /// down for a re-read.
    fn deliver(&self, plan: Plan, round: &Round, now: Instant) {
        let version = round.version;
        let mut outbox = Vec::new();
        {
            let mut state = self.lock();
            for delivery in &plan.summaries {
                let client = state.clients.get_mut(&delivery.client_id);
                let Some(client) = client.filter(|client| !client.paused) else {
                    continue;
                };
                let Some(tracked) = client.sessions.get_mut(&delivery.session_id) else {
                    continue;
                };
                let loaded = round.summaries.get(&delivery.session_id);
                if let Some((kind, summary)) = tracked.deliver(delivery, loaded, version, now) {
                    outbox.push(client.frame(
                        &delivery.client_id,
                        &delivery.session_id,
                        kind,
                        Some(summary),
                        None,
                    ));
                }
            }
            for delivery in &plan.fulls {
                let client = state.clients.get_mut(&delivery.client_id);
                let Some(client) = client.filter(|client| !client.paused) else {
                    continue;
                };
                let Some(tracked) = client.full.get_mut(&delivery.session_id) else {
                    continue;
                };
                let loaded = delivery.read().and_then(|read| round.fulls.get(read));
                if let Some((kind, full)) = tracked.deliver(delivery, loaded, version, now) {
                    outbox.push(client.frame(
                        &delivery.client_id,
                        &delivery.session_id,
                        kind,
                        None,
                        full,
                    ));
                }
            }
            if let Some(version) = version {
                for client in state.clients.values_mut().filter(|client| !client.paused) {
                    for tracked in client.sessions.values_mut() {
                        tracked.check_version(version, now);
                    }
                    for tracked in client.full.values_mut() {
                        tracked.check_version(version, now);
                    }
                }
            }
        }
        // Sent without the lock: a frame can be large, and an interest call
        // must not wait behind its serialization.
        for Outgoing {
            sink,
            client_id,
            connection,
            frame,
        } in outbox
        {
            #[cfg(feature = "perf-harness")]
            crate::perf_harness::stream_frames::sent(&frame);
            match sink.send(frame) {
                Ok(()) => {}
                Err(SinkError::Closed) => self.disconnect(&client_id, Some(connection)),
                Err(SinkError::Dropped) => {
                    log::warn!("Session stream frame for client {client_id} was dropped");
                }
            }
        }
    }
}

pub(super) fn earliest(next: &mut Option<Instant>, at: Instant) {
    *next = Some(next.map_or(at, |current| current.min(at)));
}

pub(super) fn fingerprint(fields: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    fields.hash(&mut hasher);
    hasher.finish()
}

/// Do a round's reads on the blocking pool.
///
/// With `version`, the database's `data_version` is taken first. Taken
/// before the reads, it can only be older than what they saw, so a commit
/// that lands in between is caught by the next check and costs a repeated
/// read, where the other order would lose it.
async fn load(
    store: Option<Arc<Store>>,
    summaries: Vec<summary::SummaryRead>,
    fulls: Vec<full::Read>,
    version: bool,
) -> Round {
    let Some(store) = store else {
        return Round::default();
    };
    let round = tokio::task::spawn_blocking(move || Round {
        version: version
            .then(|| {
                store
                    .data_version()
                    .map_err(|e| log::warn!("Session stream could not read the data version: {e}"))
                    .ok()
            })
            .flatten(),
        summaries: summary::load(&store, summaries),
        fulls: full::load(&store, fulls),
    })
    .await;
    round.unwrap_or_else(|e| {
        log::warn!("Session stream read did not finish: {e}");
        Round::default()
    })
}

/// Drain the feed and send frames, for the life of the app.
///
/// `store` is asked for the store on every round, since a reset replaces it.
///
/// With nothing owed the task is parked on its two wake-ups and no timer
/// runs, unless a client tracks a session another process may write: then
/// one runs once a second, for the `data_version` read (`watch.rs`). With
/// no unpaused client the feed is inactive, so session writes do not wake
/// the task either.
pub(super) async fn run(hub: Arc<SessionStreamHub>, store: impl Fn() -> Option<Arc<Store>>) {
    /// Runs when the task ends however it ends, a panic included.
    struct Stop<'a>(&'a SessionStreamHub);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            self.0.stop();
        }
    }
    let _stop = Stop(&hub);
    loop {
        // Drained before the read, so the read holds every write these marks
        // stand for. A write that lands in between is in the read too and
        // marks again, which costs one repeated row.
        let marks = hub.feed.drain();
        let mut plan = hub.plan(&marks, Instant::now());
        if plan.has_deliveries() || plan.check_version {
            let round = load(
                store(),
                summary::reads_for(&plan.summaries),
                full::reads_for(&mut plan.fulls),
                plan.wants_version(),
            )
            .await;
            hub.deliver(plan, &round, Instant::now());
            continue;
        }
        let next = plan.next;
        let woken = async {
            tokio::select! {
                _ = hub.feed.marked() => {}
                _ = hub.poke.notified() => {}
            }
        };
        match next {
            Some(at) => {
                tokio::select! {
                    _ = tokio::time::sleep_until(at) => {}
                    _ = woken => {}
                }
            }
            None => woken.await,
        }
    }
}
