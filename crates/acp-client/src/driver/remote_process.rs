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

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, Command};
use tokio_util::sync::CancellationToken;

use super::remote_liveness::{self, Liveness};
use super::remote_routing::{RemoteTransport, RoutedClientStreams};

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
    released: bool,
}

impl RemoteProcessLease {
    /// Cancelled once the proxy is observed to be gone, whichever session's
    /// release (or failure) ended it.
    pub(super) fn exited(&self) -> &CancellationToken {
        &self.process.exited
    }

    /// Why the agent was lost under the attached sessions, if it was.
    pub(super) fn lost_cause(&self) -> Option<String> {
        self.process.transport.lost_cause()
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
                Some(Entry::Live { process, .. }) if process.transport.closed().is_cancelled() => {
                    process.stopped.clone()
                }
                Some(Entry::Live { process, leases }) => {
                    *leases += 1;
                    log::info!(
                        "{}: attaching a session alongside {} already attached",
                        process.describe(),
                        *leases - 1
                    );
                    return Ok(lease(key, Arc::clone(process)));
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
                    return Ok(lease(key, process));
                }
            }
        };
        log::info!("[{label}] Waiting for the previous remote agent proxy to stop");
        stopping.cancelled().await;
    }
}

fn lease(key: ProxyKey, process: Arc<SharedProcess>) -> (RemoteProcessLease, RoutedClientStreams) {
    let streams = process.transport.attach();
    let lease = RemoteProcessLease {
        key,
        process,
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
        stop: CancellationToken::new(),
        exited: CancellationToken::new(),
        stopped: CancellationToken::new(),
    });
    tokio::spawn(drain_stderr(stderr, Arc::clone(&process)));
    if ready.send(Ok(Arc::clone(&process))).is_err() {
        // Nobody holds a lease, so nobody would ever stop it.
        process.stop.cancel();
    }
    let watch = remote_liveness::Target::of(&key.program, &key.args)
        .map(|target| tokio::spawn(watch_liveness(Arc::clone(&process), target)));

    tokio::select! {
        _ = process.stop.cancelled() => {}
        _ = process.exited.cancelled() => {}
        _ = process.transport.closed().cancelled() => {}
    }
    if let Some(watch) = watch {
        watch.abort();
    }
    let attached = process.retire(&key);
    if process.stop.is_cancelled() {
        // Stopping on purpose: its output ending is no loss to report.
        process.transport.close();
    } else {
        process
            .transport
            .lose("the `sq blox acp` proxy exited".to_string());
    }
    if attached > 0 {
        log::warn!(
            "{}: lost its agent with {attached} session(s) attached: {}",
            process.describe(),
            process.transport.lost_cause().unwrap_or_default()
        );
    }
    super::graceful_stop(&mut child, true).await;
    process.transport.close();
    process.exited.cancel();
    process.unregister(&key);
    process.stopped.cancel();
}

/// While sessions are attached, ask bloxlet whether the agent process still
/// runs whenever its stream recently disconnected or a session's tool call
/// has gone quiet, and fail the sessions once it does not.
async fn watch_liveness(process: Arc<SharedProcess>, target: remote_liveness::Target) {
    let mut last_check: Option<Instant> = None;
    let mut reported_no_command_id = false;
    let mut tick = tokio::time::interval(remote_liveness::WATCH_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let since_check = last_check.map(|at| at.elapsed());
        let last_disconnect = process.report.lock().unwrap().last_disconnect;
        let reason = if last_disconnect
            .is_some_and(|at| at.elapsed() < remote_liveness::DISCONNECT_WATCH)
            && since_check
                .is_none_or(|elapsed| elapsed >= remote_liveness::CHECK_AFTER_DISCONNECT_EVERY)
        {
            "its output stream disconnected"
        } else if since_check
            .is_none_or(|elapsed| elapsed >= remote_liveness::STALLED_TOOL_CALL_AFTER)
            && process
                .transport
                .has_stalled_tool_call(remote_liveness::STALLED_TOOL_CALL_AFTER)
        {
            "a session's tool call has gone quiet"
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
            Ok(Liveness::Running) => {
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
        process
            .report
            .lock()
            .unwrap()
            .note(&blox_cli::strip_ansi_escape_sequences(&line));
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
    fn note(&mut self, line: &str) {
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
