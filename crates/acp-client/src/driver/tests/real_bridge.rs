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
//! picks the bridge (`claude`, the default, `codex`, or `goose`). Goose is not
//! a managed bridge: it resolves from the login shell's `PATH` and runs as
//! `goose acp`, and under a headless runner it needs `GOOSE_DISABLE_KEYRING=1`
//! — otherwise the CLI blocks in `SecKeychainFindGenericPassword` on a Keychain
//! prompt nobody can click, and never answers `session/new`. Every writer
//! event and every `log` line is printed to stderr with its offset from
//! connect, so under `--nocapture` the output is the record of what the bridge
//! did; `ACP_REAL_BRIDGE_LOG=debug` raises the printed `log` level (debug
//! lines are always captured for assertions). The checks are the invariants,
//! not the agent's wording: one user row, in transcript order, the turn
//! resolving once, the same child across an interrupt, no hang.

use super::*;
use crate::driver::{SessionConnection, SessionSettled, SteerOutcome, TurnControlHandle};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
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
const REMEMBER_PROMPT: &str = "Remember this code word: PELICAN-7342. Do not use any tools. Reply \
    with exactly the single word: REMEMBERED";
const RECALL_PROMPT: &str = "What was the code word I asked you to remember earlier in this \
    conversation? Do not use any tools. Reply with exactly one line: CODEWORD <the code word>";
/// How long a held permission request stays open before the writer gives up
/// waiting for the connection to resolve it.
const PERMISSION_HOLD: Duration = Duration::from_secs(45);

fn provider_id() -> String {
    std::env::var("ACP_REAL_BRIDGE").unwrap_or_else(|_| "claude".to_string())
}

/// Goose is the installed bridge that genuinely does not advertise steering;
/// against the others the interrupt path has to be forced with
/// `with_steering_disabled`.
fn bridge_advertises_steering() -> bool {
    provider_id() != "goose"
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
    ToolCall {
        id: String,
        title: String,
    },
    ToolResult {
        id: String,
    },
    Permission {
        title: String,
    },
    /// A held permission request resolved by the connection cancelling it.
    PermissionCancelled {
        title: String,
    },
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
    /// Hold the next permission request open — the way a dialog nobody has
    /// answered yet does — until the connection cancels it, the test releases
    /// it, or [`PERMISSION_HOLD`] passes.
    hold_next_permission: AtomicBool,
    release_permission: tokio::sync::Notify,
}

impl Trace {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            started: Instant::now(),
            events: std::sync::Mutex::new(Vec::new()),
            hold_next_permission: AtomicBool::new(false),
            release_permission: tokio::sync::Notify::new(),
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
                Ev::ToolResult { .. }
                | Ev::Permission { .. }
                | Ev::PermissionCancelled { .. }
                | Ev::Event(_) => {}
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
        let title = request.tool_title.clone().unwrap_or_default();
        self.push(Ev::Permission {
            title: title.clone(),
        });
        if cancel_token.is_cancelled() {
            return AcpPermissionDecision::Cancelled;
        }
        if self.hold_next_permission.swap(false, Ordering::SeqCst) {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    self.push(Ev::PermissionCancelled { title });
                    return AcpPermissionDecision::Cancelled;
                }
                _ = self.release_permission.notified() => {
                    self.push(Ev::Event("permission-hold-released".to_string()));
                }
                _ = tokio::time::sleep(PERMISSION_HOLD) => {
                    self.push(Ev::Event("permission-hold-expired".to_string()));
                }
            }
        }
        autoapprove_permission_decision(&request)
    }
}

// -----------------------------------------------------------------------------
// Logger: the driver's own `log` lines, on the same clock
// -----------------------------------------------------------------------------

struct StderrLogger {
    started: Instant,
    /// Lines at or above this level are printed; every line is captured.
    print_level: log::LevelFilter,
    lines: std::sync::Mutex<Vec<String>>,
}

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.target().starts_with("acp_client")
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let line = format!("{} {}", record.level(), record.args());
            if record.level() <= self.print_level {
                eprintln!(
                    "[log {:>7.2}s] {line}",
                    self.started.elapsed().as_secs_f64()
                );
            }
            self.lines.lock().unwrap().push(line);
        }
    }

    fn flush(&self) {}
}

static LOGGER: OnceLock<StderrLogger> = OnceLock::new();

fn install_logger() {
    let logger = LOGGER.get_or_init(|| StderrLogger {
        started: Instant::now(),
        print_level: match std::env::var("ACP_REAL_BRIDGE_LOG").as_deref() {
            Ok("debug") => log::LevelFilter::Debug,
            Ok("trace") => log::LevelFilter::Trace,
            _ => log::LevelFilter::Info,
        },
        lines: std::sync::Mutex::new(Vec::new()),
    });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}

/// Whether the driver logged a line containing `needle` (any level).
fn logged(needle: &str) -> bool {
    LOGGER.get().is_some_and(|logger| {
        logger
            .lines
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains(needle))
    })
}

// -----------------------------------------------------------------------------
// Session: one real connection
// -----------------------------------------------------------------------------

/// The store as the app would hold it: the agent's session id, and the
/// transcript rows the replay matcher is handed on a resume.
#[derive(Default)]
struct BridgeStore {
    agent_session_ids: std::sync::Mutex<Vec<(String, String)>>,
    messages: std::sync::Mutex<Vec<(String, String)>>,
}

#[async_trait::async_trait]
impl Store for BridgeStore {
    fn set_agent_session_id(&self, session_id: &str, agent_session_id: &str) -> Result<(), String> {
        self.agent_session_ids
            .lock()
            .unwrap()
            .push((session_id.to_string(), agent_session_id.to_string()));
        Ok(())
    }

    fn get_session_messages(&self, _session_id: &str) -> Result<Vec<(String, String)>, String> {
        Ok(self.messages.lock().unwrap().clone())
    }
}

/// An earlier connection's session to load instead of creating a new one.
struct Resume {
    agent_session_id: String,
    /// That connection's transcript as `(role, content)` rows.
    messages: Vec<(String, String)>,
    /// Its working dir — a load happens where the session was created.
    dir: PathBuf,
}

#[derive(Default)]
struct ConnectOptions {
    steering_disabled: bool,
    /// Environment set over the process's own for the bridge.
    env: Vec<(&'static str, &'static str)>,
    resume: Option<Resume>,
}

struct Session {
    connection: SessionConnection,
    handle: TurnControlHandle,
    trace: Arc<Trace>,
    store: Arc<BridgeStore>,
    cancel: CancellationToken,
    settled_rx: Option<oneshot::Receiver<SessionSettled>>,
    dir: PathBuf,
}

impl Session {
    /// Spawn the installed bridge in a fresh temp dir, with the process's own
    /// environment as the snapshot (so the shim and its Node resolve the way
    /// they do under Staged) and, like the app, a post-turn background hold.
    async fn connect(label: &str, steering_disabled: bool) -> Self {
        Self::connect_with(
            label,
            ConnectOptions {
                steering_disabled,
                ..ConnectOptions::default()
            },
        )
        .await
    }

    async fn connect_with(label: &str, options: ConnectOptions) -> Self {
        install_logger();
        pin_bridge_dir();
        let provider = provider_id();
        let mut env: Vec<(String, String)> = std::env::vars().collect();
        for (key, value) in &options.env {
            env.retain(|(name, _)| name != key);
            env.push((key.to_string(), value.to_string()));
        }
        let driver = AcpDriver::new(&provider)
            .unwrap_or_else(|e| panic!("{e}: put the bridge on PATH (see the module docs)"))
            .with_env_snapshot(env)
            .with_background_hold(Some(BackgroundHoldConfig::default()))
            .with_steering_disabled(options.steering_disabled);
        let dir = match &options.resume {
            Some(resume) => resume.dir.clone(),
            None => std::env::temp_dir().join(format!(
                "acp-real-bridge-{label}-{}-{}",
                std::process::id(),
                Instant::now().elapsed().as_nanos()
            )),
        };
        std::fs::create_dir_all(&dir).expect("temp working dir");
        let trace = Arc::new(Trace::new(label));
        let writer: Arc<dyn MessageWriter> = Arc::clone(&trace) as Arc<dyn MessageWriter>;
        let store = Arc::new(BridgeStore::default());
        if let Some(resume) = &options.resume {
            *store.messages.lock().unwrap() = resume.messages.clone();
        }
        let dyn_store: Arc<dyn Store> = Arc::clone(&store) as Arc<dyn Store>;
        let cancel = CancellationToken::new();
        eprintln!(
            "[{label}] connecting {provider} ({}) in {} (steering_disabled={}, env={:?}, resume={:?})",
            driver.binary_path.display(),
            dir.display(),
            options.steering_disabled,
            options.env,
            options.resume.as_ref().map(|resume| &resume.agent_session_id)
        );
        let mut connection = driver
            .connect(
                &format!("real-{label}"),
                &dir,
                &dyn_store,
                &writer,
                &cancel,
                options
                    .resume
                    .as_ref()
                    .map(|resume| resume.agent_session_id.as_str()),
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

    /// The agent's id for this session, as the store recorded it at setup.
    fn agent_session_id(&self) -> String {
        self.store
            .agent_session_ids
            .lock()
            .unwrap()
            .first()
            .map(|(_, id)| id.clone())
            .expect("the store should have the agent's session id")
    }

    /// This session's transcript as the app's store would replay it: the
    /// prompt as the first user row, then the recorded assistant and user
    /// rows.
    fn legacy_messages(&self, prompt: &str) -> Vec<(String, String)> {
        let mut messages = vec![("user".to_string(), prompt.to_string())];
        for row in self.trace.rows() {
            match row {
                Row::Assistant(text) => messages.push(("assistant".to_string(), text)),
                Row::User(text) => messages.push(("user".to_string(), text)),
                Row::Tool(_) => {}
            }
        }
        messages
    }

    /// Cancel the session (ending the hold) and wait for teardown.
    async fn finish(self) {
        self.finish_inner(true).await;
    }

    /// [`Self::finish`], keeping the working dir for a later resume.
    async fn finish_keeping_dir(self) {
        self.finish_inner(false).await;
    }

    async fn finish_inner(mut self, remove_dir: bool) {
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
        if remove_dir {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// The bridge processes this test spawned: direct children whose command
/// line names the bridge.
fn bridge_pids() -> Vec<u32> {
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
        // `claude-agent-acp` / `codex-acp` by name, `goose acp` by subcommand.
        if command.contains("-acp") || command.contains(" acp") {
            pids.push(pid);
        }
    }
    pids
}

/// Wait for every bridge child of this process to be gone.
async fn wait_for_bridge_exit(timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !bridge_pids().is_empty() {
        assert!(
            Instant::now() < deadline,
            "bridge children still running after {timeout:?}: {:?}",
            bridge_pids()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Assistant rows before the user row that reappear after it.
fn echoed_rows(rows: &[Row]) -> Vec<&Row> {
    let Some(user) = rows.iter().position(Row::is_user) else {
        return Vec::new();
    };
    rows[..user]
        .iter()
        .filter(|row| matches!(row, Row::Assistant(_)) && rows[user + 1..].contains(row))
        .collect()
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
            let cancel = session.cancel.clone();
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
                if outcome.is_err() {
                    // Nothing more to learn from the long turn.
                    cancel.cancel();
                }
                (turn_still_live, answered_at, outcome)
            });

            let turn = tokio::time::timeout(TURN_TIMEOUT, session.connection.prompt(COUNT_PROMPT, &[]))
                .await
                .expect("the steered turn should end");
            prompt_done.set(true);
            let resolved_at = session.trace.elapsed();
            eprintln!("[steer-live] prompt() resolved {turn:?} at {:.2}s", resolved_at.as_secs_f64());
            let (turn_still_live, answered_at, outcome) = steer.await.expect("steer task");
            if !bridge_advertises_steering() {
                eprintln!(
                    "SCENARIO 1: N/A — {} does not advertise steering; the steer was refused \
                     locally ({outcome:?}) and nothing was written",
                    provider_id()
                );
                assert!(outcome
                    .as_ref()
                    .is_err_and(|e| e.contains("does not advertise steering")));
                assert_eq!(user_rows(&session.trace), 0);
                session.finish().await;
                return;
            }
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
    if !bridge_advertises_steering() {
        eprintln!(
            "SCENARIO 2: N/A — {} does not advertise steering (and has no background subagent tool)",
            provider_id()
        );
        return;
    }
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
// Scenario 3: interrupt-and-reprompt (steering unavailable: not advertised by
// the bridge, or disabled on the driver for one that does)
// -----------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
#[ignore = "spawns the installed ACP bridge and calls the model"]
async fn real_bridge_interrupt_reprompts_on_the_same_child() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let advertises = bridge_advertises_steering();
            let mut session = Session::connect("interrupt", advertises).await;
            let pids_before = bridge_pids();
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
                // The re-prompt is live now: the child it runs on. (Read
                // here, not after the turn — a bridge with no observable
                // background work gets no post-turn hold, so the child is
                // gone the moment prompt() resolves.)
                let pids_during_reprompt = bridge_pids();
                // No terminal state mid-way: the interrupted prompt() is
                // still pending (and so, under the hold, is teardown).
                let prompt_still_pending = !prompt_done_for_interrupt.get();
                eprintln!(
                    "[interrupt] interrupt sent at {:.2}s, answered {outcome:?} at {:.2}s — cancel acked after {:.3}s (prompt still pending: {prompt_still_pending}, child: {pids_during_reprompt:?})",
                    sent_at.as_secs_f64(),
                    answered_at.as_secs_f64(),
                    (answered_at - sent_at).as_secs_f64()
                );
                (prompt_still_pending, outcome, pids_during_reprompt)
            });

            let turn = tokio::time::timeout(TURN_TIMEOUT, session.connection.prompt(COUNT_PROMPT, &[]))
                .await
                .expect("the re-prompted turn should end");
            prompt_done.set(true);
            let resolved_at = session.trace.elapsed();
            eprintln!("[interrupt] prompt() resolved {turn:?} at {:.2}s", resolved_at.as_secs_f64());
            let (prompt_still_pending, outcome, pids_during_reprompt) =
                interrupt.await.expect("interrupt task");

            // What initialize said, before any forcing (logged by the
            // connection task during setup, so read after the turn).
            let advertised = format!("steering advertised: {advertises}");
            assert!(logged(&advertised), "driver did not log {advertised:?}");
            eprintln!("[interrupt] {advertised} (forced off on the driver: {advertises})");
            assert_eq!(outcome, Ok(()));
            assert!(
                prompt_still_pending,
                "the interrupted prompt() resolves with the final turn, not at the cancel"
            );
            assert_eq!(turn, Ok(AgentRunOutcome::Completed));
            assert_eq!(
                pids_during_reprompt, pids_before,
                "the re-prompt runs on the same child"
            );
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
            Ok(_) => Some(true),
            Err(e) => {
                // The caller's fallback: the connection is holding, so this
                // is served as a plain new turn — or, on a hold-less bridge
                // whose connection tore down at turn end, fails fast for
                // the caller to route as a resume.
                let fallback = tokio::time::timeout(
                    REPLY_TIMEOUT,
                    handle.interrupt_and_reprompt(RACE_MESSAGE, &[]),
                )
                .await
                .expect("the fallback must not hang");
                eprintln!("[{label_owned}] steer failed ({e}); fallback answered {fallback:?}");
                match &fallback {
                    Ok(()) => Some(true),
                    Err(e) if e.contains("no longer running") => None,
                    Err(_) => Some(false),
                }
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
    let Some(delivered) = delivered else {
        eprintln!(
            "SCENARIO 4 ({label}): the turn ended and the connection tore down before the \
             message arrived; the caller routes that as a resume"
        );
        assert_eq!(user_rows(&session.trace), 0);
        session.finish().await;
        return;
    };
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
    assert!(
        echoed_rows(&rows).is_empty(),
        "pre-steer assistant row echoed after the user row: {rows:#?}"
    );
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

// -----------------------------------------------------------------------------
// Scenario 5: interrupt while the bridge waits on a permission decision
// -----------------------------------------------------------------------------

/// The first permission request is held open (a dialog nobody has answered)
/// when the interrupt arrives. The cancel must still be acknowledged, the
/// held request resolved rather than left dangling, and the next turn's own
/// permission request must reach the handler. Skipped when the bridge asks
/// for nothing: `GOOSE_MODE=approve` makes Goose ask for every tool call;
/// the managed bridges ignore the variable and approve their shell calls
/// themselves.
#[tokio::test(flavor = "current_thread")]
#[ignore = "spawns the installed ACP bridge and calls the model"]
async fn real_bridge_interrupt_while_a_permission_request_is_pending() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut session = Session::connect_with(
                "interrupt-perm",
                ConnectOptions {
                    steering_disabled: bridge_advertises_steering(),
                    env: vec![("GOOSE_MODE", "approve")],
                    ..ConnectOptions::default()
                },
            )
            .await;
            session
                .trace
                .hold_next_permission
                .store(true, Ordering::SeqCst);
            let pids_before = bridge_pids();
            let trace = Arc::clone(&session.trace);
            let handle = session.handle.clone();
            let interrupt = tokio::task::spawn_local(async move {
                let deadline = Instant::now() + TURN_START_TIMEOUT;
                loop {
                    if trace.count(|ev| matches!(ev, Ev::Permission { .. })) > 0 {
                        break;
                    }
                    // A tool call that completed without asking: this bridge
                    // does not request permission for shell calls.
                    if trace.count(|ev| matches!(ev, Ev::ToolResult { .. })) > 0
                        || Instant::now() > deadline
                    {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                let sent_at = trace.elapsed();
                let outcome = handle.interrupt_and_reprompt(INTERRUPT_MESSAGE, &[]).await;
                let answered_at = trace.elapsed();
                let pids_during_reprompt = bridge_pids();
                eprintln!(
                    "[interrupt-perm] interrupt sent at {:.2}s with a permission request pending, answered {outcome:?} at {:.2}s — cancel acked after {:.3}s (child: {pids_during_reprompt:?})",
                    sent_at.as_secs_f64(),
                    answered_at.as_secs_f64(),
                    (answered_at - sent_at).as_secs_f64()
                );
                Some((sent_at, answered_at, outcome, pids_during_reprompt))
            });

            let turn = tokio::time::timeout(TURN_TIMEOUT, session.connection.prompt(COUNT_PROMPT, &[]))
                .await
                .expect("the turn should end");
            eprintln!(
                "[interrupt-perm] prompt() resolved {turn:?} at {:.2}s",
                session.trace.elapsed().as_secs_f64()
            );
            let Some((sent_at, answered_at, outcome, pids_during_reprompt)) =
                interrupt.await.expect("interrupt task")
            else {
                eprintln!(
                    "SCENARIO 5: SKIPPED — the bridge issued no session/request_permission for its shell calls"
                );
                session.finish().await;
                return;
            };

            let events = session.trace.events();
            let user_at = events
                .iter()
                .find(|(_, ev)| matches!(ev, Ev::User(_)))
                .map(|(at, _)| *at);
            let cancelled_at = events
                .iter()
                .find(|(_, ev)| matches!(ev, Ev::PermissionCancelled { .. }))
                .map(|(at, _)| *at);
            let hold_ended_otherwise = events.iter().any(|(_, ev)| {
                matches!(ev, Ev::Event(kind) if kind.starts_with("permission-hold-"))
            });
            eprintln!(
                "SCENARIO 5: cancel acked after {:.3}s; held permission cancelled at {:?} (hold ended another way: {hold_ended_otherwise}); user row at {:?}",
                (answered_at - sent_at).as_secs_f64(),
                cancelled_at.map(|at| at.as_secs_f64()),
                user_at.map(|at| at.as_secs_f64())
            );
            assert_eq!(
                outcome,
                Ok(()),
                "the interrupt must be acknowledged while a permission request is pending"
            );
            assert_eq!(turn, Ok(AgentRunOutcome::Completed));
            assert_eq!(pids_during_reprompt, pids_before, "same child");
            let rows = session.trace.rows();
            assert_eq!(user_rows(&session.trace), 1, "exactly one user row: {rows:#?}");
            let user_at = user_at.unwrap();
            // The held request was resolved by the interrupt, before the
            // next turn, not left for someone to answer later.
            assert!(
                cancelled_at.is_some_and(|at| at <= user_at),
                "the pending permission request was not resolved by the interrupt: {events:#?}"
            );
            let after = session.trace.assistant_text_after_user();
            assert!(after.contains("INTERRUPTED-DONE"), "text after the user row: {after:?}");
            // Turn 2's own request (the `pwd`) reached the handler and was
            // answered: the turn could not have completed otherwise.
            let permissions_after = events
                .iter()
                .filter(|(at, ev)| *at > user_at && matches!(ev, Ev::Permission { .. }))
                .count();
            eprintln!("[interrupt-perm] permission requests in the second turn: {permissions_after}");
            assert!(permissions_after > 0, "turn 2's permission request never reached the handler");
            session.finish().await;
        })
        .await;
}

// -----------------------------------------------------------------------------
// Scenario 6: interrupt racing the end of the turn
// -----------------------------------------------------------------------------

/// Send an interrupt `when` the short turn is at (or past) its end. Whatever
/// the timing, nothing hangs and the message lands at most once: as an
/// interrupt of the live turn, as a new turn on the held connection, or —
/// once a hold-less connection has torn down — as a fast failure the caller
/// routes to a resume.
async fn race_interrupt(label: &str, after_first_chunk: bool) {
    let mut session = Session::connect(label, bridge_advertises_steering()).await;
    let trace = Arc::clone(&session.trace);
    let handle = session.handle.clone();
    let label_owned = label.to_string();
    let interrupt = tokio::task::spawn_local(async move {
        if after_first_chunk {
            trace
                .wait_for("the first chunk", TURN_START_TIMEOUT, |evs| {
                    evs.iter().any(|ev| matches!(ev, Ev::Append(_)))
                })
                .await;
        }
        let sent_at = trace.elapsed();
        let outcome = tokio::time::timeout(
            REPLY_TIMEOUT,
            handle.interrupt_and_reprompt(RACE_MESSAGE, &[]),
        )
        .await
        .expect("an interrupt at the turn boundary must not hang");
        eprintln!(
            "[{label_owned}] interrupt sent at {:.2}s answered {outcome:?} at {:.2}s",
            sent_at.as_secs_f64(),
            trace.elapsed().as_secs_f64()
        );
        outcome
    });

    let turn = tokio::time::timeout(TURN_TIMEOUT, session.connection.prompt(RACE_PROMPT, &[]))
        .await
        .expect("the short turn should end");
    eprintln!(
        "[{label}] prompt() resolved {turn:?} at {:.2}s",
        session.trace.elapsed().as_secs_f64()
    );
    let outcome = interrupt.await.expect("interrupt task");
    assert_eq!(turn, Ok(AgentRunOutcome::Completed));
    match &outcome {
        Ok(()) => {
            session
                .trace
                .wait_for("the reply to the interrupt", REPLY_TIMEOUT, |evs| {
                    streamed_text(evs).contains("RACE-DONE")
                })
                .await;
            // Let any trailing frames land before counting rows.
            tokio::time::sleep(Duration::from_secs(3)).await;
            let rows = session.trace.rows();
            eprintln!(
                "SCENARIO 6 ({label}): delivered; user rows {}",
                user_rows(&session.trace)
            );
            assert_eq!(user_rows(&session.trace), 1, "{rows:#?}");
            assert!(
                echoed_rows(&rows).is_empty(),
                "pre-interrupt assistant row echoed after the user row: {rows:#?}"
            );
        }
        Err(e) if e.contains("no longer running") => {
            eprintln!(
                "SCENARIO 6 ({label}): the turn ended and the connection tore down before the \
                 interrupt arrived ({e}); the caller routes that as a resume"
            );
            assert_eq!(user_rows(&session.trace), 0);
        }
        other => panic!("unexpected interrupt outcome at the turn boundary: {other:?}"),
    }
    session.finish().await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "spawns the installed ACP bridge and calls the model"]
async fn real_bridge_interrupt_sent_with_the_prompt_lands_once() {
    tokio::task::LocalSet::new()
        .run_until(race_interrupt("irace-at-send", false))
        .await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "spawns the installed ACP bridge and calls the model"]
async fn real_bridge_interrupt_sent_at_the_first_chunk_lands_once() {
    tokio::task::LocalSet::new()
        .run_until(race_interrupt("irace-at-chunk", true))
        .await;
}

// -----------------------------------------------------------------------------
// Scenario 7: resume a settled session with session/load
// -----------------------------------------------------------------------------

/// One connection teaches the agent a code word and settles; a second,
/// on a new child, loads the agent's session in the same dir and asks for
/// it back. The replayed history must be matched or dropped, never recorded
/// as fresh rows.
#[tokio::test(flavor = "current_thread")]
#[ignore = "spawns the installed ACP bridge and calls the model"]
async fn real_bridge_resume_loads_the_session_on_a_new_child() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut first = Session::connect("resume-1", bridge_advertises_steering()).await;
            let pids_first = bridge_pids();
            let turn = tokio::time::timeout(TURN_TIMEOUT, first.connection.prompt(REMEMBER_PROMPT, &[]))
                .await
                .expect("the first turn should end");
            assert_eq!(turn, Ok(AgentRunOutcome::Completed));
            let first_text = streamed_text(
                &first.trace.events().into_iter().map(|(_, ev)| ev).collect::<Vec<_>>(),
            );
            assert!(first_text.contains("REMEMBERED"), "first turn: {first_text:?}");
            let agent_session_id = first.agent_session_id();
            let messages = first.legacy_messages(REMEMBER_PROMPT);
            let dir = first.dir.clone();
            first.finish_keeping_dir().await;
            wait_for_bridge_exit(SETTLE_WAIT).await;

            let mut second = Session::connect_with(
                "resume-2",
                ConnectOptions {
                    steering_disabled: bridge_advertises_steering(),
                    resume: Some(Resume {
                        agent_session_id: agent_session_id.clone(),
                        messages,
                        dir: dir.clone(),
                    }),
                    ..ConnectOptions::default()
                },
            )
            .await;
            let pids_second = bridge_pids();
            assert_eq!(pids_second.len(), 1, "one new child: {pids_second:?}");
            assert_ne!(pids_second, pids_first, "the resume runs on a new child");
            let turn = tokio::time::timeout(TURN_TIMEOUT, second.connection.prompt(RECALL_PROMPT, &[]))
                .await
                .expect("the resumed turn should end");
            eprintln!(
                "[resume-2] prompt() resolved {turn:?} at {:.2}s",
                second.trace.elapsed().as_secs_f64()
            );
            assert_eq!(turn, Ok(AgentRunOutcome::Completed));
            let rows = second.trace.rows();
            let text = streamed_text(
                &second.trace.events().into_iter().map(|(_, ev)| ev).collect::<Vec<_>>(),
            );
            eprintln!(
                "SCENARIO 7: loaded {agent_session_id} on {pids_second:?}; store ids {:?}; reply {text:?}",
                second.store.agent_session_ids.lock().unwrap()
            );
            assert_eq!(user_rows(&second.trace), 0, "{rows:#?}");
            assert!(
                !rows.iter().any(|row| row.text().contains("REMEMBERED")),
                "replayed history recorded as new rows: {rows:#?}"
            );
            assert!(
                text.contains("PELICAN-7342"),
                "the loaded session should remember the code word: {text:?}"
            );
            second.finish().await;
            let _ = std::fs::remove_dir_all(&dir);
        })
        .await;
}
