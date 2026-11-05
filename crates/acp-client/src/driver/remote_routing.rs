//! Per-session routing for remote ACP transports.
//!
//! `sq blox acp` reuses a workstation's running agent process whenever one
//! matches the requested command, and every client attached to that process
//! reads its whole stdout. Two Staged sessions on one workstation therefore
//! see each other's `session/update`s, permission requests, raw SDK frames
//! and responses: unrouted, each one records the other's transcript and
//! answers the other's requests.
//!
//! [`RemoteSessionRouter`] restores per-session delivery on top of that
//! broadcast. Outgoing request ids are rewritten into a per-client namespace,
//! so a response maps back to exactly one sender no matter how the clients
//! allocate ids; agent-initiated frames go to the client that owns the ACP
//! `sessionId` they name; a frame nobody owns is logged and dropped rather
//! than written into some other session's transcript.
//!
//! It also notes when each session last heard from the agent and which of
//! its prompts are in flight, so a session waiting on an agent that died
//! without closing the stream can be noticed (see
//! [`RemoteSessionRouter::has_stalled_prompt`]). A client that detaches with
//! a prompt still in flight has that session cancelled: the shared process
//! would otherwise keep serving a turn nobody is waiting on.
//!
//! Responses to requests other app instances sent reach every attachment
//! too. They are dropped, except that one ending a prompt can be watched for
//! (see [`RemoteSessionRouter::watch_foreign_prompts`]): it is how a session
//! reattached to a dead instance's turn learns that turn has ended.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// One client (one Staged session's ACP connection) on a router.
pub(super) type RemoteClientId = u64;

/// Client ids are unique per app process, so two routers fed the same
/// broadcast never mistake each other's rewritten request ids for their own.
static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

/// Frames held back while a `session/new` is in flight, at most.
const MAX_EARLY_FRAMES: usize = 256;

/// Buffer size of each client's in-process pipes.
const CLIENT_PIPE_BYTES: usize = 64 * 1024;

/// How long `sq` is given to relay a `session/cancel` written to its stdin
/// before it may be interrupted: it drops input it has not yet sent on to
/// the orchestrator when it stops.
#[cfg(not(test))]
const CANCEL_RELAY_GRACE: Duration = Duration::from_secs(1);
#[cfg(test)]
const CANCEL_RELAY_GRACE: Duration = Duration::from_millis(300);

const SESSION_NEW_METHOD: &str = "session/new";
const SESSION_PROMPT_METHOD: &str = "session/prompt";
const CANCEL_REQUEST_METHOD: &str = "$/cancel_request";

/// Demultiplexes one agent process's frames across the clients sharing it.
pub(super) struct RemoteSessionRouter {
    state: Mutex<RouterState>,
}

#[derive(Default)]
struct RouterState {
    /// Where each attached client's frames are delivered.
    clients: HashMap<RemoteClientId, mpsc::UnboundedSender<String>>,
    /// Requests awaiting a response, keyed by their rewritten wire id.
    pending: HashMap<String, PendingRequest>,
    /// ACP session id → the client that owns it.
    owners: HashMap<String, RemoteClientId>,
    /// Frames naming a session nobody owns yet, held while a `session/new`
    /// is in flight: an agent may announce a new session before it answers.
    early_frames: VecDeque<EarlyFrame>,
    /// Unowned session ids already reported, so each is logged once.
    reported_unowned: HashSet<String>,
    /// When the agent last sent a frame naming each session.
    last_frame_at: HashMap<String, Instant>,
    /// The agent's output has ended; nothing more will be routed.
    closed: bool,
    /// How the attached sessions lost their agent, when the router closed
    /// other than by [`RemoteSessionRouter::close`].
    loss: Option<RemoteLoss>,
    /// Watches for the end of prompts another app instance sent.
    foreign_prompt_watches: Vec<ForeignPromptWatch>,
    /// When a `session/cancel` was last queued for the agent.
    last_cancel_at: Option<Instant>,
}

/// Answers `ended` once a response ending a prompt whose request id starts
/// with `id_prefix` arrives (with stop reason `stop_reason`, when set).
/// Dropped once its receiver is.
struct ForeignPromptWatch {
    id_prefix: String,
    stop_reason: Option<&'static str>,
    ended: oneshot::Sender<()>,
}

/// How the sessions on a remote transport lost their agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RemoteLoss {
    pub(super) cause: String,
    /// Only the proxy went: bloxlet still runs (or may still run) the agent
    /// process, so its turns can be reattached to.
    pub(super) agent_may_survive: bool,
}

struct PendingRequest {
    client: RemoteClientId,
    original_id: Value,
    opens_session: bool,
    /// The session a `session/prompt` request prompts.
    prompts: Option<String>,
    sent_at: Instant,
}

struct EarlyFrame {
    session_id: String,
    line: String,
}

impl RemoteSessionRouter {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(RouterState::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, RouterState> {
        self.state.lock().unwrap()
    }

    /// Register a client; frames routed to it are sent on `inbound`. Once the
    /// router is closed, `inbound` is dropped at once, so the client sees
    /// end-of-stream instead of waiting on output that will never come.
    pub(super) fn attach(&self, inbound: mpsc::UnboundedSender<String>) -> RemoteClientId {
        let client = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state();
        if !state.closed {
            state.clients.insert(client, inbound);
        }
        client
    }

    /// Forget a client along with its sessions and in-flight requests,
    /// returning the sessions it still had a prompt in flight on, unless the
    /// agent's output has already ended.
    pub(super) fn detach(&self, client: RemoteClientId) -> Vec<String> {
        let mut state = self.state();
        let state = &mut *state;
        state.clients.remove(&client);
        state.owners.retain(|session_id, owner| {
            let keep = *owner != client;
            if !keep {
                state.last_frame_at.remove(session_id);
            }
            keep
        });
        let mut abandoned = Vec::new();
        state.pending.retain(|_, pending| {
            if pending.client != client {
                return true;
            }
            if let Some(session_id) = pending.prompts.take() {
                abandoned.push(session_id);
            }
            false
        });
        if !state.session_new_pending() {
            state.drop_early_frames();
        }
        if state.closed {
            abandoned.clear();
        }
        abandoned.sort();
        abandoned.dedup();
        abandoned
    }

    /// Drop every client's delivery channel, so each sees end-of-stream.
    pub(super) fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        state.clients.clear();
    }

    /// Close because the agent was lost, recording how unless the router was
    /// already closed.
    pub(super) fn lose(&self, loss: RemoteLoss) {
        let mut state = self.state();
        if state.closed {
            return;
        }
        state.loss = Some(loss);
        state.closed = true;
        state.clients.clear();
    }

    /// How the agent was lost, if it was.
    pub(super) fn loss(&self) -> Option<RemoteLoss> {
        self.state().loss.clone()
    }

    /// When the agent last sent a frame naming `session_id`, unless the
    /// client owning the session has since detached.
    pub(super) fn last_heard(&self, session_id: &str) -> Option<Instant> {
        self.state().last_frame_at.get(session_id).copied()
    }

    /// Answered once the agent answers a `session/prompt` that the app
    /// instance with pid `owner_pid` sent through some other connection —
    /// a request this router never saw, whose response the agent broadcasts
    /// to every attachment — and, when `stop_reason` is set, ends it for
    /// that reason. Prompts this router sent never count.
    pub(super) fn watch_foreign_prompts(
        &self,
        owner_pid: u32,
        stop_reason: Option<&'static str>,
    ) -> oneshot::Receiver<()> {
        let (ended, answered) = oneshot::channel();
        let mut state = self.state();
        state
            .foreign_prompt_watches
            .retain(|watch| !watch.ended.is_closed());
        state.foreign_prompt_watches.push(ForeignPromptWatch {
            id_prefix: request_id_prefix(owner_pid),
            stop_reason,
            ended,
        });
        answered
    }

    /// Note that a `session/cancel` is being queued for the agent.
    fn note_cancel(&self) {
        self.state().last_cancel_at = Some(Instant::now());
    }

    /// Whether a session has a prompt in flight and has heard nothing from
    /// the agent for `quiet_for` since sending it. That is how a session on
    /// an agent that died mid-turn looks while the stream stays open,
    /// whether the agent was running a tool or still generating.
    pub(super) fn has_stalled_prompt(&self, quiet_for: Duration) -> bool {
        let state = self.state();
        state.pending.values().any(|pending| {
            let Some(session_id) = &pending.prompts else {
                return false;
            };
            let heard_at = state
                .last_frame_at
                .get(session_id)
                .map_or(pending.sent_at, |at| (*at).max(pending.sent_at));
            heard_at.elapsed() >= quiet_for
        })
    }

    /// Prepare one frame `client` is sending to the agent, returning the line
    /// to write. Requests get a namespaced id; any `params.sessionId` marks
    /// that session as the client's.
    pub(super) fn outgoing(&self, client: RemoteClientId, line: &str) -> String {
        let Ok(Value::Object(mut frame)) = serde_json::from_str::<Value>(line) else {
            return line.to_string();
        };
        let mut state = self.state();
        if let Some(session_id) = session_id_in(&frame, "params") {
            state.claim(session_id, client);
        }
        let method = frame
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = frame.get("id").filter(|id| !id.is_null()).cloned();
        match (method.as_deref(), id) {
            (Some(method), Some(original_id)) => {
                let wire_id = wire_request_id(client, &original_id);
                let prompts = (method == SESSION_PROMPT_METHOD)
                    .then(|| session_id_in(&frame, "params").map(str::to_owned))
                    .flatten();
                state.pending.insert(
                    wire_id.clone(),
                    PendingRequest {
                        client,
                        original_id,
                        opens_session: method == SESSION_NEW_METHOD,
                        prompts,
                        sent_at: Instant::now(),
                    },
                );
                frame.insert("id".to_string(), Value::String(wire_id));
            }
            // A request cancellation names its request by the sender's id.
            (Some(CANCEL_REQUEST_METHOD), None) => {
                let Some(request_id) = frame
                    .get_mut("params")
                    .and_then(|params| params.get_mut("requestId"))
                else {
                    return line.to_string();
                };
                let wire_id = wire_request_id(client, request_id);
                if !state.pending.contains_key(&wire_id) {
                    return line.to_string();
                }
                *request_id = Value::String(wire_id);
            }
            // Notifications and answers to the agent's own requests pass
            // through untouched.
            _ => return line.to_string(),
        }
        serde_json::to_string(&frame).unwrap_or_else(|_| line.to_string())
    }

    /// Deliver one frame from the agent to the client it belongs to.
    pub(super) fn incoming(&self, line: &str) {
        let Ok(Value::Object(frame)) = serde_json::from_str::<Value>(line) else {
            log::warn!("Dropping remote ACP frame that is not a JSON-RPC object");
            return;
        };
        let mut state = self.state();
        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            state.route_response(frame);
            return;
        };
        let is_request = frame.get("id").is_some_and(|id| !id.is_null());
        match session_id_in(&frame, "params") {
            Some(session_id) => {
                state
                    .last_frame_at
                    .insert(session_id.to_string(), Instant::now());
                state.route_session_frame(session_id, line);
            }
            None if is_request => state.route_sessionless_request(method, line),
            None => {
                let clients: Vec<_> = state.clients.keys().copied().collect();
                for client in clients {
                    state.deliver(client, line.to_string());
                }
            }
        }
    }
}

impl RouterState {
    fn claim(&mut self, session_id: &str, client: RemoteClientId) {
        if let Some(previous) = self.owners.insert(session_id.to_string(), client) {
            if previous != client {
                log::warn!(
                    "Remote ACP session {session_id} moved from client {previous} to client {client}"
                );
            }
        }
    }

    fn session_new_pending(&self) -> bool {
        self.pending.values().any(|pending| pending.opens_session)
    }

    fn route_session_frame(&mut self, session_id: &str, line: &str) {
        if let Some(&client) = self.owners.get(session_id) {
            self.deliver(client, line.to_string());
            return;
        }
        if !self.session_new_pending() {
            self.report_unowned(session_id);
            return;
        }
        if self.early_frames.len() >= MAX_EARLY_FRAMES {
            if let Some(evicted) = self.early_frames.pop_front() {
                self.report_unowned(&evicted.session_id);
            }
        }
        self.early_frames.push_back(EarlyFrame {
            session_id: session_id.to_string(),
            line: line.to_string(),
        });
    }

    fn route_sessionless_request(&mut self, method: &str, line: &str) {
        // Every ACP client method names its session. Without one there is no
        // telling which session should answer, and two answers are worse
        // than none — unless only one client could be asked.
        let mut clients = self.clients.keys().copied();
        match (clients.next(), clients.next()) {
            (Some(client), None) => self.deliver(client, line.to_string()),
            _ => log::warn!(
                "Dropping remote ACP request '{method}' without a sessionId: \
                 {} clients share the agent process",
                self.clients.len()
            ),
        }
    }

    fn route_response(&mut self, mut frame: Map<String, Value>) {
        let pending = frame
            .get("id")
            .and_then(Value::as_str)
            .and_then(|wire_id| self.pending.remove(wire_id));
        let Some(pending) = pending else {
            self.note_foreign_prompt_end(&frame);
            // Another client's response, or the proxy's liveness probe.
            log::debug!(
                "Dropping remote ACP response for a request this connection did not send (id {})",
                frame.get("id").unwrap_or(&Value::Null)
            );
            return;
        };
        let session_id = session_id_in(&frame, "result").map(str::to_owned);
        if let Some(session_id) = &session_id {
            self.claim(session_id, pending.client);
        }
        if pending.opens_session {
            self.release_early_frames(session_id.as_deref(), pending.client);
        }
        frame.insert("id".to_string(), pending.original_id);
        match serde_json::to_string(&frame) {
            Ok(line) => self.deliver(pending.client, line),
            Err(error) => log::warn!("Failed to re-encode remote ACP response: {error}"),
        }
    }

    /// Answer the watches on other connections' prompts that `frame`, a
    /// response to a request this router does not have pending, ends. Only a
    /// result carrying a stop reason ends a prompt; the other requests of
    /// the instance that sent it are answered without one.
    fn note_foreign_prompt_end(&mut self, frame: &Map<String, Value>) {
        let Some(wire_id) = frame.get("id").and_then(Value::as_str) else {
            return;
        };
        let Some(stop_reason) = frame
            .get("result")
            .and_then(|result| result.get("stopReason"))
            .and_then(Value::as_str)
        else {
            return;
        };
        let watches = std::mem::take(&mut self.foreign_prompt_watches);
        for watch in watches {
            if watch.ended.is_closed() {
                continue;
            }
            let ends_watched_prompt = wire_id.starts_with(&watch.id_prefix)
                && watch.stop_reason.is_none_or(|reason| reason == stop_reason);
            if ends_watched_prompt {
                log::info!(
                    "A prompt another connection sent (request {wire_id}) has ended: {stop_reason}"
                );
                let _ = watch.ended.send(());
            } else {
                self.foreign_prompt_watches.push(watch);
            }
        }
    }

    /// Deliver the frames a new session received before its `session/new`
    /// response, in order, ahead of that response.
    fn release_early_frames(&mut self, session_id: Option<&str>, client: RemoteClientId) {
        let still_pending = self.session_new_pending();
        for frame in std::mem::take(&mut self.early_frames) {
            if session_id == Some(frame.session_id.as_str()) {
                self.deliver(client, frame.line);
            } else if still_pending {
                self.early_frames.push_back(frame);
            } else {
                self.report_unowned(&frame.session_id);
            }
        }
    }

    fn drop_early_frames(&mut self) {
        for frame in std::mem::take(&mut self.early_frames) {
            self.report_unowned(&frame.session_id);
        }
    }

    fn report_unowned(&mut self, session_id: &str) {
        if self.reported_unowned.insert(session_id.to_string()) {
            log::info!(
                "Dropping remote ACP frames for session {session_id}: \
                 no session on this connection owns it"
            );
        }
    }

    fn deliver(&mut self, client: RemoteClientId, line: String) {
        if let Some(inbound) = self.clients.get(&client) {
            if inbound.send(line).is_err() {
                self.clients.remove(&client);
            }
        }
    }
}

fn session_id_in<'a>(frame: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    frame.get(field)?.get("sessionId")?.as_str()
}

/// The id a client's request carries on the shared transport. Client ids are
/// unique within this process and the pid separates app instances, so no two
/// clients — even ones allocating ids from the same counter — can collide.
fn wire_request_id(client: RemoteClientId, original_id: &Value) -> String {
    let original = match original_id {
        Value::String(id) => id.clone(),
        other => other.to_string(),
    };
    format!(
        "{}{client}:{original}",
        request_id_prefix(std::process::id())
    )
}

/// How every request id the app instance with pid `pid` sends begins.
fn request_id_prefix(pid: u32) -> String {
    format!("staged-{pid}-")
}

/// One agent process's stdio, routed: clients attach to get their own
/// streams for an ACP connection.
#[derive(Clone)]
pub(super) struct RemoteTransport {
    router: Arc<RemoteSessionRouter>,
    to_agent: mpsc::UnboundedSender<ToAgent>,
    /// Cancelled once the agent's output has ended, which the transport
    /// leaves for its owner to explain (see [`Self::output_end`]).
    output_ended: CancellationToken,
    output_end: Arc<Mutex<Option<String>>>,
    /// Cancelled once the transport has closed: every client has been told
    /// the agent's output ended.
    closed: CancellationToken,
}

/// What is queued for the agent's stdin.
enum ToAgent {
    Frame(String),
    /// Answered once every frame queued before it has been written.
    Flush(oneshot::Sender<()>),
}

/// One client's ends of a [`RemoteTransport`], for `ByteStreams`.
pub(super) struct RoutedClientStreams {
    /// The ACP connection writes its frames here.
    pub(super) outgoing: DuplexStream,
    /// The ACP connection reads the frames routed to it here.
    pub(super) incoming: DuplexStream,
}

impl RemoteTransport {
    /// Start routing `stdout` (the proxy's raw output) to attached clients and
    /// their frames into `stdin`.
    pub(super) fn spawn<W, R>(stdin: W, stdout: R) -> Self
    where
        W: AsyncWrite + Send + Unpin + 'static,
        R: AsyncRead + Send + Unpin + 'static,
    {
        let router = Arc::new(RemoteSessionRouter::new());
        let (to_agent, to_agent_rx) = mpsc::unbounded_channel();
        let output_ended = CancellationToken::new();
        let output_end = Arc::new(Mutex::new(None));
        tokio::spawn(write_agent_stdin(stdin, to_agent_rx));
        tokio::spawn(read_agent_stdout(
            stdout,
            Arc::clone(&router),
            Arc::clone(&output_end),
            output_ended.clone(),
        ));
        Self {
            router,
            to_agent,
            output_ended,
            output_end,
            closed: CancellationToken::new(),
        }
    }

    /// Cancelled once the transport has closed.
    pub(super) fn closed(&self) -> &CancellationToken {
        &self.closed
    }

    /// Cancelled once the agent's output has ended. The clients' streams stay
    /// open until the owner closes the transport, having found out whether
    /// the agent is gone with it.
    pub(super) fn output_ended(&self) -> &CancellationToken {
        &self.output_ended
    }

    /// How the agent's output ended, once it has.
    pub(super) fn output_end(&self) -> Option<String> {
        self.output_end.lock().unwrap().clone()
    }

    /// End every attached client's stream.
    pub(super) fn close(&self) {
        self.router.close();
        self.closed.cancel();
    }

    /// End every attached client's stream because the agent was lost.
    pub(super) fn lose(&self, cause: String) {
        self.end(RemoteLoss {
            cause,
            agent_may_survive: false,
        });
    }

    /// End every attached client's stream because the connection to the
    /// agent was lost while the agent itself may still be running.
    pub(super) fn detach_sessions(&self, cause: String) {
        self.end(RemoteLoss {
            cause,
            agent_may_survive: true,
        });
    }

    fn end(&self, loss: RemoteLoss) {
        self.router.lose(loss);
        self.closed.cancel();
    }

    /// How the agent was lost, if it was.
    pub(super) fn loss(&self) -> Option<RemoteLoss> {
        self.router.loss()
    }

    /// See [`RemoteSessionRouter::has_stalled_prompt`].
    pub(super) fn has_stalled_prompt(&self, quiet_for: Duration) -> bool {
        self.router.has_stalled_prompt(quiet_for)
    }

    /// See [`RemoteSessionRouter::last_heard`].
    pub(super) fn last_heard(&self, session_id: &str) -> Option<Instant> {
        self.router.last_heard(session_id)
    }

    /// See [`RemoteSessionRouter::watch_foreign_prompts`].
    pub(super) fn watch_foreign_prompts(
        &self,
        owner_pid: u32,
        stop_reason: Option<&'static str>,
    ) -> oneshot::Receiver<()> {
        self.router.watch_foreign_prompts(owner_pid, stop_reason)
    }

    /// Ask the agent to cancel `session_id`'s turn, whoever prompted it.
    pub(super) fn cancel_session(&self, session_id: &str) {
        self.router.note_cancel();
        let _ = self
            .to_agent
            .send(ToAgent::Frame(session_cancel_frame(session_id)));
    }

    /// Before the proxy is stopped: wait (up to `limit`) until every frame
    /// queued for the agent has been written, and give `sq` a moment to
    /// relay a recent `session/cancel`, so stopping it cannot drop one.
    pub(super) async fn deliver_queued_frames(&self, limit: Duration) {
        let deadline = tokio::time::Instant::now() + limit;
        let (flushed, written) = oneshot::channel();
        if self.to_agent.send(ToAgent::Flush(flushed)).is_ok() {
            let _ = tokio::time::timeout_at(deadline, written).await;
        }
        let last_cancel_at = self.router.state().last_cancel_at;
        if let Some(relayed_by) = last_cancel_at.map(|at| at + CANCEL_RELAY_GRACE) {
            tokio::time::sleep_until(deadline.min(relayed_by.into())).await;
        }
    }

    /// Attach one ACP connection. It detaches once it stops writing.
    pub(super) fn attach(&self) -> RoutedClientStreams {
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let client = self.router.attach(inbound_tx);
        let (outgoing, from_client) = tokio::io::duplex(CLIENT_PIPE_BYTES);
        let (to_client, incoming) = tokio::io::duplex(CLIENT_PIPE_BYTES);
        tokio::spawn(forward_client_frames(
            Arc::clone(&self.router),
            client,
            from_client,
            self.to_agent.clone(),
        ));
        tokio::spawn(deliver_client_frames(inbound_rx, to_client));
        RoutedClientStreams { outgoing, incoming }
    }
}

async fn write_agent_stdin<W: AsyncWrite + Unpin>(
    mut stdin: W,
    mut frames: mpsc::UnboundedReceiver<ToAgent>,
) {
    while let Some(frame) = frames.recv().await {
        let line = match frame {
            ToAgent::Frame(line) => line,
            ToAgent::Flush(flushed) => {
                let _ = flushed.send(());
                continue;
            }
        };
        let mut bytes = line.into_bytes();
        bytes.push(b'\n');
        let written = async {
            stdin.write_all(&bytes).await?;
            stdin.flush().await
        };
        if let Err(error) = written.await {
            log::warn!("Failed to write to remote ACP stdin: {error}");
            return;
        }
    }
}

async fn read_agent_stdout<R: AsyncRead + Unpin>(
    stdout: R,
    router: Arc<RemoteSessionRouter>,
    output_end: Arc<Mutex<Option<String>>>,
    output_ended: CancellationToken,
) {
    let cause =
        match super::normalize_remote_acp_stdout(stdout, |line| router.incoming(&line)).await {
            Ok(()) => "the remote agent's output stream ended".to_string(),
            Err(error) => {
                log::error!("remote ACP stdout normalization failed: {error}");
                format!("reading the remote agent's output failed: {error}")
            }
        };
    *output_end.lock().unwrap() = Some(cause);
    output_ended.cancel();
}

async fn forward_client_frames(
    router: Arc<RemoteSessionRouter>,
    client: RemoteClientId,
    from_client: DuplexStream,
    to_agent: mpsc::UnboundedSender<ToAgent>,
) {
    let mut lines = BufReader::new(from_client).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) if line.trim().is_empty() => {}
            Ok(Some(line)) => {
                if to_agent
                    .send(ToAgent::Frame(router.outgoing(client, &line)))
                    .is_err()
                {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                log::warn!("Failed to read ACP client frame for the remote agent: {error}");
                break;
            }
        }
    }
    // Nobody is waiting on these sessions' turns any more. Unlike a process
    // of its own, the shared agent would keep serving them — using memory,
    // and perhaps committing to a branch Staged has given up on — until the
    // last session on it is done, which can be hours.
    for session_id in router.detach(client) {
        log::info!(
            "Cancelling remote ACP session {session_id}: its connection detached with a \
             prompt in flight"
        );
        router.note_cancel();
        let _ = to_agent.send(ToAgent::Frame(session_cancel_frame(&session_id)));
    }
}

/// A `session/cancel` notification for `session_id`.
pub(super) fn session_cancel_frame(session_id: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": {"sessionId": session_id},
    })
    .to_string()
}

async fn deliver_client_frames(
    mut inbound: mpsc::UnboundedReceiver<String>,
    mut to_client: DuplexStream,
) {
    while let Some(line) = inbound.recv().await {
        let mut bytes = line.into_bytes();
        bytes.push(b'\n');
        if to_client.write_all(&bytes).await.is_err() {
            return;
        }
    }
    let _ = to_client.shutdown().await;
}
