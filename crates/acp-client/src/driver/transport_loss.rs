//! Telling a lost transport apart from an agent that answered with an error.
//!
//! A remote session's turn runs on the workstation, not in the proxy process
//! this connection talks through, so when the proxy's byte stream goes away
//! the turn may well still be running there. Callers that can come back for
//! it (a remote session left `disconnected` rather than failed) need to know
//! which of the two happened, and the error string is the wrong place to
//! look: the SDK fails every pending request with a structured
//! transport-closed error, and the proxy reports a dead remote process on
//! stderr while its stdout stays open. [`TransportMonitor`] collects those
//! structural signals for one connection and turns them into the
//! [`TransportLoss`] verdict on [`SessionSettled`]; the phrase list behind
//! [`is_transport_loss_error`] is only a fallback for an SDK that aborted the
//! session before it could see its own request fail.
use super::*;

/// What a reusing `sq blox acp` CLI logs on stderr when the remote ACP process
/// it relays for has gone. Its stdout stays open and the CLI keeps running
/// until its stdin closes, so without this neither an EOF nor a child exit
/// would ever say the connection is dead.
pub(super) const ORCHESTRATOR_EOF_LINE: &str = "Received EOF signal from orchestrator";

/// How long a request whose transport was just reported lost still waits for
/// a real answer before the loss is taken as its outcome. A reply already on
/// its way (the frame that raced the signal) still wins.
pub(super) const TRANSPORT_LOSS_REPLY_GRACE: Duration = Duration::from_secs(2);

/// Where the connection was when its transport was lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportLossPhase {
    /// The agent session exists but no prompt was in flight: setup, replay,
    /// or waiting for the first prompt. Nothing was running for this session.
    Setup,
    /// A `session/prompt` was in flight. A remote turn may still be running
    /// on the workstation.
    Turn,
    /// Holding for background work after a completed turn. The turn's
    /// outcome stands; only the wait was cut short.
    Hold,
}

/// The connection's transport to the agent went away, as opposed to the agent
/// answering with an error. `None` on [`SessionSettled::transport_loss`] means
/// the connection ended for a reason the agent gave, or before any agent
/// session existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportLoss {
    pub phase: TransportLossPhase,
}

/// Phrases (matched case-insensitively) that only appear in an error when the
/// byte stream to the agent went away: the SDK's transport-closed errors and
/// the OS-level pipe failures underneath them. An agent that *answered* — a
/// refused prompt, an auth challenge, a config selection it does not know —
/// never produces any of them.
///
/// Process-exit wording is deliberately absent. The bridges relay their own
/// subprocess dying as a prompt *error over a live transport*: claude-agent-acp
/// forwards the Agent SDK's "Claude Code process exited with code N", and
/// goose reports "subprocess exited with status" / "Child exited gracefully"
/// for the tools it runs. Those arrive as an answer, with the bridge still
/// connected and nothing left running behind it, so a caller that treated
/// them as a lost connection would wait for work that is not happening. The
/// SDK's own closed-pipe message ("Incoming transport closed") is what both
/// bridges, and the Blox proxy, produce when the stream really goes away.
const TRANSPORT_LOSS_PHRASES: &[&str] = &[
    "transport closed",
    "connection closed",
    "connection reset",
    "broken pipe",
    "unexpected eof",
    "early eof",
    "unexpected end of file",
];

/// Whether a run error *reads* like the connection to the agent was lost (the
/// process died, the proxy dropped, the pipe closed) rather than the agent
/// failing the request for a reason of its own.
///
/// Wording is a fallback. The connection's own verdict is
/// [`SessionSettled::transport_loss`], built from structural signals (the
/// SDK's transport-closed error, stdout EOF, the proxy's orchestrator-EOF
/// line); prefer it wherever a settled event is available.
pub fn is_transport_loss_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    TRANSPORT_LOSS_PHRASES
        .iter()
        .any(|phrase| lower.contains(phrase))
}

/// Whether a formatted request error carries the SDK's transport-closed
/// discriminator. Setup errors are rendered with `{error:?}` before they reach
/// the session loop, and that rendering keeps the error's `data`, so this is
/// the structured `reason` field read back — not wording an agent could send.
pub(super) fn formatted_error_is_transport_closed(error: &str) -> bool {
    error.contains(agent_client_protocol::INCOMING_TRANSPORT_CLOSED_REASON)
}

/// A structural sign that the transport went away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TransportSignal {
    /// The agent's stdout reached EOF (or failed to read).
    StdoutClosed,
    /// The proxy reported that its remote process is gone
    /// ([`ORCHESTRATOR_EOF_LINE`]).
    OrchestratorEof,
    /// The caller abandoned the connection on purpose ([`DetachHandle`]).
    Detached,
}

impl std::fmt::Display for TransportSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TransportSignal::StdoutClosed => "the agent's stdout closed",
            TransportSignal::OrchestratorEof => "the proxy lost its remote ACP process",
            TransportSignal::Detached => "the connection was detached",
        })
    }
}

/// How the connection task lets go of the agent process at teardown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProxyStop {
    /// The usual stop: an in-flight remote prompt is cancelled once its
    /// connection is gone, and the proxy, when no other session holds it,
    /// gets [`graceful_stop`]'s SIGINT before SIGKILL.
    Graceful,
    /// The connection was detached, so its remote turn is meant to outlive
    /// it: no `session/cancel` is sent for it, and the proxy is only ever
    /// SIGKILLed — SIGINT to the `sq blox acp` CLI that started the remote
    /// ACP process stops that process and every turn on it.
    ///
    /// A transport lost on its own does not ask for this: the proxy's own
    /// thread stops a proxy whose stream or agent went (see
    /// `remote_process::run_proxy`), and a session cut off from a proxy
    /// that others still use has its turn cancelled like any other.
    KillOnly,
}

#[derive(Default)]
struct MonitorState {
    /// `None` until the agent session is named; see [`TransportLossPhase`].
    phase: Option<TransportLossPhase>,
    /// The first structural signal, and the phase it landed in.
    first_signal: Option<(TransportSignal, Option<TransportLossPhase>)>,
    /// Set where a request is known to have failed *because* the transport
    /// went (the SDK's own transport-closed error, or the reply grace running
    /// out after a signal), with the phase that request belonged to.
    confirmed: Option<TransportLossPhase>,
    detach_requested: bool,
    /// The remote session's lease on its shared proxy, while a detach may
    /// still let go of it. Cleared when teardown begins, before the lease is
    /// released, so a detach never touches a proxy this connection has
    /// already let go of.
    lease: Option<super::remote_process::LeaseDetacher>,
}

/// One connection's transport-loss evidence, shared by the stdout normalizer,
/// the stderr reader, the session loop, the connection task, and the
/// [`DetachHandle`] (which may be on another thread).
pub(super) struct TransportMonitor {
    state: std::sync::Mutex<MonitorState>,
    /// Fired by the first [`TransportSignal`].
    lost: CancellationToken,
}

impl TransportMonitor {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: std::sync::Mutex::new(MonitorState::default()),
            lost: CancellationToken::new(),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, MonitorState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The agent session is named: from here a loss is a session's.
    pub(super) fn session_named(&self) {
        let mut state = self.state();
        if state.phase.is_none() {
            state.phase = Some(TransportLossPhase::Setup);
        }
    }

    /// A prompt is about to go out. Refused (`false`) once a signal is in: the
    /// prompt would go to a transport already known dead, and a loss recorded
    /// as [`TransportLossPhase::Turn`] would claim a turn is running remotely
    /// that never started. Checked and set under one lock so a detach from
    /// another thread lands either before (refused) or after (a turn).
    pub(super) fn begin_turn(&self) -> bool {
        let mut state = self.state();
        if state.first_signal.is_some() {
            return false;
        }
        state.phase = Some(TransportLossPhase::Turn);
        true
    }

    pub(super) fn begin_hold(&self) {
        self.state().phase = Some(TransportLossPhase::Hold);
    }

    pub(super) fn signal(&self, signal: TransportSignal) {
        {
            let mut state = self.state();
            if state.first_signal.is_none() {
                state.first_signal = Some((signal, state.phase));
            }
        }
        self.lost.cancel();
    }

    /// Fires once any [`TransportSignal`] is in.
    pub(super) fn lost(&self) -> &CancellationToken {
        &self.lost
    }

    /// The first signal, if any.
    pub(super) fn first_signal(&self) -> Option<TransportSignal> {
        self.state().first_signal.map(|(signal, _)| signal)
    }

    /// The error a wait the loss cut short settles with.
    pub(super) fn lost_error(&self, during: &str) -> String {
        match self.first_signal() {
            Some(signal) => format!("ACP transport lost {during} ({signal})"),
            None => format!("ACP transport lost {during}"),
        }
    }

    /// A request belonging to `phase` failed because the transport went. Only
    /// a named session can lose one: before that there is nothing remote to
    /// come back to.
    pub(super) fn confirm(&self, phase: TransportLossPhase) {
        let mut state = self.state();
        if state.phase.is_some() && state.confirmed.is_none() {
            state.confirmed = Some(phase);
        }
    }

    pub(super) fn detach_requested(&self) -> bool {
        self.state().detach_requested
    }

    /// Record the remote session's lease for [`DetachHandle`].
    pub(super) fn arm_lease(&self, lease: super::remote_process::LeaseDetacher) {
        self.state().lease = Some(lease);
    }

    /// Teardown is starting: disarm the detach (the lease is about to be
    /// released) and say how to stop the agent.
    pub(super) fn begin_teardown(&self) -> ProxyStop {
        let mut state = self.state();
        state.lease = None;
        if state.detach_requested {
            ProxyStop::KillOnly
        } else {
            ProxyStop::Graceful
        }
    }

    /// The connection's verdict, once `run_acp_session` has ended.
    ///
    /// `session_returned` is false when the SDK ended the connection under the
    /// session loop (its background actors failed) instead of the loop
    /// returning: no request got to fail with the transport-closed error, so
    /// any signal — or, failing that, the error's wording — stands in for it.
    pub(super) fn verdict(
        &self,
        session_returned: bool,
        error: Option<&str>,
    ) -> Option<TransportLoss> {
        let state = self.state();
        if let Some(phase) = state.confirmed {
            return Some(TransportLoss { phase });
        }
        if session_returned {
            return None;
        }
        let signalled = state.first_signal.is_some();
        if !signalled && !error.is_some_and(is_transport_loss_error) {
            return None;
        }
        let phase = state
            .first_signal
            .and_then(|(_, phase)| phase)
            .or(state.phase)?;
        Some(TransportLoss { phase })
    }

    /// See [`DetachHandle::detach`].
    fn detach(&self) -> bool {
        {
            let mut state = self.state();
            let Some(lease) = state.lease.clone() else {
                return false;
            };
            state.detach_requested = true;
            if state.first_signal.is_none() {
                state.first_signal = Some((TransportSignal::Detached, state.phase));
            }
            // Under the lock, so teardown cannot disarm and release in
            // between.
            lease.detach_keeping_agent();
        }
        self.lost.cancel();
        true
    }
}

/// SIGKILL a remote proxy's whole process group: the `sq` wrapper and the
/// `blox.exoskeleton` it spawned, with no SIGINT first. An already-empty group
/// answers ESRCH, which is the expected outcome after a wrapper exit.
pub(super) fn kill_process_group(pid: i32) {
    #[cfg(unix)]
    if let Err(e) = signal::kill(Pid::from_raw(-pid), Signal::SIGKILL) {
        log::trace!("SIGKILL of remote proxy process group {pid}: {e}");
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Teardown for a connection that must leave its remote turn running: SIGKILL
/// the proxy group (never SIGINT, see [`ProxyStop::KillOnly`]), then reap.
pub(super) async fn kill_without_interrupt(child: &mut tokio::process::Child, is_remote: bool) {
    if is_remote {
        if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
            kill_process_group(pid);
        }
    }
    let _ = child.kill().await;
}

/// Abandons a remote session's connection while leaving its turn running on
/// the workstation: its routed stream on the shared proxy ends at once, with
/// no `session/cancel` for its turn, and the proxy is from then on never sent
/// SIGINT (which would stop a process `sq` started, and every turn on it).
/// When no other session holds the proxy its process group is SIGKILLed
/// there and then. The connection settles as soon as it notices.
///
/// What the connection settles with depends on where it was:
/// - a prompt in flight: an error with [`SessionSettleReason::Detached`] and a
///   [`TransportLossPhase::Turn`] verdict — the turn is still running remotely;
/// - holding after a completed turn: the completed turn, settled
///   [`SessionSettleReason::HoldStopped`];
/// - anywhere before the prompt: [`AgentRunOutcome::Cancelled`].
///
/// Cloneable and `Send`, for callers on another thread (the quit path).
#[derive(Clone)]
pub struct DetachHandle {
    monitor: Arc<TransportMonitor>,
}

impl DetachHandle {
    pub(super) fn new(monitor: Arc<TransportMonitor>) -> Self {
        Self { monitor }
    }

    /// Detach now. `false` when there is nothing to detach from: a local
    /// session (no proxy lease), or a connection already tearing down, which
    /// lets go of its proxy itself.
    pub fn detach(&self) -> bool {
        self.monitor.detach()
    }
}
