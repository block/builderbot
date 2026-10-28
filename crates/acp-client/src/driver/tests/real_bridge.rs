//! Turn-control against the bridges Staged actually runs: steering into a
//! live turn, interrupt-and-reprompt on the same child, and the steer /
//! turn-end race. Ignored by default — every test spawns an installed bridge
//! as the local user and calls the model, so it needs the bridge on `PATH`, a
//! signed-in CLI, network, and a minute or two.
//!
//! ```sh
//! cargo test -p acp-client real_bridge -- --ignored --nocapture --test-threads=1
//! ```
//!
//! The bridge is taken from `ACP_REAL_BRIDGE_DIR`, defaulting to Staged's
//! managed install (`~/.staged/packages/bin`) — pinned explicitly because
//! plain resolution goes through the login shell's `PATH`, where a stale
//! Homebrew `claude-agent-acp` can shadow it. `ACP_REAL_BRIDGE=<provider id>`
//! picks the bridge (`claude`, the default, or `codex`). Every writer event and every `log` line is printed to stderr
//! with its offset from connect, so under `--nocapture` the output is the
//! record of what the bridge did; `ACP_REAL_BRIDGE_LOG=debug` raises the
//! `log` level. The checks are the invariants, not the agent's wording: one
//! user row, in transcript order, the turn resolving once, the same child
//! across an interrupt, no hang.

use super::*;
use crate::driver::{SessionConnection, SessionSettled, SteerOutcome, TurnControlHandle};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;

const TURN_START_TIMEOUT: Duration = Duration::from_secs(90);
const TURN_TIMEOUT: Duration = Duration::from_secs(240);
const REPLY_TIMEOUT: Duration = Duration::from_secs(90);
const SETTLE_WAIT: Duration = Duration::from_secs(30);

/// A long, multi-step turn: one shell call per number keeps the agent busy
/// for a minute or more, with plenty of boundaries for a steer to land on.
const COUNT_PROMPT: &str = "Count from 1 to 40 slowly. For EACH number: first run the shell \
    command `sleep 1` with your shell tool as its own separate tool call (never a loop, never a \
    script, never several sleeps in one command), then write that number on its own line. Keep \
    going until you reach 40. Do not explain what you are doing.";
const STEER_MESSAGE: &str = "Stop counting right now and do not output any more numbers. \
    Instead reply with exactly one line: STEERED-DONE <basename of the current working directory>";
const INTERRUPT_MESSAGE: &str = "Stop what you were doing. Run the shell command `pwd` with \
    your shell tool, then reply with exactly one line: INTERRUPTED-DONE <basename of that directory>";
const RACE_PROMPT: &str = "Reply with exactly the single word: pong";
const RACE_MESSAGE: &str = "Reply with exactly one line: RACE-DONE";
const HELD_TURN_PROMPT: &str = "Use your Agent (subagent) tool with run_in_background set to \
    true to start one subagent whose task is exactly: run the shell command `sleep 30` and then \
    reply done. As soon as it is launched — without waiting for it — reply with exactly the single \
    word: LAUNCHED";
const HELD_STEER_MESSAGE: &str = "New instruction: reply with exactly one line: HELD-DONE";

fn provider_id() -> String {
    std::env::var("ACP_REAL_BRIDGE").unwrap_or_else(|_| "claude".to_string())
}

/// Pin bridge lookup to the managed install dir (process-wide, once).
fn pin_bridge_dir() {
    let dir = std::env::var_os("ACP_REAL_BRIDGE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".staged/packages/bin"))
        });
    if let Some(dir) = dir.filter(|dir| dir.is_dir()) {
        crate::types::set_bundled_tools_dir(dir);
    }
}

/// All assistant text so far, as the agent streamed it.
fn streamed_text(evs: &[Ev]) -> String {
    evs.iter()
        .filter_map(|ev| match ev {
            Ev::Append(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

// -----------------------------------------------------------------------------
// Trace: the writer, as a timestamped event log
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Ev {
    Append(String),
    Finalize,
    User(String),
    ToolCall { id: String, title: String },
    ToolResult { id: String },
    Permission { title: String },
    Event(String),
}

/// Transcript rows as a store would hold them: assistant text between
/// boundaries, user rows, tool calls.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    Assistant(String),
    User(String),
    Tool(String),
}

impl Row {
    fn is_user(&self) -> bool {
        matches!(self, Row::User(_))
    }

    fn text(&self) -> &str {
        match self {
            Row::Assistant(text) | Row::User(text) | Row::Tool(text) => text,
        }
    }
}

struct Trace {
    label: String,
    started: Instant,
    events: std::sync::Mutex<Vec<(Duration, Ev)>>,
}

impl Trace {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            started: Instant::now(),
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn push(&self, event: Ev) {
        let at = self.elapsed();
        eprintln!("[{} {:>7.2}s] {:?}", self.label, at.as_secs_f64(), event);
        self.events.lock().unwrap().push((at, event));
    }

    fn events(&self) -> Vec<(Duration, Ev)> {
        self.events.lock().unwrap().clone()
    }

    fn count(&self, pred: impl Fn(&Ev) -> bool) -> usize {
        self.events().iter().filter(|(_, ev)| pred(ev)).count()
    }

    /// Timestamp of the first event (after `from`) that `pred` accepts.
    fn first_at(&self, from: Duration, pred: impl Fn(&Ev) -> bool) -> Option<Duration> {
        self.events()
            .into_iter()
            .find(|(at, ev)| *at >= from && pred(ev))
            .map(|(at, _)| at)
    }

    async fn wait_for(&self, what: &str, timeout: Duration, pred: impl Fn(&[Ev]) -> bool) {
        let deadline = Instant::now() + timeout;
        loop {
            let snapshot: Vec<Ev> = self.events().into_iter().map(|(_, ev)| ev).collect();
            if pred(&snapshot) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "[{}] timed out after {timeout:?} waiting for {what}; events: {:#?}",
                self.label,
                self.events()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut open = String::new();
        let flush = |open: &mut String, rows: &mut Vec<Row>| {
            if !open.trim().is_empty() {
                rows.push(Row::Assistant(std::mem::take(open)));
            } else {
                open.clear();
            }
        };
        for (_, ev) in self.events() {
            match ev {
                Ev::Append(text) => open.push_str(&text),
                Ev::Finalize => flush(&mut open, &mut rows),
                Ev::User(text) => {
                    flush(&mut open, &mut rows);
                    rows.push(Row::User(text));
                }
                Ev::ToolCall { title, .. } => {
                    flush(&mut open, &mut rows);
                    rows.push(Row::Tool(title));
                }
                Ev::ToolResult { .. } | Ev::Permission { .. } | Ev::Event(_) => {}
            }
        }
        flush(&mut open, &mut rows);
        rows
    }

    fn dump_rows(&self) {
        eprintln!("[{}] transcript rows:", self.label);
        for (index, row) in self.rows().iter().enumerate() {
            let text: String = row.text().chars().take(160).collect();
            let text = text.replace('\n', "⏎");
            match row {
                Row::Assistant(_) => eprintln!("  {index:>2} assistant | {text}"),
                Row::User(_) => eprintln!("  {index:>2} USER      | {text}"),
                Row::Tool(_) => eprintln!("  {index:>2} tool      | {text}"),
            }
        }
    }

    /// Assistant text in the rows after the single user row.
    fn assistant_text_after_user(&self) -> String {
        let rows = self.rows();
        let user = rows
            .iter()
            .position(Row::is_user)
            .unwrap_or_else(|| panic!("no user row in {rows:#?}"));
        rows[user + 1..]
            .iter()
            .filter_map(|row| match row {
                Row::Assistant(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[async_trait::async_trait]
impl MessageWriter for Trace {
    async fn append_text(&self, text: &str) {
        self.push(Ev::Append(text.to_string()));
    }

    async fn finalize(&self) {
        self.push(Ev::Finalize);
    }

    async fn record_user_message(&self, text: &str, _images: &[PromptImage]) {
        self.push(Ev::User(text.to_string()));
    }

    async fn record_tool_call(
        &self,
        tool_call_id: &str,
        title: &str,
        _raw_input: Option<&serde_json::Value>,
    ) {
        self.push(Ev::ToolCall {
            id: tool_call_id.to_string(),
            title: title.to_string(),
        });
    }

    async fn update_tool_call_title(
        &self,
        _tool_call_id: &str,
        _title: Option<&str>,
        _raw_input: Option<&serde_json::Value>,
    ) {
    }

    async fn record_tool_result(&self, tool_call_id: &str, _content: &str) {
        self.push(Ev::ToolResult {
            id: tool_call_id.to_string(),
        });
    }

    async fn record_acp_event_metadata(&self, metadata: AcpEventMetadata) {
        self.push(Ev::Event(
            metadata.event_kind.unwrap_or_else(|| "?".to_string()),
        ));
    }

    async fn request_permission(
        &self,
        request: AcpPermissionRequest,
        cancel_token: CancellationToken,
    ) -> AcpPermissionDecision {
        self.push(Ev::Permission {
            title: request.tool_title.clone().unwrap_or_default(),
        });
        if cancel_token.is_cancelled() {
            AcpPermissionDecision::Cancelled
        } else {
            autoapprove_permission_decision(&request)
        }
    }
}

// -----------------------------------------------------------------------------
// Logger: the driver's own `log` lines, on the same clock
// -----------------------------------------------------------------------------

struct StderrLogger {
    started: Instant,
}

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.target().starts_with("acp_client")
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "[log {:>7.2}s] {} {}",
                self.started.elapsed().as_secs_f64(),
                record.level(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

fn install_logger() {
    static LOGGER: OnceLock<StderrLogger> = OnceLock::new();
    let logger = LOGGER.get_or_init(|| StderrLogger {
        started: Instant::now(),
    });
    if log::set_logger(logger).is_ok() {
        let level = match std::env::var("ACP_REAL_BRIDGE_LOG").as_deref() {
            Ok("debug") => log::LevelFilter::Debug,
            Ok("trace") => log::LevelFilter::Trace,
            _ => log::LevelFilter::Info,
        };
        log::set_max_level(level);
    }
}

// -----------------------------------------------------------------------------
// Session: one real connection
// -----------------------------------------------------------------------------

struct Session {
    connection: SessionConnection,
    handle: TurnControlHandle,
    trace: Arc<Trace>,
    store: Arc<RecordingStore>,
    cancel: CancellationToken,
    settled_rx: Option<oneshot::Receiver<SessionSettled>>,
    dir: PathBuf,
}

impl Session {
    /// Spawn the installed bridge in a fresh temp dir, with the process's own
    /// environment as the snapshot (so the shim and its Node resolve the way
    /// they do under Staged) and, like the app, a post-turn background hold.
    async fn connect(label: &str, steering_disabled: bool) -> Self {
        install_logger();
        pin_bridge_dir();
        let provider = provider_id();
        let driver = AcpDriver::new(&provider)
            .unwrap_or_else(|e| panic!("{e}: put the bridge on PATH (see the module docs)"))
            .with_env_snapshot(std::env::vars().collect())
            .with_background_hold(Some(BackgroundHoldConfig::default()))
            .with_steering_disabled(steering_disabled);
        let dir = std::env::temp_dir().join(format!(
            "acp-real-bridge-{label}-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp working dir");
        let trace = Arc::new(Trace::new(label));
        let writer: Arc<dyn MessageWriter> = Arc::clone(&trace) as Arc<dyn MessageWriter>;
        let store = Arc::new(RecordingStore::default());
        let dyn_store: Arc<dyn Store> = Arc::clone(&store) as Arc<dyn Store>;
        let cancel = CancellationToken::new();
        eprintln!(
            "[{label}] connecting {provider} ({}) in {} (steering_disabled={steering_disabled})",
            driver.binary_path.display(),
            dir.display()
        );
        let mut connection = driver
            .connect(
                &format!("real-{label}"),
                &dir,
                &dyn_store,
                &writer,
                &cancel,
                None,
                &[],
                None,
            )
            .await
            .expect("bridge should spawn");
        let handle = connection.turn_control_handle();
        let settled_rx = connection.take_settled_receiver();
        Self {
            connection,
            handle,
            trace,
            store,
            cancel,
            settled_rx,
            dir,
        }
    }

    fn basename(&self) -> String {
        self.dir.file_name().unwrap().to_string_lossy().into_owned()
    }

    /// The bridge processes this test spawned: direct children whose command
    /// line names the bridge.
    fn bridge_pids(&self) -> Vec<u32> {
        let out = std::process::Command::new("pgrep")
            .args(["-P", &std::process::id().to_string()])
            .output()
            .expect("pgrep");
        let mut pids = Vec::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Ok(pid) = line.trim().parse::<u32>() else {
                continue;
            };
            let ps = std::process::Command::new("ps")
                .args(["-o", "command=", "-p", &pid.to_string()])
                .output()
                .expect("ps");
            let command = String::from_utf8_lossy(&ps.stdout);
            if command.contains("-acp") {
                pids.push(pid);
            }
        }
        pids
    }

    /// Cancel the session (ending the hold) and wait for teardown.
    async fn finish(mut self) {
        self.trace.dump_rows();
        self.cancel.cancel();
        if let Some(rx) = self.settled_rx.take() {
            match tokio::time::timeout(SETTLE_WAIT, rx).await {
                Ok(Ok(settled)) => eprintln!(
                    "[{}] settled: {:?} / {:?}",
                    self.trace.label, settled.outcome, settled.reason
                ),
                Ok(Err(_)) => eprintln!("[{}] settled sender dropped", self.trace.label),
                Err(_) => panic!("[{}] teardown hung after cancel", self.trace.label),
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn activity(ev: &Ev) -> bool {
    matches!(
        ev,
        Ev::Append(_) | Ev::ToolCall { .. } | Ev::Permission { .. }
    )
}

fn user_rows(trace: &Trace) -> usize {
    trace.count(|ev| matches!(ev, Ev::User(_)))
}

// -----------------------------------------------------------------------------
// Scenario 1: steer during a live turn
// -----------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
#[ignore = "spawns the installed ACP bridge and calls the model"]
async fn real_bridge_steer_mid_turn_redirects_the_live_turn() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::connect("steer-live", false).await;
            let trace = Arc::clone(&session.trace);
            let handle = session.handle.clone();
            let prompt_done = Rc::new(Cell::new(false));
            let prompt_done_for_steer = Rc::clone(&prompt_done);
            let steer = tokio::task::spawn_local(async move {
                trace
                    .wait_for("the turn's first activity", TURN_START_TIMEOUT, |evs| {
                        evs.iter().any(activity)
                    })
                    .await;
                tokio::time::sleep(Duration::from_secs(2)).await;
                let sent_at = trace.elapsed();
                let outcome = handle.steer(STEER_MESSAGE, &[]).await;
                let answered_at = trace.elapsed();
                let turn_still_live = !prompt_done_for_steer.get();
                eprintln!(
                    "[steer-live] steer sent at {:.2}s, answered {outcome:?} at {:.2}s (turn still live: {turn_still_live})",
                    sent_at.as_secs_f64(),
                    answered_at.as_secs_f64()
                );
                (turn_still_live, answered_at, outcome)
            });

            let turn = tokio::time::timeout(TURN_TIMEOUT, session.connection.prompt(COUNT_PROMPT, &[]))
                .await
                .expect("the steered turn should end");
            prompt_done.set(true);
            let resolved_at = session.trace.elapsed();
            eprintln!("[steer-live] prompt() resolved {turn:?} at {:.2}s", resolved_at.as_secs_f64());
            let (turn_still_live, answered_at, outcome) = steer.await.expect("steer task");
            assert!(turn_still_live, "the steer must land inside the live turn, not after it");

            assert_eq!(outcome, Ok(SteerOutcome::Injected));
            assert_eq!(turn, Ok(AgentRunOutcome::Completed));
            let rows = session.trace.rows();
            let users: Vec<usize> = rows
                .iter()
                .enumerate()
                .filter(|(_, row)| row.is_user())
                .map(|(index, _)| index)
                .collect();
            assert_eq!(users.len(), 1, "exactly one user row: {rows:#?}");
            assert!(
                users[0] > 0,
                "the steer lands after the turn's own output: {rows:#?}"
            );
            assert_eq!(rows[users[0]], Row::User(STEER_MESSAGE.to_string()));
            let after = session.trace.assistant_text_after_user();
            assert!(
                after.contains("STEERED-DONE"),
                "the agent should answer the steer; text after the user row: {after:?}"
            );
            assert!(
                after.contains(&session.basename()),
                "the agent should answer the steer's question; got {after:?}"
            );
            // Course change: the first thing after the steer was taken should
            // follow within model latency, not after the count ran out.
            let user_at = session
                .trace
                .first_at(Duration::ZERO, |ev| matches!(ev, Ev::User(_)))
                .unwrap();
            let next = session
                .trace
                .first_at(user_at, |ev| matches!(ev, Ev::Append(_)))
                .expect("assistant text after the user row");
            eprintln!(
                "[steer-live] first assistant text {:.2}s after the steer was answered",
                (next - answered_at).as_secs_f64()
            );
            assert!(
                next - answered_at < Duration::from_secs(30),
                "the agent should change course within a few seconds"
            );
            assert_eq!(session.store.agent_session_ids.lock().unwrap().len(), 1);
            session.finish().await;
        })
        .await;
}

// -----------------------------------------------------------------------------
// Scenario 2: steer while the bridge holds the turn open for background work
// -----------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
#[ignore = "spawns the installed ACP bridge and calls the model"]
async fn real_bridge_steer_while_the_bridge_holds_the_turn_for_a_subagent() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::connect("steer-held", false).await;
            let trace = Arc::clone(&session.trace);
            let handle = session.handle.clone();
            let prompt_done = Rc::new(Cell::new(false));
            let prompt_done_for_steer = Rc::clone(&prompt_done);
            let steer = tokio::task::spawn_local(async move {
                trace
                    .wait_for("LAUNCHED", TURN_START_TIMEOUT, |evs| {
                        streamed_text(evs).contains("LAUNCHED")
                    })
                    .await;
                tokio::time::sleep(Duration::from_secs(3)).await;
                let held = !prompt_done_for_steer.get();
                eprintln!("[steer-held] bridge still holding the turn 3s after LAUNCHED: {held}");
                let outcome = handle.steer(HELD_STEER_MESSAGE, &[]).await;
                eprintln!("[steer-held] steer answered {outcome:?} at {:.2}s", trace.elapsed().as_secs_f64());
                (held, outcome)
            });

            let turn = tokio::time::timeout(TURN_TIMEOUT, session.connection.prompt(HELD_TURN_PROMPT, &[]))
                .await
                .expect("the held turn should end");
            prompt_done.set(true);
            eprintln!("[steer-held] prompt() resolved {turn:?} at {:.2}s", session.trace.elapsed().as_secs_f64());
            let (held, outcome) = steer.await.expect("steer task");
            if held {
                eprintln!("SCENARIO 2: bridge held the turn; steer outcome {outcome:?}");
                assert_eq!(outcome, Ok(SteerOutcome::Injected));
            } else {
                // Not every bridge holds a turn for background subagents
                // (codex-acp does not): the steer then lands in the client's
                // own post-turn hold — a fresh turn on the same connection —
                // or, once that hold has quiesced and torn down, fails fast
                // for the caller to route as a resume.
                eprintln!(
                    "SCENARIO 2: bridge did NOT hold the turn open (prompt resolved before the steer): \
                     {outcome:?}"
                );
                match &outcome {
                    Ok(SteerOutcome::NewTurnStarted) => {}
                    Err(e) if e.contains("no longer running") => {
                        assert_eq!(user_rows(&session.trace), 0);
                        session.finish().await;
                        return;
                    }
                    other => panic!("unexpected steer outcome out of turn: {other:?}"),
                }
            }
            session
                .trace
                .wait_for("the reply to the steer", REPLY_TIMEOUT, |evs| {
                    streamed_text(evs).contains("HELD-DONE")
                })
                .await;
            assert_eq!(user_rows(&session.trace), 1);
            session.finish().await;
        })
        .await;
}

// -----------------------------------------------------------------------------
// Scenario 3: interrupt-and-reprompt (steering disabled on the driver)
// -----------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
#[ignore = "spawns the installed ACP bridge and calls the model"]
async fn real_bridge_interrupt_reprompts_on_the_same_child() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::connect("interrupt", true).await;
            let pids_before = session.bridge_pids();
            assert_eq!(pids_before.len(), 1, "one bridge child: {pids_before:?}");
            let trace = Arc::clone(&session.trace);
            let handle = session.handle.clone();
            let prompt_done = Rc::new(Cell::new(false));
            let prompt_done_for_interrupt = Rc::clone(&prompt_done);
            let interrupt = tokio::task::spawn_local(async move {
                trace
                    .wait_for("the turn's first activity", TURN_START_TIMEOUT, |evs| {
                        evs.iter().any(activity)
                    })
                    .await;
                tokio::time::sleep(Duration::from_secs(2)).await;
                // The caller's routing: steer first, interrupt on refusal.
                let steer = handle.steer(INTERRUPT_MESSAGE, &[]).await;
                eprintln!("[interrupt] steer refused as expected: {steer:?}");
                assert!(steer
                    .as_ref()
                    .is_err_and(|e| e.contains("does not advertise steering")));
                assert_eq!(user_rows(&trace), 0, "a refused steer leaves no row");
                let sent_at = trace.elapsed();
                let outcome = handle.interrupt_and_reprompt(INTERRUPT_MESSAGE, &[]).await;
                let answered_at = trace.elapsed();
                // No terminal state mid-way: the interrupted prompt() is
                // still pending (and so, under the hold, is teardown).
                let prompt_still_pending = !prompt_done_for_interrupt.get();
                eprintln!(
                    "[interrupt] interrupt sent at {:.2}s, answered {outcome:?} at {:.2}s (prompt still pending: {prompt_still_pending})",
                    sent_at.as_secs_f64(),
                    answered_at.as_secs_f64()
                );
                (prompt_still_pending, outcome)
            });

            let turn = tokio::time::timeout(TURN_TIMEOUT, session.connection.prompt(COUNT_PROMPT, &[]))
                .await
                .expect("the re-prompted turn should end");
            prompt_done.set(true);
            let resolved_at = session.trace.elapsed();
            eprintln!("[interrupt] prompt() resolved {turn:?} at {:.2}s", resolved_at.as_secs_f64());
            let (prompt_still_pending, outcome) = interrupt.await.expect("interrupt task");
            let pids_after = session.bridge_pids();

            assert_eq!(outcome, Ok(()));
            assert!(
                prompt_still_pending,
                "the interrupted prompt() resolves with the final turn, not at the cancel"
            );
            assert_eq!(turn, Ok(AgentRunOutcome::Completed));
            assert_eq!(pids_after, pids_before, "the re-prompt runs on the same child");
            assert_eq!(session.store.agent_session_ids.lock().unwrap().len(), 1);
            let rows = session.trace.rows();
            let users: Vec<usize> = rows
                .iter()
                .enumerate()
                .filter(|(_, row)| row.is_user())
                .map(|(index, _)| index)
                .collect();
            assert_eq!(users.len(), 1, "exactly one user row: {rows:#?}");
            assert!(users[0] > 0, "the interrupted turn's output precedes the row: {rows:#?}");
            let after = session.trace.assistant_text_after_user();
            assert!(after.contains("INTERRUPTED-DONE"), "text after the user row: {after:?}");
            assert!(after.contains(&session.basename()), "text after the user row: {after:?}");
            // Permission requests in the second turn still reach the handler:
            // if the bridge asked for the first turn's shell calls, it must
            // have asked for the `pwd` too.
            let user_at = session
                .trace
                .first_at(Duration::ZERO, |ev| matches!(ev, Ev::User(_)))
                .unwrap();
            let permissions_before = session
                .trace
                .events()
                .iter()
                .filter(|(at, ev)| *at < user_at && matches!(ev, Ev::Permission { .. }))
                .count();
            let permissions_after = session
                .trace
                .events()
                .iter()
                .filter(|(at, ev)| *at >= user_at && matches!(ev, Ev::Permission { .. }))
                .count();
            eprintln!("[interrupt] permission requests before/after the interrupt: {permissions_before}/{permissions_after}");
            if permissions_before > 0 {
                assert!(permissions_after > 0, "turn 2's permission request never reached the handler");
            }
            session.finish().await;
        })
        .await;
}

// -----------------------------------------------------------------------------
// Scenario 4: steer racing the end of the turn
// -----------------------------------------------------------------------------

/// Send a steer `when` the short turn is at (or past) its end; whatever the
/// bridge answers, the caller's routing must end with the message delivered
/// exactly once and nothing hung.
async fn race_steer(label: &str, steer_after_first_chunk: bool) {
    let mut session = Session::connect(label, false).await;
    let trace = Arc::clone(&session.trace);
    let handle = session.handle.clone();
    let label_owned = label.to_string();
    let steer = tokio::task::spawn_local(async move {
        if steer_after_first_chunk {
            trace
                .wait_for("the first chunk", TURN_START_TIMEOUT, |evs| {
                    evs.iter().any(|ev| matches!(ev, Ev::Append(_)))
                })
                .await;
        }
        let sent_at = trace.elapsed();
        let outcome = tokio::time::timeout(REPLY_TIMEOUT, handle.steer(RACE_MESSAGE, &[]))
            .await
            .expect("a steer at the turn boundary must not hang");
        eprintln!(
            "[{label_owned}] steer sent at {:.2}s answered {outcome:?} at {:.2}s",
            sent_at.as_secs_f64(),
            trace.elapsed().as_secs_f64()
        );
        let delivered = match &outcome {
            Ok(_) => true,
            Err(e) => {
                // The caller's fallback: the connection is holding, so this
                // is served as a plain new turn.
                let fallback = tokio::time::timeout(
                    REPLY_TIMEOUT,
                    handle.interrupt_and_reprompt(RACE_MESSAGE, &[]),
                )
                .await
                .expect("the fallback must not hang");
                eprintln!("[{label_owned}] steer failed ({e}); fallback answered {fallback:?}");
                fallback.is_ok()
            }
        };
        (outcome, delivered)
    });

    let turn = tokio::time::timeout(TURN_TIMEOUT, session.connection.prompt(RACE_PROMPT, &[]))
        .await
        .expect("the short turn should end");
    eprintln!(
        "[{label}] prompt() resolved {turn:?} at {:.2}s",
        session.trace.elapsed().as_secs_f64()
    );
    let (outcome, delivered) = steer.await.expect("steer task");
    assert_eq!(turn, Ok(AgentRunOutcome::Completed));
    assert!(
        delivered,
        "neither the steer nor the fallback delivered: {outcome:?}"
    );
    session
        .trace
        .wait_for("the reply to the steer", REPLY_TIMEOUT, |evs| {
            streamed_text(evs).contains("RACE-DONE")
        })
        .await;
    // Let any trailing frames land before counting rows.
    tokio::time::sleep(Duration::from_secs(3)).await;
    eprintln!(
        "SCENARIO 4 ({label}): steer outcome {outcome:?}; user rows {}",
        user_rows(&session.trace)
    );
    let rows = session.trace.rows();
    assert_eq!(user_rows(&session.trace), 1, "{rows:#?}");
    // The aborted cycle's result must not re-echo the pre-steer message as a
    // row after the user row (claude-agent-acp emits that result; the client
    // has to recognise its text as already streamed).
    let user = rows.iter().position(Row::is_user).unwrap();
    for before in rows[..user]
        .iter()
        .filter(|row| matches!(row, Row::Assistant(_)))
    {
        assert!(
            !rows[user + 1..].contains(before),
            "pre-steer assistant row echoed after the user row: {rows:#?}"
        );
    }
    session.finish().await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "spawns the installed ACP bridge and calls the model"]
async fn real_bridge_steer_sent_with_the_prompt_lands_once() {
    tokio::task::LocalSet::new()
        .run_until(race_steer("race-at-send", false))
        .await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "spawns the installed ACP bridge and calls the model"]
async fn real_bridge_steer_sent_at_the_first_chunk_lands_once() {
    tokio::task::LocalSet::new()
        .run_until(race_steer("race-at-chunk", true))
        .await;
}
