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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::mpsc;
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

const SESSION_NEW_METHOD: &str = "session/new";
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
    /// The agent's output has ended; nothing more will be routed.
    closed: bool,
}

struct PendingRequest {
    client: RemoteClientId,
    original_id: Value,
    opens_session: bool,
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

    /// Forget a client along with its sessions and in-flight requests.
    pub(super) fn detach(&self, client: RemoteClientId) {
        let mut state = self.state();
        state.clients.remove(&client);
        state.owners.retain(|_, owner| *owner != client);
        state.pending.retain(|_, pending| pending.client != client);
        if !state.session_new_pending() {
            state.drop_early_frames();
        }
    }

    /// Drop every client's delivery channel, so each sees end-of-stream.
    pub(super) fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        state.clients.clear();
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
                state.pending.insert(
                    wire_id.clone(),
                    PendingRequest {
                        client,
                        original_id,
                        opens_session: method == SESSION_NEW_METHOD,
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
            Some(session_id) => state.route_session_frame(session_id, line),
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
    format!("staged-{}-{client}:{original}", std::process::id())
}

/// One agent process's stdio, routed: clients attach to get their own
/// streams for an ACP connection.
#[derive(Clone)]
pub(super) struct RemoteTransport {
    router: Arc<RemoteSessionRouter>,
    to_agent: mpsc::UnboundedSender<String>,
    /// Cancelled once the agent's stdout has ended and every client has been
    /// told so.
    closed: CancellationToken,
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
        let closed = CancellationToken::new();
        tokio::spawn(write_agent_stdin(stdin, to_agent_rx));
        tokio::spawn(read_agent_stdout(
            stdout,
            Arc::clone(&router),
            closed.clone(),
        ));
        Self {
            router,
            to_agent,
            closed,
        }
    }

    /// Cancelled once the agent's stdout has ended.
    pub(super) fn closed(&self) -> &CancellationToken {
        &self.closed
    }

    /// End every attached client's stream.
    pub(super) fn close(&self) {
        self.router.close();
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
    mut frames: mpsc::UnboundedReceiver<String>,
) {
    while let Some(line) = frames.recv().await {
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
    closed: CancellationToken,
) {
    if let Err(error) =
        super::normalize_remote_acp_stdout(stdout, |line| router.incoming(&line)).await
    {
        log::error!("remote ACP stdout normalization failed: {error}");
    }
    router.close();
    closed.cancel();
}

async fn forward_client_frames(
    router: Arc<RemoteSessionRouter>,
    client: RemoteClientId,
    from_client: DuplexStream,
    to_agent: mpsc::UnboundedSender<String>,
) {
    let mut lines = BufReader::new(from_client).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) if line.trim().is_empty() => {}
            Ok(Some(line)) => {
                if to_agent.send(router.outgoing(client, &line)).is_err() {
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
    router.detach(client);
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
