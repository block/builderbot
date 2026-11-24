//! The hub: who is subscribed to what, what each of them is still owed, and
//! the loop that turns marks on the session feed into frames.
//!
//! All of it is in memory. The hub task is the only reader of the feed and
//! the only sender on any sink, so for one client a frame is always built
//! from a read made after the frame before it, and frames reach the sink in
//! that order. The commands only change the state under its lock and wake
//! the task; they never read the store or send.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

use crate::store::{SessionDirty, SessionFeed, SessionSummary, Store, SummaryRowsQuery};

use super::frames::{
    SessionStreamFrame, SessionStreamFrameKind, SessionStreamInterest, SessionStreamLevel,
};

/// How long a change waits before it is read, so the rest of its burst is
/// read with it.
pub(super) const GATHER: Duration = Duration::from_millis(30);

/// The least time between a frame and the next delta for one session to one
/// client, at either level.
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
}

#[derive(Default)]
struct HubState {
    clients: HashMap<String, Client>,
    /// Hands out connection tokens and tracked-session revisions.
    counter: u64,
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
    sessions: HashMap<String, Tracked>,
}

/// One session one client wants, and what it is still owed.
struct Tracked {
    /// `Title` or `Summary`.
    level: SessionStreamLevel,
    /// Changes whenever an interest call makes or alters this entry, so a
    /// frame planned for the entry as it was is not sent for what it became.
    revision: u64,
    /// The next frame is a snapshot: none has been sent yet, or the one that
    /// was due found no row to read.
    needs_snapshot: bool,
    /// From when what is owed may be sent, before the cadence is applied.
    /// `None` when nothing is owed.
    ready_at: Option<Instant>,
    /// Rows written since the last frame. Not kept while a snapshot is owed:
    /// the snapshot reads the newest rows whichever they are.
    rows: BTreeSet<i64>,
    last_sent: Option<Instant>,
    /// The session fields of the last frame, as a fingerprint. A delta that
    /// would repeat them and carry no row is not sent: most writes to a
    /// session row touch fields this level does not carry.
    sent_fields: Option<u64>,
    /// The opening prompt's id once a read has found it, so later reads skip
    /// the lookup.
    opening_prompt_id: Option<i64>,
}

impl Tracked {
    fn new(level: SessionStreamLevel, revision: u64, ready_at: Option<Instant>) -> Self {
        Self {
            level,
            revision,
            needs_snapshot: true,
            ready_at,
            rows: BTreeSet::new(),
            last_sent: None,
            sent_fields: None,
            opening_prompt_id: None,
        }
    }

    /// Take a drained mark. Queue and metadata marks belong to the full
    /// level, and row marks do not reach the title level.
    fn mark(&mut self, dirty: &SessionDirty, now: Instant) {
        let rows_matter = self.level == SessionStreamLevel::Summary && !dirty.rows.is_empty();
        if !dirty.session && !rows_matter {
            return;
        }
        if self.needs_snapshot {
            // A snapshot that found no row waits for a write like this one.
            self.ready_at.get_or_insert(now);
            return;
        }
        if rows_matter {
            self.rows.extend(&dirty.rows);
        }
        self.ready_at.get_or_insert(now + GATHER);
    }

    fn due_at(&self) -> Option<Instant> {
        let ready = self.ready_at?;
        Some(match self.last_sent {
            Some(sent) if !self.needs_snapshot => ready.max(sent + SUMMARY_INTERVAL),
            _ => ready,
        })
    }
}

/// A frame the task has decided to build, with what it took out of the
/// client's state to do so.
struct Delivery {
    client_id: String,
    session_id: String,
    revision: u64,
    level: SessionStreamLevel,
    snapshot: bool,
    window: u32,
    rows: BTreeSet<i64>,
    opening_prompt_id: Option<i64>,
}

/// One session's read for a round of deliveries, shared by every client that
/// is owed a frame for it.
struct SummaryRead {
    session_id: String,
    /// `None` when no delivery reads the transcript.
    rows: Option<SummaryRowsQuery>,
}

enum Loaded {
    Summary {
        summary: SessionSummary,
        title_fields: u64,
        summary_fields: u64,
    },
    /// The session row does not exist, or there is no store to read.
    Gone,
    Failed,
}

impl SessionStreamHub {
    pub fn new(feed: Arc<SessionFeed>) -> Self {
        Self {
            feed,
            state: Mutex::default(),
            poke: Notify::new(),
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
    /// one.
    pub fn set_interest(
        &self,
        client_id: &str,
        interest: SessionStreamInterest,
    ) -> Result<(), String> {
        let mut wanted: BTreeMap<String, SessionStreamLevel> = BTreeMap::new();
        for session in interest.sessions {
            if session.level == SessionStreamLevel::Full {
                return Err("The full level of the session stream is not served yet".to_string());
            }
            let level = wanted.entry(session.session_id).or_insert(session.level);
            *level = (*level).max(session.level);
        }
        let now = Instant::now();
        {
            let mut state = self.lock();
            let HubState { clients, counter } = &mut *state;
            let client = clients
                .get_mut(client_id)
                .ok_or_else(|| format!("No session stream is open for client {client_id}"))?;
            let window = interest.summary_window.min(MAX_SUMMARY_WINDOW);
            let new_generation = client.generation != Some(interest.generation);
            let restart = new_generation
                || (client.paused && !interest.paused)
                || client.summary_window != window;
            if new_generation {
                client.seq = 0;
            }
            // A paused client is owed nothing until it asks again.
            let ready_at = (!interest.paused).then_some(now);
            let mut previous = std::mem::take(&mut client.sessions);
            for (session_id, level) in wanted {
                let kept = if restart {
                    None
                } else {
                    previous.remove(&session_id)
                };
                let mut revision = || {
                    *counter += 1;
                    *counter
                };
                let tracked = match kept {
                    Some(tracked) if tracked.level == level => tracked,
                    // Lowered to the title level: the rows are no longer
                    // wanted, and whatever else is owed still is.
                    Some(tracked) if tracked.level > level => Tracked {
                        level,
                        revision: revision(),
                        rows: BTreeSet::new(),
                        sent_fields: None,
                        ..tracked
                    },
                    _ => Tracked::new(level, revision(), ready_at),
                };
                client.sessions.insert(session_id, tracked);
            }
            if interest.paused {
                for tracked in client.sessions.values_mut() {
                    tracked.ready_at = None;
                    tracked.rows.clear();
                }
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
            .any(|client| !client.paused && !client.sessions.is_empty());
        if self.feed.is_active() != wanted {
            self.feed.set_active(wanted);
        }
    }

    /// The hub must outlive a panic on a thread that held its state: the
    /// state is ids, flags and instants, usable as it stands.
    fn lock(&self) -> MutexGuard<'_, HubState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hand the drained marks to the clients that want them, and take out
    /// every frame that is due and has room in its sink. Also returns when
    /// the next one falls due.
    fn plan(
        &self,
        marks: &HashMap<String, SessionDirty>,
        now: Instant,
    ) -> (Vec<Delivery>, Option<Instant>) {
        let mut state = self.lock();
        let mut deliveries = Vec::new();
        let mut next: Option<Instant> = None;
        for (client_id, client) in state.clients.iter_mut() {
            if client.paused {
                continue;
            }
            for (session_id, dirty) in marks {
                if let Some(tracked) = client.sessions.get_mut(session_id) {
                    tracked.mark(dirty, now);
                }
            }
            let mut due = Vec::new();
            for (session_id, tracked) in client.sessions.iter_mut() {
                match tracked.due_at() {
                    Some(at) if at <= now => due.push((at, session_id, tracked)),
                    Some(at) => earliest(&mut next, at),
                    None => {}
                }
            }
            // Longest owed first, so a sink short of room starves nobody.
            due.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
            let mut room = client.sink.room();
            for (_, session_id, tracked) in due {
                if room == 0 {
                    // What is owed stays owed and keeps coalescing.
                    earliest(&mut next, now + SINK_RETRY);
                    break;
                }
                room -= 1;
                tracked.ready_at = None;
                deliveries.push(Delivery {
                    client_id: client_id.clone(),
                    session_id: session_id.clone(),
                    revision: tracked.revision,
                    level: tracked.level,
                    snapshot: tracked.needs_snapshot,
                    window: client.summary_window,
                    rows: std::mem::take(&mut tracked.rows),
                    opening_prompt_id: tracked.opening_prompt_id,
                });
            }
        }
        (deliveries, next)
    }

    /// Build the planned frames from what was read and send them.
    fn deliver(&self, deliveries: Vec<Delivery>, loaded: &HashMap<String, Loaded>, now: Instant) {
        let mut outbox = Vec::new();
        {
            let mut state = self.lock();
            for delivery in deliveries {
                let Some(client) = state.clients.get_mut(&delivery.client_id) else {
                    continue;
                };
                if client.paused {
                    continue;
                }
                let Some(tracked) = client.sessions.get_mut(&delivery.session_id) else {
                    continue;
                };
                if tracked.revision != delivery.revision {
                    // The interest changed this entry while the read ran.
                    // What was taken out for the frame is covered by a
                    // snapshot of the entry as it is now.
                    tracked.needs_snapshot = true;
                    tracked.ready_at.get_or_insert(now);
                    continue;
                }
                let Some(Loaded::Summary {
                    summary,
                    title_fields,
                    summary_fields,
                }) = loaded.get(&delivery.session_id)
                else {
                    // No row: a deleted session, or one not written yet.
                    // Nothing is sent and nothing is retried; if the row is
                    // ever written, that write brings a snapshot. A read
                    // that failed is tried again.
                    tracked.needs_snapshot = true;
                    tracked.ready_at = match loaded.get(&delivery.session_id) {
                        Some(Loaded::Failed) => Some(now + READ_RETRY),
                        _ => None,
                    };
                    continue;
                };
                let opening_prompt_id = summary.opening_prompt_id.or(tracked.opening_prompt_id);
                tracked.opening_prompt_id = opening_prompt_id;
                let title_only = delivery.level == SessionStreamLevel::Title;
                let fields = if title_only {
                    *title_fields
                } else {
                    *summary_fields
                };
                let rows: Vec<_> = if title_only {
                    Vec::new()
                } else if delivery.snapshot {
                    // The read holds at least this client's window of newest
                    // rows, and anything else it holds is older.
                    let skip = summary.rows.len().saturating_sub(delivery.window as usize);
                    summary.rows[skip..].to_vec()
                } else {
                    summary
                        .rows
                        .iter()
                        .filter(|row| delivery.rows.contains(&row.id))
                        .cloned()
                        .collect()
                };
                if !delivery.snapshot && rows.is_empty() && tracked.sent_fields == Some(fields) {
                    continue;
                }
                let frame_summary = SessionSummary {
                    status: summary.status.clone(),
                    acp_title: summary.acp_title.clone(),
                    working_dir: summary.working_dir.clone().filter(|_| !title_only),
                    pipeline: summary.pipeline.clone().filter(|_| !title_only),
                    opening_prompt_id: opening_prompt_id.filter(|_| !title_only),
                    rows,
                };
                client.seq += 1;
                tracked.needs_snapshot = false;
                tracked.last_sent = Some(now);
                tracked.sent_fields = Some(fields);
                outbox.push((
                    Arc::clone(&client.sink),
                    delivery.client_id,
                    client.connection,
                    SessionStreamFrame {
                        // Set by the interest call that made this entry.
                        generation: client.generation.unwrap_or_default(),
                        seq: client.seq,
                        session_id: delivery.session_id,
                        kind: if delivery.snapshot {
                            SessionStreamFrameKind::Snapshot
                        } else {
                            SessionStreamFrameKind::Delta
                        },
                        summary: Some(frame_summary),
                    },
                ));
            }
        }
        // Sent without the lock: a frame can be large, and an interest call
        // must not wait behind its serialization.
        for (sink, client_id, connection, frame) in outbox {
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

fn earliest(next: &mut Option<Instant>, at: Instant) {
    *next = Some(next.map_or(at, |current| current.min(at)));
}

fn fingerprint(fields: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    fields.hash(&mut hasher);
    hasher.finish()
}

impl Loaded {
    fn summary(summary: SessionSummary) -> Self {
        let status = summary.status.as_str();
        let pipeline = summary
            .pipeline
            .as_ref()
            .and_then(|pipeline| serde_json::to_string(pipeline).ok());
        Self::Summary {
            title_fields: fingerprint((status, &summary.acp_title)),
            summary_fields: fingerprint((
                status,
                &summary.acp_title,
                &summary.working_dir,
                &pipeline,
            )),
            summary,
        }
    }
}

/// One read per session for a round of deliveries: the widest snapshot
/// window any client is owed, and every row any client is owed a delta of.
fn reads_for(deliveries: &[Delivery]) -> Vec<SummaryRead> {
    let mut reads: BTreeMap<&str, Option<SummaryRowsQuery>> = BTreeMap::new();
    for delivery in deliveries {
        let rows = reads.entry(delivery.session_id.as_str()).or_default();
        if delivery.level != SessionStreamLevel::Summary
            || (!delivery.snapshot && delivery.rows.is_empty())
        {
            continue;
        }
        let query = rows.get_or_insert_with(SummaryRowsQuery::default);
        if delivery.snapshot {
            query.newest = query.newest.max(delivery.window);
        } else {
            query.ids.extend(&delivery.rows);
        }
        query.opening_prompt_id = query.opening_prompt_id.or(delivery.opening_prompt_id);
    }
    reads
        .into_iter()
        .map(|(session_id, rows)| SummaryRead {
            session_id: session_id.to_owned(),
            rows,
        })
        .collect()
}

/// Read the sessions on the blocking pool, each under its own hold of the
/// store's connection, so a writer waits for one session's read at most.
async fn load(store: Option<Arc<Store>>, reads: Vec<SummaryRead>) -> HashMap<String, Loaded> {
    let Some(store) = store else {
        return HashMap::new();
    };
    let loaded = tokio::task::spawn_blocking(move || {
        reads
            .into_iter()
            .map(|read| {
                let loaded = match store.read_session_summary(&read.session_id, read.rows.as_ref())
                {
                    Ok(Some(summary)) => Loaded::summary(summary),
                    Ok(None) => Loaded::Gone,
                    Err(e) => {
                        log::warn!(
                            "Session stream could not read session {}: {e}",
                            read.session_id
                        );
                        Loaded::Failed
                    }
                };
                (read.session_id, loaded)
            })
            .collect()
    })
    .await;
    loaded.unwrap_or_else(|e| {
        log::warn!("Session stream read did not finish: {e}");
        HashMap::new()
    })
}

/// Drain the feed and send frames, for the life of the app.
///
/// `store` is asked for the store on every round, since a reset replaces it.
///
/// With nothing owed the task is parked on its two wake-ups and no timer
/// runs. With no unpaused client the feed is inactive, so session writes do
/// not wake it either.
pub(super) async fn run(hub: Arc<SessionStreamHub>, store: impl Fn() -> Option<Arc<Store>>) {
    loop {
        // Drained before the read, so the read holds every write these marks
        // stand for. A write that lands in between is in the read too and
        // marks again, which costs one repeated row.
        let marks = hub.feed.drain();
        let (deliveries, next) = hub.plan(&marks, Instant::now());
        if !deliveries.is_empty() {
            let loaded = load(store(), reads_for(&deliveries)).await;
            hub.deliver(deliveries, &loaded, Instant::now());
            continue;
        }
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
