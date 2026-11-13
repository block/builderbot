//! Tests for how the executor buffers, bounds, retains and delivers output.
//!
//! Most of them run a short `/bin/sh` script through `execute_remote`, which
//! spawns the program directly on pipes: unlike the local path it does not
//! load the user's shell profile, so the output is exactly what the script
//! writes.
//!
//! On a busy macOS machine a child is now and then killed with `SIGKILL`
//! right after it is spawned, before it runs any of its script. The executor
//! before these changes shows the same under the same load, so it is not
//! something these tests are about. They start their scripts through
//! [`finish`] and [`run_sh_until_output`], which run a script again when that
//! happens instead of failing on it.

use std::future::Future;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use super::*;

/// Upper bound on any single wait. Generous, so a loaded machine does not
/// fail a test; a healthy run takes milliseconds.
const WAIT: Duration = Duration::from_secs(60);

/// How many times a script is started before a test gives up on a child that
/// keeps being killed at birth. One retry is already rare.
const SPAWN_ATTEMPTS: usize = 5;

/// Forwards every execution event to the test.
struct Recorder {
    events: Sender<ExecutionEvent>,
    /// How long handling one output event takes. The executor calls its
    /// listener synchronously, so this is how far behind a reader falls.
    output_delay: Duration,
}

#[async_trait]
impl ExecutionListener for Recorder {
    async fn on_event(&self, event: ExecutionEvent) {
        if matches!(event, ExecutionEvent::Output { .. }) {
            thread::sleep(self.output_delay);
        }
        let _ = self.events.send(event);
    }
}

fn recorder() -> (Arc<dyn ExecutionListener>, Receiver<ExecutionEvent>) {
    slow_recorder(Duration::ZERO)
}

fn slow_recorder(output_delay: Duration) -> (Arc<dyn ExecutionListener>, Receiver<ExecutionEvent>) {
    let (events, receiver) = mpsc::channel();
    (
        Arc::new(Recorder {
            events,
            output_delay,
        }),
        receiver,
    )
}

/// What an execution reported up to and including its terminal status.
struct Outcome {
    /// The text of every output event, in order.
    streamed: Vec<String>,
    /// The offset each of those events said its text starts at.
    offsets: Vec<u64>,
    status: ActionStatus,
    exit_code: Option<i32>,
}

impl Outcome {
    fn streamed_text(&self) -> String {
        self.streamed.concat()
    }

    /// Whether the child died from a signal without writing anything: the
    /// kill described in the module docs. No script here ends that way.
    fn killed_at_birth(&self) -> bool {
        self.status == ActionStatus::Failed && self.exit_code.is_none() && self.streamed.is_empty()
    }

    /// Fail with the exit code and the end of the output, which is where a
    /// shell says what went wrong, unless the execution exited cleanly.
    fn assert_completed(&self) {
        let text = self.streamed_text();
        let tail: String = text
            .chars()
            .skip(text.chars().count().saturating_sub(400))
            .collect();
        assert!(
            self.status == ActionStatus::Completed && self.exit_code == Some(0),
            "expected a clean exit, got {:?} with exit code {:?}; output ends: {tail:?}",
            self.status,
            self.exit_code,
        );
    }
}

fn wait_for_finish(events: &Receiver<ExecutionEvent>) -> Outcome {
    let mut streamed = Vec::new();
    let mut offsets = Vec::new();
    loop {
        match events
            .recv_timeout(WAIT)
            .expect("the execution did not finish")
        {
            ExecutionEvent::Started { .. } => {}
            ExecutionEvent::Output { chunk, offset, .. } => {
                streamed.push(chunk);
                offsets.push(offset);
            }
            ExecutionEvent::StatusChanged {
                status, exit_code, ..
            } => {
                return Outcome {
                    streamed,
                    offsets,
                    status,
                    exit_code,
                }
            }
        }
    }
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// Limits for tests that are not about the drain timeout: a slow machine must
/// never cut the wait for the readers short.
fn patient_limits() -> Limits {
    Limits {
        reader_drain_timeout: WAIT,
        ..Limits::default()
    }
}

async fn run_sh(executor: &ActionExecutor, script: &str) -> (String, Receiver<ExecutionEvent>) {
    run_sh_with(executor, script, recorder()).await
}

async fn run_sh_with(
    executor: &ActionExecutor,
    script: &str,
    (listener, events): (Arc<dyn ExecutionListener>, Receiver<ExecutionEvent>),
) -> (String, Receiver<ExecutionEvent>) {
    let execution_id = executor
        .execute_remote(
            PathBuf::from("/bin/sh"),
            vec!["-c".to_string(), script.to_string()],
            listener,
        )
        .await
        .expect("failed to spawn /bin/sh");
    (execution_id, events)
}

/// An execution that ran to its terminal status.
struct Finished {
    id: String,
    /// Whatever the execution reports after its terminal status.
    events: Receiver<ExecutionEvent>,
    outcome: Outcome,
}

/// Run what `start` starts to its terminal status.
///
/// An attempt whose child was killed at birth is started again. Its empty
/// log is cleared first, so it does not count as a finished execution.
async fn finish<F, Fut>(executor: &ActionExecutor, start: F) -> Finished
where
    F: Fn() -> Fut,
    Fut: Future<Output = (String, Receiver<ExecutionEvent>)>,
{
    for _ in 0..SPAWN_ATTEMPTS {
        let (id, events) = start().await;
        let outcome = wait_for_finish(&events);
        if !outcome.killed_at_birth() {
            return Finished {
                id,
                events,
                outcome,
            };
        }
        executor.clear_execution(&id);
    }
    panic!("the child was killed before running {SPAWN_ATTEMPTS} times in a row");
}

/// Run a script to its terminal status.
async fn run_sh_to_finish(executor: &ActionExecutor, script: &str) -> Finished {
    finish(executor, || run_sh(executor, script)).await
}

/// Start a script that then blocks on a [`Gate`], and wait until its
/// buffered output is exactly `first`.
///
/// Like [`finish`], starts the script again if its child is killed at birth.
async fn run_sh_until_output(
    executor: &ActionExecutor,
    script: &str,
    first: &str,
) -> (String, Receiver<ExecutionEvent>) {
    for _ in 0..SPAWN_ATTEMPTS {
        let (id, events) = run_sh(executor, script).await;
        let deadline = Instant::now() + WAIT;
        while executor.is_running(&id) {
            if buffered_text(executor, &id).as_deref() == Some(first) {
                return (id, events);
            }
            assert!(Instant::now() < deadline, "timed out waiting for {first:?}");
            thread::sleep(Duration::from_millis(10));
        }
        // The gate is still closed, so the script cannot have got this far.
        let outcome = wait_for_finish(&events);
        assert!(
            outcome.killed_at_birth(),
            "the script exited at its gate: {:?} with exit code {:?}",
            outcome.status,
            outcome.exit_code,
        );
        executor.clear_execution(&id);
    }
    panic!("the child was killed before running {SPAWN_ATTEMPTS} times in a row");
}

fn buffered_text(executor: &ActionExecutor, execution_id: &str) -> Option<String> {
    executor
        .get_buffered_output(execution_id)
        .map(|output| output.chunks.iter().map(|c| c.chunk.as_str()).collect())
}

/// A file a script waits on, so a test can hold an action open and release
/// it. Dropping the gate releases it too, so a failed test leaves no script
/// behind.
struct Gate(PathBuf);

impl Gate {
    fn closed() -> Self {
        let path = std::env::temp_dir().join(format!(
            "builderbot-actions-test-gate-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"").expect("failed to create the gate file");
        Self(path)
    }

    /// Shell that blocks until the gate opens.
    fn wait(&self) -> String {
        format!("while [ -e '{}' ]; do sleep 0.05; done", self.0.display())
    }

    fn open(&self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.open();
    }
}

/// A stream that returns the given reads one at a time, then ends.
struct ScriptedReads(std::vec::IntoIter<Vec<u8>>);

impl Read for ScriptedReads {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let Some(next) = self.0.next() else {
            return Ok(0);
        };
        buf[..next.len()].copy_from_slice(&next);
        Ok(next.len())
    }
}

// =============================================================================
// Item 1: the buffer moves at exit, after the readers finish
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_written_just_before_exit_is_in_the_finished_log() {
    let executor = ActionExecutor::with_limits(patient_limits());
    // The output fits in the pipe, so the shell exits at once. The listener
    // is slow, so most of the output is still unread when it does.
    let Finished {
        id,
        events,
        outcome,
    } = finish(&executor, || {
        run_sh_with(
            &executor,
            "head -c 8000 /dev/zero | tr '\\0' x; printf END",
            slow_recorder(Duration::from_millis(20)),
        )
    })
    .await;

    outcome.assert_completed();
    let streamed = outcome.streamed_text();
    assert_eq!(streamed.len(), 8_003);
    assert!(streamed.ends_with("xEND"));
    // The finished log is whole the moment the terminal status arrives, and
    // no output event follows that status.
    assert_eq!(buffered_text(&executor, &id).as_deref(), Some(&*streamed));
    assert!(events.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finishing_moves_the_buffer_instead_of_copying_it() {
    let executor = ActionExecutor::with_limits(patient_limits());
    let gate = Gate::closed();
    let (id, events) = run_sh_until_output(
        &executor,
        &format!("printf start; {}; printf end", gate.wait()),
        "start",
    )
    .await;
    let live_buffer = {
        let running = executor.running.lock().unwrap();
        running.get(&id).unwrap().output_buffer.clone()
    };

    gate.open();
    wait_for_finish(&events);

    assert!(!executor.is_running(&id));
    let finished_buffer = executor.finished.lock().unwrap().get(&id).unwrap();
    assert!(Arc::ptr_eq(&live_buffer, &finished_buffer));
    assert_eq!(buffered_text(&executor, &id).as_deref(), Some("startend"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_process_that_outlives_the_action_does_not_block_completion() {
    let executor = ActionExecutor::with_limits(Limits {
        reader_drain_timeout: Duration::from_millis(200),
        ..Limits::default()
    });
    let gate = Gate::closed();
    // The background job inherits the pipes and keeps them open after the
    // shell exits, so the readers cannot reach the end of the output.
    let Finished { id, outcome, .. } = run_sh_to_finish(
        &executor,
        &format!("printf first; ({}; printf late) &", gate.wait()),
    )
    .await;

    // Completion gave up on the readers instead of waiting for the gate.
    outcome.assert_completed();
    wait_until("the output written before exit", || {
        buffered_text(&executor, &id).as_deref() == Some("first")
    });

    // What the survivor writes afterwards still reaches the finished log.
    gate.open();
    wait_until("the late output", || {
        buffered_text(&executor, &id).as_deref() == Some("firstlate")
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_execution_keeps_its_last_output() {
    let executor = ActionExecutor::with_limits(patient_limits());
    // The local path runs the user's login shell on a PTY, whose profile may
    // print as well, so only look for the action's own line.
    let Finished { id, outcome, .. } = finish(&executor, || async {
        let (listener, events) = recorder();
        let id = executor
            .execute(
                "printf 'tail-%s\\n' marker".to_string(),
                std::env::temp_dir().to_string_lossy().to_string(),
                listener,
            )
            .await
            .expect("failed to spawn the login shell");
        (id, events)
    })
    .await;

    outcome.assert_completed();
    assert!(outcome.streamed_text().contains("tail-marker"));
    assert_eq!(buffered_text(&executor, &id), Some(outcome.streamed_text()));
}

// =============================================================================
// Item 2: the buffer is capped and coalesced
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffered_output_is_capped_to_the_most_recent_bytes() {
    let max_output_bytes = 16 * 1024;
    let executor = ActionExecutor::with_limits(Limits {
        max_output_bytes,
        ..patient_limits()
    });
    let Finished { id, outcome, .. } = run_sh_to_finish(&executor, "seq 1 20000").await;

    // Streaming is not capped: every line was delivered as it was written.
    let written: String = (1..=20_000).map(|n| format!("{n}\n")).collect();
    assert_eq!(outcome.streamed_text(), written);

    let chunks = executor.get_buffered_output(&id).unwrap().chunks;
    let (notice, retained) = chunks.split_first().unwrap();
    let retained: String = retained.iter().map(|c| c.chunk.as_str()).collect();
    // The buffer holds the end of the output, on a line boundary, within the
    // cap...
    assert!(retained.len() <= max_output_bytes);
    assert!(retained.len() > max_output_bytes / 2);
    assert!(written.ends_with(&retained));
    let first_line: u32 = retained.lines().next().unwrap().parse().unwrap();
    assert!(written.contains(&format!("\n{first_line}\n")));
    // ...and says up front that the rest is gone.
    assert_eq!(notice.stream, "stderr");
    assert!(
        notice.chunk.starts_with("[earlier output truncated: ")
            && notice.chunk.ends_with(" KB dropped]\n"),
        "unexpected notice {:?}",
        notice.chunk
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_and_the_snapshot_share_one_set_of_offsets() {
    // A small cap, so most of the output is gone from the buffer by the end,
    // and small batches, so there are many events.
    let executor = ActionExecutor::with_limits(Limits {
        max_output_bytes: 16 * 1024,
        emit_max_bytes: 1024,
        ..patient_limits()
    });
    let Finished { id, outcome, .. } = run_sh_to_finish(&executor, "seq 1 20000").await;
    assert!(outcome.streamed.len() > 10);

    // Each event starts where the one before it ended, counting from the
    // start of the output.
    let mut expected = 0;
    for (chunk, offset) in outcome.streamed.iter().zip(&outcome.offsets) {
        assert_eq!(*offset, expected);
        expected += chunk.len() as u64;
    }

    // The snapshot ends where the last event did, although it no longer holds
    // the start of the output. A viewer that takes it now has nothing left to
    // apply; one that took it earlier would apply the events from its end on.
    let output = executor.get_buffered_output(&id).unwrap();
    assert!(output.chunks[0]
        .chunk
        .starts_with("[earlier output truncated: "));
    assert_eq!(output.end_offset, expected);
    assert_eq!(output.end_offset, outcome.streamed_text().len() as u64);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_of_one_stream_are_coalesced() {
    let executor = ActionExecutor::with_limits(patient_limits());
    let written: String = (1..=3_000).map(|n| format!("{n}\n")).collect();

    for (script, stream) in [("seq 1 3000", "stdout"), ("seq 1 3000 >&2", "stderr")] {
        let Finished { id, outcome, .. } = run_sh_to_finish(&executor, script).await;

        // Many reads were streamed, and they are buffered as a single block.
        assert_eq!(outcome.streamed_text(), written);
        let chunks = executor.get_buffered_output(&id).unwrap().chunks;
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].stream, stream);
        assert_eq!(chunks[0].chunk, written);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_character_split_across_reads_is_decoded_whole() {
    let (listener, events) = recorder();
    let buffer = OutputBuffer::shared(MAX_OUTPUT_BYTES);
    let output = output_emitter::spawn(
        "exec".to_string(),
        listener,
        buffer.clone(),
        Handle::current(),
        WAIT,
        EMIT_MAX_BYTES,
    );
    let (reader_done, readers_done) = mpsc::channel();
    // "café €" with both multi-byte characters cut by a read boundary.
    let reads = vec![
        b"caf\xc3".to_vec(),
        b"\xa9 \xe2".to_vec(),
        b"\x82".to_vec(),
        b"\xac".to_vec(),
    ];

    spawn_stream_reader(
        ScriptedReads(reads.into_iter()),
        "stdout",
        output.clone(),
        reader_done,
    );
    assert!(wait_for_readers(&readers_done, Instant::now() + WAIT));
    assert!(output.flush_by(Instant::now() + WAIT));

    let streamed: Vec<String> = events
        .try_iter()
        .map(|event| match event {
            ExecutionEvent::Output { chunk, .. } => chunk,
            other => panic!("unexpected event {other:?}"),
        })
        .collect();
    // Each character is delivered once, and whole.
    assert_eq!(streamed, vec!["café €"]);
    let chunks = buffer.lock().unwrap().snapshot();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].chunk, "café €");
}

// =============================================================================
// Item 3: finished buffers are evicted
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_most_recently_finished_executions_keep_their_output() {
    let executor = ActionExecutor::with_limits(Limits {
        max_finished: 2,
        ..patient_limits()
    });

    let mut ids = Vec::new();
    for word in ["one", "two", "three"] {
        ids.push(
            run_sh_to_finish(&executor, &format!("printf {word}"))
                .await
                .id,
        );
    }

    assert_eq!(buffered_text(&executor, &ids[0]), None);
    assert_eq!(buffered_text(&executor, &ids[1]).as_deref(), Some("two"));
    assert_eq!(buffered_text(&executor, &ids[2]).as_deref(), Some("three"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_default_limit_keeps_a_handful_of_finished_executions() {
    let executor = ActionExecutor::with_limits(patient_limits());

    let mut ids = Vec::new();
    for n in 0..MAX_FINISHED_EXECUTIONS + 2 {
        ids.push(run_sh_to_finish(&executor, &format!("printf {n}")).await.id);
    }

    let retained = ids
        .iter()
        .filter(|id| executor.get_buffered_output(id).is_some())
        .count();
    assert_eq!(retained, MAX_FINISHED_EXECUTIONS);
    assert_eq!(buffered_text(&executor, &ids[0]), None);
    assert_eq!(buffered_text(&executor, &ids[1]), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_execution_frees_a_finished_execution_only() {
    let executor = ActionExecutor::with_limits(patient_limits());
    let gate = Gate::closed();
    let (id, events) =
        run_sh_until_output(&executor, &format!("printf out; {}", gate.wait()), "out").await;

    // Running: nothing is cleared.
    assert!(!executor.clear_execution(&id));
    assert_eq!(buffered_text(&executor, &id).as_deref(), Some("out"));

    gate.open();
    wait_for_finish(&events);

    // Finished: cleared once.
    assert_eq!(buffered_text(&executor, &id).as_deref(), Some("out"));
    assert!(executor.clear_execution(&id));
    assert_eq!(buffered_text(&executor, &id), None);
    assert!(!executor.clear_execution(&id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discarding_a_running_execution_frees_its_output_at_exit() {
    let executor = ActionExecutor::with_limits(patient_limits());
    let gate = Gate::closed();
    let (id, events) = run_sh_until_output(
        &executor,
        &format!("printf start; {}; printf end", gate.wait()),
        "start",
    )
    .await;

    assert!(executor.discard_output(&id));
    // Still readable while it runs, and still streamed to its listener.
    assert_eq!(buffered_text(&executor, &id).as_deref(), Some("start"));
    gate.open();
    let outcome = wait_for_finish(&events);
    assert_eq!(outcome.streamed_text(), "startend");

    // Never retained once it has exited.
    assert_eq!(buffered_text(&executor, &id), None);
    assert!(executor.finished.lock().unwrap().get(&id).is_none());
    assert!(!executor.discard_output(&id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discarding_a_finished_execution_frees_its_output_now() {
    let executor = ActionExecutor::with_limits(patient_limits());
    let Finished { id, .. } = run_sh_to_finish(&executor, "printf out").await;
    assert_eq!(buffered_text(&executor, &id).as_deref(), Some("out"));

    assert!(executor.discard_output(&id));
    assert_eq!(buffered_text(&executor, &id), None);
    assert!(!executor.discard_output("no-such-execution"));
}

// =============================================================================
// Item 4: output is delivered to the listener in batches
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_is_delivered_in_batches_instead_of_read_by_read() {
    // The interval never elapses in this test, and the output is smaller than
    // a batch may be, so nothing delivers it before the execution ends.
    let executor = ActionExecutor::with_limits(Limits {
        emit_interval: WAIT,
        ..patient_limits()
    });
    let started = Instant::now();
    let Finished {
        events, outcome, ..
    } = run_sh_to_finish(&executor, "seq 1 3000").await;

    outcome.assert_completed();
    // Dozens of reads reach the listener as one event. It arrives ahead of
    // the terminal status, which does not wait out the interval for it, and
    // nothing follows that status.
    let written: String = (1..=3_000).map(|n| format!("{n}\n")).collect();
    assert_eq!(outcome.streamed, vec![written]);
    assert!(started.elapsed() < WAIT);
    assert!(events.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_batch_is_delivered_without_waiting_for_the_interval() {
    let emit_max_bytes = 4096;
    let executor = ActionExecutor::with_limits(Limits {
        emit_interval: WAIT,
        emit_max_bytes,
        ..patient_limits()
    });
    let Finished { outcome, .. } = run_sh_to_finish(&executor, "seq 1 20000").await;

    outcome.assert_completed();
    let written: String = (1..=20_000).map(|n| format!("{n}\n")).collect();
    assert_eq!(outcome.streamed_text(), written);
    // Every batch but the last went out on reaching the size, so it is at
    // least that large and overshoots by less than the one read that filled
    // it.
    let (last, full) = outcome.streamed.split_last().unwrap();
    let largest_read = 1024;
    assert!(full.len() >= 20, "{} batches", full.len());
    for batch in full {
        assert!(
            (emit_max_bytes..emit_max_bytes + largest_read).contains(&batch.len()),
            "a batch of {} bytes",
            batch.len()
        );
    }
    assert!(last.len() < emit_max_bytes + largest_read);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_reaches_the_listener_while_the_action_is_still_running() {
    // The production interval and batch size.
    let executor = ActionExecutor::with_limits(patient_limits());
    let gate = Gate::closed();
    let (id, events) = run_sh_until_output(
        &executor,
        &format!("printf start; {}; printf end", gate.wait()),
        "start",
    )
    .await;

    // The script is waiting at its gate and the batch is far from full: the
    // interval alone delivers it.
    let first = loop {
        match events.recv_timeout(WAIT).expect("no output was delivered") {
            ExecutionEvent::Started { .. } => {}
            ExecutionEvent::Output { chunk, .. } => break chunk,
            other => panic!("unexpected event {other:?}"),
        }
    };
    assert_eq!(first, "start");
    assert!(executor.is_running(&id));

    gate.open();
    let outcome = wait_for_finish(&events);
    outcome.assert_completed();
    assert_eq!(outcome.streamed, vec!["end"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_batch_still_pending_at_exit_is_delivered_before_the_status() {
    // The interval never elapses, so "first" is still waiting in its batch
    // when the shell exits. The readers then time out, because the background
    // job holds the pipes open.
    //
    // This test has to meet the timeout, not just outlast it: "first" must be
    // read before the wait for the readers gives up, and delivered before the
    // flush does. The wait always runs its full length here, so the timeout
    // is test time; it is long enough that a starved machine still makes it.
    let executor = ActionExecutor::with_limits(Limits {
        emit_interval: WAIT,
        reader_drain_timeout: Duration::from_secs(10),
        ..Limits::default()
    });
    let gate = Gate::closed();
    let Finished {
        id,
        events,
        outcome,
    } = run_sh_to_finish(
        &executor,
        &format!("printf first; ({}; printf late) &", gate.wait()),
    )
    .await;

    // What had been read went out ahead of the terminal status all the same,
    // and is in the finished log.
    outcome.assert_completed();
    assert_eq!(outcome.streamed, vec!["first"]);
    assert_eq!(buffered_text(&executor, &id).as_deref(), Some("first"));

    // What the survivor writes afterwards is still delivered, after it.
    gate.open();
    match events.recv_timeout(WAIT).expect("no late output") {
        ExecutionEvent::Output { chunk, .. } => assert_eq!(chunk, "late"),
        other => panic!("unexpected event {other:?}"),
    }
    assert_eq!(buffered_text(&executor, &id).as_deref(), Some("firstlate"));
}

// =============================================================================
// Between the process exiting and its terminal status
// =============================================================================

/// Start a script whose shell prints "first" and exits at once, leaving a
/// background job that holds the output open until `gate` opens and then
/// prints "late".
///
/// Returns once the executor has noticed the shell's exit. The execution is
/// then waiting for the rest of its output, and cannot report its terminal
/// status before the gate opens: the executor must be one whose wait for the
/// readers does not run out.
async fn run_sh_until_exited_with_output_open(
    executor: &ActionExecutor,
    gate: &Gate,
) -> (String, Receiver<ExecutionEvent>) {
    let script = format!("printf first; ({}; printf late) &", gate.wait());
    let (id, events) = run_sh_until_output(executor, &script, "first").await;
    wait_until("the shell's exit to be noticed", || {
        let running = executor.running.lock().unwrap();
        running
            .get(&id)
            .is_none_or(|state| state.child_pid.is_none())
    });
    (id, events)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_execution_is_running_until_its_terminal_status_is_due() {
    let executor = ActionExecutor::with_limits(patient_limits());
    let gate = Gate::closed();
    let (id, events) = run_sh_until_exited_with_output_open(&executor, &gate).await;

    // The shell has exited, but nobody has been told so. Whoever asks now is
    // still shown the execution, and will get its terminal status.
    assert!(executor.is_running(&id));
    assert_eq!(executor.get_running_ids(), vec![id.clone()]);
    assert!(!executor.wait_for_executions(std::slice::from_ref(&id), Duration::from_millis(50)));
    // Its output is read as a running execution's is, and is not yet
    // retained as a finished one's.
    assert_eq!(buffered_text(&executor, &id).as_deref(), Some("first"));
    assert!(executor.finished.lock().unwrap().get(&id).is_none());
    assert!(!executor.clear_execution(&id));

    gate.open();
    let outcome = wait_for_finish(&events);

    // The status follows the last of the output, and by the time it is
    // reported the execution is finished to everything that asks.
    outcome.assert_completed();
    assert_eq!(outcome.streamed_text(), "firstlate");
    assert!(!executor.is_running(&id));
    assert!(executor.get_running_ids().is_empty());
    assert!(executor.wait_for_executions(std::slice::from_ref(&id), Duration::ZERO));
    assert_eq!(buffered_text(&executor, &id).as_deref(), Some("firstlate"));
    assert!(executor.finished.lock().unwrap().get(&id).is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_after_the_process_has_exited_leaves_nothing_behind() {
    let executor = ActionExecutor::with_limits(patient_limits());
    let gate = Gate::closed();
    let (id, events) = run_sh_until_exited_with_output_open(&executor, &gate).await;

    // The execution still shows as running, so it can still be stopped, but
    // there is no process left to stop. The mark a stop leaves is cleared
    // when the process exits, which has already happened, so none is left.
    assert!(executor.is_running(&id));
    executor.stop(&id).unwrap();
    assert!(executor.stopped.lock().unwrap().is_empty());

    gate.open();
    let outcome = wait_for_finish(&events);

    // It reports how it really ended. The job that outlived the shell was not
    // signalled either: it ran on and wrote its line.
    outcome.assert_completed();
    assert_eq!(outcome.streamed_text(), "firstlate");
    assert!(executor.stopped.lock().unwrap().is_empty());

    // Nor does stopping a finished execution, or one that never was, leave a
    // mark.
    executor.stop(&id).unwrap();
    executor.stop("no-such-execution").unwrap();
    assert!(executor.stopped.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_execution_reports_stopped_and_clears_its_mark() {
    let executor = ActionExecutor::with_limits(patient_limits());
    let gate = Gate::closed();
    let (id, events) = run_sh_until_output(
        &executor,
        &format!("printf start; {}; printf end", gate.wait()),
        "start",
    )
    .await;

    // No escalation to SIGKILL: the hangup ends a plain `sh`, and a timer
    // must not outlive the test holding a pid that may be reused.
    executor
        .stop_with_options(
            &id,
            StopOptions {
                force_kill_after: None,
            },
        )
        .unwrap();
    let outcome = wait_for_finish(&events);

    assert_eq!(outcome.status, ActionStatus::Stopped);
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.streamed_text(), "start");
    assert!(executor.stopped.lock().unwrap().is_empty());
    assert!(!executor.is_running(&id));
}
