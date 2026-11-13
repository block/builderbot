//! Tests for how the executor buffers, bounds and retains output.
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
    loop {
        match events
            .recv_timeout(WAIT)
            .expect("the execution did not finish")
        {
            ExecutionEvent::Started { .. } => {}
            ExecutionEvent::Output { chunk, .. } => streamed.push(chunk),
            ExecutionEvent::StatusChanged {
                status, exit_code, ..
            } => {
                return Outcome {
                    streamed,
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
        .map(|chunks| chunks.iter().map(|c| c.chunk.as_str()).collect())
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

    let chunks = executor.get_buffered_output(&id).unwrap();
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
async fn reads_of_one_stream_are_coalesced() {
    let executor = ActionExecutor::with_limits(patient_limits());
    let written: String = (1..=3_000).map(|n| format!("{n}\n")).collect();

    for (script, stream) in [("seq 1 3000", "stdout"), ("seq 1 3000 >&2", "stderr")] {
        let Finished { id, outcome, .. } = run_sh_to_finish(&executor, script).await;

        // Many reads were streamed, and they are buffered as a single block.
        assert!(
            outcome.streamed.len() > 10,
            "{} reads",
            outcome.streamed.len()
        );
        let chunks = executor.get_buffered_output(&id).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].stream, stream);
        assert_eq!(chunks[0].chunk, written);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_character_split_across_reads_is_decoded_whole() {
    let (listener, events) = recorder();
    let buffer = OutputBuffer::shared(MAX_OUTPUT_BYTES);
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
        "exec".to_string(),
        listener,
        buffer.clone(),
        Handle::current(),
        reader_done,
    );
    assert!(wait_for_readers(&readers_done, WAIT));

    let streamed: Vec<String> = events
        .try_iter()
        .map(|event| match event {
            ExecutionEvent::Output { chunk, .. } => chunk,
            other => panic!("unexpected event {other:?}"),
        })
        .collect();
    // Each character is delivered once, with the read that completes it.
    assert_eq!(streamed, vec!["caf", "é ", "€"]);
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
