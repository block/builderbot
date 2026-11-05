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
//! would otherwise keep serving a turn nobody is waiting on. That holds when
//! the proxy is being stopped too, since `sq` leaves a process it reused
//! running; only once the agent is lost are such cancels pointless.
//!
//! Each client's frames queue for its connection, so one connection that
//! stops reading (its handler blocked on storage, say) holds up no other
//! session sharing the process. The queue is bounded, though: a connection
//! that falls [`MAX_CLIENT_BACKLOG_BYTES`] behind is cut off — its backlog
//! dropped, its stream ended, the turns it had in flight cancelled — rather
//! than left to grow the app's memory for as long as the agent writes.
//!
//! Responses to requests other app instances sent reach every attachment
//! too. They are dropped, except that one ending a prompt can be watched for
//! (see [`RemoteSessionRouter::watch_foreign_prompts`]): it is how a session
//! reattached to a dead instance's turn learns that turn has ended.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::{mpsc, oneshot, watch};
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

/// How far a client may fall behind the agent's output — bytes routed to it
/// that its connection has not yet taken — before it is cut off. The shared
/// proxy has to keep reading for the other sessions, so a connection that
/// stops reading gets no backpressure to `sq`; without a bound its frames
/// would queue for as long as the agent writes. Far beyond the bursts a
/// connection that is reading lets build up.
#[cfg(not(test))]
pub(super) const MAX_CLIENT_BACKLOG_BYTES: usize = 64 * 1024 * 1024;
#[cfg(test)]
pub(super) const MAX_CLIENT_BACKLOG_BYTES: usize = 256 * 1024;

/// How long `sq` is given to relay a `session/cancel` written to its stdin
/// before it may be interrupted: it drops input it has not yet sent on to
/// the orchestrator when it stops.
#[cfg(not(test))]
const CANCEL_RELAY_GRACE: Duration = Duration::from_secs(1);
#[cfg(test)]
const CANCEL_RELAY_GRACE: Duration = Duration::from_millis(300);

const SESSION_NEW_METHOD: &str = "session/new";
const SESSION_PROMPT_METHOD: &str = "session/prompt";
const SESSION_CANCEL_METHOD: &str = "session/cancel";
const CANCEL_REQUEST_METHOD: &str = "$/cancel_request";

/// The stop reason the agent answers a cancelled `session/prompt` with.
pub(super) const CANCELLED_STOP_REASON: &str = "cancelled";

/// Demultiplexes one agent process's frames across the clients sharing it.
pub(super) struct RemoteSessionRouter {
    state: Mutex<RouterState>,
}

#[derive(Default)]
struct RouterState {
    /// Where each attached client's frames are delivered.
    clients: HashMap<RemoteClientId, ClientOutbox>,
    /// Clients cut off for falling behind, and why (see
    /// [`RouterState::cut_off`]). Kept after they detach, so their sessions
    /// can still tell why their streams ended.
    cut_off: HashMap<RemoteClientId, String>,
    /// Sessions whose client was cut off with a prompt in flight, still to
    /// be cancelled (see [`RemoteSessionRouter::incoming`]).
    to_cancel: Vec<String>,
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
    /// The agent's output has ended, or the proxy is being stopped; nothing
    /// more will be routed.
    closed: bool,
    /// How the attached sessions lost their agent, when the router closed
    /// other than by [`RemoteSessionRouter::close`].
    loss: Option<RemoteLoss>,
    /// Watches for the end of prompts another app instance sent.
    foreign_prompt_watches: Vec<ForeignPromptWatch>,
    /// When a `session/cancel` was last queued for the agent.
    last_cancel_at: Option<Instant>,
    /// Clients let go of on purpose with their turns left running on the
    /// agent (see [`RemoteSessionRouter::keep`]): detaching them cancels
    /// nothing.
    kept: HashSet<RemoteClientId>,
}

/// Answers `ended` with the stop reason once a response ending a prompt
/// whose request id starts with `id_prefix` arrives (with stop reason
/// `stop_reason`, when set). Dropped once its receiver is.
struct ForeignPromptWatch {
    id_prefix: String,
    stop_reason: Option<&'static str>,
    ended: oneshot::Sender<String>,
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

/// The router's end of one client's frame queue.
struct ClientOutbox {
    frames: mpsc::UnboundedSender<String>,
    /// Bytes of the frames queued that the client has not yet taken.
    backlog: Arc<AtomicUsize>,
    /// Cancelled when the client is cut off.
    cut_off: CancellationToken,
}

/// Where the frames routed to one client wait for its connection to take
/// them.
pub(super) struct ClientInbox {
    frames: mpsc::UnboundedReceiver<String>,
    backlog: Arc<AtomicUsize>,
    cut_off: CancellationToken,
}

impl ClientInbox {
    /// The next frame routed to the client, or `None` once the router has
    /// let it go (closed, or cut it off) and nothing is left queued.
    pub(super) async fn recv(&mut self) -> Option<String> {
        let line = self.frames.recv().await?;
        Some(self.taken(line))
    }

    /// The next frame already routed to the client, if any.
    #[cfg(test)]
    pub(super) fn try_recv(&mut self) -> Option<String> {
        let line = self.frames.try_recv().ok()?;
        Some(self.taken(line))
    }

    fn taken(&self, line: String) -> String {
        self.backlog.fetch_sub(line.len(), Ordering::Relaxed);
        line
    }

    /// Cancelled once the client is cut off for falling behind: what is
    /// still queued for it is dropped rather than delivered.
    pub(super) fn cut_off(&self) -> &CancellationToken {
        &self.cut_off
    }
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

    /// Register a client; frames routed to it queue in the returned inbox.
    /// Once the router is closed, the inbox's queue is dropped at once, so
    /// the client sees end-of-stream instead of waiting on output that will
    /// never come.
    #[cfg(test)]
    pub(super) fn attach(&self) -> (RemoteClientId, ClientInbox) {
        self.attach_owning(None)
    }

    /// [`Self::attach`] for a client that owns `session_id` from the start,
    /// claimed in the same step: a reattach to a turn still running, whose
    /// frames the agent relays from the moment the proxy connects, while the
    /// session's `session/load` — which would otherwise claim it — waits
    /// behind `initialize`. The connection holds them until the load is out
    /// (see `AcpNotificationHandler::admit`).
    pub(super) fn attach_owning(&self, session_id: Option<&str>) -> (RemoteClientId, ClientInbox) {
        let client = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
        let (frames, inbound) = mpsc::unbounded_channel();
        let backlog = Arc::new(AtomicUsize::new(0));
        let cut_off = CancellationToken::new();
        let inbox = ClientInbox {
            frames: inbound,
            backlog: Arc::clone(&backlog),
            cut_off: cut_off.clone(),
        };
        let mut state = self.state();
        if !state.closed {
            state.clients.insert(
                client,
                ClientOutbox {
                    frames,
                    backlog,
                    cut_off,
                },
            );
            if let Some(session_id) = session_id {
                state.claim(session_id, client);
            }
        }
        (client, inbox)
    }

    /// Forget a client along with its sessions and in-flight requests,
    /// returning the sessions it still had a prompt in flight on, unless the
    /// agent was lost (see [`Self::lose`]). A router closed on purpose (see
    /// [`Self::close`]) still names them: the proxy is being stopped, and
    /// `sq` leaves a process it reused running, so they need cancelling.
    pub(super) fn detach(&self, client: RemoteClientId) -> Vec<String> {
        let mut state = self.state();
        state.clients.remove(&client);
        let kept = state.kept.remove(&client);
        let abandoned = state.forget(client);
        if !state.session_new_pending() {
            state.drop_early_frames();
        }
        if kept {
            Vec::new()
        } else {
            abandoned
        }
    }

    /// Let `client` go while leaving its turns running on the agent: its
    /// stream ends now, so its connection sees end-of-stream, and once that
    /// connection stops writing [`Self::detach`] cancels nothing for it. How
    /// a Blox session whose turn should outlive its connection lets go (see
    /// [`super::DetachHandle`]).
    pub(super) fn keep(&self, client: RemoteClientId) {
        let mut state = self.state();
        state.kept.insert(client);
        state.clients.remove(&client);
    }

    /// Why `client` was cut off for falling behind the agent's output, if it
    /// was (see [`MAX_CLIENT_BACKLOG_BYTES`]).
    pub(super) fn cut_off(&self, client: RemoteClientId) -> Option<String> {
        self.state().cut_off.get(&client).cloned()
    }

    /// Drop every client's delivery channel, so each sees end-of-stream,
    /// because the proxy is being stopped.
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

    /// Answered with the stop reason once the agent answers a
    /// `session/prompt` that the app instance with pid `owner_pid` sent
    /// through some other connection — a request this router never saw,
    /// whose response the agent broadcasts to every attachment — and, when
    /// `stop_reason` is set, ends it for that reason. Prompts this router
    /// sent never count.
    pub(super) fn watch_foreign_prompts(
        &self,
        owner_pid: u32,
        stop_reason: Option<&'static str>,
    ) -> oneshot::Receiver<String> {
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
    /// that session as the client's. A client cut off for falling behind has
    /// been detached from the agent: nothing it sends is written (`None`).
    pub(super) fn outgoing(&self, client: RemoteClientId, line: &str) -> Option<String> {
        let mut state = self.state();
        if state.cut_off.contains_key(&client) {
            return None;
        }
        Some(state.outgoing(client, line))
    }

    /// Deliver one frame from the agent to the client it belongs to,
    /// returning the sessions to cancel on the agent: those a client cut off
    /// for falling behind had a prompt in flight on.
    pub(super) fn incoming(&self, line: &str) -> Vec<String> {
        let mut state = self.state();
        state.incoming(line);
        std::mem::take(&mut state.to_cancel)
    }
}

impl RouterState {
    fn outgoing(&mut self, client: RemoteClientId, line: &str) -> String {
        let Ok(Value::Object(mut frame)) = serde_json::from_str::<Value>(line) else {
            return line.to_string();
        };
        if let Some(session_id) = session_id_in(&frame, "params") {
            self.claim(session_id, client);
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
                self.pending.insert(
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
                if !self.pending.contains_key(&wire_id) {
                    return line.to_string();
                }
                *request_id = Value::String(wire_id);
            }
            // Noted, so that stopping the proxy right after gives `sq` time
            // to relay it (see `RemoteTransport::deliver_queued_frames`).
            (Some(SESSION_CANCEL_METHOD), None) => {
                self.last_cancel_at = Some(Instant::now());
                return line.to_string();
            }
            // Notifications and answers to the agent's own requests pass
            // through untouched.
            _ => return line.to_string(),
        }
        serde_json::to_string(&frame).unwrap_or_else(|_| line.to_string())
    }

    fn incoming(&mut self, line: &str) {
        let Ok(Value::Object(frame)) = serde_json::from_str::<Value>(line) else {
            log::warn!("Dropping remote ACP frame that is not a JSON-RPC object");
            return;
        };
        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            self.route_response(frame);
            return;
        };
        let is_request = frame.get("id").is_some_and(|id| !id.is_null());
        match session_id_in(&frame, "params") {
            Some(session_id) => {
                self.last_frame_at
                    .insert(session_id.to_string(), Instant::now());
                self.route_session_frame(session_id, line);
            }
            None if is_request => self.route_sessionless_request(method, line),
            None => {
                let clients: Vec<_> = self.clients.keys().copied().collect();
                for client in clients {
                    self.deliver(client, line.to_string());
                }
            }
        }
    }

    /// Forget `client`'s sessions and in-flight requests, returning the
    /// sessions it had a prompt in flight on, unless the agent was lost.
    fn forget(&mut self, client: RemoteClientId) -> Vec<String> {
        self.owners.retain(|session_id, owner| {
            let keep = *owner != client;
            if !keep {
                self.last_frame_at.remove(session_id);
            }
            keep
        });
        let mut abandoned = Vec::new();
        self.pending.retain(|_, pending| {
            if pending.client != client {
                return true;
            }
            if let Some(session_id) = pending.prompts.take() {
                abandoned.push(session_id);
            }
            false
        });
        if self.loss.is_some() {
            abandoned.clear();
        }
        abandoned.sort();
        abandoned.dedup();
        abandoned
    }

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
                let _ = watch.ended.send(stop_reason.to_string());
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
        let Some(outbox) = self.clients.get(&client) else {
            return;
        };
        let queued = outbox.backlog.fetch_add(line.len(), Ordering::Relaxed);
        // A client that has taken everything gets the frame, however large.
        let backlog = queued + line.len();
        if queued > 0 && backlog > MAX_CLIENT_BACKLOG_BYTES {
            self.cut_off(client, backlog);
            return;
        }
        if outbox.frames.send(line).is_err() {
            self.clients.remove(&client);
        }
    }

    /// Cut off `client`, which has fallen `backlog` bytes behind: drop what
    /// is queued for it and end its stream, forget its sessions — so the
    /// agent's further frames for them are dropped as nobody's — and queue
    /// cancels for the turns it had in flight, which nobody is following any
    /// more. What it sends from now on is not written to the agent.
    fn cut_off(&mut self, client: RemoteClientId, backlog: usize) {
        if let Some(outbox) = self.clients.remove(&client) {
            outbox.cut_off.cancel();
        }
        let abandoned = self.forget(client);
        let cause = format!(
            "its connection stopped taking the remote agent's output and fell {:.1} MiB \
             behind, so it was disconnected from the agent{}",
            backlog as f64 / (1024.0 * 1024.0),
            if abandoned.is_empty() {
                ""
            } else {
                " and its turn there cancelled"
            }
        );
        log::warn!(
            "Cutting off remote ACP client {client} (sessions with a prompt in flight: \
             {abandoned:?}): {cause}"
        );
        self.cut_off.insert(client, cause);
        self.to_cancel.extend(abandoned);
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
    /// How many attached connections are still forwarding frames to the
    /// agent: one that is going away may still have a frame on its way.
    forwarding: Arc<watch::Sender<usize>>,
}

/// What is queued for the agent's stdin.
enum ToAgent {
    Frame(String),
    /// Answered once every frame queued before it has been written.
    Flush(oneshot::Sender<()>),
    /// Close the agent's stdin once every frame queued before it has been
    /// written.
    Close,
}

/// One client's ends of a [`RemoteTransport`], for `ByteStreams`.
pub(super) struct RoutedClientStreams {
    /// The client these streams are, for [`RemoteTransport::cut_off`].
    pub(super) client: RemoteClientId,
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
            to_agent.clone(),
            Arc::clone(&output_end),
            output_ended.clone(),
        ));
        Self {
            router,
            to_agent,
            output_ended,
            output_end,
            closed: CancellationToken::new(),
            forwarding: Arc::new(watch::Sender::new(0)),
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

    /// See [`RemoteSessionRouter::cut_off`].
    pub(super) fn cut_off(&self, client: RemoteClientId) -> Option<String> {
        self.router.cut_off(client)
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
    ) -> oneshot::Receiver<String> {
        self.router.watch_foreign_prompts(owner_pid, stop_reason)
    }

    /// See [`RemoteSessionRouter::keep`].
    pub(super) fn keep_client(&self, client: RemoteClientId) {
        self.router.keep(client);
    }

    /// Ask the agent to cancel `session_id`'s turn, whoever prompted it.
    /// The cancel is queued on the agent's stdin directly, so it is written
    /// even when no connection is left to send it.
    pub(super) fn cancel_session(&self, session_id: &str) {
        queue_cancel(&self.router, &self.to_agent, session_id);
    }

    /// Close the agent's stdin once the frames already queued are written.
    /// `sq` exits once its stdin closes; nothing written after reaches it.
    pub(super) fn close_agent_input(&self) {
        let _ = self.to_agent.send(ToAgent::Close);
    }

    /// Before the proxy is stopped: wait (up to `limit`) until the
    /// connections going away have forwarded their last frames — such as a
    /// stopped session's `session/cancel`, or the cancels detaching queues
    /// (see [`RemoteSessionRouter::detach`]) — and every frame queued for
    /// the agent has been written, then give `sq` a moment to relay a recent
    /// `session/cancel`, so stopping it cannot drop one.
    pub(super) async fn deliver_queued_frames(&self, limit: Duration) {
        let deadline = tokio::time::Instant::now() + limit;
        let mut forwarding = self.forwarding.subscribe();
        let _ = tokio::time::timeout_at(deadline, forwarding.wait_for(|count| *count == 0)).await;
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
    #[cfg(test)]
    pub(super) fn attach(&self) -> RoutedClientStreams {
        self.attach_owning(None)
    }

    /// [`Self::attach`] for a connection that owns `session_id` from the
    /// start (see [`RemoteSessionRouter::attach_owning`]).
    pub(super) fn attach_owning(&self, session_id: Option<&str>) -> RoutedClientStreams {
        let (client, inbox) = self.router.attach_owning(session_id);
        let (outgoing, from_client) = tokio::io::duplex(CLIENT_PIPE_BYTES);
        let (to_client, incoming) = tokio::io::duplex(CLIENT_PIPE_BYTES);
        self.forwarding.send_modify(|count| *count += 1);
        let forwarding = ForwardingGuard(Arc::clone(&self.forwarding));
        let router = Arc::clone(&self.router);
        let to_agent = self.to_agent.clone();
        tokio::spawn(async move {
            forward_client_frames(router, client, from_client, to_agent).await;
            drop(forwarding);
        });
        tokio::spawn(deliver_client_frames(inbox, to_client));
        RoutedClientStreams {
            client,
            outgoing,
            incoming,
        }
    }
}

/// Counts a connection as forwarding until dropped.
struct ForwardingGuard(Arc<watch::Sender<usize>>);

impl Drop for ForwardingGuard {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count -= 1);
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
            ToAgent::Close => {
                let _ = stdin.shutdown().await;
                break;
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
            break;
        }
    }
    drop(stdin);
    // Whatever is sent from now on goes nowhere. Taking it, rather than
    // dropping the queue, keeps the connections attached — instead of each
    // failing on its next frame — until the proxy ends their streams saying
    // how the agent went.
    while let Some(frame) = frames.recv().await {
        if let ToAgent::Flush(flushed) = frame {
            let _ = flushed.send(());
        }
    }
}

async fn read_agent_stdout<R: AsyncRead + Unpin>(
    stdout: R,
    router: Arc<RemoteSessionRouter>,
    to_agent: mpsc::UnboundedSender<ToAgent>,
    output_end: Arc<Mutex<Option<String>>>,
    output_ended: CancellationToken,
) {
    let route = |line: String| {
        // Turns a client cut off for falling behind had in flight: nobody
        // follows them any more.
        for session_id in router.incoming(&line) {
            queue_cancel(&router, &to_agent, &session_id);
        }
    };
    let cause = match super::normalize_remote_acp_stdout(stdout, route).await {
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
                // A client cut off for falling behind keeps being read, so
                // its writes don't block, but nothing it sends goes on.
                let Some(line) = router.outgoing(client, &line) else {
                    continue;
                };
                if to_agent.send(ToAgent::Frame(line)).is_err() {
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
        queue_cancel(&router, &to_agent, &session_id);
    }
}

/// Queue a `session/cancel` for `session_id` on the agent's stdin.
fn queue_cancel(
    router: &RemoteSessionRouter,
    to_agent: &mpsc::UnboundedSender<ToAgent>,
    session_id: &str,
) {
    router.note_cancel();
    let _ = to_agent.send(ToAgent::Frame(session_cancel_frame(session_id)));
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

/// Write the frames routed to one client into its connection's pipe. Once
/// the client is cut off, the frames still queued are dropped and the pipe
/// with them: the connection reads what the pipe holds, then end-of-stream.
async fn deliver_client_frames(mut inbox: ClientInbox, mut to_client: DuplexStream) {
    let cut_off = inbox.cut_off().clone();
    loop {
        let line = tokio::select! {
            biased;
            _ = cut_off.cancelled() => return,
            line = inbox.recv() => line,
        };
        let Some(line) = line else {
            break;
        };
        let mut bytes = line.into_bytes();
        bytes.push(b'\n');
        let written = tokio::select! {
            biased;
            _ = cut_off.cancelled() => return,
            written = to_client.write_all(&bytes) => written,
        };
        if written.is_err() {
            return;
        }
    }
    let _ = to_client.shutdown().await;
}
