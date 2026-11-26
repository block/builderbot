use super::*;
use crate::store::{Project, Session};
use std::sync::atomic::{AtomicBool, Ordering};

/// How long a test waits for the writer thread to get somewhere.
const WRITER_WAIT: Duration = Duration::from_secs(30);

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

fn tool_row(slot: &RowSlot, session_id: &str, content: &str, call_id: Option<&str>) -> RowWrite {
    RowWrite {
        role: MessageRole::ToolCall,
        on_insert: call_id.map(|call_id| AcpMessageMetadata {
            acp_tool_call_id: Some(call_id.to_string()),
            ..Default::default()
        }),
        ..row(slot, session_id, content)
    }
}

/// Hold the write connection and queue a write for a row of its own, then
/// wait until the writer thread has taken that write and is stuck on the
/// lock with it. Everything queued after this waits in the queue, in a
/// known state, until the returned guard is dropped.
fn hold_writer_on_a_blocker<'a>(
    store: &'a Store,
    session_id: &str,
) -> crate::store::timed_lock::TimedGuard<'a, rusqlite::Connection> {
    let held = store.hold_connection();
    store.queue_row_write(row(&RowSlot::new(), session_id, "blocker"));
    wait_until("the writer thread to take the blocker", || {
        store.pending_writes() == 0 && !store.writes_idle()
    });
    held
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + WRITER_WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
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

/// Writes to the same row queued while the writer is held up collapse to
/// one job carrying the latest content, whether or not writes for other
/// rows were queued in between.
#[tokio::test]
async fn writes_to_a_row_collapse() {
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
    assert!(store.pending_writes() <= 2, "{}", store.pending_writes());
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

/// A job that has started can still learn that its caller stopped waiting:
/// the queue's own check is made before the job runs, and the job may wait
/// a long time after that. The job here stands for one on the connection
/// lock: it is running, and held up until the test lets it go.
#[tokio::test]
async fn a_running_job_sees_its_requester_go() {
    let (store, _session_id) = store_with_session();
    let run = |store: &Store| {
        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel::<()>();
        let (seen_tx, seen_rx) = std::sync::mpsc::channel::<(bool, bool)>();
        let request = store.queued_cancellable(move |_, requester| {
            let at_start = requester.gone();
            let _ = started_tx.send(());
            let _ = resume_rx.recv();
            let _ = seen_tx.send((at_start, requester.gone()));
        });
        started_rx
            .recv_timeout(WRITER_WAIT)
            .expect("the writer thread starts the job");
        (request, resume_tx, seen_rx)
    };

    // The caller goes while the job is running.
    let (request, resume, seen) = run(&store);
    drop(request);
    resume.send(()).unwrap();
    assert_eq!(seen.recv_timeout(WRITER_WAIT).unwrap(), (false, true));

    // The caller waits: the job never sees it gone, and it gets the result.
    let (request, resume, seen) = run(&store);
    resume.send(()).unwrap();
    request.await.unwrap();
    assert_eq!(seen.recv_timeout(WRITER_WAIT).unwrap(), (false, false));
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

/// The fold reaches a write that is not at the back of the queue: text for
/// row A, an insert for row B, then newer text for A leaves two jobs. A is
/// still inserted first, so its id is below B's, and it ends with the
/// newer text.
#[tokio::test]
async fn a_later_write_folds_into_the_rows_waiting_write_past_other_rows() {
    let (store, session_id) = store_with_session();
    let held = hold_writer_on_a_blocker(&store, &session_id);
    let a = RowSlot::new();
    let b = RowSlot::new();
    store.queue_row_write(row(&a, &session_id, "A v1"));
    store.queue_row_write(row(&b, &session_id, "B"));
    store.queue_row_write(row(&a, &session_id, "A v2"));
    assert_eq!(store.pending_writes(), 2);
    assert_eq!(store.indexed_row_writes(), 2);
    drop(held);

    store.flush_writes().await;
    assert_eq!(
        contents(&store, &session_id),
        vec![
            (MessageRole::Assistant, "blocker".to_string()),
            (MessageRole::Assistant, "A v2".to_string()),
            (MessageRole::Assistant, "B".to_string()),
        ]
    );
    assert!(a.id().unwrap() < b.id().unwrap());
}

/// A fold into a job that is not at the back keeps the insert metadata the
/// waiting write carried, and the role it inserts with.
#[tokio::test]
async fn insert_metadata_survives_a_fold_past_other_rows() {
    let (store, session_id) = store_with_session();
    let held = hold_writer_on_a_blocker(&store, &session_id);
    let call = RowSlot::new();
    let text = RowSlot::new();
    store.queue_row_write(tool_row(&call, &session_id, "Bash", Some("call-1")));
    store.queue_row_write(row(&text, &session_id, "text"));
    // A later write for the row that carries no metadata of its own.
    store.queue_row_write(tool_row(&call, &session_id, "Bash: ls", None));
    assert_eq!(store.pending_writes(), 2);
    drop(held);
    store.flush_writes().await;

    let messages = store.get_session_messages(&session_id).unwrap();
    assert_eq!(messages.len(), 3);
    let stored = &messages[1];
    assert_eq!(Some(stored.id), call.id());
    assert_eq!(stored.role, MessageRole::ToolCall);
    assert_eq!(stored.content, "Bash: ls");
    assert_eq!(stored.acp.acp_tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(messages[2].content, "text");
}

/// A job queued after a row's insert, which looks the row up by its slot
/// (as tool-call metadata does), still finds the id when a later write for
/// the row was folded into the insert ahead of it.
#[tokio::test]
async fn a_job_queued_after_a_folded_insert_finds_the_row() {
    let (store, session_id) = store_with_session();
    let held = hold_writer_on_a_blocker(&store, &session_id);
    let call = RowSlot::new();
    store.queue_row_write(tool_row(&call, &session_id, "Bash", Some("call-1")));
    let seen = Arc::new(Mutex::new(None));
    store.queue_write({
        let call = call.clone();
        let seen = Arc::clone(&seen);
        let sid = session_id.clone();
        move |store| {
            let row_id = call.id();
            *seen.lock().unwrap() = Some(row_id);
            let metadata = AcpMessageMetadata {
                acp_tool_status: Some("completed".to_string()),
                ..Default::default()
            };
            // What the agent writer does: update the call's row if it is
            // stored, otherwise add a row for the metadata.
            match row_id {
                Some(row_id) => store.update_message_acp_metadata(row_id, &metadata),
                None => store.add_acp_metadata_message(&sid, &metadata).map(|_| ()),
            }
            .unwrap();
        }
    });
    store.queue_row_write(row(&RowSlot::new(), &session_id, "text"));
    store.queue_row_write(tool_row(&call, &session_id, "Bash: ls", Some("call-1")));
    assert_eq!(store.pending_writes(), 3);
    drop(held);
    store.flush_writes().await;

    assert_eq!(*seen.lock().unwrap(), Some(call.id()));
    assert!(call.id().is_some());
    let messages = store.get_session_messages(&session_id).unwrap();
    assert_eq!(messages.len(), 3, "no separate metadata row");
    assert_eq!(messages[1].content, "Bash: ls");
    assert_eq!(messages[1].acp.acp_tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(
        messages[1].acp.acp_tool_status.as_deref(),
        Some("completed")
    );
}

/// A write for a row whose job the writer thread has already taken joins
/// the queue: there is nothing waiting to fold it into. Later ones fold
/// into that.
#[tokio::test]
async fn a_write_for_a_row_being_written_is_queued() {
    let (store, session_id) = store_with_session();
    let slot = RowSlot::new();
    let held = store.hold_connection();
    store.queue_row_write(row(&slot, &session_id, "v1"));
    wait_until("the writer thread to take the row's write", || {
        store.pending_writes() == 0 && !store.writes_idle()
    });
    assert_eq!(store.indexed_row_writes(), 0);

    store.queue_row_write(row(&slot, &session_id, "v2"));
    assert_eq!(store.pending_writes(), 1);
    store.queue_row_write(row(&slot, &session_id, "v3"));
    assert_eq!(store.pending_writes(), 1);
    drop(held);
    store.flush_writes().await;

    assert_eq!(
        contents(&store, &session_id),
        vec![(MessageRole::Assistant, "v3".to_string())]
    );
}

/// An insert that fails leaves the row's slot empty and nothing in the
/// index, so the next write for the row joins the queue and inserts it.
#[tokio::test]
async fn a_failed_insert_is_retried_by_the_next_write() {
    let (store, _other_session) = store_with_session();
    // Not stored yet: a message row for it breaks the foreign key.
    let session = Session::new_running("prompt", std::path::Path::new("/tmp"));
    let slot = RowSlot::new();
    let after = RowSlot::new();

    let held = store.hold_connection();
    store.queue_row_write(row(&slot, &session.id, "v1"));
    // Folded or not, depending on whether the writer thread took v1.
    store.queue_row_write(row(&slot, &session.id, "v2"));
    drop(held);
    store.flush_writes().await;
    assert_eq!(slot.id(), None);
    assert_eq!(store.indexed_row_writes(), 0);
    assert!(contents(&store, &session.id).is_empty());

    store.create_session(&session).unwrap();
    let held = store.hold_connection();
    store.queue_row_write(row(&after, &session.id, "after"));
    store.queue_row_write(row(&slot, &session.id, "v3"));
    store.queue_row_write(row(&slot, &session.id, "v4"));
    drop(held);
    store.flush_writes().await;

    assert_eq!(
        contents(&store, &session.id),
        vec![
            (MessageRole::Assistant, "after".to_string()),
            (MessageRole::Assistant, "v4".to_string()),
        ]
    );
    assert!(after.id().unwrap() < slot.id().unwrap());
}

/// The backlog streaming leaves behind does not grow with time: several
/// sessions each flushing their row many times, interleaved, while the lock
/// is held, leave one waiting write per session.
#[tokio::test]
async fn interleaved_streams_leave_one_waiting_write_per_row() {
    const SESSIONS: usize = 8;
    const FLUSHES: usize = 200;
    let (store, first_session) = store_with_session();
    let mut sessions = vec![first_session];
    for _ in 1..SESSIONS {
        let session = Session::new_running("prompt", std::path::Path::new("/tmp"));
        store.create_session(&session).unwrap();
        sessions.push(session.id);
    }
    let slots: Vec<RowSlot> = sessions.iter().map(|_| RowSlot::new()).collect();

    let held = hold_writer_on_a_blocker(&store, &sessions[0]);
    for flush in 1..=FLUSHES {
        for (session_id, slot) in sessions.iter().zip(&slots) {
            store.queue_row_write(row(slot, session_id, &"x".repeat(flush)));
            assert!(store.pending_writes() <= SESSIONS);
        }
    }
    assert_eq!(store.pending_writes(), SESSIONS);
    assert_eq!(store.indexed_row_writes(), SESSIONS);
    drop(held);
    store.flush_writes().await;

    let mut last_id = 0;
    for (session_id, slot) in sessions.iter().zip(&slots) {
        let messages = store.get_session_messages(session_id).unwrap();
        let streamed = messages.last().unwrap();
        assert_eq!(streamed.content, "x".repeat(FLUSHES));
        assert_eq!(Some(streamed.id), slot.id());
        // Inserted in the order each row's first write was queued.
        assert!(streamed.id > last_id);
        last_id = streamed.id;
    }
    assert_eq!(store.get_session_messages(&sessions[0]).unwrap().len(), 2);
}

/// The row index holds only waiting writes: it is empty once the queue has
/// drained, whatever mix of rows, folds and other jobs went through.
#[tokio::test]
async fn the_row_index_is_empty_once_the_queue_drains() {
    let (store, session_id) = store_with_session();
    let held = hold_writer_on_a_blocker(&store, &session_id);
    let slots: Vec<RowSlot> = (0..20).map(|_| RowSlot::new()).collect();
    for round in 0..3 {
        for (n, slot) in slots.iter().enumerate() {
            store.queue_row_write(row(slot, &session_id, &format!("row {n}, round {round}")));
        }
        let sid = session_id.clone();
        store.queue_write(move |store| {
            store
                .add_session_message(&sid, MessageRole::ToolCall, "between")
                .unwrap();
        });
    }
    assert_eq!(store.indexed_row_writes(), slots.len());
    assert_eq!(store.pending_writes(), slots.len() + 3);
    drop(held);
    store.flush_writes().await;
    assert_eq!(store.indexed_row_writes(), 0);
    assert_eq!(store.pending_writes(), 0);

    // Stored rows are indexed by id from here on; that drains as well.
    for (n, slot) in slots.iter().enumerate() {
        store.queue_row_write(row(slot, &session_id, &format!("row {n}, final")));
    }
    store.flush_writes().await;
    assert_eq!(store.indexed_row_writes(), 0);
    assert!(store.writes_idle());
    let stored = contents(&store, &session_id);
    assert_eq!(stored.len(), 1 + slots.len() + 3);
    assert_eq!(stored[1].1, "row 0, final");
    assert_eq!(stored[20].1, "row 19, final");
}

/// Two slots can name one stored row: the slot a writer added it through
/// and a [`RowSlot::known`] made from its id by a writer catching the
/// transcript up. Their writes fold into each other by the row's id, and
/// the last one queued is what the row ends with, whichever slot it came
/// through and wherever the earlier ones sit.
#[tokio::test]
async fn two_slots_for_one_stored_row_keep_the_last_write_queued() {
    let (store, session_id) = store_with_session();
    let first = RowSlot::new();
    store.queue_row_write(row(&first, &session_id, "v1"));
    store.flush_writes().await;
    let second = RowSlot::known(first.id().unwrap());

    let held = hold_writer_on_a_blocker(&store, &session_id);
    store.queue_row_write(row(&first, &session_id, "v2, first slot"));
    store.queue_row_write(row(&RowSlot::new(), &session_id, "another row"));
    store.queue_row_write(row(&second, &session_id, "v3, second slot"));
    store.queue_row_write(row(&first, &session_id, "v4, first slot"));
    assert_eq!(store.pending_writes(), 2);
    drop(held);
    store.flush_writes().await;
    assert_eq!(
        contents(&store, &session_id),
        vec![
            (MessageRole::Assistant, "v4, first slot".to_string()),
            (MessageRole::Assistant, "blocker".to_string()),
            (MessageRole::Assistant, "another row".to_string()),
        ]
    );
}

/// The same when the first slot's write was queued before its row had an
/// id, so that it waits under the slot while the second slot's waits under
/// the id: a later write through the first slot goes into the later of the
/// two, not into its own slot's, and is what the row ends with.
#[tokio::test]
async fn a_write_through_the_first_slot_folds_into_the_later_one_by_id() {
    let (store, session_id) = store_with_session();
    let first = RowSlot::new();
    let held = store.hold_connection();
    store.queue_row_write(row(&first, &session_id, "v1"));
    wait_until("the writer thread to take the insert", || {
        store.pending_writes() == 0 && !store.writes_idle()
    });
    // A job that stops the writer thread without the connection, so the
    // insert can finish while the write behind it stays in the queue.
    let (open_gate, gate) = std::sync::mpsc::channel::<()>();
    store.queue_write(move |_| {
        let _ = gate.recv_timeout(WRITER_WAIT);
    });
    // Queued with the insert still running: the row has no id yet.
    store.queue_row_write(row(&first, &session_id, "v2"));
    drop(held);
    wait_until("the insert to be stored", || first.id().is_some());
    wait_until("the writer thread to reach the gate", || {
        store.pending_writes() == 1
    });

    let second = RowSlot::known(first.id().unwrap());
    store.queue_row_write(row(&second, &session_id, "v3, second slot"));
    store.queue_row_write(row(&first, &session_id, "v4, first slot"));
    assert_eq!(store.pending_writes(), 2);
    assert_eq!(store.indexed_row_writes(), 2);
    open_gate.send(()).unwrap();
    store.flush_writes().await;
    assert_eq!(
        contents(&store, &session_id),
        vec![(MessageRole::Assistant, "v4, first slot".to_string())]
    );
    assert_eq!(store.indexed_row_writes(), 0);
}

/// The queue shows the backlog report what is waiting: its depth, how many
/// are row writes, how long the front job has waited (a fold does not make
/// it younger) and how long the writer thread has been on its current job.
#[test]
fn the_backlog_report_sees_the_queue() {
    let start = Instant::now();
    let at = |secs: f64| start + Duration::from_secs_f64(secs);
    let mut state = State::default();
    let slot = RowSlot::new();

    state.push(Job::Run(Box::new(|_| {})), None, at(0.0));
    assert!(state.take(at(0.1)).is_some());
    assert!(!state.push_row(row(&slot, "s", "v1"), at(0.2)));
    state.push(Job::Run(Box::new(|_| {})), None, at(0.3));
    assert_eq!(state.observe(at(0.4)), None);
    assert!(state.push_row(row(&slot, "s", "v2"), at(3.0)));

    let report = state.observe(at(3.0)).expect("the front job is 2.8s old");
    assert_eq!(
        report.text,
        "Store write queue is backed up: 2 writes waiting (1 of them row writes), the oldest \
         for 2.8s; the writer thread has been on its current write for 2.9s; 1 row write \
         folded into waiting ones since the queue was last empty"
    );

    // Taking the jobs empties the index with the queue.
    assert!(matches!(state.take(at(3.1)), Some(Job::Row(write)) if write.content == "v2"));
    assert!(state.rows.is_empty());
    assert!(state.take(at(3.2)).is_some());
    assert!(state.take(at(3.3)).is_none());
    let caught_up = state.backlog.drained(at(3.3)).unwrap();
    assert!(
        caught_up.text.starts_with("Store write queue caught up"),
        "{}",
        caught_up.text
    );
}
