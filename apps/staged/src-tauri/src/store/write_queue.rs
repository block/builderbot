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
//!   one queued right behind another for the same row replaces it in the
//!   queue instead of joining it: a session streaming faster than the
//!   writer can store sees its flushes collapse to the latest.
//! - **Answers.** A write whose result the caller needs ([`Store::queued`])
//!   gets it through a oneshot the caller awaits, which leaves its runtime
//!   free while it waits. A request whose caller has stopped waiting (a
//!   setup step that timed out) is dropped unrun, so a write the caller
//!   gave up on does not land after it moved on.
//! - **Flushing.** [`Store::flush_writes`] waits until everything queued
//!   before it is stored, and [`Store::flush_writes_blocking`] does the same
//!   for a thread that is not on a runtime (the quit path). Dropping the
//!   store drains the queue before the connection closes.
//!
//! Writes from anywhere else (UI commands, the session runner between
//! turns) still go straight to the store. They are not ordered against
//! queued ones, which is why a session flushes when its connection ends:
//! anything that reads the transcript afterwards, or writes to it from
//! another thread, sees it complete.

use std::collections::VecDeque;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use super::{AcpMessageMetadata, MessageRole, Store, StoreError};

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

    fn is(&self, other: &RowSlot) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
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
                if let Some(mut metadata) = on_insert.filter(|m| *m != AcpMessageMetadata::default())
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

#[derive(Default)]
struct State {
    jobs: VecDeque<Job>,
    /// The store is going away: drain what is queued, then stop.
    closed: bool,
    /// A job is off the queue and running.
    busy: bool,
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
        let mut state = self.shared.state.lock().unwrap();
        state.jobs.push_back(job);
        drop(state);
        self.shared.work.notify_one();
    }

    /// Queue `write`, folding it into the job at the back of the queue when
    /// that one sets the same row.
    fn push_row(&self, write: RowWrite) {
        let mut state = self.shared.state.lock().unwrap();
        if let Some(Job::Row(last)) = state.jobs.back_mut() {
            if last.slot.is(&write.slot) {
                last.content = write.content;
                if write.on_insert.is_some() {
                    last.on_insert = write.on_insert;
                }
                return;
            }
        }
        state.jobs.push_back(Job::Row(Box::new(write)));
        drop(state);
        self.shared.work.notify_one();
    }

    /// Jobs waiting, not counting one running.
    #[cfg(test)]
    fn pending(&self) -> usize {
        self.shared.state.lock().unwrap().jobs.len()
    }

    /// Whether nothing is queued or running.
    fn idle(&self) -> bool {
        let state = self.shared.state.lock().unwrap();
        state.jobs.is_empty() && !state.busy
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
        let job = {
            let mut state = shared.state.lock().unwrap();
            state.busy = false;
            loop {
                if let Some(job) = state.jobs.pop_front() {
                    state.busy = true;
                    break job;
                }
                if state.closed {
                    return;
                }
                state = shared.work.wait(state).unwrap();
            }
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
    /// A store sharing this one's connection and change and session feeds,
    /// for the writer
    /// thread to run jobs against. It has no queue of its own: a job that
    /// queues another write runs it in place, which is already its turn.
    fn writer_view(&self) -> Store {
        Store {
            conn: Arc::clone(&self.conn),
            change_tx: self.change_tx.clone(),
            session_feed: self.session_feed.clone(),
            writes: OnceLock::new(),
            is_writer_view: true,
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

    /// Queue `write`. Consecutive writes to the same row collapse to the
    /// latest (see the module docs).
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Project, Session};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn store_with_session() -> (Arc<Store>, String) {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = Project::new("owner/repo");
        store.create_project(&project).unwrap();
        let session = Session::new_running("prompt", std::path::Path::new("/tmp"));
        store.create_session(&session).unwrap();
        (store, session.id)
    }

    fn row(slot: &RowSlot, session_id: &str, content: &str) -> RowWrite {
        RowWrite {
            slot: slot.clone(),
            session_id: session_id.to_string(),
            role: MessageRole::Assistant,
            content: content.to_string(),
            on_insert: None,
        }
    }

    fn contents(store: &Store, session_id: &str) -> Vec<(MessageRole, String)> {
        store
            .get_session_messages(session_id)
            .unwrap()
            .into_iter()
            .map(|m| (m.role, m.content))
            .collect()
    }

    /// Writes land in the order they were queued, row writes and other jobs
    /// interleaved, and a request queued after them sees all of them.
    #[tokio::test]
    async fn queued_writes_land_in_order() {
        let (store, session_id) = store_with_session();
        let first = RowSlot::new();
        store.queue_row_write(row(&first, &session_id, "one"));
        let sid = session_id.clone();
        store.queue_write(move |store| {
            store
                .add_session_message(&sid, MessageRole::ToolCall, "two")
                .unwrap();
        });
        let second = RowSlot::new();
        store.queue_row_write(row(&second, &session_id, "three"));
        store.queue_row_write(row(&first, &session_id, "one, edited"));

        let sid = session_id.clone();
        let seen = store
            .queued(move |store| contents(store, &sid))
            .await
            .unwrap();
        assert_eq!(
            seen,
            vec![
                (MessageRole::Assistant, "one, edited".to_string()),
                (MessageRole::ToolCall, "two".to_string()),
                (MessageRole::Assistant, "three".to_string()),
            ]
        );
        assert!(first.id().is_some() && second.id().is_some());
        assert!(first.id() < second.id());
    }

    /// Writes to the same row queued back to back while the writer is held
    /// up collapse to one job carrying the latest content. One for another
    /// row in between keeps them apart.
    #[tokio::test]
    async fn consecutive_writes_to_a_row_collapse() {
        let (store, session_id) = store_with_session();
        let streaming = RowSlot::new();
        let other = RowSlot::new();

        let held = store.hold_connection();
        // Whichever way the writer thread is caught (blocked on the first
        // write, or not yet started on it), what is queued behind it
        // collapses the same way.
        store.queue_row_write(row(&other, &session_id, "other"));
        for text in ["H", "He", "Hel", "Hello"] {
            store.queue_row_write(row(&streaming, &session_id, text));
        }
        assert!(store.pending_writes() <= 2, "{}", store.pending_writes());
        store.queue_row_write(row(&other, &session_id, "other, edited"));
        store.queue_row_write(row(&streaming, &session_id, "Hello there"));
        assert!(store.pending_writes() <= 4, "{}", store.pending_writes());
        drop(held);

        store.flush_writes().await;
        assert_eq!(
            contents(&store, &session_id),
            vec![
                (MessageRole::Assistant, "other, edited".to_string()),
                (MessageRole::Assistant, "Hello there".to_string()),
            ]
        );
        assert!(store.writes_idle());
    }

    /// A collapsed write keeps the insert metadata the first one carried.
    #[tokio::test]
    async fn a_collapsed_insert_keeps_its_metadata() {
        let (store, session_id) = store_with_session();
        let slot = RowSlot::new();
        let held = store.hold_connection();
        let blocker = RowSlot::new();
        store.queue_row_write(row(&blocker, &session_id, "blocker"));
        store.queue_row_write(RowWrite {
            on_insert: Some(AcpMessageMetadata {
                acp_tool_call_id: Some("call-1".to_string()),
                ..Default::default()
            }),
            role: MessageRole::ToolResult,
            ..row(&slot, &session_id, "partial")
        });
        store.queue_row_write(RowWrite {
            role: MessageRole::ToolResult,
            ..row(&slot, &session_id, "complete")
        });
        drop(held);
        store.flush_writes().await;

        let messages = store.get_session_messages(&session_id).unwrap();
        let result = messages.last().unwrap();
        assert_eq!(result.content, "complete");
        assert_eq!(result.acp.acp_tool_call_id.as_deref(), Some("call-1"));
    }

    /// Dropping the store stores everything still queued first.
    #[test]
    fn dropping_the_store_drains_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let session_id = {
            let store = Store::new(&path).unwrap();
            let project = Project::new("owner/repo");
            store.create_project(&project).unwrap();
            let session = Session::new_running("prompt", std::path::Path::new("/tmp"));
            store.create_session(&session).unwrap();
            let held = store.hold_connection();
            let slot = RowSlot::new();
            for text in ["a", "ab", "abc"] {
                store.queue_row_write(row(&slot, &session.id, text));
            }
            let sid = session.id.clone();
            store.queue_write(move |store| {
                store
                    .add_session_message(&sid, MessageRole::ToolCall, "last")
                    .unwrap();
            });
            drop(held);
            session.id
        };
        let store = Store::new(&path).unwrap();
        assert_eq!(
            contents(&store, &session_id),
            vec![
                (MessageRole::Assistant, "abc".to_string()),
                (MessageRole::ToolCall, "last".to_string()),
            ]
        );
    }

    /// The blocking flush waits for queued writes, and gives up at its
    /// timeout while the lock is held.
    #[test]
    fn blocking_flush_waits_for_queued_writes() {
        let (store, session_id) = store_with_session();
        assert!(store.flush_writes_blocking(Duration::from_millis(10)));

        let held = store.hold_connection();
        store.queue_row_write(row(&RowSlot::new(), &session_id, "queued"));
        assert!(!store.flush_writes_blocking(Duration::from_millis(50)));
        drop(held);
        assert!(store.flush_writes_blocking(Duration::from_secs(10)));
        assert_eq!(contents(&store, &session_id).len(), 1);
    }

    /// A request whose caller stopped waiting before it ran is dropped, not
    /// run late.
    #[tokio::test]
    async fn an_abandoned_request_does_not_run() {
        let (store, _session_id) = store_with_session();
        let ran = Arc::new(AtomicBool::new(false));
        let held = store.hold_connection();
        let request = {
            let ran = Arc::clone(&ran);
            store.queued(move |_| ran.store(true, Ordering::SeqCst))
        };
        drop(request);
        drop(held);
        store.flush_writes().await;
        assert!(!ran.load(Ordering::SeqCst));
    }

    /// The point of the queue: a session thread (a current-thread runtime)
    /// that queues writes and awaits a queued request keeps running its
    /// other tasks, a timer here, while another thread holds the connection
    /// lock. The request completes once the lock is free.
    #[test]
    fn a_session_thread_keeps_running_while_the_lock_is_held() {
        let (store, session_id) = store_with_session();
        let held = store.hold_connection();
        let (ticked_tx, ticked_rx) = std::sync::mpsc::channel::<usize>();

        let session_store = Arc::clone(&store);
        let sid_for_session = session_id.clone();
        let session = std::thread::spawn(move || {
            let session_id = sid_for_session;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();
            let local = tokio::task::LocalSet::new();
            local.block_on(&runtime, async move {
                let slot = RowSlot::new();
                for text in ["s", "st", "str"] {
                    session_store.queue_row_write(row(&slot, &session_id, text));
                }
                let sid = session_id.clone();
                let saved = session_store
                    .queued(move |store| store.set_agent_session_id(&sid, "agent-1").is_ok());
                let ticker = tokio::task::spawn_local(async move {
                    for tick in 1..=5 {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        let _ = ticked_tx.send(tick);
                    }
                });
                ticker.await.unwrap();
                saved.await.unwrap()
            })
        });

        // Every tick arrives with the lock still held here.
        for expected in 1..=5 {
            let tick = ticked_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("the session thread's timer fires while the lock is held");
            assert_eq!(tick, expected);
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !session.is_finished(),
            "the queued request cannot finish while the lock is held"
        );
        drop(held);
        assert!(session.join().unwrap());
        assert_eq!(
            store
                .get_session(&session_id)
                .unwrap()
                .and_then(|s| s.agent_id)
                .as_deref(),
            Some("agent-1")
        );
        assert_eq!(
            contents(&store, &session_id),
            vec![(MessageRole::Assistant, "str".to_string())]
        );
    }
}
