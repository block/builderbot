//! Store writes handed to one writer thread.
//!
//! A session thread reads its agent's output, runs its setup timers and
//! serves its MCP servers, all on one current-thread runtime. A store call
//! on that thread waits for the connection lock, and while it waits none of
//! the rest runs: one slow lock holder anywhere in the app could stop a
//! session from reading an agent that had long since answered, and fire its
//! setup deadline late.
//!
//! So a session thread does not write to the store itself. It puts each
//! write on this queue and carries on; one writer thread per store takes
//! them off in order and runs them. A long lock hold then delays what
//! appears on screen, not the session.
//!
//! - **Order.** The queue is first in, first out, so a session's writes land
//!   in the order it made them, and a request queued after them (a read, a
//!   barrier) sees all of them.
//! - **Rows not written yet.** A write that adds a transcript row and later
//!   ones that update it name the row by a [`RowSlot`], which the writer
//!   thread fills with the row id when it inserts the row. The session
//!   never needs the id itself.
//! - **Streaming text.** A [`RowWrite`] replaces the row's whole content, so
//!   one queued while another for the same row is still waiting is folded
//!   into that one, wherever it sits in the queue, instead of joining it. A
//!   session streaming faster than the writer can store has one write
//!   waiting per row, carrying the latest text. The waiting write keeps its
//!   place, so the row is still inserted where its first write was queued
//!   and row ids keep their order. The folded text is stored at that
//!   place, ahead of whatever was queued between the two writes; nothing
//!   queued reads a streamed row's content, and each write is the whole
//!   text. A write already running takes no fold: the next one for its row
//!   joins the queue.
//! - **Answers.** A write whose result the caller needs ([`Store::queued`])
//!   gets it through a oneshot the caller awaits, which leaves its runtime
//!   free while it waits. A request whose caller has stopped waiting (a
//!   setup step that timed out) is dropped unrun, so a write the caller
//!   gave up on does not land after it moved on.
//! - **Flushing.** [`Store::flush_writes`] waits until everything queued
//!   before it is stored, and [`Store::flush_writes_blocking`] does the same
//!   for a thread that is not on a runtime (the quit path). Dropping the
//!   store drains the queue before the connection closes.
//! - **Backlog.** The queue has no limit, since a limit would make a
//!   session thread wait. Writes that cannot fold (tool-call metadata,
//!   event rows, delivered messages) still pile up while the writer thread
//!   is held up, so a deep queue or an old write at its front is reported
//!   in the log (see [`backlog`]).
//!
//! Writes from anywhere else (UI commands, the session runner between
//! turns) still go straight to the store. They are not ordered against
//! queued ones, which is why a session flushes when its connection ends:
//! anything that reads the transcript afterwards, or writes to it from
//! another thread, sees it complete.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use self::backlog::{Backlog, QueueSnapshot, Report};
use super::{AcpMessageMetadata, MessageRole, Store, StoreError};

mod backlog;
#[cfg(test)]
mod tests;

/// A queued write, run on the writer thread against the store.
type WriteJob = Box<dyn FnOnce(&Store) + Send>;

/// A transcript row that a queued write adds and later ones update.
///
/// Empty until the writer thread inserts the row; the first [`RowWrite`]
/// that finds it empty inserts, and every later one updates. A failed insert
/// leaves it empty, so the next write for the row tries the insert again.
/// Clones name the same row.
#[derive(Clone, Default)]
pub struct RowSlot(Arc<OnceLock<i64>>);

impl RowSlot {
    /// A row no write has added yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// A row already in the store, under `id`.
    pub fn known(id: i64) -> Self {
        let slot = Self::new();
        let _ = slot.0.set(id);
        slot
    }

    /// The row's id, once the writer thread has inserted it.
    pub fn id(&self) -> Option<i64> {
        self.0.get().copied()
    }

    /// The key a waiting write for this row is found under while the row
    /// has not been inserted.
    fn unwritten_key(&self) -> RowKey {
        RowKey::Unwritten(Arc::as_ptr(&self.0) as usize)
    }

    /// The key a waiting write for this row is found under once the row is
    /// in the store.
    fn stored_key(&self) -> Option<RowKey> {
        self.id().map(RowKey::Stored)
    }
}

/// What the queue knows a transcript row by, to find the write waiting for
/// it.
///
/// A row in the store goes by its id, not by its slot: two slots can name
/// one stored row ([`RowSlot::known`], which a writer catching up a
/// transcript makes for rows another writer added), and their writes must
/// fold into each other, so that the last one queued is the last one
/// stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum RowKey {
    /// A row in the store, by id.
    Stored(i64),
    /// A row whose slot was empty when its write was queued, by the slot's
    /// address. The waiting write holds the slot, so the address names no
    /// other row for as long as the key is in the queue's index. Nothing
    /// else can name the row yet: it has no id.
    Unwritten(usize),
}

impl std::fmt::Debug for RowSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.id() {
            Some(id) => write!(f, "RowSlot({id})"),
            None => f.write_str("RowSlot(unwritten)"),
        }
    }
}

/// Set a transcript row's content: insert the row if its slot is empty,
/// otherwise replace its content.
pub struct RowWrite {
    pub slot: RowSlot,
    pub session_id: String,
    /// The role an insert gives the row. An update leaves the role alone.
    pub role: MessageRole,
    /// The row's whole content, not a delta.
    pub content: String,
    /// ACP metadata to store on the row when this write inserts it.
    pub on_insert: Option<AcpMessageMetadata>,
}

impl RowWrite {
    fn apply(self, store: &Store) {
        let RowWrite {
            slot,
            session_id,
            role,
            content,
            on_insert,
        } = self;
        if let Some(id) = slot.id() {
            if let Err(e) = store.update_message_content(id, &content) {
                log::error!("Failed to update {} message {id}: {e}", role.as_str());
            }
            return;
        }
        // A tool row's call id goes in with the insert, in one statement, so
        // no read finds the row untied to its call; any other metadata
        // follows as a second write.
        let mut on_insert = on_insert;
        let tool_call_id = on_insert
            .as_mut()
            .and_then(|metadata| metadata.acp_tool_call_id.take());
        let inserted = match &tool_call_id {
            Some(tool_call_id) => {
                store.add_tool_session_message(&session_id, role.clone(), &content, tool_call_id)
            }
            None => store.add_session_message(&session_id, role.clone(), &content),
        };
        match inserted {
            Ok(id) => {
                let _ = slot.0.set(id);
                if let Some(mut metadata) =
                    on_insert.filter(|m| *m != AcpMessageMetadata::default())
                {
                    metadata.acp_tool_call_id = tool_call_id;
                    if let Err(e) = store.update_message_acp_metadata(id, &metadata) {
                        log::error!("Failed to persist ACP metadata on message {id}: {e}");
                    }
                }
            }
            Err(e) => log::error!("Failed to insert {} message: {e}", role.as_str()),
        }
    }
}

enum Job {
    Run(WriteJob),
    /// Boxed: the metadata makes a row write far larger than a job pointer.
    Row(Box<RowWrite>),
}

/// A job waiting in the queue.
struct Queued {
    job: Job,
    /// When it joined the queue. A row write folded into it does not move
    /// this: the row has had something to store since then.
    queued_at: Instant,
    /// The key a row write is indexed under, for as long as it waits.
    row: Option<RowKey>,
}

#[derive(Default)]
struct State {
    jobs: VecDeque<Queued>,
    /// How many jobs have been taken off the front so far. Jobs are
    /// numbered in the order they joined, and only ever leave from the
    /// front, so a waiting job's place in `jobs` is its number less this.
    taken: u64,
    /// The waiting write for each row, by job number: where a later write
    /// for the row is folded in. A job is in here from when it joins the
    /// queue until the writer thread takes it, so the index is never larger
    /// than the queue.
    rows: HashMap<RowKey, u64>,
    /// The store is going away: drain what is queued, then stop.
    closed: bool,
    /// When the writer thread took the job it is running. `None` between
    /// jobs.
    running_since: Option<Instant>,
    /// What the log has been told about the queue falling behind.
    backlog: Backlog,
}

impl State {
    /// Add `job` at the back of the queue.
    fn push(&mut self, job: Job, row: Option<RowKey>, now: Instant) {
        if let Some(row) = row {
            let number = self.taken + self.jobs.len() as u64;
            let replaced = self.rows.insert(row, number);
            debug_assert!(replaced.is_none(), "{row:?} already had a waiting write");
        }
        self.jobs.push_back(Queued {
            job,
            queued_at: now,
            row,
        });
    }

    /// Queue `write`: fold it into the write waiting for the same row if
    /// there is one, wherever that sits, otherwise add it at the back.
    /// Whether it was folded.
    fn push_row(&mut self, write: RowWrite, now: Instant) -> bool {
        // By id first. A write waiting under the row's id was queued after
        // any still waiting under its slot (the row had no id then), and
        // the later of the two is the one that must end up with this
        // content.
        let stored = write.slot.stored_key();
        let unwritten = write.slot.unwritten_key();
        let waiting = stored
            .and_then(|key| self.rows.get(&key))
            .or_else(|| self.rows.get(&unwritten))
            .copied();
        if let Some(number) = waiting {
            let place = usize::try_from(number - self.taken).ok();
            if let Some(Queued {
                job: Job::Row(waiting),
                ..
            }) = place.and_then(|place| self.jobs.get_mut(place))
            {
                waiting.content = write.content;
                if write.on_insert.is_some() {
                    waiting.on_insert = write.on_insert;
                }
                self.backlog.folded();
                return true;
            }
            debug_assert!(false, "the row index named a job that is not a row write");
        }
        self.push(
            Job::Row(Box::new(write)),
            Some(stored.unwrap_or(unwritten)),
            now,
        );
        false
    }

    /// Take the job at the front for the writer thread to run. Its row
    /// leaves the index with it: a write queued for the row from here on
    /// joins the queue, since this one's content is already on its way to
    /// the store.
    fn take(&mut self, now: Instant) -> Option<Job> {
        let queued = self.jobs.pop_front()?;
        self.taken += 1;
        if let Some(row) = queued.row {
            self.rows.remove(&row);
        }
        self.running_since = Some(now);
        Some(queued.job)
    }

    /// Show the queue as it is now to the backlog report.
    fn observe(&mut self, now: Instant) -> Option<Report> {
        let queue = QueueSnapshot {
            now,
            waiting: self.jobs.len(),
            row_writes: self.rows.len(),
            oldest_queued_at: self.jobs.front().map(|queued| queued.queued_at),
            running_since: self.running_since,
        };
        self.backlog.observe(&queue)
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Signalled when a job is queued or the queue closes.
    work: Condvar,
}

/// A store's write queue and the thread draining it.
pub(super) struct WriteQueue {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
    worker_id: ThreadId,
}

impl WriteQueue {
    /// Start the writer thread over `view`, which shares the store's
    /// connection (see [`Store::writer_view`]). `None` if the thread could
    /// not be spawned; writes then run inline as they did before.
    fn start(view: Store) -> Option<Self> {
        let shared = Arc::new(Shared::default());
        let worker_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("store-writer".to_string())
            .spawn(move || drain(&worker_shared, &view));
        match spawned {
            Ok(worker) => Some(Self {
                shared,
                worker_id: worker.thread().id(),
                worker: Mutex::new(Some(worker)),
            }),
            Err(e) => {
                log::error!("Couldn't start the store writer thread; writing inline: {e}");
                None
            }
        }
    }

    fn push(&self, job: Job) {
        let now = Instant::now();
        let mut state = self.shared.state.lock().unwrap();
        state.push(job, None, now);
        let report = state.observe(now);
        drop(state);
        self.shared.work.notify_one();
        if let Some(report) = report {
            report.log();
        }
    }

    /// Queue `write`, folding it into the write already waiting for the
    /// same row, if there is one.
    fn push_row(&self, write: RowWrite) {
        let now = Instant::now();
        let mut state = self.shared.state.lock().unwrap();
        let folded = state.push_row(write, now);
        let report = state.observe(now);
        drop(state);
        if !folded {
            self.shared.work.notify_one();
        }
        if let Some(report) = report {
            report.log();
        }
    }

    /// Jobs waiting, not counting one running.
    #[cfg(test)]
    fn pending(&self) -> usize {
        self.shared.state.lock().unwrap().jobs.len()
    }

    /// Rows with a write waiting that a later one would fold into.
    #[cfg(test)]
    fn indexed_rows(&self) -> usize {
        self.shared.state.lock().unwrap().rows.len()
    }

    /// Whether nothing is queued or running.
    fn idle(&self) -> bool {
        let state = self.shared.state.lock().unwrap();
        state.jobs.is_empty() && state.running_since.is_none()
    }
}

impl Drop for WriteQueue {
    /// Drain the queue before the store (and its connection) goes.
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().closed = true;
        self.shared.work.notify_all();
        if std::thread::current().id() == self.worker_id {
            return;
        }
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

/// The writer thread: run jobs in order until the queue closes and is empty.
fn drain(shared: &Shared, store: &Store) {
    loop {
        let (job, report) = {
            let mut state = shared.state.lock().unwrap();
            state.running_since = None;
            loop {
                let now = Instant::now();
                // Before taking the job, so the one about to run counts as
                // waiting: it is the oldest.
                let report = state.observe(now);
                if let Some(job) = state.take(now) {
                    break (Some(job), report);
                }
                // Nothing is waiting. Say so if a backlog was reported, and
                // come back round: the lock is not held while logging.
                if let Some(caught_up) = state.backlog.drained(now) {
                    break (None, Some(caught_up));
                }
                if state.closed {
                    return;
                }
                state = shared.work.wait(state).unwrap();
            }
        };
        if let Some(report) = report {
            report.log();
        }
        let Some(job) = job else {
            continue;
        };
        let ran = catch_unwind(AssertUnwindSafe(|| match job {
            Job::Run(job) => job(store),
            Job::Row(write) => write.apply(store),
        }));
        if ran.is_err() {
            log::error!("A queued store write panicked; the writer carries on with the next");
        }
    }
}

impl Store {
    /// A store sharing this one's connections and change and session feeds,
    /// for the writer thread to run jobs against. It has no queue of its
    /// own: a job that queues another write runs it in place, which is
    /// already its turn.
    fn writer_view(&self) -> Store {
        Store {
            conn: Arc::clone(&self.conn),
            readers: Arc::clone(&self.readers),
            change_tx: self.change_tx.clone(),
            session_feed: self.session_feed.clone(),
            writes: OnceLock::new(),
            is_writer_view: true,
            #[cfg(test)]
            temp_dir: None,
        }
    }

    /// This store's queue, started on first use. `None` on the writer
    /// thread's own view, and if the thread could not be started.
    fn write_queue(&self) -> Option<&WriteQueue> {
        if self.is_writer_view {
            return None;
        }
        self.writes
            .get_or_init(|| WriteQueue::start(self.writer_view()))
            .as_ref()
    }

    /// The queue if it has been started, without starting it.
    fn started_write_queue(&self) -> Option<&WriteQueue> {
        self.writes.get().and_then(Option::as_ref)
    }

    /// Run `job` on the writer thread after everything queued before it. For
    /// a session thread's writes, which must not wait on the connection lock.
    pub fn queue_write(&self, job: impl FnOnce(&Store) + Send + 'static) {
        match self.write_queue() {
            Some(queue) => queue.push(Job::Run(Box::new(job))),
            None => job(self),
        }
    }

    /// Queue `write`. If a write for the same row is still waiting, this
    /// one is folded into it: the row's writes collapse to the latest (see
    /// the module docs).
    pub fn queue_row_write(&self, write: RowWrite) {
        match self.write_queue() {
            Some(queue) => queue.push_row(write),
            None => write.apply(self),
        }
    }

    /// Run `job` on the writer thread after everything queued before it, and
    /// resolve to what it returns.
    ///
    /// The job is queued now, not when the future is first polled, so it is
    /// ordered against the caller's other writes by where it was called.
    /// Dropping the future before the job runs drops the job unrun. Resolves
    /// to an error only if the job itself never finished (it panicked).
    pub fn queued<T: Send + 'static>(
        &self,
        job: impl FnOnce(&Store) -> T + Send + 'static,
    ) -> impl Future<Output = Result<T, StoreError>> + Send + 'static {
        let (tx, rx) = oneshot::channel();
        let job = move |store: &Store| {
            if tx.is_closed() {
                return;
            }
            let _ = tx.send(job(store));
        };
        match self.write_queue() {
            Some(queue) => queue.push(Job::Run(Box::new(job))),
            None => job(self),
        }
        async move {
            rx.await
                .map_err(|_| StoreError("A queued store write did not finish".to_string()))
        }
    }

    /// Resolve once everything queued before this call is stored.
    pub fn flush_writes(&self) -> impl Future<Output = ()> + Send + 'static {
        let barrier = self.started_write_queue().map(|queue| {
            let (tx, rx) = oneshot::channel::<()>();
            queue.push(Job::Run(Box::new(move |_| {
                let _ = tx.send(());
            })));
            rx
        });
        async move {
            if let Some(rx) = barrier {
                let _ = rx.await;
            }
        }
    }

    /// [`Store::flush_writes`] for a thread that is not on a runtime: block
    /// until everything queued before this call is stored, or `timeout`
    /// passes. Whether it was all stored.
    pub fn flush_writes_blocking(&self, timeout: Duration) -> bool {
        let Some(queue) = self.started_write_queue() else {
            return true;
        };
        if std::thread::current().id() == queue.worker_id {
            return true;
        }
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        queue.push(Job::Run(Box::new(move |_| {
            let _ = tx.send(());
        })));
        let deadline = Instant::now() + timeout;
        rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_ok()
    }

    /// Whether no queued write is waiting or running.
    pub fn writes_idle(&self) -> bool {
        self.started_write_queue().map_or(true, WriteQueue::idle)
    }

    /// Jobs waiting on the writer thread, not counting one running.
    #[cfg(test)]
    pub(crate) fn pending_writes(&self) -> usize {
        self.started_write_queue().map_or(0, WriteQueue::pending)
    }

    /// Rows the queue has a waiting write indexed for.
    #[cfg(test)]
    pub(crate) fn indexed_row_writes(&self) -> usize {
        self.started_write_queue()
            .map_or(0, WriteQueue::indexed_rows)
    }
}
