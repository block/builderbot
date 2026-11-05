//! Remote agent processes shared between sessions.
//!
//! `sq blox acp <workspace> --command=<agent>` attaches to any running agent
//! process on the workstation whose command matches, so concurrent sessions
//! on one workspace land in one agent process. When an `sq` invocation exits
//! it stops that process only if it was the one that started it. Giving each
//! session its own `sq` therefore tied the shared agent's lifetime to
//! whichever session happened to launch it: that session finishing stopped
//! the agent under every other session attached to it.
//!
//! Instead, sessions on the same workspace and agent command lease one shared
//! `sq` proxy ([`attach`]). The first lease starts it, and only releasing the
//! last one interrupts it, so the process lives exactly as long as some
//! session is attached — whichever session or app instance first launched
//! the remote process. One limit remains: `sq` cannot stop a process it did
//! not start, so when the proxy attached to one an earlier app instance left
//! running, interrupting it leaves that process up for the next session to
//! reuse.
//!
//! The proxy also fails every attached session when its agent is lost: when
//! its output ends while sessions are attached, or when bloxlet reports the
//! agent process stopped (see [`super::remote_liveness`]). Each session then
//! ends with a [`super::REMOTE_AGENT_LOST_PREFIX`] error naming the cause.
//! When only the proxy went — `sq` exited while bloxlet still runs a process
//! it had reused — the sessions end with a
//! [`super::REMOTE_AGENT_DETACHED_PREFIX`] error instead, so their turns can
//! be reattached to.
//!
//! A session a previous app instance left running reattaches through the same
//! proxy, so `sq` attaches to the agent process still serving its turn; a
//! [`ReattachedTurn`] then says whether that process survived and when the
//! turn has ended.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, Command};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::remote_liveness::{self, Liveness};
use super::remote_routing::{RemoteLoss, RemoteTransport, RoutedClientStreams};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// What `sq blox acp` matches a reusable agent process on: its arguments name
/// the workspace and the agent command.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProxyKey {
    program: OsString,
    args: Vec<OsString>,
}

enum Entry {
    /// Running, with this many sessions attached.
    Live {
        process: Arc<SharedProcess>,
        leases: usize,
    },
    /// Being stopped. A replacement waits for it: started any earlier, its
    /// `sq` could attach to the agent process this one is about to stop.
    Stopping {
        generation: u64,
        stopped: CancellationToken,
    },
}

fn registry() -> MutexGuard<'static, HashMap<ProxyKey, Entry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<ProxyKey, Entry>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default).lock().unwrap()
}

/// Whether `sq` started the agent process or attached to a running one, as
/// its log reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Attachment {
    #[default]
    Unknown,
    Started,
    Reused,
}

/// What the proxy's log has said about its agent process.
#[derive(Default)]
struct ProxyReport {
    attachment: Attachment,
    /// bloxlet's id for the agent process's command.
    command_id: Option<String>,
    /// When the proxy last lost its output stream and began retrying.
    last_disconnect: Option<Instant>,
}

/// One running `sq` proxy and its routed stdio. The child itself is owned by
/// the proxy's thread (see [`run_proxy`]).
struct SharedProcess {
    generation: u64,
    label: String,
    pid: Option<u32>,
    transport: RemoteTransport,
    report: Mutex<ProxyReport>,
    /// Woken when the proxy's log says bloxlet should be asked about the
    /// agent right away.
    check_now: Notify,
    /// Cancelled to interrupt the proxy.
    stop: CancellationToken,
    /// Cancelled once the proxy is observed to be gone.
    exited: CancellationToken,
    /// Cancelled once the proxy has been stopped and unregistered.
    stopped: CancellationToken,
}

impl SharedProcess {
    fn describe(&self) -> String {
        match self.pid {
            Some(pid) => format!("[{}] remote agent proxy (pid {pid})", self.label),
            None => format!("[{}] remote agent proxy", self.label),
        }
    }

    /// Mark the proxy as stopping unless a lease release already did,
    /// returning how many sessions were still attached.
    fn retire(&self, key: &ProxyKey) -> usize {
        let mut registry = registry();
        let leases = match registry.get(key) {
            Some(Entry::Live { process, leases }) if process.generation == self.generation => {
                *leases
            }
            _ => return 0,
        };
        registry.insert(
            key.clone(),
            Entry::Stopping {
                generation: self.generation,
                stopped: self.stopped.clone(),
            },
        );
        leases
    }

    fn unregister(&self, key: &ProxyKey) {
        let mut registry = registry();
        if matches!(
            registry.get(key),
            Some(Entry::Stopping { generation, .. }) if *generation == self.generation
        ) {
            registry.remove(key);
        }
    }
}

/// One session's hold on a shared remote agent proxy. Releasing (or dropping)
/// it detaches the session; the last one to go stops the proxy.
pub(super) struct RemoteProcessLease {
    key: ProxyKey,
    process: Arc<SharedProcess>,
    /// This lease started the proxy, rather than joining one already
    /// running, so what `sq` said about its process is about this session.
    created_proxy: bool,
    released: bool,
}

impl RemoteProcessLease {
    /// Cancelled once the proxy is observed to be gone, whichever session's
    /// release (or failure) ended it.
    pub(super) fn exited(&self) -> &CancellationToken {
        &self.process.exited
    }

    /// How the agent was lost under the attached sessions, if it was.
    pub(super) fn loss(&self) -> Option<RemoteLoss> {
        self.process.transport.loss()
    }

    /// For a session reattaching to a turn prompted at `prompted_at` (ms
    /// since the epoch) by an app instance that is gone: a watch on that
    /// turn, when bloxlet can be asked about its agent.
    pub(super) fn reattached_turn(&self, prompted_at: i64) -> Option<ReattachedTurn> {
        Some(ReattachedTurn {
            process: Arc::clone(&self.process),
            target: remote_liveness::Target::of(&self.key.program, &self.key.args)?,
            trust_attachment: self.created_proxy,
            prompted_at_secs: prompted_at.div_euclid(1000),
        })
    }

    /// Detach this session. When it was the last one attached, wait for the
    /// proxy to stop, like stopping a session's own agent process would.
    pub(super) async fn release(mut self) {
        if let Some(stopped) = self.detach() {
            stopped.cancelled().await;
        }
    }

    /// Give up this lease, returning what to wait on when the proxy is (or
    /// already was) stopping.
    fn detach(&mut self) -> Option<CancellationToken> {
        if std::mem::replace(&mut self.released, true) {
            return None;
        }
        let process = &self.process;
        let remaining = {
            let mut registry = registry();
            let remaining = match registry.get_mut(&self.key) {
                Some(Entry::Live {
                    process: live,
                    leases,
                }) if live.generation == process.generation => {
                    *leases -= 1;
                    *leases
                }
                // It ended on its own and is already being stopped.
                _ => return Some(process.stopped.clone()),
            };
            if remaining == 0 {
                registry.insert(
                    self.key.clone(),
                    Entry::Stopping {
                        generation: process.generation,
                        stopped: process.stopped.clone(),
                    },
                );
            }
            remaining
        };
        if remaining > 0 {
            log::info!(
                "{}: a session detached; {remaining} still attached, so it keeps running",
                process.describe()
            );
            return None;
        }
        if process.report.lock().unwrap().attachment == Attachment::Reused {
            log::info!(
                "{}: the last session detached; stopping it. It attached to an agent process \
                 that was already running on the workstation, which `sq` leaves running",
                process.describe()
            );
        } else {
            log::info!(
                "{}: the last session detached; stopping it",
                process.describe()
            );
        }
        process.stop.cancel();
        Some(process.stopped.clone())
    }
}

impl Drop for RemoteProcessLease {
    fn drop(&mut self) {
        self.detach();
    }
}

/// A turn a previous app instance started on a remote agent, which a session
/// has reattached to through the proxy.
pub(super) struct ReattachedTurn {
    process: Arc<SharedProcess>,
    target: remote_liveness::Target,
    /// Whether the proxy was started for this session, so that `sq` starting
    /// or reusing a process answers whether the turn's process survived.
    trust_attachment: bool,
    /// When the turn was prompted, in seconds since the epoch.
    prompted_at_secs: i64,
}

impl ReattachedTurn {
    /// Wait for the turn to end, which is when bloxlet reports its agent
    /// process idle. bloxlet tracks prompts per process, so this also waits
    /// out prompts other sessions on it have in flight.
    ///
    /// Fails, naming the cause, when the process that ran the turn is gone:
    /// `sq` found none to attach to and started a new one, or bloxlet reports
    /// it stopped. Also fails when the proxy loses its connection to it.
    pub(super) async fn ended(self) -> Result<(), String> {
        let command_id = self.surviving_command().await?;
        log::info!(
            "{}: reattached to agent process (command {command_id}); waiting for its turn to end",
            self.process.describe()
        );
        let closed = self.process.transport.closed().clone();
        let poll = async {
            loop {
                match remote_liveness::check(&self.target, &command_id).await {
                    Ok(Liveness::Idle) => return Ok(()),
                    Ok(Liveness::Running) => {}
                    Ok(Liveness::Gone(cause)) => return Err(cause),
                    Err(error) => log::warn!(
                        "{}: could not check command {command_id}: {error}",
                        self.process.describe()
                    ),
                }
                tokio::time::sleep(remote_liveness::REATTACHED_TURN_POLL).await;
            }
        };
        tokio::select! {
            ended = poll => ended,
            // Nothing streams the rest of the turn to the session any more.
            _ = closed.cancelled() => Err(self
                .process
                .transport
                .loss()
                .map(|loss| loss.cause)
                .unwrap_or_else(|| "the remote agent proxy stopped".to_string())),
        }
    }

    /// bloxlet's id for the agent process `sq` attached to, provided it was
    /// already running when the turn was prompted.
    async fn surviving_command(&self) -> Result<String, String> {
        // `sq` logs which process it attached to before it relays any frame,
        // so by the time the session is set up it has normally been read.
        let deadline = Instant::now() + Duration::from_secs(10);
        let command_id = loop {
            {
                let report = self.process.report.lock().unwrap();
                match (report.attachment, &report.command_id) {
                    (Attachment::Reused, Some(command_id)) if self.trust_attachment => {
                        return Ok(command_id.clone())
                    }
                    (Attachment::Started, _) if self.trust_attachment => {
                        return Err("the agent process that ran it is gone: `sq blox acp` \
                                    found none to reattach to and started a new one"
                            .to_string())
                    }
                    (Attachment::Started | Attachment::Reused, Some(command_id)) => {
                        break command_id.clone()
                    }
                    _ => {}
                }
            }
            if Instant::now() >= deadline {
                return Err(
                    "`sq blox acp` never said which agent process it attached to, \
                            so whether the one that ran it survived is unknown"
                        .to_string(),
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        // The session joined a proxy that was already running, so whether
        // `sq` started or reused its process says nothing about this turn
        // (another app instance may have run it on a process that has since
        // died). The process serving the workspace now ran the turn only if
        // it was already running when the turn was prompted.
        match remote_liveness::command_started_at(&self.target, &command_id).await {
            Ok(Some(started_at)) if started_at <= self.prompted_at_secs => Ok(command_id),
            Ok(Some(_)) => Err(format!(
                "the agent process that ran it is gone: the one serving the workspace now \
                 (command {command_id}) started after the turn was prompted"
            )),
            Ok(None) => Err(format!(
                "bloxlet does not say when agent process {command_id} started, so whether \
                 it is the one that ran the turn is unknown"
            )),
            Err(error) => Err(format!(
                "whether agent process {command_id} is the one that ran the turn is \
                 unknown: {error}"
            )),
        }
    }
}

/// Attach a session to the remote agent proxy `cmd` runs, starting it unless
/// another session already holds it. `cmd` must pipe all three stdio streams.
pub(super) async fn attach(
    cmd: Command,
    label: &str,
) -> io::Result<(RemoteProcessLease, RoutedClientStreams)> {
    let key = ProxyKey {
        program: cmd.as_std().get_program().to_owned(),
        args: cmd.as_std().get_args().map(OsStr::to_owned).collect(),
    };
    let mut cmd = Some(cmd);
    loop {
        let stopping = {
            let mut registry = registry();
            match registry.get_mut(&key) {
                // Lost its agent and about to stop: start another once it has.
                Some(Entry::Live { process, .. })
                    if process.transport.closed().is_cancelled()
                        || process.transport.output_ended().is_cancelled() =>
                {
                    process.stopped.clone()
                }
                Some(Entry::Live { process, leases }) => {
                    *leases += 1;
                    log::info!(
                        "{}: attaching a session alongside {} already attached",
                        process.describe(),
                        *leases - 1
                    );
                    return Ok(lease(key, Arc::clone(process), false));
                }
                Some(Entry::Stopping { stopped, .. }) => stopped.clone(),
                None => {
                    let cmd = cmd
                        .take()
                        .expect("a proxy is spawned at most once per attach");
                    let process = spawn_proxy(key.clone(), cmd, label)?;
                    registry.insert(
                        key.clone(),
                        Entry::Live {
                            process: Arc::clone(&process),
                            leases: 1,
                        },
                    );
                    return Ok(lease(key, process, true));
                }
            }
        };
        log::info!("[{label}] Waiting for the previous remote agent proxy to stop");
        stopping.cancelled().await;
    }
}

fn lease(
    key: ProxyKey,
    process: Arc<SharedProcess>,
    created_proxy: bool,
) -> (RemoteProcessLease, RoutedClientStreams) {
    let streams = process.transport.attach();
    let lease = RemoteProcessLease {
        key,
        process,
        created_proxy,
        released: false,
    };
    (lease, streams)
}

/// Start the proxy on its own thread and runtime: it outlives the session
/// that starts it, and a child's pipes are bound to the runtime that spawned
/// it.
fn spawn_proxy(key: ProxyKey, cmd: Command, label: &str) -> io::Result<Arc<SharedProcess>> {
    let (ready_tx, ready_rx) = std_mpsc::sync_channel(1);
    let label = label.to_string();
    std::thread::Builder::new()
        .name("remote-acp-proxy".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            runtime.block_on(run_proxy(key, cmd, label, ready_tx));
        })?;
    ready_rx
        .recv()
        .unwrap_or_else(|_| Err(io::Error::other("the remote agent proxy thread exited")))
}

async fn run_proxy(
    key: ProxyKey,
    mut cmd: Command,
    label: String,
    ready: std_mpsc::SyncSender<io::Result<Arc<SharedProcess>>>,
) {
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill().await;
        let _ = ready.send(Err(io::Error::other(
            "the remote agent proxy's stdio is not piped",
        )));
        return;
    };
    let process = Arc::new(SharedProcess {
        generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
        label,
        pid: child.id(),
        transport: RemoteTransport::spawn(stdin, stdout),
        report: Mutex::new(ProxyReport::default()),
        check_now: Notify::new(),
        stop: CancellationToken::new(),
        exited: CancellationToken::new(),
        stopped: CancellationToken::new(),
    });
    tokio::spawn(drain_stderr(stderr, Arc::clone(&process)));
    if ready.send(Ok(Arc::clone(&process))).is_err() {
        // Nobody holds a lease, so nobody would ever stop it.
        process.stop.cancel();
    }
    let target = remote_liveness::Target::of(&key.program, &key.args).map(Arc::new);
    let watch = target
        .clone()
        .map(|target| tokio::spawn(watch_liveness(Arc::clone(&process), target)));

    tokio::select! {
        _ = process.stop.cancelled() => {}
        _ = process.exited.cancelled() => {}
        _ = process.transport.output_ended().cancelled() => {}
        _ = process.transport.closed().cancelled() => {}
    }
    if let Some(watch) = watch {
        watch.abort();
    }
    let attached = process.retire(&key);
    if process.stop.is_cancelled() {
        // Stopping on purpose: its output ending is no loss to report.
        process.transport.close();
    } else if !process.transport.closed().is_cancelled() {
        process.explain_exit(attached, target.as_deref()).await;
    }
    if let Some(loss) = process.transport.loss().filter(|_| attached > 0) {
        log::warn!(
            "{}: {} with {attached} session(s) attached: {}",
            process.describe(),
            if loss.agent_may_survive {
                "lost its connection to the agent"
            } else {
                "lost its agent"
            },
            loss.cause
        );
    }
    super::graceful_stop(&mut child, true).await;
    process.transport.close();
    process.exited.cancel();
    process.unregister(&key);
    process.stopped.cancel();
}

impl SharedProcess {
    /// The proxy's output ended (or `sq` exited) without being asked to
    /// stop: end the attached sessions, telling them whether their agent
    /// went with it.
    ///
    /// `sq` stops a process it started when it exits, so that one is gone.
    /// A process it only reused outlives it — a local `sq` crash, an expired
    /// login or a stray kill takes nothing on the workstation down — so
    /// bloxlet is asked first, and while it still runs the process (or
    /// cannot say) the sessions are detached rather than failed, for their
    /// turns to be reattached to.
    async fn explain_exit(&self, attached: usize, target: Option<&remote_liveness::Target>) {
        let cause = if self.exited.is_cancelled() {
            "the `sq blox acp` proxy exited".to_string()
        } else {
            self.transport
                .output_end()
                .unwrap_or_else(|| "the remote agent's output stream ended".to_string())
        };
        let (attachment, command_id) = {
            let report = self.report.lock().unwrap();
            (report.attachment, report.command_id.clone())
        };
        let (true, Attachment::Reused, Some(command_id), Some(target)) =
            (attached > 0, attachment, command_id, target)
        else {
            self.transport.lose(cause);
            return;
        };
        log::info!(
            "{}: {cause}; asking bloxlet whether the agent process it reused \
             (command {command_id}) still runs",
            self.describe()
        );
        let verdict = tokio::time::timeout(
            remote_liveness::QUICK_CHECK_TIMEOUT,
            remote_liveness::check(target, &command_id),
        )
        .await
        .unwrap_or_else(|_| Err("`sq blox ws commands` timed out".to_string()));
        match verdict {
            Ok(Liveness::Running | Liveness::Idle) => self.transport.detach_sessions(format!(
                "{cause}, but bloxlet still runs the agent process (command {command_id})"
            )),
            Ok(Liveness::Gone(gone)) => self.transport.lose(gone),
            Err(error) => self.transport.detach_sessions(format!(
                "{cause}, and whether the agent process (command {command_id}) still runs \
                 is unknown: {error}"
            )),
        }
    }
}

/// While sessions are attached, ask bloxlet whether the agent process still
/// runs whenever its stream recently disconnected or ended, or a session's
/// prompt has gone quiet, and fail the sessions once it does not.
async fn watch_liveness(process: Arc<SharedProcess>, target: Arc<remote_liveness::Target>) {
    let mut last_check: Option<Instant> = None;
    let mut reported_no_command_id = false;
    let mut tick = tokio::time::interval(remote_liveness::WATCH_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let check_now = tokio::select! {
            _ = tick.tick() => false,
            _ = process.check_now.notified() => true,
        };
        let since_check = last_check.map(|at| at.elapsed());
        let last_disconnect = process.report.lock().unwrap().last_disconnect;
        let reason = if check_now {
            "`sq` reports the orchestrator ended the agent's output stream"
        } else if last_disconnect.is_some_and(|at| at.elapsed() < remote_liveness::DISCONNECT_WATCH)
            && since_check
                .is_none_or(|elapsed| elapsed >= remote_liveness::CHECK_AFTER_DISCONNECT_EVERY)
        {
            "its output stream disconnected"
        } else if since_check.is_none_or(|elapsed| elapsed >= remote_liveness::STALLED_PROMPT_AFTER)
            && process
                .transport
                .has_stalled_prompt(remote_liveness::STALLED_PROMPT_AFTER)
        {
            "a session's prompt has gone quiet"
        } else {
            continue;
        };
        let Some(command_id) = process.report.lock().unwrap().command_id.clone() else {
            if !std::mem::replace(&mut reported_no_command_id, true) {
                log::warn!(
                    "{}: {reason}, but `sq` never named the agent's command, so bloxlet \
                     cannot be asked about it",
                    process.describe()
                );
            }
            continue;
        };
        last_check = Some(Instant::now());
        log::info!(
            "{}: {reason}; asking bloxlet whether command {command_id} still runs",
            process.describe()
        );
        match remote_liveness::check(&target, &command_id).await {
            Ok(Liveness::Running | Liveness::Idle) => {
                log::info!("{}: command {command_id} still runs", process.describe());
            }
            Ok(Liveness::Gone(cause)) => {
                log::warn!("{}: {cause}", process.describe());
                process.transport.lose(cause);
                return;
            }
            Err(error) => {
                log::warn!(
                    "{}: could not check command {command_id}: {error}",
                    process.describe()
                );
            }
        }
    }
}

/// Log the proxy's stderr (spawn and auth failures surface only here) and
/// note what it says about the agent process.
async fn drain_stderr(stderr: ChildStderr, process: Arc<SharedProcess>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let check_now = process
            .report
            .lock()
            .unwrap()
            .note(&blox_cli::strip_ansi_escape_sequences(&line));
        if check_now {
            process.check_now.notify_one();
        }
        match process.pid {
            Some(pid) => log::warn!("[{} stderr][proxy pid {pid}] {line}", process.label),
            None => log::warn!("[{} stderr] {line}", process.label),
        }
    }
    // stderr EOF (or a read error) means the proxy has exited or torn down
    // its stderr; either way its agent can no longer be trusted to report.
    process.exited.cancel();
}

impl ProxyReport {
    /// Take in one line of the proxy's log, returning whether bloxlet should
    /// be asked about the agent right away.
    fn note(&mut self, line: &str) -> bool {
        if let Some(attachment) = attachment_reported_by(line) {
            self.attachment = attachment;
            if let Some(command_id) =
                logged_value(line, "process_id").or_else(|| logged_value(line, "command_id"))
            {
                self.command_id = Some(command_id.to_string());
            }
        }
        if line.contains("Stream disconnected, retrying")
            || line.contains("failed to start receive stream, retrying")
        {
            self.last_disconnect = Some(Instant::now());
        }
        // The orchestrator ends the output stream when the agent process
        // has exited. `sq` then only logs this and keeps its stdout open
        // until its stdin closes, so nothing else would tell the sessions.
        if line.contains("Received EOF signal from orchestrator") {
            self.last_disconnect = Some(Instant::now());
            return true;
        }
        false
    }
}

/// The value of `key=value` in a log line.
fn logged_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let start = line
        .match_indices(&format!("{key}="))
        .find(|(at, _)| *at == 0 || line.as_bytes()[at - 1].is_ascii_whitespace())?
        .0
        + key.len()
        + 1;
    let value = line[start..].split_whitespace().next()?.trim_matches('"');
    (!value.is_empty()).then_some(value)
}

fn attachment_reported_by(line: &str) -> Option<Attachment> {
    if line.contains("Started new ACP process") {
        Some(Attachment::Started)
    } else if line.contains("Found existing ACP process, reusing")
        || line.contains("ACP process by command ID")
    {
        Some(Attachment::Reused)
    } else {
        None
    }
}
