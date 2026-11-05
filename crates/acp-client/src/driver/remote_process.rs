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
//! bloxlet reports the agent process stopped, whether the proxy's output
//! ended or not (see [`super::remote_liveness`]), or when the output of a
//! proxy that is not `sq blox acp`, so has no bloxlet to ask, ends. Each
//! session then ends with a [`super::REMOTE_AGENT_LOST_PREFIX`] error naming
//! the cause. When only the proxy went — `sq` exited, or its output ended,
//! while bloxlet still runs the agent process, whether `sq` started or
//! reused it, or bloxlet can't be asked — the sessions end with a
//! [`super::REMOTE_AGENT_DETACHED_PREFIX`] error instead, so their turns can
//! be reattached to (see [`SharedProcess::explain_exit`]). Once `sq` says the
//! orchestrator ended the agent's output stream, nothing more comes through
//! the proxy, so it takes no new sessions and is made to exit (see
//! [`SharedProcess::end_stream`]).
//!
//! A session a previous app instance left running reattaches through the same
//! proxy, so `sq` attaches to the agent process still serving its turn; a
//! [`ReattachedTurn`] then says whether that process survived and when the
//! turn has ended.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, Command};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::remote_liveness::{self, CommandStart, Liveness};
use super::remote_routing::{
    RemoteClientId, RemoteLoss, RemoteTransport, RoutedClientStreams, CANCELLED_STOP_REASON,
};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// How long `sq` is given to exit once its stdin is closed after the
/// orchestrator ended the agent's output stream, before it is interrupted.
#[cfg(not(test))]
const STREAM_END_EXIT_GRACE: Duration = Duration::from_secs(10);
#[cfg(test)]
const STREAM_END_EXIT_GRACE: Duration = Duration::from_millis(300);

/// How far bloxlet's clock may run ahead of this app's when the start of an
/// agent process is compared with when a connection to it was up.
const PROCESS_START_CLOCK_SLACK_SECS: i64 = 30;

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
    /// Cancelled once `sq` reports the orchestrator ended the agent's output
    /// stream (see [`Self::end_stream`]).
    stream_ended: CancellationToken,
    /// Per app instance, held while one of its quiet reattached turns waits
    /// for the answer to its cancel (see [`ReattachedTurn::cancel_quiet_turn`]).
    cancel_windows: Mutex<HashMap<u32, Arc<tokio::sync::Mutex<()>>>>,
    /// Set once a session let go of the proxy leaving its turn running on
    /// the agent (see [`LeaseDetacher`]), or ended in a way that must not
    /// interrupt it (see [`RemoteProcessLease::never_interrupt`]): from then
    /// on the proxy is never sent SIGINT, which would stop a process `sq`
    /// started — and every turn on it — but only SIGKILLed.
    keep_agent: AtomicBool,
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

    /// `sq` reports the orchestrator ended the agent's output stream, which
    /// it does once the agent process's output ends. `sq` relays nothing
    /// after that, yet keeps its stdout open until its stdin closes, while
    /// bloxlet may go on listing the process as running for a minute or two.
    /// So no session may join the proxy any more (see [`attach`]), and its
    /// stdin is closed for `sq` to exit, leaving [`Self::explain_exit`] to
    /// tell whether the attached sessions' agent went with the stream.
    fn end_stream(&self) {
        if self.stream_ended.is_cancelled() {
            return;
        }
        log::warn!(
            "{}: `sq` reports the orchestrator ended the agent's output stream; closing its \
             input so it exits, and starting another proxy for new sessions",
            self.describe()
        );
        self.stream_ended.cancel();
        self.transport.close_agent_input();
    }

    /// Held while a quiet reattached turn of the app instance with pid
    /// `owner_pid` waits for the answer to its cancel.
    fn cancel_window(&self, owner_pid: u32) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.cancel_windows
                .lock()
                .unwrap()
                .entry(owner_pid)
                .or_default(),
        )
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
    /// The session's client on the proxy's router.
    client: RemoteClientId,
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

    /// Why this session was cut off for falling behind the agent's output,
    /// if it was (see [`RemoteTransport::cut_off`]).
    pub(super) fn cut_off(&self) -> Option<String> {
        self.process.transport.cut_off(self.client)
    }

    /// For a session reattaching to `turn`, which a connection that is gone
    /// prompted: a watch on that turn, when bloxlet can be asked about its
    /// agent.
    pub(super) fn reattached_turn(&self, turn: &super::ReattachTurn) -> Option<ReattachedTurn> {
        let target = remote_liveness::Target::of(&self.key.program, &self.key.args)?;
        // Watched from before the session is loaded: the answer can arrive
        // as soon as the agent relays the turn to this connection.
        let prompt_answered = turn
            .owner_pid
            .filter(|_| turn.sole_turn_of_owner)
            .map(|pid| self.process.transport.watch_foreign_prompts(pid, None));
        Some(ReattachedTurn {
            process: Arc::clone(&self.process),
            target,
            trust_attachment: self.created_proxy,
            process_up_by_secs: process_up_by(turn).div_euclid(1000)
                + PROCESS_START_CLOCK_SLACK_SECS,
            agent_session_id: turn.agent_session_id.clone(),
            owner_pid: turn.owner_pid,
            owner_answers_watched: prompt_answered.is_some(),
            prompt_answered,
            cap_cancelled: OnceLock::new(),
        })
    }

    /// Lets go of this session's lease on purpose, leaving its turn running
    /// on the agent (see [`super::DetachHandle`]).
    pub(super) fn detacher(&self) -> LeaseDetacher {
        LeaseDetacher {
            key: self.key.clone(),
            process: Arc::clone(&self.process),
            client: self.client,
        }
    }

    /// Before [`Self::release`]: never send the proxy SIGINT, whichever
    /// session lets go of it last (see [`SharedProcess::keep_agent`] and
    /// `TransportMonitor::spares_proxy`). This session's own prompts still in
    /// flight are cancelled as usual once its connection is gone.
    pub(super) fn never_interrupt(&self) {
        self.process.keep_agent.store(true, Ordering::Relaxed);
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

/// Lets go of one session's lease on a shared proxy at once, from any
/// thread, leaving the session's turn running on the agent: what a
/// [`super::DetachHandle`] does to a remote session.
#[derive(Clone)]
pub(super) struct LeaseDetacher {
    key: ProxyKey,
    process: Arc<SharedProcess>,
    client: RemoteClientId,
}

impl LeaseDetacher {
    /// End the session's stream without cancelling its turn, and mark the
    /// proxy as one never to send SIGINT. When no other session holds it,
    /// SIGKILL its process group now rather than waiting for the session's
    /// teardown, so a quit that exits right after cannot leave `sq` behind
    /// to stop the agent on its way out.
    pub(super) fn detach_keeping_agent(&self) {
        self.process.keep_agent.store(true, Ordering::Relaxed);
        self.process.transport.keep_client(self.client);
        let mut registry = registry();
        let sole_lease = matches!(
            registry.get(&self.key),
            Some(Entry::Live { process, leases })
                if process.generation == self.process.generation && *leases == 1
        );
        if !sole_lease {
            log::info!(
                "{}: a session detached leaving its turn running; others still use the proxy",
                self.process.describe()
            );
            return;
        }
        log::info!(
            "{}: the last session detached leaving its turn running; killing the proxy \
             without interrupting its agent",
            self.process.describe()
        );
        // While the entry is live the proxy's thread has not retired it, so
        // the child is not reaped and its pgid cannot have been reused.
        if let Some(pid) = self.process.pid.and_then(|pid| i32::try_from(pid).ok()) {
            super::transport_loss::kill_process_group(pid);
        }
        registry.insert(
            self.key.clone(),
            Entry::Stopping {
                generation: self.process.generation,
                stopped: self.process.stopped.clone(),
            },
        );
        drop(registry);
        self.process.stop.cancel();
    }
}

/// When, in ms since the epoch, the agent process that ran `turn` was running
/// by: once the connection that prompted it had initialized — or, when that is
/// not known, once that connection's setup could at the latest have reached
/// it after the prompt, which is written before the connection starts `sq`.
fn process_up_by(turn: &super::ReattachTurn) -> i64 {
    turn.connected_at.unwrap_or_else(|| {
        let setup_ms = i64::try_from(super::ACP_SETUP_TIMEOUT.as_millis()).unwrap_or(i64::MAX);
        turn.prompted_at.saturating_add(setup_ms)
    })
}

/// A turn a previous app instance started on a remote agent, which a session
/// has reattached to through the proxy.
pub(super) struct ReattachedTurn {
    process: Arc<SharedProcess>,
    target: remote_liveness::Target,
    /// Whether the proxy was started for this session, so that `sq` starting
    /// or reusing a process answers whether the turn's process survived.
    trust_attachment: bool,
    /// By when the turn's agent process was running (see [`process_up_by`]),
    /// in seconds since the epoch, plus slack for clock skew: a process
    /// bloxlet says started later is not the one that ran the turn.
    process_up_by_secs: i64,
    /// The agent's id for the turn's session.
    agent_session_id: String,
    /// The pid of the app instance that prompted the turn, when known.
    owner_pid: Option<u32>,
    /// Answered with the stop reason once the agent answers the turn's
    /// `session/prompt`, when that response can be told apart from other
    /// sessions'.
    prompt_answered: Option<oneshot::Receiver<String>>,
    /// Whether `prompt_answered` was set up.
    owner_answers_watched: bool,
    /// Set, to how long the turn had been quiet, once the quiet cap has
    /// cancelled it.
    cap_cancelled: OnceLock<Duration>,
}

impl ReattachedTurn {
    /// Wait for the turn to end: when the agent answers its prompt (see
    /// [`super::ReattachTurn::sole_turn_of_owner`]), or else when bloxlet
    /// reports its agent process idle. bloxlet tracks prompts per process, so
    /// the latter also waits out prompts other sessions on it have in flight
    /// — at most until the turn's session has heard nothing for
    /// [`remote_liveness::REATTACHED_TURN_QUIET_CAP`], when the turn is
    /// cancelled (see [`Self::cancel_quiet_turn`]).
    ///
    /// Fails with a [`super::REMOTE_AGENT_LOST_PREFIX`] error naming the
    /// cause when the process that ran the turn is gone: `sq` found none to
    /// attach to and started a new one, or bloxlet reports it stopped. Also
    /// fails when the proxy loses its connection to it, and when the agent
    /// answers the turn's prompt as cancelled, since it was cut short.
    pub(super) async fn ended(mut self) -> Result<(), String> {
        let prompt_answered = self.prompt_answered.take();
        let closed = self.process.transport.closed().clone();
        let command_id = tokio::select! {
            command_id = self.surviving_command() => command_id
                .map_err(|cause| format!("{}{cause}", super::REMOTE_AGENT_LOST_PREFIX))?,
            _ = closed.cancelled() => return Err(self.proxy_gone()),
        };
        log::info!(
            "{}: reattached to agent process (command {command_id}); waiting for its turn to end",
            self.process.describe()
        );
        let watching_from = Instant::now();
        let poll = async {
            loop {
                match remote_liveness::check(&self.target, &command_id).await {
                    Ok(Liveness::Idle) => return Ok(()),
                    Ok(Liveness::Running) => {
                        let heard_at = self
                            .process
                            .transport
                            .last_heard(&self.agent_session_id)
                            .map_or(watching_from, |at| at.max(watching_from));
                        if heard_at.elapsed() >= remote_liveness::REATTACHED_TURN_QUIET_CAP {
                            return self.cancel_quiet_turn(heard_at.elapsed()).await;
                        }
                    }
                    Ok(Liveness::Gone(cause)) => {
                        return Err(format!("{}{cause}", super::REMOTE_AGENT_LOST_PREFIX))
                    }
                    Err(error) => log::warn!(
                        "{}: could not check command {command_id}: {error}",
                        self.process.describe()
                    ),
                }
                tokio::time::sleep(remote_liveness::REATTACHED_TURN_POLL).await;
            }
        };
        let answered = async {
            if let Some(answered) = prompt_answered {
                if let Ok(stop_reason) = answered.await {
                    return stop_reason;
                }
            }
            std::future::pending::<String>().await
        };
        tokio::select! {
            biased;
            ended = poll => ended,
            stop_reason = answered => self.owner_prompt_answered(&stop_reason),
            // Nothing streams the rest of the turn to the session any more.
            _ = closed.cancelled() => Err(self.proxy_gone()),
        }
    }

    /// Cancels the turn on its agent when the session is stopped.
    pub(super) fn stopper(&self) -> ReattachedTurnStop {
        ReattachedTurnStop {
            transport: self.process.transport.clone(),
            agent_session_id: self.agent_session_id.clone(),
        }
    }

    /// The error the turn ends with once its proxy has lost its connection.
    fn proxy_gone(&self) -> String {
        format!(
            "{}{}",
            super::REMOTE_AGENT_LOST_PREFIX,
            self.process
                .transport
                .loss()
                .map(|loss| loss.cause)
                .unwrap_or_else(|| "the remote agent proxy stopped".to_string())
        )
    }

    /// How the turn ended, now that the agent has answered its prompt with
    /// `stop_reason`. A cancelled answer means it was cut short rather than
    /// finished — usually by the quiet cap, whose cancel window leaves this
    /// watch to hear the answer (see [`Self::cancel_quiet_turn`]).
    fn owner_prompt_answered(&self, stop_reason: &str) -> Result<(), String> {
        if stop_reason != CANCELLED_STOP_REASON {
            log::info!(
                "{}: the agent answered the reattached turn's prompt: {stop_reason}",
                self.process.describe()
            );
            return Ok(());
        }
        Err(match self.cap_cancelled.get() {
            Some(quiet_for) => cut_short_by_quiet_cap(*quiet_for),
            None => format!(
                "{}the agent answered the turn's prompt as cancelled, so it did not finish",
                super::REMOTE_AGENT_LOST_PREFIX
            ),
        })
    }

    /// Stop waiting on a turn whose session has heard nothing for
    /// `quiet_for` while bloxlet still reports its agent process running.
    ///
    /// Either the turn has ended and other sessions keep the process busy,
    /// or it is still running, usually blocked on a permission request the
    /// connection that prompted it never answered (which is not re-sent to
    /// a new one). Cancelling ends the latter and is a no-op for the former.
    /// Then, when the instance that prompted the turn is known, the agent
    /// answering one of its prompts as cancelled within
    /// [`remote_liveness::CANCELLED_TURN_ANSWER_WAIT`] tells the two apart:
    /// the turn fails as lost, since it was cut short. With no such answer
    /// (or no way to recognise one) it is taken as having ended on its own.
    ///
    /// That answer's request id names the instance, not the session, so an
    /// instance's quiet turns on the process take turns: only one of them
    /// has a cancel window open at a time, and a cancelled answer within it
    /// is taken for its turn's. A turn that is its instance's only one is
    /// already watched for any answer to that instance's prompts (see
    /// [`Self::ended`]), so that watch hears this one too.
    ///
    /// Not covered: a cancel sent some other way while a window is open —
    /// the user stopping another reattached turn of the same instance, or
    /// another app instance cancelling one — can still have its answer
    /// taken for this turn's, failing as lost a turn that had ended.
    async fn cancel_quiet_turn(&self, quiet_for: Duration) -> Result<(), String> {
        log::warn!(
            "{}: the reattached turn on session {} has been quiet for {}s while bloxlet still \
             runs its agent; cancelling it",
            self.process.describe(),
            self.agent_session_id,
            quiet_for.as_secs()
        );
        let Some(owner_pid) = self.owner_pid else {
            self.process
                .transport
                .cancel_session(&self.agent_session_id);
            return Ok(());
        };
        let window = self.process.cancel_window(owner_pid);
        let _open = window.lock().await;
        let cancelled = (!self.owner_answers_watched).then(|| {
            self.process
                .transport
                .watch_foreign_prompts(owner_pid, Some(CANCELLED_STOP_REASON))
        });
        // Set before the cancel goes out, for `ended`'s watch to see it.
        let _ = self.cap_cancelled.set(quiet_for);
        self.process
            .transport
            .cancel_session(&self.agent_session_id);
        let wait = remote_liveness::CANCELLED_TURN_ANSWER_WAIT;
        let answered_cancelled = match cancelled {
            Some(cancelled) => matches!(tokio::time::timeout(wait, cancelled).await, Ok(Ok(_))),
            None => {
                tokio::time::sleep(wait).await;
                false
            }
        };
        if answered_cancelled {
            return Err(cut_short_by_quiet_cap(quiet_for));
        }
        log::info!(
            "{}: the agent did not answer the quiet turn's prompt as cancelled, so it had \
             already ended",
            self.process.describe()
        );
        Ok(())
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
        // it was already running once the turn's connection was up. bloxlet
        // failing to answer says nothing either way, so it is asked again.
        loop {
            match remote_liveness::command_started_at(&self.target, &command_id).await {
                Ok(CommandStart::At(started_at)) if started_at <= self.process_up_by_secs => {
                    return Ok(command_id)
                }
                Ok(CommandStart::At(_)) => {
                    return Err(format!(
                        "the agent process that ran it is gone: the one serving the workspace \
                         now (command {command_id}) started after the turn's connection to the \
                         agent was up"
                    ))
                }
                Ok(CommandStart::NotActive) => {
                    return Err(format!(
                        "bloxlet no longer lists the agent process (command {command_id}) as \
                         running"
                    ))
                }
                Ok(CommandStart::Unknown) => {
                    log::warn!(
                        "{}: bloxlet does not say when agent process {command_id} started; \
                         taking it for the one that ran the turn",
                        self.process.describe()
                    );
                    return Ok(command_id);
                }
                Err(error) => log::warn!(
                    "{}: could not ask bloxlet when agent process {command_id} started, so \
                     asking again: {error}",
                    self.process.describe()
                ),
            }
            tokio::time::sleep(remote_liveness::REATTACHED_TURN_POLL).await;
        }
    }
}

/// The error a reattached turn the quiet cap cut short ends with.
fn cut_short_by_quiet_cap(quiet_for: Duration) -> String {
    format!(
        "{}its turn went {} min without output while bloxlet still ran the agent, most \
         likely waiting on a permission request only the connection that prompted it could \
         answer, so it was cancelled",
        super::REMOTE_AGENT_LOST_PREFIX,
        quiet_for.as_secs() / 60
    )
}

/// Cancels a reattached turn on its agent (see [`ReattachedTurn::stopper`]).
pub(super) struct ReattachedTurnStop {
    transport: RemoteTransport,
    agent_session_id: String,
}

impl ReattachedTurnStop {
    /// Ask the agent to cancel the turn. Nothing waits for its answer, and
    /// the session may let go of the proxy right after — when it is the last
    /// one, stopping the proxy. So the cancel is queued on the proxy itself
    /// rather than sent through the session's connection: stopping the proxy
    /// first delivers what is queued (see
    /// [`RemoteTransport::deliver_queued_frames`]).
    pub(super) fn cancel(&self) {
        self.transport.cancel_session(&self.agent_session_id);
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
                // Lost its agent or its stream, and about to stop: start
                // another once it has.
                Some(Entry::Live { process, .. })
                    if process.transport.closed().is_cancelled()
                        || process.transport.output_ended().is_cancelled()
                        || process.stream_ended.is_cancelled() =>
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
        client: streams.client,
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
        stream_ended: CancellationToken::new(),
        cancel_windows: Mutex::default(),
        keep_agent: AtomicBool::new(false),
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
        // `sq` should exit once its stdin is closed; stop it should it not.
        _ = async {
            process.stream_ended.cancelled().await;
            tokio::time::sleep(STREAM_END_EXIT_GRACE).await;
        } => {}
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
    // A session cancelled on its way out (see `forward_client_frames`), or
    // stopped while reattached (see `ReattachedTurnStop`), must reach an
    // agent the proxy leaves running. The router was closed first, so the
    // connections still attached see their streams end and go.
    process
        .transport
        .deliver_queued_frames(Duration::from_secs(2))
        .await;
    if process.keep_agent.load(Ordering::Relaxed) {
        super::transport_loss::kill_without_interrupt(&mut child, true).await;
    } else {
        super::graceful_stop(&mut child, true).await;
    }
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
    /// Only an interrupted `sq` stops the process it started; one SIGKILLed,
    /// crashed or cut off by the network leaves it running, as it always
    /// leaves a process it reused. So whichever it reached, bloxlet is asked
    /// first, and while it still runs the process — or cannot say — the
    /// sessions are detached rather than failed, for their turns to be
    /// reattached to. When `sq` never logged which process it reached,
    /// bloxlet is asked about the one the proxy's arguments name.
    async fn explain_exit(&self, attached: usize, target: Option<&remote_liveness::Target>) {
        let cause = if self.stream_ended.is_cancelled() {
            "the orchestrator ended the agent's output stream".to_string()
        } else if self.exited.is_cancelled() {
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
        let (true, Some(target)) = (attached > 0, target) else {
            self.transport.lose(cause);
            return;
        };
        let process = match &command_id {
            Some(command_id) => format!("command {command_id}"),
            None => target.requested(),
        };
        log::info!(
            "{}: {cause}; asking bloxlet whether the agent process it {} ({process}) still runs",
            self.describe(),
            match attachment {
                Attachment::Started => "started",
                Attachment::Reused => "reused",
                Attachment::Unknown => "reached",
            }
        );
        let verdict = tokio::time::timeout(remote_liveness::QUICK_CHECK_TIMEOUT, async {
            match &command_id {
                Some(command_id) => remote_liveness::check(target, command_id).await,
                None => remote_liveness::check_requested(target).await,
            }
        })
        .await
        .unwrap_or_else(|_| Err("`sq blox ws commands` timed out".to_string()));
        match verdict {
            Ok(Liveness::Running | Liveness::Idle) => self.transport.detach_sessions(format!(
                "{cause}, but bloxlet still runs the agent process ({process})"
            )),
            Ok(Liveness::Gone(gone)) => self.transport.lose(gone),
            Err(error) => self.transport.detach_sessions(format!(
                "{cause}, and whether the agent process ({process}) still runs is unknown: \
                 {error}"
            )),
        }
    }
}

/// While sessions are attached, ask bloxlet whether the agent process still
/// runs whenever its stream recently disconnected, or a session's prompt has
/// gone quiet, and fail the sessions once it does not.
async fn watch_liveness(process: Arc<SharedProcess>, target: Arc<remote_liveness::Target>) {
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
        let stream_ended = process
            .report
            .lock()
            .unwrap()
            .note(&blox_cli::strip_ansi_escape_sequences(&line));
        match process.pid {
            Some(pid) => log::warn!("[{} stderr][proxy pid {pid}] {line}", process.label),
            None => log::warn!("[{} stderr] {line}", process.label),
        }
        if stream_ended {
            process.end_stream();
        }
    }
    // stderr EOF (or a read error) means the proxy has exited or torn down
    // its stderr; either way its agent can no longer be trusted to report.
    process.exited.cancel();
}

impl ProxyReport {
    /// Take in one line of the proxy's log, returning whether it says the
    /// orchestrator ended the agent's output stream.
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
        // The orchestrator ends the output stream when the agent process's
        // output has ended. `sq` then only logs this and keeps its stdout
        // open until its stdin closes, so nothing else would tell the
        // sessions (see `SharedProcess::end_stream`).
        line.contains("Received EOF signal from orchestrator")
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
