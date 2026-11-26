//! The full level: what one client is owed of one session it has open as a
//! chat, and the frame that pays it.
//!
//! A full frame is the pane's poll (`Store::poll_session`) sent on change
//! instead of on a timer. It has the poll's four parts and reads them
//! through the poll's own reads, so rows arrive in the projection the pane
//! already merges: an in-flight tool row whole, a settled one with its large
//! bodies as markers.
//!
//! - **Snapshot.** What `poll_session` returns for the cursor the client
//!   gave with its interest. It is incremental from what the client holds,
//!   so the client merges it and does not replace its window. A client asks
//!   for another one by giving a cursor again. Rows written while a
//!   snapshot is owed are read with it by id, whether or not the cursor
//!   covers them, so a delta that was still owed when the cursor arrived is
//!   not lost.
//! - **Delta.** The parts that were written: the session row when a write to
//!   it was marked and it differs from the one last sent, the whole queue
//!   when a queue write was marked, and the marked rows read by id. A
//!   metadata row is read around the transcript's metadata filter, so the
//!   update that settles a tool call is delivered and not filtered away.
//!   Rows are upserted by id; applying a delta twice changes nothing.
//! - **Cadence.** After the gather, at most one delta per [`FULL_INTERVAL`],
//!   which is how often the agent writer flushes text.
//! - **Large tool rows.** A tool call still in flight is rewritten whole on
//!   every update. One whose row is over [`LARGE_ROW_BYTES`] is sent at most
//!   once per [`LARGE_ROW_INTERVAL`]; the copies in between are skipped and
//!   the newest goes when the interval is up. The update that settles it is
//!   small and goes at once.
//!
//! **Status arrives on two channels.** `session-status-changed` is emitted
//! by whoever wrote the status, after the write, and reaches a client by a
//! route that is not ordered with this stream. The event carries nothing a
//! frame can be ordered against, so a frame read just before a status write
//! can reach a client after that write's event. What the stream guarantees
//! instead, and what a pane relies on in place of the poll's
//! `statusEventVersion` guard:
//!
//! 1. Every write to the session row is followed by a frame carrying the
//!    row as read after that write, unless the client was already sent
//!    exactly that row. So the frame that agrees with a status event always
//!    comes, and it comes last.
//! 2. Session rows reach a client in the order they were read.
//! 3. A frame that carries the session row holds back nothing that is owed.
//!    The runner writes a turn's last text before the status that ends the
//!    turn, so the frame reporting a terminal status holds that text, or it
//!    was sent before.
//!
//! So a pane that has applied a status event keeps the event's status over
//! the row of a frame that disagrees with it, and takes the status from
//! frames again from the first row that agrees.
//!
//! **Another Staged process.** The feed marks this process's writes only.
//! `data_version` says that another process committed, not what, and the
//! rows a full client holds are too many to compare here as the summary
//! level's are (`watch.rs`). So for a session this process is not running,
//! the client is sent a `Resync` and answers with its cursor, at most once a
//! second and once per answer. For a session this process *is* running, the
//! transcript is its own and only the session row can be written elsewhere
//! (another instance's Stop), so the row alone is read again and sent if it
//! differs. That is why the version is read for every full subscription,
//! whoever runs the session. Once that row says the session no longer runs
//! here, it is a session another process may write, and the client is asked
//! once for whatever that may have been.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use tokio::time::Instant;

use crate::store::{
    in_flight_tool_row_len, SessionDirty, SessionFull, SessionMessage, SessionPollRequest, Store,
};

use super::frames::SessionStreamFrameKind;
use super::hub::{earliest, fingerprint, GATHER, READ_RETRY};
use super::watch;

/// The least time between a full frame and the next delta for one session
/// to one client.
pub(super) const FULL_INTERVAL: Duration = Duration::from_millis(150);

/// The serialized size past which an in-flight tool row is spaced out.
pub(super) const LARGE_ROW_BYTES: usize = 32 * 1024;

/// The least time between two copies of one large in-flight tool row.
pub(super) const LARGE_ROW_INTERVAL: Duration = Duration::from_millis(500);

/// What a delta is owed: ids and flags, as the feed's marks are.
#[derive(Clone, Default)]
struct Owed {
    rows: BTreeSet<i64>,
    metadata_rows: BTreeSet<i64>,
    session: bool,
    queue: bool,
}

impl Owed {
    fn merge(&mut self, other: &Owed) {
        self.rows.extend(&other.rows);
        self.metadata_rows.extend(&other.metadata_rows);
        self.session |= other.session;
        self.queue |= other.queue;
    }
}

/// One session one client wants at the full level, and what it is still
/// owed.
pub(super) struct Tracked {
    /// Changes whenever the client gives a cursor, so a frame planned
    /// before that is not sent after it.
    revision: u64,
    /// Where the next snapshot reads from: the last cursor the client gave.
    cursor: SessionPollRequest,
    /// The next frame is a snapshot: none has been sent since the cursor
    /// was given, or the one that was due found no session row.
    needs_snapshot: bool,
    /// From when what is owed may be sent, before the cadence is applied.
    /// `None` when nothing new is owed.
    ready_at: Option<Instant>,
    /// Written since the last frame. While a snapshot is owed, the rows in
    /// it are read with the snapshot beside the cursor's own.
    owed: Owed,
    /// When the large rows that were held back, which are in `owed` again,
    /// may go.
    held_until: Option<Instant>,
    /// When a `Resync` is to be sent.
    resync_at: Option<Instant>,
    /// A `Resync` was sent and the client has not given a cursor since.
    awaiting_cursor: bool,
    last_sent: Option<Instant>,
    /// The session row last sent, as a fingerprint.
    sent_session: Option<u64>,
    /// When each large in-flight tool row was last sent, among the visible
    /// rows and among the metadata rows. A visible tool row is in both.
    large_rows: HashMap<i64, Instant>,
    large_metadata: HashMap<i64, Instant>,
    /// The last read of the session row found it running under this
    /// process.
    ours: bool,
    /// The database's `data_version` just before the last read that covered
    /// everything another process could have written.
    read_version: Option<i64>,
}

impl Tracked {
    pub(super) fn new(
        cursor: SessionPollRequest,
        revision: u64,
        ready_at: Option<Instant>,
    ) -> Self {
        Self {
            revision,
            cursor,
            needs_snapshot: true,
            ready_at,
            owed: Owed::default(),
            held_until: None,
            resync_at: None,
            awaiting_cursor: false,
            last_sent: None,
            sent_session: None,
            large_rows: HashMap::new(),
            large_metadata: HashMap::new(),
            ours: false,
            read_version: None,
        }
    }

    /// The last cursor the client gave.
    pub(super) fn into_cursor(self) -> SessionPollRequest {
        self.cursor
    }

    /// The client gave a cursor for a session it already holds: a snapshot
    /// from it is owed. What was owed before is owed still, since the
    /// cursor says what the client holds and not what it was about to be
    /// sent.
    pub(super) fn renewed(
        self,
        cursor: SessionPollRequest,
        revision: u64,
        ready_at: Option<Instant>,
    ) -> Self {
        Self {
            owed: self.owed,
            ..Self::new(cursor, revision, ready_at)
        }
    }

    /// Take a drained mark. Every part of one reaches this level.
    pub(super) fn mark(&mut self, dirty: &SessionDirty, now: Instant) {
        self.owed.rows.extend(&dirty.rows);
        self.owed.metadata_rows.extend(&dirty.metadata_rows);
        self.owed.session |= dirty.session;
        self.owed.queue |= dirty.queue;
        // A snapshot does not wait for the rest of a burst: it reads
        // whatever is there. One that found no row waits for a write like
        // this one.
        let gather = if self.needs_snapshot {
            Duration::ZERO
        } else {
            GATHER
        };
        self.ready_at.get_or_insert(now + gather);
    }

    /// Another process may have written the session since it was last read
    /// in full, given the database's `data_version` now.
    pub(super) fn check_version(&mut self, version: i64, now: Instant) {
        if self.read_version == Some(version) {
            return;
        }
        if self.needs_snapshot {
            // One that found no row: the row may exist now.
            self.ready_at.get_or_insert(now);
        } else if self.ours {
            // Its transcript is written here alone. The session row is not:
            // read it, and the delta drops it again if it is as it was.
            self.owed.session = true;
            self.ready_at.get_or_insert(now);
        } else if !self.awaiting_cursor {
            self.resync_at.get_or_insert(now);
        }
    }

    /// The client is paused: it is owed nothing until it asks again.
    pub(super) fn pause(&mut self) {
        self.ready_at = None;
        self.owed = Owed::default();
        self.held_until = None;
        self.resync_at = None;
    }

    pub(super) fn due_at(&self) -> Option<Instant> {
        if self.needs_snapshot {
            return self.ready_at;
        }
        let delta = self.ready_at.map(|ready| match self.last_sent {
            Some(sent) => ready.max(sent + FULL_INTERVAL),
            None => ready,
        });
        [delta, self.held_until, self.resync_at]
            .into_iter()
            .flatten()
            .min()
    }

    /// Take what is due out for a frame. A `Resync` goes alone; a delta
    /// that is due as well is taken on the next round.
    pub(super) fn take(&mut self, client_id: &str, session_id: &str, now: Instant) -> Delivery {
        let what = if self.needs_snapshot {
            self.ready_at = None;
            self.held_until = None;
            let owed = std::mem::take(&mut self.owed);
            let mut request = self.cursor.clone();
            also(&mut request.refetch_ids, &owed.rows);
            also(&mut request.metadata_refetch_ids, &owed.metadata_rows);
            What::Snapshot { request, owed }
        } else if self.resync_at.is_some_and(|at| at <= now) {
            self.resync_at = None;
            self.awaiting_cursor = true;
            What::Resync
        } else {
            self.ready_at = None;
            self.held_until = None;
            What::Delta(std::mem::take(&mut self.owed))
        };
        Delivery {
            client_id: client_id.to_owned(),
            session_id: session_id.to_owned(),
            revision: self.revision,
            what,
            read: None,
        }
    }

    /// Build the frame for `delivery` from what was read, or `None` when
    /// there is nothing to send.
    ///
    /// `version` is the database's `data_version` from before the read, when
    /// the round took it.
    pub(super) fn deliver(
        &mut self,
        delivery: &Delivery,
        loaded: Option<&Loaded>,
        version: Option<i64>,
        now: Instant,
    ) -> Option<(SessionStreamFrameKind, Option<SessionFull>)> {
        if self.revision != delivery.revision {
            // The client gave a cursor while the read ran. The snapshot
            // from that cursor is owed, and reads what this frame held.
            if let What::Snapshot { owed, .. } | What::Delta(owed) = &delivery.what {
                self.owed.merge(owed);
                self.ready_at.get_or_insert(now);
            }
            return None;
        }
        let (snapshot, owed) = match &delivery.what {
            What::Resync => return Some((SessionStreamFrameKind::Resync, None)),
            What::Snapshot { owed, .. } => (true, owed),
            What::Delta(owed) => (false, owed),
        };
        let read = match loaded {
            Some(Loaded::Read(read)) if !read.gone() => read,
            Some(Loaded::Read(_)) => {
                // No session row: a deleted session, or one not written
                // yet. Nothing is sent and nothing is retried; if the row
                // is ever written, that write brings a snapshot, whichever
                // process makes it.
                self.needs_snapshot = true;
                self.pause();
                self.ours = false;
                self.read_version = version;
                return None;
            }
            // The read failed and is tried again, or there was no store to
            // read. Either way what was taken out is owed still.
            failed => {
                self.owed.merge(owed);
                self.ready_at = matches!(failed, Some(Loaded::Failed)).then_some(now + READ_RETRY);
                return None;
            }
        };
        if read.full.session.is_some() {
            self.ours = read.ours;
        }
        let (kind, full) = match snapshot {
            true => {
                self.needs_snapshot = false;
                self.sent_session = read.session_print;
                self.large_rows = read.large_rows.iter().map(|&id| (id, now)).collect();
                self.large_metadata = read.large_metadata.iter().map(|&id| (id, now)).collect();
                // Everything another process could have written is in it.
                self.read_version = version;
                (SessionStreamFrameKind::Snapshot, read.full.clone())
            }
            false => {
                let mut full = SessionFull::default();
                if owed.session {
                    if read.session_print != self.sent_session {
                        full.session = read.full.session.clone();
                        self.sent_session = read.session_print;
                    }
                    // Of a session running here, the row is all another
                    // process can write, and it has just been read.
                    if self.ours {
                        self.read_version = version;
                    }
                }
                if owed.queue {
                    full.queued_messages = read.full.queued_messages.clone();
                }
                // A frame with the session row holds nothing back, so the
                // status it reports never precedes a row written before it.
                let mut pick = Pick {
                    flush: full.session.is_some(),
                    now,
                    held: &mut self.held_until,
                };
                full.messages = pick.rows(
                    &read.full.messages,
                    &owed.rows,
                    &read.large_rows,
                    &mut self.large_rows,
                    &mut self.owed.rows,
                );
                full.acp_metadata_messages = pick.rows(
                    &read.full.acp_metadata_messages,
                    &owed.metadata_rows,
                    &read.large_metadata,
                    &mut self.large_metadata,
                    &mut self.owed.metadata_rows,
                );
                if full.is_empty() {
                    return None;
                }
                (SessionStreamFrameKind::Delta, full)
            }
        };
        self.last_sent = Some(now);
        Some((kind, Some(full)))
    }
}

/// Choosing the rows of one delta: the ones this client is owed, less the
/// large in-flight tool rows it was sent too recently.
struct Pick<'a> {
    /// Nothing is held back.
    flush: bool,
    now: Instant,
    /// When the earliest held row may go.
    held: &'a mut Option<Instant>,
}

impl Pick<'_> {
    /// `read` is one of a read's two lists, `large` the ids in it of large
    /// in-flight tool rows, and `sent` when this client was last sent each
    /// such row. A row held back goes into `owed` again.
    fn rows(
        &mut self,
        read: &[SessionMessage],
        wanted: &BTreeSet<i64>,
        large: &BTreeSet<i64>,
        sent: &mut HashMap<i64, Instant>,
        owed: &mut BTreeSet<i64>,
    ) -> Vec<SessionMessage> {
        let mut picked = Vec::new();
        for row in read.iter().filter(|row| wanted.contains(&row.id)) {
            if !large.contains(&row.id) {
                // Small, or settled: it goes, and is spaced no longer.
                sent.remove(&row.id);
            } else {
                match sent.get(&row.id) {
                    Some(&last) if !self.flush && last + LARGE_ROW_INTERVAL > self.now => {
                        owed.insert(row.id);
                        earliest(self.held, last + LARGE_ROW_INTERVAL);
                        continue;
                    }
                    _ => {
                        sent.insert(row.id, self.now);
                    }
                }
            }
            picked.push(row.clone());
        }
        picked
    }
}

/// A frame the task has decided to build, with what it took out of the
/// client's state to do so.
pub(super) struct Delivery {
    pub(super) client_id: String,
    pub(super) session_id: String,
    revision: u64,
    what: What,
    /// Which of the round's reads it is built from. `None` for a `Resync`,
    /// which reads nothing.
    read: Option<usize>,
}

enum What {
    /// Read with `request`: the client's cursor, and the rows of `owed` by
    /// id.
    Snapshot {
        request: SessionPollRequest,
        owed: Owed,
    },
    Delta(Owed),
    Resync,
}

/// Add `ids` to the ids a cursor reads by id.
fn also(refetch: &mut Vec<i64>, ids: &BTreeSet<i64>) {
    for id in ids {
        if !refetch.contains(id) {
            refetch.push(*id);
        }
    }
}

impl Delivery {
    /// Whether the frame is built from a read. The round then takes the
    /// database's `data_version` first, to tag the read with.
    pub(super) fn reads(&self) -> bool {
        !matches!(self.what, What::Resync)
    }

    pub(super) fn read(&self) -> Option<usize> {
        self.read
    }
}

/// One read for a round of deliveries.
pub(super) struct Read {
    session_id: String,
    request: SessionPollRequest,
    session: bool,
    queue: bool,
}

pub(super) enum Loaded {
    Read(Box<LoadedFull>),
    Failed,
}

/// What was read, with what the hub works out from it once for every
/// client.
pub(super) struct LoadedFull {
    full: SessionFull,
    /// The session row was asked for, so its absence means it does not
    /// exist.
    asked_session: bool,
    /// The fingerprint of the session row, when it was read.
    session_print: Option<u64>,
    /// Whether this process is the one writing the session. Only meaningful
    /// with the session row.
    ours: bool,
    /// The ids of the large in-flight tool rows among `full.messages` and
    /// among `full.acp_metadata_messages`.
    large_rows: BTreeSet<i64>,
    large_metadata: BTreeSet<i64>,
}

impl LoadedFull {
    fn gone(&self) -> bool {
        self.asked_session && self.full.session.is_none()
    }
}

/// The reads for a round of deliveries, each of which is told which one is
/// its own. A snapshot is read for its client alone, since each client has
/// its own cursor. The deltas of one session share a read: every row and
/// part any client is owed.
pub(super) fn reads_for(deliveries: &mut [Delivery]) -> Vec<Read> {
    let mut reads = Vec::new();
    let mut deltas: BTreeMap<String, (usize, Owed)> = BTreeMap::new();
    for delivery in deliveries {
        match &delivery.what {
            What::Resync => {}
            What::Snapshot { request, .. } => {
                delivery.read = Some(reads.len());
                reads.push(Read {
                    session_id: delivery.session_id.clone(),
                    request: request.clone(),
                    session: true,
                    queue: true,
                });
            }
            What::Delta(owed) => {
                let (read, all) = deltas
                    .entry(delivery.session_id.clone())
                    .or_insert_with(|| {
                        reads.push(Read {
                            session_id: delivery.session_id.clone(),
                            request: SessionPollRequest::default(),
                            session: false,
                            queue: false,
                        });
                        (reads.len() - 1, Owed::default())
                    });
                all.merge(owed);
                delivery.read = Some(*read);
            }
        }
    }
    for (read, owed) in deltas.into_values() {
        let read = &mut reads[read];
        read.request = SessionPollRequest::by_ids(
            owed.rows.into_iter().collect(),
            owed.metadata_rows.into_iter().collect(),
        );
        read.session = owed.session;
        read.queue = owed.queue;
    }
    reads
}

/// Do the reads, each under its own hold of the store's connection, so a
/// writer waits for one session's read at most. Runs on the blocking pool.
pub(super) fn load(store: &Store, reads: Vec<Read>) -> Vec<Loaded> {
    reads
        .into_iter()
        .map(|read| {
            match store.read_session_full(&read.session_id, &read.request, read.session, read.queue)
            {
                Ok(full) => Loaded::read(full, read.session),
                Err(e) => {
                    log::warn!(
                        "Session stream could not read session {} in full: {e}",
                        read.session_id
                    );
                    Loaded::Failed
                }
            }
        })
        .collect()
}

impl Loaded {
    fn read(full: SessionFull, asked_session: bool) -> Self {
        let large = |rows: &[SessionMessage]| {
            rows.iter()
                .filter(|row| in_flight_tool_row_len(row).is_some_and(|len| len > LARGE_ROW_BYTES))
                .map(|row| row.id)
                .collect()
        };
        Self::Read(Box::new(LoadedFull {
            asked_session,
            session_print: full
                .session
                .as_ref()
                .map(|session| fingerprint(serde_json::to_string(session).ok())),
            ours: full
                .session
                .as_ref()
                .is_some_and(|session| watch::is_ours(&session.status, session.owner_pid)),
            large_rows: large(&full.messages),
            large_metadata: large(&full.acp_metadata_messages),
            full,
        }))
    }
}
