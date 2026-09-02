//! Full-featured ACP driver for session management and streaming.
//!
//! This module provides the complete ACP integration including:
//! - Session initialization and resumption
//! - Streaming text and tool calls
//! - Permission handling
//! - Remote workspace support via Blox
//! - Cancellation support

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use agent_client_protocol::{
    schema::{
        v1::{
            AgentCapabilities, AgentNotification, AuthMethod, AuthenticateRequest,
            CancelNotification, ClientCapabilities, ContentBlock as AcpContentBlock, ContentChunk,
            ExtNotification, ImageContent, Implementation, InitializeRequest, InitializeResponse,
            LoadSessionRequest, McpCapabilities, McpServer, Meta, NewSessionRequest,
            PermissionOption as SchemaPermissionOption, PermissionOptionId,
            PermissionOptionKind as SchemaPermissionOptionKind, PromptRequest, PromptResponse,
            RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
            SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption,
            SessionConfigOptionCategory, SessionConfigSelectOptions, SessionInfoUpdate,
            SessionModeState, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
            StopReason, TextContent, ToolCallContent,
        },
        ProtocolVersion,
    },
    Agent, ByteStreams, Client, ConnectionTo, JsonRpcMessage, UntypedMessage,
};
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
use nix::sys::signal::{self, Signal};
#[cfg(unix)]
use nix::unistd::Pid;

use crate::types::blox_acp_command;

static PERMISSION_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

// =============================================================================
// Public traits and types
// =============================================================================

/// Protocol-agnostic message writer — streams agent output.
///
/// This trait allows different storage backends (database, in-memory, etc.)
/// to receive streaming agent output without coupling to the ACP protocol.
#[async_trait]
pub trait MessageWriter: Send + Sync {
    /// Append a text chunk to the current assistant message.
    async fn append_text(&self, text: &str);

    /// Flush all buffered text and close the current message block.
    async fn finalize(&self);

    /// Record a tool call with its ID, title, and optional raw input parameters.
    async fn record_tool_call(
        &self,
        tool_call_id: &str,
        title: &str,
        raw_input: Option<&serde_json::Value>,
    );

    /// Update a previously recorded tool call's title and/or raw input.
    ///
    /// When `title` is `None`, the implementation should preserve the
    /// existing title while updating only `raw_input`.
    async fn update_tool_call_title(
        &self,
        tool_call_id: &str,
        title: Option<&str>,
        raw_input: Option<&serde_json::Value>,
    );

    /// Record the result/output of a tool call.
    async fn record_tool_result(&self, tool_call_id: &str, content: &str);

    /// Called when session info is updated (title, timestamps, etc.).
    ///
    /// Delivered via `SessionUpdate::SessionInfoUpdate` notifications during a
    /// session, or extracted from setup responses.
    async fn on_session_info_update(&self, _info: &SessionInfoUpdate) {}

    /// Called when mode state is received from session setup responses.
    ///
    /// `SessionModeState` is delivered in `NewSessionResponse` and
    /// `LoadSessionResponse`. Mid-session mode/model changes are surfaced through
    /// `on_config_option_update` via `ConfigOptionUpdate` with category `Model`.
    async fn on_model_state_update(&self, _state: &SessionModeState) {}

    /// Called when session configuration options change.
    async fn on_config_option_update(&self, _options: &[SessionConfigOption]) {}

    /// Called after ACP initialization has negotiated agent capabilities.
    async fn on_initialize(&self, _metadata: &AcpInitializeMetadata) {}

    /// Attach rich ACP tool-call metadata to an existing tool-call row.
    async fn record_tool_call_metadata(&self, _metadata: AcpToolCallMetadata) {}

    /// Persist an ACP event that does not map cleanly to a visible transcript row.
    async fn record_acp_event_metadata(&self, _metadata: AcpEventMetadata) {}

    /// Ask the client UI to resolve an ACP permission request.
    ///
    /// Implementations that cannot prompt should automatically approve the
    /// request unless the prompt turn has been cancelled.
    async fn request_permission(
        &self,
        request: AcpPermissionRequest,
        cancel_token: CancellationToken,
    ) -> AcpPermissionDecision {
        if cancel_token.is_cancelled() {
            AcpPermissionDecision::Cancelled
        } else {
            autoapprove_permission_decision(&request)
        }
    }
}

fn permission_option_is_approval(option: &AcpPermissionOption) -> bool {
    match option.kind.approval_status() {
        Some(is_approval) => is_approval,
        None => legacy_permission_option_is_approval(option),
    }
}

fn legacy_permission_option_is_approval(option: &AcpPermissionOption) -> bool {
    let option_id = option.option_id.to_ascii_lowercase();
    let name = option.name.to_ascii_lowercase();

    option_id.starts_with("allow")
        || option_id.starts_with("approve")
        || name.contains("allow")
        || name.contains("approve")
}

/// The safe answer to a permission request this client cannot correlate to
/// state it knows, or one it must resolve without asking: the request's
/// rejection option, or `Cancelled` when it offers none.
///
/// Never blocks and never approves — an id we can't account for
/// ([claude-agent-acp#851]) must not be able to authorize a tool call.
///
/// [claude-agent-acp#851]: https://github.com/agentclientprotocol/claude-agent-acp/issues/851
fn defensive_permission_decision(request: &AcpPermissionRequest) -> AcpPermissionDecision {
    request
        .options
        .iter()
        .find(|option| {
            matches!(
                option.kind,
                AcpPermissionOptionKind::RejectOnce | AcpPermissionOptionKind::RejectAlways
            )
        })
        .map(|option| AcpPermissionDecision::Selected {
            option_id: option.option_id.clone(),
        })
        .unwrap_or(AcpPermissionDecision::Cancelled)
}

/// How long an out-of-turn permission request waits for its own `tool_call`
/// update to land before the missing announcement is treated as the #851
/// desync. The two frames are sent back-to-back by the agent, so this only
/// has to cover dispatch reordering — not human latency.
const PERMISSION_ANNOUNCEMENT_GRACE: Duration = Duration::from_millis(500);

pub fn autoapprove_permission_decision(request: &AcpPermissionRequest) -> AcpPermissionDecision {
    request
        .options
        .iter()
        .find(|option| permission_option_is_approval(option))
        .or_else(|| request.options.first())
        .map(|option| AcpPermissionDecision::Selected {
            option_id: option.option_id.clone(),
        })
        .unwrap_or(AcpPermissionDecision::Selected {
            option_id: "approve".to_string(),
        })
}

#[derive(Debug, Clone, Default)]
pub struct AcpInitializeMetadata {
    pub protocol_version: String,
    pub agent_capabilities: Option<serde_json::Value>,
    pub auth_methods: Option<serde_json::Value>,
    pub agent_info: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct AcpToolCallMetadata {
    pub event_kind: Option<String>,
    pub message_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_kind: Option<String>,
    pub tool_status: Option<String>,
    pub raw_input: Option<serde_json::Value>,
    pub raw_output: Option<serde_json::Value>,
    pub content: Option<serde_json::Value>,
    pub locations: Option<serde_json::Value>,
    /// Attribution for records that did not come from a live user turn — see
    /// [`background_continuation_origin`]. `None` is a live turn.
    pub origin: Option<String>,
}

impl AcpToolCallMetadata {
    fn has_update_fields(&self) -> bool {
        self.tool_kind.is_some()
            || self.tool_status.is_some()
            || self.raw_input.is_some()
            || self.raw_output.is_some()
            || self.content.is_some()
            || self.locations.is_some()
    }
}

#[derive(Debug, Clone, Default)]
pub struct AcpEventMetadata {
    pub event_kind: Option<String>,
    pub message_id: Option<String>,
    pub content: Option<serde_json::Value>,
    pub usage: Option<serde_json::Value>,
    /// Attribution for records that did not come from a live user turn — see
    /// [`background_continuation_origin`]. `None` is a live turn.
    pub origin: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AcpPermissionRequest {
    pub request_id: String,
    pub session_id: String,
    pub tool_call_id: String,
    pub tool_title: Option<String>,
    pub tool_kind: Option<String>,
    pub tool_status: Option<String>,
    pub raw_input: Option<serde_json::Value>,
    pub raw_output: Option<serde_json::Value>,
    pub content: Option<serde_json::Value>,
    pub locations: Option<serde_json::Value>,
    pub options: Vec<AcpPermissionOption>,
    pub raw_request: Option<serde_json::Value>,
    /// Attribution when the request arrived *outside* a live user turn — see
    /// [`background_continuation_origin`]. Clients must present such a request
    /// as a background continuation's, not the already-finished turn's. `None`
    /// is a live turn.
    pub origin: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AcpPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: AcpPermissionOptionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpPermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Unknown,
}

impl AcpPermissionOptionKind {
    fn approval_status(self) -> Option<bool> {
        match self {
            Self::AllowOnce | Self::AllowAlways => Some(true),
            Self::RejectOnce | Self::RejectAlways => Some(false),
            Self::Unknown => None,
        }
    }
}

impl From<SchemaPermissionOptionKind> for AcpPermissionOptionKind {
    fn from(kind: SchemaPermissionOptionKind) -> Self {
        match kind {
            SchemaPermissionOptionKind::AllowOnce => Self::AllowOnce,
            SchemaPermissionOptionKind::AllowAlways => Self::AllowAlways,
            SchemaPermissionOptionKind::RejectOnce => Self::RejectOnce,
            SchemaPermissionOptionKind::RejectAlways => Self::RejectAlways,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpPermissionDecision {
    Selected { option_id: String },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpSessionConfigOptionSelection {
    pub category: SessionConfigOptionCategory,
    pub config_id: String,
    pub value_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayBoundary {
    pub role: String,
    pub content: String,
    pub acp_message_id: Option<String>,
    pub acp_tool_call_id: Option<String>,
}

impl ReplayBoundary {
    pub fn legacy(role: String, content: String) -> Self {
        Self {
            role,
            content,
            acp_message_id: None,
            acp_tool_call_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRunOutcome {
    Completed,
    Cancelled,
}

impl AgentRunOutcome {
    fn from_stop_reason(stop_reason: StopReason) -> Self {
        match stop_reason {
            StopReason::Cancelled => Self::Cancelled,
            StopReason::EndTurn
            | StopReason::MaxTokens
            | StopReason::MaxTurnRequests
            | StopReason::Refusal => Self::Completed,
            _ => Self::Completed,
        }
    }
}

/// Attribution stamped on every record produced while the connection is
/// holding open for background work ([`SessionLifetime::BackgroundHolding`])
/// instead of serving a live user turn.
///
/// Recorded on [`AcpEventMetadata::origin`], [`AcpToolCallMetadata::origin`]
/// and [`AcpPermissionRequest::origin`] so a continuation is never presented
/// as part of the turn that preceded it (or the one that follows it).
pub const BACKGROUND_CONTINUATION_ORIGIN: &str = "background-continuation";

/// Origin tag for a background continuation, refined with the Claude bridge's
/// own `origin.kind` when a raw `_claude/sdkMessage` frame revealed it (e.g.
/// `background-continuation:task-notification` for an autonomous cycle woken
/// by a settled background task).
///
/// The bare [`BACKGROUND_CONTINUATION_ORIGIN`] is the phase-only attribution:
/// the update arrived out-of-turn, but nothing on the wire named the cycle.
pub fn background_continuation_origin(origin_kind: Option<&str>) -> String {
    match origin_kind {
        Some(kind) => format!("{BACKGROUND_CONTINUATION_ORIGIN}:{kind}"),
        None => BACKGROUND_CONTINUATION_ORIGIN.to_string(),
    }
}

/// Longest task-name segment [`labeled_background_continuation_origin`] will
/// put in an origin tag.
///
/// The name is the agent's, not ours: for a background shell the bridge falls
/// back to the spawn's description, which is the *command* — arbitrarily long
/// and often multi-line. This value is persisted as an attribution column, so
/// it gets bounded rather than embedding a whole script in every row a
/// continuation writes.
const ORIGIN_TASK_NAME_MAX_CHARS: usize = 64;

/// Reduce a task name to a single-line, length-bounded label safe to persist
/// inside an origin tag.
///
/// Runs of whitespace — including the newlines and indentation a multi-line
/// shell command carries — collapse to single spaces, and the result is
/// truncated on a char boundary with a trailing ellipsis so a clipped label
/// reads as clipped. `None` when nothing is left: an all-whitespace name
/// labels nothing, so the tag stays at its unlabeled form.
fn origin_task_name_label(name: &str) -> Option<String> {
    let collapsed: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let mut label: String = collapsed.chars().take(ORIGIN_TASK_NAME_MAX_CHARS).collect();
    if collapsed.chars().count() > ORIGIN_TASK_NAME_MAX_CHARS {
        label.push('…');
    }
    Some(label)
}

/// [`background_continuation_origin`], additionally labeled with the name of
/// the task that woke the cycle when the connection knows it — typed
/// asyncTasks announce every spawn with a `name`, and the task most recently
/// settled during a hold is what a `task-notification` wake reports on. The
/// label only ever extends that one kind: no other cycle kind is woken by a
/// settled task, and consumers of the persisted tag match on the
/// [`BACKGROUND_CONTINUATION_ORIGIN`] prefix, so the suffix stays additive.
///
/// The name is sanitized and bounded first — see [`origin_task_name_label`].
pub fn labeled_background_continuation_origin(
    origin_kind: Option<&str>,
    woke_task_name: Option<&str>,
) -> String {
    match (origin_kind, woke_task_name.and_then(origin_task_name_label)) {
        (Some(TASK_NOTIFICATION_ORIGIN), Some(label)) => {
            format!("{BACKGROUND_CONTINUATION_ORIGIN}:{TASK_NOTIFICATION_ORIGIN}:{label}")
        }
        _ => background_continuation_origin(origin_kind),
    }
}

/// What to do with a `session/request_permission` that arrives while the
/// connection is holding for background work — i.e. a request belonging to a
/// background continuation rather than to any live user turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutOfTurnPermissionPolicy {
    /// Present it like any other permission request, attributed to the
    /// background continuation, and wait for the client's decision.
    #[default]
    Prompt,
    /// Resolve it immediately by selecting the request's approval option,
    /// without prompting.
    AutoAllow,
    /// Resolve it immediately by selecting the request's rejection option
    /// (falling back to `Cancelled` when it offers none), without prompting.
    ///
    /// The safe setting for an unattended session: nothing can wedge waiting
    /// on a decision no one is there to make.
    AutoDeny,
}

/// Configuration for the post-turn background hold.
///
/// When set on a driver (see [`AcpDriver::with_background_hold`]), a
/// [`SessionConnection`] does not tear the agent down the moment a prompt
/// resolves. It holds the connection open — so out-of-turn `session/update`s
/// from background shells keep flowing — until the live background-task set
/// drains (plus a debounce) or one of the hard stop conditions fires. `None`
/// (the default) preserves the legacy teardown-immediately behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundHoldConfig {
    /// Hard cap on how long a session may hold after a turn resolves.
    ///
    /// Bounds every hold regardless of signals — background shells can hang
    /// forever, so an unbounded wait is never safe. On expiry the session
    /// tears down flagged [`SessionSettleReason::HeldUntilCap`]. Defaults to
    /// 10 minutes, matching the Bash tool's own max timeout so a single
    /// blocking shell can't outlive its own ceiling by much.
    pub hold_cap: Duration,
    /// Quiet window that must elapse — no notifications at all — with the
    /// task set empty before the session is declared quiescent, once a
    /// background task has been seen on the connection.
    ///
    /// This is only ever a secondary confirmation on an already-drained task
    /// set, never the sole teardown gate: a background shell can be silent
    /// for minutes, so time-since-last-update alone says nothing.
    pub debounce: Duration,
    /// The shorter quiet window used when the connection never saw a
    /// background task at all.
    ///
    /// A turn that started no background work has nothing to drain — the
    /// full [`BackgroundHoldConfig::debounce`] would be a flat tax on every
    /// such turn (terminal status, commit detection, note generation all wait
    /// on it). What remains worth absorbing is frame-ordering slop around the
    /// prompt resolving, which ~a second covers.
    pub taskless_debounce: Duration,
    /// Quiet window after which a busy idle-latch reading goes stale and
    /// stops blocking quiescence.
    ///
    /// A `Busy` reading blocks quiescence — it is the direct signal that a
    /// continuation cycle is running after the task set has emptied — but its
    /// release rides a single trailing `idle` frame: lose that frame and the
    /// latch would read busy forever, condemning every later hold on the
    /// connection to the full [`BackgroundHoldConfig::hold_cap`]. So a busy
    /// reading only blocks while the connection shows signs of life: once
    /// this much time passes with no notifications at all, the reading is
    /// stale — a live continuation streams *something* (message chunks, tool
    /// calls, state transitions) well within this window — and quiescence
    /// proceeds on the ordinary debounce terms. Never shortens the active
    /// debounce (the longer of the two windows applies), and the cap still
    /// bounds everything.
    pub idle_latch_staleness: Duration,
    /// How permission requests that arrive during the hold are resolved.
    ///
    /// Only reachable with a hold configured: without one the agent is gone
    /// before it could ask anything out-of-turn.
    pub out_of_turn_permissions: OutOfTurnPermissionPolicy,
}

impl Default for BackgroundHoldConfig {
    fn default() -> Self {
        Self {
            hold_cap: Duration::from_secs(600),
            debounce: Duration::from_secs(10),
            taskless_debounce: Duration::from_secs(1),
            idle_latch_staleness: Duration::from_secs(120),
            out_of_turn_permissions: OutOfTurnPermissionPolicy::default(),
        }
    }
}

/// Presentational snapshot of a connection's post-turn background hold,
/// published to a [`BackgroundHoldObserver`] so a client can show the wait.
///
/// Only [`SessionLifetime::BackgroundHolding`] has a sub-state of its own: the
/// session stays *running* while it holds (holding is not a terminal status),
/// so a client renders "waiting on background task (N)" with a stop
/// affordance instead of its usual running indicator. Every other lifetime
/// state is already covered by the session's own status — `TurnLive` is
/// running, `Quiescent`/`TornDown` complete, `Cancelled` cancelled, `Failed`
/// an error — and reports the cleared default here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackgroundHoldStatus {
    /// Whether the connection is holding open past turn end right now.
    pub holding: bool,
    /// Live background tasks the agent is reporting (the count a client shows
    /// alongside the wait). Always 0 when not holding.
    pub live_tasks: usize,
    /// The live tasks by name, when the connection can name them — typed
    /// asyncTasks announce each spawn with its metadata, so typed mode renders
    /// one row per task (each stoppable via
    /// [`SessionConnection::stop_async_task`]). Raw mode only ever knows
    /// opaque task ids, so it keeps the bare count and this stays empty.
    /// Always empty when not holding.
    pub tasks: Vec<BackgroundHoldTask>,
}

impl BackgroundHoldStatus {
    /// The hold status for a lifetime state and the current live-task signal.
    pub fn for_lifetime(
        lifetime: SessionLifetime,
        live_tasks: usize,
        tasks: Vec<BackgroundHoldTask>,
    ) -> Self {
        match lifetime {
            SessionLifetime::BackgroundHolding => Self {
                holding: true,
                live_tasks,
                tasks,
            },
            _ => Self::default(),
        }
    }
}

/// One live background task in a [`BackgroundHoldStatus`], as announced by a
/// typed `async_task_spawned` update. The id keys a per-task stop
/// ([`SessionConnection::stop_async_task`]); the rest is presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundHoldTask {
    /// The agent's task id (`asyncTaskId` on the wire).
    pub id: String,
    /// Human-readable task name from the spawn announcement.
    pub name: Option<String>,
    pub description: Option<String>,
    /// File the task's output streams into, when the agent reports one.
    pub output_file_path: Option<String>,
}

/// Callback invoked with every [`BackgroundHoldStatus`] change on a
/// connection, so a client can surface the wait while it lasts.
///
/// Called from the connection task, which runs on the caller's `LocalSet`
/// thread: keep it cheap and non-blocking (emit an event, don't do I/O).
pub type BackgroundHoldObserver = Arc<dyn Fn(BackgroundHoldStatus) + Send + Sync>;

/// Session-connection lifetime states.
///
/// `TurnLive` and `BackgroundHolding` are the two live states; the rest are
/// terminal (except `Quiescent`, which either tears down immediately or
/// re-arms back into holding when a task starts during the transition).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLifetime {
    /// A `session/prompt` is in flight.
    TurnLive,
    /// The prompt resolved but the connection is held open for background
    /// work (out-of-turn continuations).
    BackgroundHolding,
    /// The background-task set drained and the debounce elapsed — safe to
    /// tear down.
    Quiescent,
    /// The agent was (or is being) torn down deliberately.
    TornDown,
    /// The user closed the session.
    Cancelled,
    /// The connection failed (protocol error, or the agent process exited).
    Failed,
}

impl SessionLifetime {
    /// Legal transitions of the session-lifetime state machine.
    pub fn can_transition_to(self, next: SessionLifetime) -> bool {
        use SessionLifetime::*;
        match self {
            TurnLive => matches!(next, BackgroundHolding | TornDown | Cancelled | Failed),
            BackgroundHolding => {
                matches!(next, TurnLive | Quiescent | TornDown | Cancelled | Failed)
            }
            // Quiescent tears down immediately, unless a `task_started`
            // re-arms the hold in the transition window.
            Quiescent => matches!(next, BackgroundHolding | TornDown),
            TornDown | Cancelled | Failed => false,
        }
    }
}

/// Record (and debug-log) a lifetime transition for a session connection.
fn transition_lifetime(
    session_id: &str,
    current: &mut Option<SessionLifetime>,
    to: SessionLifetime,
) {
    match *current {
        Some(from) if from == to => return,
        Some(from) => {
            debug_assert!(
                from.can_transition_to(to),
                "illegal session lifetime transition {from:?} -> {to:?}"
            );
            log::debug!("ACP session {session_id}: lifetime {from:?} -> {to:?}");
        }
        None => log::debug!("ACP session {session_id}: lifetime -> {to:?}"),
    }
    *current = Some(to);
}

/// Why a session connection settled (tore down).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSettleReason {
    /// Teardown immediately after the prompt resolved — background holding
    /// disabled, the legacy per-turn behavior.
    Immediate,
    /// The background task set drained and the debounce elapsed.
    Quiescent,
    /// The hard hold cap expired before quiescence could be confirmed; the
    /// wait was truncated and any still-running background work abandoned.
    HeldUntilCap,
    /// The post-turn hold was stopped before quiescence — the user closed
    /// the session mid-hold, or the agent process exited under it. The turn
    /// itself had already completed, so its outcome stands; only the *wait*
    /// for its background work was truncated, like [`Self::HeldUntilCap`].
    HoldStopped,
    /// The user cancelled the session before its turn completed (during
    /// setup, replay, or the prompt itself). A cancel that lands during the
    /// post-turn hold settles [`Self::HoldStopped`] instead.
    Cancelled,
    /// The connection failed (protocol error, or the agent process exited).
    Failed,
}

/// Event emitted exactly once by the connection task when the session
/// settles: the connection has left its turn/holding states and the agent has
/// been torn down (writer finalized, child stopped).
///
/// Callers must gate post-completion work (commit detection, note generation)
/// on this event rather than on [`SessionConnection::prompt`] resolving: with
/// background holding enabled, the prompt resolves while the connection may
/// still be serving out-of-turn background work.
#[derive(Debug, Clone)]
pub struct SessionSettled {
    /// Final connection outcome. [`SessionSettled::fold_turn_result`] merges
    /// it with a per-turn result.
    pub outcome: Result<AgentRunOutcome, String>,
    pub reason: SessionSettleReason,
}

impl SessionSettled {
    /// Merge a turn's own result with this settled outcome: a hold that was
    /// cancelled or failed *after* the turn completed overrides the completed
    /// turn, while a turn's own failure or cancellation is never upgraded.
    pub fn fold_turn_result(
        &self,
        turn_result: Result<AgentRunOutcome, String>,
    ) -> Result<AgentRunOutcome, String> {
        match (turn_result, &self.outcome) {
            (Err(e), _) => Err(e),
            (Ok(AgentRunOutcome::Cancelled), _) => Ok(AgentRunOutcome::Cancelled),
            (Ok(AgentRunOutcome::Completed), Ok(outcome)) => Ok(*outcome),
            (Ok(AgentRunOutcome::Completed), Err(e)) => Err(e.clone()),
        }
    }
}

fn serialize_as_string<T: serde::Serialize>(value: &T) -> Option<String> {
    match serde_json::to_value(value).ok()? {
        serde_json::Value::String(s) => Some(s),
        other => Some(other.to_string()),
    }
}

fn serialize_non_empty<T: serde::Serialize>(items: &[T]) -> Option<serde_json::Value> {
    if items.is_empty() {
        None
    } else {
        serde_json::to_value(items).ok()
    }
}

fn serialize_value<T: serde::Serialize>(value: &T) -> Option<serde_json::Value> {
    serde_json::to_value(value).ok()
}

/// Storage interface for persisting agent session data.
///
/// This trait abstracts the storage backend, allowing different implementations
/// (SQLite, PostgreSQL, in-memory, etc.) without changing the driver logic.
#[async_trait]
pub trait Store: Send + Sync {
    /// Save the agent's session ID for resumption.
    fn set_agent_session_id(&self, session_id: &str, agent_session_id: &str) -> Result<(), String>;

    /// Retrieve existing visible session messages as `(role, content)` pairs.
    ///
    /// This is the legacy replay fallback for stores that do not expose ACP
    /// message IDs via [`Store::get_session_replay_boundaries`].
    fn get_session_messages(&self, _session_id: &str) -> Result<Vec<(String, String)>, String> {
        Ok(vec![])
    }

    /// Retrieve replay boundaries for session resumption.
    ///
    /// Implementations should prefer ACP message IDs and tool-call IDs from
    /// persisted metadata when available, while still including visible
    /// transcript rows as a fallback.
    fn get_session_replay_boundaries(
        &self,
        session_id: &str,
    ) -> Result<Vec<ReplayBoundary>, String> {
        self.get_session_messages(session_id).map(|messages| {
            messages
                .into_iter()
                .map(|(role, content)| ReplayBoundary::legacy(role, content))
                .collect()
        })
    }
}

/// Everything needed to run one turn of an agent.
///
/// Implementors own the protocol details (spawning a process, connecting,
/// sending the prompt, translating streaming events into [`MessageWriter`]
/// calls).
#[async_trait(?Send)]
#[allow(clippy::too_many_arguments)]
pub trait AgentDriver {
    /// Run a single turn: send `prompt`, stream results via `writer`.
    ///
    /// `images` contains `(base64_data, mime_type)` pairs that are sent as
    /// `ContentBlock::Image` entries alongside the text prompt.
    async fn run(
        &self,
        session_id: &str,
        prompt: &str,
        images: &[(String, String)],
        working_dir: &Path,
        store: &Arc<dyn Store>,
        writer: &Arc<dyn MessageWriter>,
        cancel_token: &CancellationToken,
        agent_session_id: Option<&str>,
        config_options: &[AcpSessionConfigOptionSelection],
    ) -> Result<AgentRunOutcome, String>;
}

// =============================================================================
// AcpDriver — the main driver implementation
// =============================================================================

pub struct AcpDriver {
    binary_path: PathBuf,
    acp_args: Vec<String>,
    agent_label: String,
    /// When true, this driver proxies through a remote Blox workspace.
    is_remote: bool,
    /// Extra environment variables to pass to the agent process.
    extra_env: Vec<(String, String)>,
    /// Captured interactive-login-shell environment for local sessions.
    ///
    /// When set, the agent binary is spawned *directly* with this environment
    /// (env-cleared first, then these vars applied — the same Hermit-activated
    /// snapshot the caller's `ShellEnvCache` hands pipeline steps and git ops)
    /// instead of launching an interactive `$SHELL -ils` and piping `exec
    /// <binary>` to it. This keeps the agent, its per-command shells, pipeline
    /// steps, and git ops all drawing from one env so their resolved
    /// toolchains can't diverge, and skips the per-session interactive-shell
    /// spawn. `None` (remote sessions, or when the caller couldn't capture a
    /// snapshot) falls back to the `$SHELL -ils` + `exec` spawn.
    env_snapshot: Option<Vec<(String, String)>>,
    /// Captured home/global environment used only to resolve env-shebang
    /// interpreters for bridge startup.
    ///
    /// Npm-installed ACP bridges commonly use `#!/usr/bin/env node`. The
    /// bridge must start with the user's global Node, not a repo-local Hermit
    /// Node that happens to appear first in the working-directory snapshot.
    /// When this snapshot can resolve the interpreter, the driver launches the
    /// bridge as `<interpreter> <bridge>` while still giving the child process
    /// the working-directory `env_snapshot`.
    interpreter_env_snapshot: Option<Vec<(String, String)>>,
    /// MCP servers to inject into the session via NewSessionRequest.
    /// These are *required*: if the agent doesn't support a server's transport,
    /// the session fails.
    mcp_servers: Vec<McpServer>,
    /// Override the working directory sent to the remote agent.
    /// When set, this path is used in the `NewSessionRequest` instead of the
    /// local `working_dir` passed to `run()`. This is needed because the
    /// local `working_dir` is a fallback path on the host machine, while the
    /// remote agent needs the actual workspace path (e.g. `/home/bloxer/cash-server`).
    remote_working_dir: Option<PathBuf>,
    /// Post-turn background hold. `None` (default) tears the agent down the
    /// moment each prompt resolves — the legacy behavior. See
    /// [`BackgroundHoldConfig`].
    background_hold: Option<BackgroundHoldConfig>,
    /// Notified whenever a connection enters, updates, or leaves the post-turn
    /// hold, so the client can present the wait. Only ever called with a
    /// `background_hold` configured.
    background_hold_observer: Option<BackgroundHoldObserver>,
}

const REMOTE_ACP_MAX_PENDING_LINE_BYTES: usize = 256 * 1024;
const ACP_SETUP_TIMEOUT: Duration = Duration::from_secs(90);

impl AcpDriver {
    /// Create a driver for the given provider ID (e.g. "goose", "claude").
    ///
    /// Looks up the agent in `KNOWN_AGENTS`, locates the binary on disk,
    /// and returns a ready-to-use driver.
    pub fn new(provider_id: &str) -> Result<Self, String> {
        crate::types::find_acp_agent_by_id(provider_id)
            .map(|agent| Self {
                binary_path: agent.binary_path,
                acp_args: agent.acp_args,
                agent_label: agent.label,
                is_remote: false,
                extra_env: Vec::new(),
                env_snapshot: None,
                interpreter_env_snapshot: None,
                mcp_servers: Vec::new(),
                remote_working_dir: None,
                background_hold: None,
                background_hold_observer: None,
            })
            .ok_or_else(|| format!("Unknown or unavailable agent provider: {provider_id}"))
    }

    /// Create a driver for the first available provider.
    pub fn first_available() -> Result<Self, String> {
        crate::types::find_acp_agent()
            .map(|agent| Self {
                binary_path: agent.binary_path,
                acp_args: agent.acp_args,
                agent_label: agent.label,
                is_remote: false,
                extra_env: Vec::new(),
                env_snapshot: None,
                interpreter_env_snapshot: None,
                mcp_servers: Vec::new(),
                remote_working_dir: None,
                background_hold: None,
                background_hold_observer: None,
            })
            .ok_or_else(|| {
                "No ACP agent found. Install Goose, Claude Code, Codex, Pi, or Amp and ensure it's on your PATH."
                    .to_string()
            })
    }

    pub(crate) fn from_agent(agent: &crate::types::AcpAgent) -> Self {
        Self {
            binary_path: agent.binary_path.clone(),
            acp_args: agent.acp_args.clone(),
            agent_label: agent.label.clone(),
            is_remote: false,
            extra_env: Vec::new(),
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: Vec::new(),
            remote_working_dir: None,
            background_hold: None,
            background_hold_observer: None,
        }
    }

    /// Create a driver that proxies through `sq blox acp <workspace>`.
    pub fn for_workspace(workspace_name: &str, agent_id: Option<&str>) -> Result<Self, String> {
        let binary_path = blox_cli::find_sq_binary().ok_or_else(|| {
            "Could not find `sq` binary. Install it and ensure it's on your PATH.".to_string()
        })?;

        let command = agent_id.and_then(blox_acp_command);
        let args = blox_cli::acp_proxy_args(workspace_name, command.as_deref());

        Ok(Self {
            binary_path,
            acp_args: args,
            agent_label: "Blox".to_string(),
            is_remote: true,
            extra_env: Vec::new(),
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: Vec::new(),
            remote_working_dir: None,
            background_hold: None,
            background_hold_observer: None,
        })
    }

    /// Set extra environment variables to pass to the agent process.
    pub fn with_extra_env(mut self, vars: Vec<(String, String)>) -> Self {
        self.extra_env = vars;
        self
    }

    /// Set the captured interactive-login-shell environment for a local session.
    ///
    /// `vars` is a fully-captured env snapshot (e.g. from the caller's
    /// `ShellEnvCache`, the same one pipeline steps and git ops draw from).
    /// When provided, [`AgentDriver::run`] spawns the agent binary directly
    /// with this environment instead of launching an interactive `$SHELL -ils`
    /// and `exec`-ing the binary — so the agent and its per-command shells
    /// resolve the same Hermit-activated toolchain as everything else, without
    /// paying the per-session shell-spawn cost. Ignored for remote sessions.
    pub fn with_env_snapshot(mut self, vars: Vec<(String, String)>) -> Self {
        if !self.is_remote {
            self.env_snapshot = Some(vars);
        }
        self
    }

    /// Set the captured home/global environment for env-shebang interpreter lookup.
    ///
    /// This does not become the agent process environment. It is consulted only
    /// to resolve launchers such as `#!/usr/bin/env node` to an explicit
    /// interpreter path; the agent still receives the working-directory
    /// environment from [`Self::with_env_snapshot`].
    pub fn with_interpreter_env_snapshot(mut self, vars: Vec<(String, String)>) -> Self {
        if !self.is_remote {
            self.interpreter_env_snapshot = Some(vars);
        }
        self
    }

    /// Set MCP servers to inject into the session via `NewSessionRequest` or `LoadSessionRequest`.
    pub fn with_mcp_servers(mut self, servers: Vec<McpServer>) -> Self {
        self.mcp_servers = servers;
        self
    }

    /// Set the working directory for the remote agent.
    ///
    /// For remote sessions, the `working_dir` passed to `run()` is used as
    /// `current_dir` for spawning the local proxy process. This field
    /// overrides the directory sent to the remote agent in the
    /// `NewSessionRequest`, so the agent operates in the correct repo
    /// directory on the workspace.
    pub fn with_remote_working_dir(mut self, dir: PathBuf) -> Self {
        self.remote_working_dir = Some(dir);
        self
    }

    /// Configure the post-turn background hold.
    ///
    /// `Some(config)` keeps each [`SessionConnection`] alive after a prompt
    /// resolves until its background tasks drain (or a hard stop fires);
    /// `None` (the default) preserves the legacy behavior of tearing the
    /// agent down the moment each prompt resolves.
    pub fn with_background_hold(mut self, config: Option<BackgroundHoldConfig>) -> Self {
        self.background_hold = config;
        self
    }

    /// Observe the post-turn hold so the client can present it.
    ///
    /// The observer is called on entry to the hold, whenever the live
    /// background-task count changes during it, and once more with the cleared
    /// default when the hold ends (a new turn, teardown, cancel, or child
    /// exit). Without a [`BackgroundHoldConfig`] there is no hold, so it is
    /// never called.
    pub fn with_background_hold_observer(
        mut self,
        observer: Option<BackgroundHoldObserver>,
    ) -> Self {
        self.background_hold_observer = observer;
        self
    }
}

/// Shell-escape a value by wrapping it in single quotes with interior quotes
/// escaped via the standard `'\''` trick.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn env_shebang_interpreter_from_snapshot(
    binary_path: &Path,
    snapshot: Option<&[(String, String)]>,
) -> Option<PathBuf> {
    let env = doctor::DoctorEnv::new(snapshot?.to_vec());
    let path_value = env.get("PATH")?;
    doctor::resolve::resolve_env_shebang_interpreter_from_path(binary_path, path_value)
}

fn shell_path_guard_for_agent_binary(binary_path: &Path) -> Option<String> {
    let launcher = doctor::resolve::env_shebang_launcher(binary_path)?;

    let quoted_interpreter = shell_quote(&launcher.interpreter);
    let quoted_bin_dir = shell_quote(&launcher.bin_dir.to_string_lossy());
    let path_assignment = if doctor::resolve::is_broad_toolchain_dir(&launcher.bin_dir) {
        format!(
            "if [ -n \"$PATH\" ]; then PATH=\"$PATH\":{quoted_bin_dir}; else PATH={quoted_bin_dir}; fi"
        )
    } else {
        format!(
            "if [ -n \"$PATH\" ]; then PATH={quoted_bin_dir}:\"$PATH\"; else PATH={quoted_bin_dir}; fi"
        )
    };

    Some(format!(
        "command -v {quoted_interpreter} >/dev/null 2>&1 || {{ {path_assignment}; export PATH; }}; "
    ))
}

fn shell_exec_line(
    binary_path: &Path,
    acp_args: &[String],
    interpreter_env_snapshot: Option<&[(String, String)]>,
) -> String {
    let quoted_binary = shell_quote(&binary_path.to_string_lossy());
    let quoted_args = acp_args
        .iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ");

    if let Some(interpreter) =
        env_shebang_interpreter_from_snapshot(binary_path, interpreter_env_snapshot)
    {
        let quoted_interpreter = shell_quote(&interpreter.to_string_lossy());
        if quoted_args.is_empty() {
            return format!("exec {quoted_interpreter} {quoted_binary}\n");
        }
        return format!("exec {quoted_interpreter} {quoted_binary} {quoted_args}\n");
    }

    let guard = shell_path_guard_for_agent_binary(binary_path).unwrap_or_default();

    format!("{guard}exec {quoted_binary} {quoted_args}\n")
}

#[derive(Debug)]
pub(crate) struct AcpSpawnCommand {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
    pub(crate) uses_explicit_interpreter: bool,
}

pub(crate) fn acp_spawn_command(
    binary_path: &Path,
    acp_args: &[String],
    interpreter_env_snapshot: Option<&[(String, String)]>,
) -> AcpSpawnCommand {
    if let Some(interpreter) =
        env_shebang_interpreter_from_snapshot(binary_path, interpreter_env_snapshot)
    {
        let mut args = vec![binary_path.as_os_str().to_os_string()];
        args.extend(acp_args.iter().map(OsString::from));
        return AcpSpawnCommand {
            program: interpreter,
            args,
            uses_explicit_interpreter: true,
        };
    }

    AcpSpawnCommand {
        program: binary_path.to_path_buf(),
        args: acp_args.iter().map(OsString::from).collect(),
        uses_explicit_interpreter: false,
    }
}

fn resolve_spawn_working_dir(working_dir: &Path, is_remote: bool) -> PathBuf {
    // Remote ACP sessions proxy through `sq blox acp` and don't execute against
    // the local filesystem. Use a guaranteed-existing cwd when the recorded
    // local fallback path doesn't exist, otherwise spawn fails with ENOENT.
    if is_remote && !working_dir.is_dir() {
        return std::env::temp_dir();
    }
    working_dir.to_path_buf()
}

fn absolute_local_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|e| {
            format!(
                "Failed to resolve absolute ACP cwd for {}: {e}",
                path.display()
            )
        })
}

fn resolve_acp_working_dir(
    working_dir: &Path,
    is_remote: bool,
    remote_working_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    if is_remote {
        let remote_dir = remote_working_dir.ok_or_else(|| {
            "Remote ACP sessions require an absolute remote working directory; could not resolve the workspace repo path"
                .to_string()
        })?;
        if !remote_dir.is_absolute() {
            return Err(format!(
                "Remote ACP working directory must be absolute, got {}",
                remote_dir.display()
            ));
        }
        return Ok(remote_dir.to_path_buf());
    }

    absolute_local_path(working_dir)
}

/// Result of one prompt turn — and, while teardown still follows every turn,
/// of the whole connection.
type SessionTurnResult = Result<AgentRunOutcome, String>;

/// A prompt turn queued onto a [`SessionConnection`].
struct QueuedSessionTurn {
    prompt: String,
    images: Vec<(String, String)>,
    /// Resolved by the connection task with the turn's outcome.
    reply: oneshot::Sender<SessionTurnResult>,
}

/// A per-task stop queued onto a [`SessionConnection`]'s hold-control channel,
/// served by the post-turn holding wait (see
/// [`SessionConnection::stop_async_task`]).
struct StopAsyncTaskRequest {
    /// The agent's task id (`asyncTaskId` on the wire).
    task_id: String,
    /// Resolved with the agent's `{stopped}` answer, or an error when the
    /// request could not be served (no hold to serve it, or the connection
    /// tore down first).
    reply: oneshot::Sender<Result<bool, String>>,
}

/// Cloneable, thread-safe handle for stopping one background task on a held
/// [`SessionConnection`] — the piece a client registers where its own
/// stop-task command can reach it (the connection itself lives on the
/// session's `LocalSet` thread and is busy serving the session).
#[derive(Clone)]
pub struct AsyncTaskStopHandle {
    hold_control_tx: mpsc::UnboundedSender<StopAsyncTaskRequest>,
    /// Whether the connection's post-turn holding wait is currently serving
    /// the hold-control channel. Nothing else ever drains it, so a request
    /// sent while this reads `false` would sit queued — pending, unanswered —
    /// until the next hold boundary or teardown; [`AsyncTaskStopHandle::stop`]
    /// rejects it immediately instead. Advisory: a request racing a hold
    /// boundary still lands in the queue and is answered by that boundary's
    /// rejection drain.
    hold_active: Arc<AtomicBool>,
}

impl AsyncTaskStopHandle {
    /// Stop one background task without cancelling the session. See
    /// [`SessionConnection::stop_async_task`] for the semantics.
    pub async fn stop(&self, task_id: &str) -> Result<bool, String> {
        if !self.hold_active.load(Ordering::Relaxed) {
            return Err("Async task stop not served: no background hold is active".to_string());
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        self.hold_control_tx
            .send(StopAsyncTaskRequest {
                task_id: task_id.to_string(),
                reply: reply_tx,
            })
            .map_err(|_| "ACP connection is no longer running".to_string())?;
        reply_rx
            .await
            .map_err(|_| "ACP connection closed before the task stop resolved".to_string())?
    }
}

/// A session-scoped connection to an ACP agent.
///
/// Owns the agent (bridge) child process and the JSON-RPC connection through
/// a background task spawned by [`AcpDriver::connect`], hoisting the
/// connection lifetime above individual prompt turns: turns are queued with
/// [`SessionConnection::prompt`] instead of each spawning its own process.
///
/// Without a [`BackgroundHoldConfig`] on the driver, the connection task
/// finalizes the writer and gracefully stops the child immediately after each
/// prompt resolves — the exact teardown ordering the old per-turn
/// [`AgentDriver::run`] had — so a connection serves a single turn. With one,
/// the connection *holds* after each completed turn (see
/// [`SessionLifetime::BackgroundHolding`]) so background shells' out-of-turn
/// continuations survive turn end; the [`SessionSettled`] event (via
/// [`SessionConnection::take_settled_receiver`]) marks actual teardown.
pub struct SessionConnection {
    prompt_tx: mpsc::UnboundedSender<QueuedSessionTurn>,
    /// Per-task stops queued for the post-turn holding wait (see
    /// [`SessionConnection::stop_async_task`]).
    hold_control_tx: mpsc::UnboundedSender<StopAsyncTaskRequest>,
    /// Set by the holding wait while it serves the hold-control channel; a
    /// stop issued outside that window is rejected immediately (see
    /// [`AsyncTaskStopHandle`]).
    hold_active: Arc<AtomicBool>,
    /// Handle to the connection task; awaited (once) to surface the session
    /// result when a turn's reply channel is dropped without an answer.
    connection_task: Option<tokio::task::JoinHandle<SessionTurnResult>>,
    /// Cached connection-task result so repeated callers see the same outcome.
    exit_result: Option<SessionTurnResult>,
    /// Receiver for the connection's one settled event; handed out (once) via
    /// [`SessionConnection::take_settled_receiver`].
    settled_rx: Option<oneshot::Receiver<SessionSettled>>,
}

impl SessionConnection {
    /// Stop one background task, by id, without cancelling the session.
    ///
    /// Sends the typed asyncTasks extension's `_session/async_task/stop`
    /// request; the agent stops that task alone, publishes its terminal
    /// `stopped` state, and the hold then settles on its own quiescence path —
    /// a clean [`SessionSettleReason::Quiescent`] once the rest of the set
    /// drains, not a truncated wait. Contrast the session-level cancel, which
    /// stops the *wait* (and every task with it) and settles
    /// [`SessionSettleReason::HoldStopped`].
    ///
    /// Returns the agent's own `{stopped}` answer: `false` means the agent
    /// didn't stop it (unknown id, already terminal, or a stop already in
    /// flight). Requests are served only while the connection is holding for
    /// background work — the only window a client shows per-task stops. A
    /// stop issued while no hold is active is rejected immediately (never
    /// queued), and one still queued when a hold boundary passes is answered
    /// with an error rather than left to act on a later hold.
    pub async fn stop_async_task(&self, task_id: &str) -> Result<bool, String> {
        self.async_task_stop_handle().stop(task_id).await
    }

    /// A cloneable, thread-safe handle to [`SessionConnection::stop_async_task`],
    /// for callers that route the stop from another thread (a UI command).
    pub fn async_task_stop_handle(&self) -> AsyncTaskStopHandle {
        AsyncTaskStopHandle {
            hold_control_tx: self.hold_control_tx.clone(),
            hold_active: Arc::clone(&self.hold_active),
        }
    }

    /// Take the receiver for the connection's [`SessionSettled`] event.
    ///
    /// The event fires exactly once, after the agent has been torn down.
    /// Callers that run post-completion work must await it rather than treat
    /// a resolved [`SessionConnection::prompt`] as terminal — under a
    /// background hold the prompt resolves while the connection is still
    /// alive. Returns `None` if the receiver was already taken.
    pub fn take_settled_receiver(&mut self) -> Option<oneshot::Receiver<SessionSettled>> {
        self.settled_rx.take()
    }

    /// Send one prompt turn and await its outcome.
    ///
    /// Without a background hold, the returned future resolves only after the
    /// connection task has torn the agent down (writer finalized, child
    /// stopped) — the same point at which the old per-turn
    /// [`AgentDriver::run`] returned, so callers can treat a resolved turn as
    /// fully settled. With a background hold, it resolves as soon as the
    /// turn's stop reason arrives while the connection holds for background
    /// work; teardown is signalled separately by the [`SessionSettled`]
    /// event.
    pub async fn prompt(
        &mut self,
        prompt: &str,
        images: &[(String, String)],
    ) -> Result<AgentRunOutcome, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let turn = QueuedSessionTurn {
            prompt: prompt.to_string(),
            images: images.to_vec(),
            reply: reply_tx,
        };
        if self.prompt_tx.send(turn).is_err() {
            // The connection task already exited (e.g. setup failed before
            // this turn was queued); its exit result is the turn result.
            return self.wait_for_exit().await;
        }
        match reply_rx.await {
            Ok(result) => result,
            // The task tore down without answering this turn (setup failure,
            // or cancellation before the turn was received).
            Err(_) => self.wait_for_exit().await,
        }
    }

    async fn wait_for_exit(&mut self) -> Result<AgentRunOutcome, String> {
        if self.exit_result.is_none() {
            let result = match self.connection_task.take() {
                Some(task) => task
                    .await
                    .unwrap_or_else(|e| Err(format!("ACP connection task failed: {e}"))),
                None => Err("ACP connection task result already consumed".to_string()),
            };
            self.exit_result = Some(result);
        }
        self.exit_result
            .clone()
            .expect("exit_result populated above")
    }
}

impl AcpDriver {
    /// Open a session-scoped connection to the agent.
    ///
    /// Spawns the agent child process and a connection task that owns both
    /// the child and the ACP connection for the life of the session. Prompt
    /// turns are then sent with [`SessionConnection::prompt`].
    ///
    /// Must be called from within a Tokio `LocalSet`: the ACP connection uses
    /// `!Send` futures, and the connection task is spawned with
    /// [`tokio::task::spawn_local`].
    #[allow(clippy::too_many_arguments)]
    pub async fn connect(
        &self,
        session_id: &str,
        working_dir: &Path,
        store: &Arc<dyn Store>,
        writer: &Arc<dyn MessageWriter>,
        cancel_token: &CancellationToken,
        agent_session_id: Option<&str>,
        config_options: &[AcpSessionConfigOptionSelection],
    ) -> Result<SessionConnection, String> {
        let spawn_working_dir = resolve_spawn_working_dir(working_dir, self.is_remote);
        let acp_working_dir = resolve_acp_working_dir(
            working_dir,
            self.is_remote,
            self.remote_working_dir.as_deref(),
        )?;
        if self.is_remote && spawn_working_dir.as_path() != working_dir {
            log::warn!(
                "Remote ACP spawn cwd missing ({}); falling back to {}",
                working_dir.display(),
                spawn_working_dir.display()
            );
        }

        // Local sessions need Hermit (and similar directory-based shell hooks)
        // to have activated before the agent binary runs, so the agent — and
        // every per-command shell it later spawns — resolves the same
        // toolchain as pipeline steps and git ops.
        //
        // The reliable way to get that is to apply a captured
        // interactive-login-shell env snapshot (`env_snapshot`, produced by the
        // caller's `ShellEnvCache` — the very snapshot pipeline steps and git
        // ops use) directly to the agent binary. That starting env is the
        // single determinant for everything the agent runs, so a Hermit-first
        // snapshot propagates into its per-command shells too. It also skips
        // the per-session shell spawn entirely.
        //
        // When no snapshot is supplied (remote sessions, or a capture failure
        // upstream) we fall back to the older dance: spawn an interactive login
        // shell with `-s` (stdin mode) in the working directory with a clean
        // environment. The shell initialises fully (`.zshrc` installs hooks),
        // `precmd` fires in the working directory (activating Hermit), then we
        // write an `exec <binary>` command to stdin. `exec` replaces the shell
        // with the agent binary so all subsequent stdin/stdout traffic is the
        // JSON-RPC protocol. This path is fragile (it depends on `precmd`
        // firing inside an attached interactive shell), which is exactly why
        // the snapshot path is preferred when available.
        let use_shell_spawn = !self.is_remote && self.env_snapshot.is_none();

        let mut cmd = if use_shell_spawn {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
            let mut c = Command::new(&shell);
            c.current_dir(&spawn_working_dir) // start in project dir so precmd sees hermit config
                .env_clear() // clean slate — shell init rebuilds the environment
                .env("HOME", std::env::var("HOME").unwrap_or_default())
                .env("USER", std::env::var("USER").unwrap_or_default())
                .env("SHELL", &shell)
                .arg("-i") // interactive: ensures hooks like precmd/chpwd are installed
                .arg("-l") // login: loads full profile / environment
                .arg("-s"); // read commands from stdin (after init completes)
            c
        } else {
            let spawn_command = acp_spawn_command(
                &self.binary_path,
                &self.acp_args,
                self.interpreter_env_snapshot.as_deref(),
            );
            let mut c = Command::new(&spawn_command.program);
            c.args(&spawn_command.args).current_dir(&spawn_working_dir);
            if !self.is_remote {
                if let Some(ref snapshot) = self.env_snapshot {
                    // Local session with a captured env: start the agent from the
                    // Hermit-activated snapshot. env_clear first so nothing from
                    // Staged's own (possibly Homebrew-first) environment leaks in,
                    // then apply the captured vars — mirroring `ShellEnv::apply_to`.
                    c.env_clear();
                    let mut snapshot_path: Option<String> = None;
                    for (k, v) in snapshot {
                        if k == "PATH" {
                            snapshot_path = Some(v.clone());
                        }
                        c.env(k, v);
                    }
                    // Preserve the captured PATH unless an env-shebang launcher
                    // (for example `#!/usr/bin/env node`) cannot find its
                    // interpreter from the snapshot. Only then add the agent bin
                    // dir as a targeted fallback; broad toolchain dirs are appended
                    // so they cannot jump ahead of Hermit or other project-managed
                    // paths.
                    if !spawn_command.uses_explicit_interpreter {
                        let existing_path = snapshot_path.as_deref().unwrap_or_default();
                        if let Some(new_path) =
                            doctor::resolve::guarded_path_for_env_shebang_launcher(
                                &self.binary_path,
                                existing_path,
                            )
                        {
                            c.env("PATH", new_path);
                        }
                    }
                }
            }
            c
        };

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Pipe stderr and log it. Shell init failures (Hermit activation
            // errors, .zshrc syntax errors) and — crucially — a failed shebang
            // (`env: node: No such file or directory` when the agent binary's
            // interpreter isn't on PATH) all surface here. Without this the agent
            // dies before the `initialize` response and the only symptom is an
            // opaque "server shut down unexpectedly".
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Put remote proxies in their own process group so we can send
        // SIGINT to the entire group (sq + its child processes) for graceful
        // shutdown. We must NOT do this for local interactive shells because
        // process_group(0) detaches the child from the controlling terminal,
        // which breaks zsh's job-control / precmd hooks — the shell either
        // hangs or exits immediately without running `exec`.
        if self.is_remote {
            #[cfg(unix)]
            cmd.process_group(0);
        }
        // extra_env is applied last so per-session overrides always win: over
        // the shell's clean environment (shell-spawn path), over the captured
        // snapshot (local direct-spawn path), and over the inherited
        // environment (remote spawns).
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "Failed to spawn {} (binary: {}, cwd: {}): {e}",
                self.agent_label,
                self.binary_path.display(),
                spawn_working_dir.display()
            )
        })?;

        // Fires when the agent process is observed to be gone (stderr EOF).
        // The post-turn holding wait keys its involuntary-exit trigger off
        // this: a dead bridge otherwise looks identical to a quiet one, and
        // the hold would burn its full cap waiting on a process that can
        // never report its background tasks again.
        let child_exited = CancellationToken::new();

        // Drain the agent's stderr to the log so spawn/shebang failures are
        // visible instead of vanishing into a generic "server shut down".
        if let Some(stderr) = child.stderr.take() {
            let agent_label = self.agent_label.clone();
            let session_id = session_id.to_string();
            let exited = child_exited.clone();
            tokio::task::spawn_local(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.trim().is_empty() {
                        continue;
                    }
                    log::warn!("[{agent_label} stderr][session {session_id}] {line}");
                }
                // stderr EOF (or a read error) means the agent process has
                // exited or torn down its stderr — either way it can no
                // longer be trusted to report background state.
                exited.cancel();
            });
        }

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to get stdin".to_string())?;

        // For the shell-spawn fallback, write the exec command to stdin. By the
        // time the shell reads from stdin, init is complete and `precmd` has
        // fired in the working directory (activating Hermit). `exec` replaces
        // the shell with the agent binary — from this point on, stdin belongs
        // to the agent's JSON-RPC transport. When a snapshot was applied the
        // agent binary is already the child process, so there is nothing to
        // exec.
        if use_shell_spawn {
            // If the fallback shell's initialized PATH already provides an
            // env-shebang interpreter, keep it unchanged. Otherwise add the
            // agent bin dir only as an interpreter fallback, preserving broad
            // toolchain dirs behind project-managed PATH entries.
            let exec_line = shell_exec_line(
                &self.binary_path,
                &self.acp_args,
                self.interpreter_env_snapshot.as_deref(),
            );
            stdin
                .write_all(exec_line.as_bytes())
                .await
                .map_err(|e| format!("Failed to write exec command to shell stdin: {e}"))?;
            stdin
                .flush()
                .await
                .map_err(|e| format!("Failed to flush shell stdin: {e}"))?;
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to get stdout".to_string())?;

        let stdin_compat = stdin.compat_write();
        let incoming_reader: Box<dyn tokio::io::AsyncRead + Send + Unpin> = if self.is_remote {
            let (normalized_stdout_writer, normalized_stdout_reader) = tokio::io::duplex(64 * 1024);
            tokio::task::spawn_local(async move {
                if let Err(error) =
                    normalize_remote_acp_stdout(stdout, normalized_stdout_writer).await
                {
                    log::error!("remote ACP stdout normalization failed: {error}");
                }
            });
            Box::new(normalized_stdout_reader)
        } else {
            // On the shell-spawn fallback, shell init (.zshrc, plugin banners,
            // Hermit activation) may write to stdout before `exec` replaces the
            // shell. Filter out any non-JSON lines so they don't reach the
            // JSON-RPC parser. On the direct-spawn (snapshot) path there is no
            // shell banner, so this is a harmless passthrough.
            let (normalized_stdout_writer, normalized_stdout_reader) = tokio::io::duplex(64 * 1024);
            tokio::task::spawn_local(async move {
                if let Err(error) =
                    normalize_local_acp_stdout(stdout, normalized_stdout_writer).await
                {
                    log::error!("local ACP stdout normalization failed: {error}");
                }
            });
            Box::new(normalized_stdout_reader)
        };
        let stdout_compat = incoming_reader.compat();

        let is_resuming = agent_session_id.is_some();
        let replay_boundaries = if is_resuming {
            store
                .get_session_replay_boundaries(session_id)
                .unwrap_or_else(|e| {
                    log::warn!("Failed to load session replay boundaries: {e}");
                    vec![]
                })
        } else {
            vec![]
        };
        let handler = Arc::new(
            AcpNotificationHandler::new(
                Arc::clone(writer),
                is_resuming,
                replay_boundaries,
                cancel_token.clone(),
            )
            // Only a held connection can be asked for permission out-of-turn;
            // without a hold the policy is unreachable.
            .with_out_of_turn_permissions(
                self.background_hold
                    .as_ref()
                    .map(|hold| hold.out_of_turn_permissions)
                    .unwrap_or_default(),
            ),
        );
        let transport = ByteStreams::new(stdin_compat, stdout_compat);

        let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel::<QueuedSessionTurn>();
        let (hold_control_tx, mut hold_control_rx) =
            mpsc::unbounded_channel::<StopAsyncTaskRequest>();
        let hold_active = Arc::new(AtomicBool::new(false));
        let hold_active_for_task = Arc::clone(&hold_active);
        let (settled_tx, settled_rx) = oneshot::channel::<SessionSettled>();

        // Owned copies for the connection task — its lifetime is the
        // session's, not this call's, so it cannot borrow from `self` or the
        // arguments.
        let store = Arc::clone(store);
        let writer = Arc::clone(writer);
        let cancel_token = cancel_token.clone();
        let our_session_id = session_id.to_string();
        let acp_session_id = agent_session_id.map(str::to_string);
        let config_options = config_options.to_vec();
        let mcp_servers = self.mcp_servers.clone();
        let agent_label = self.agent_label.clone();
        let is_remote = self.is_remote;
        let background_hold = self.background_hold.clone();
        let background_hold_observer = self.background_hold_observer.clone();

        let connection_task = tokio::task::spawn_local(async move {
            // Reply sender for the turn in flight, stashed by the session
            // loop. The outcome is delivered only after finalize() +
            // graceful_stop() below, so `SessionConnection::prompt` resumes
            // with the child already stopped — the old per-turn ordering.
            let pending_reply: RefCell<Option<oneshot::Sender<SessionTurnResult>>> =
                RefCell::new(None);
            let permission_handler = Arc::clone(&handler);
            let notification_handler = Arc::clone(&handler);
            let ext_notification_handler = Arc::clone(&handler);
            let protocol_result = Client
                .builder()
                .name("staged-acp-client")
                .on_receive_request(
                    async move |args: RequestPermissionRequest, responder, _connection| {
                        let response = permission_handler.request_permission(args).await?;
                        responder.respond(response)
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                // `session/update` frames — the typed `SessionUpdate` kinds
                // plus the asyncTasks extension's three `async_task_*`
                // discriminators the v1 enum cannot represent (see
                // [`IncomingSessionUpdate`]). The wrapper must parse the
                // typed updates on the same connection that advertises the
                // capability: an unparseable matched notification is a hard
                // dispatch error, not a fall-through.
                .on_receive_notification(
                    async move |notification: IncomingSessionUpdate, _connection| match notification
                    {
                        IncomingSessionUpdate::Standard(notification) => {
                            notification_handler
                                .session_notification(notification)
                                .await
                        }
                        IncomingSessionUpdate::AsyncTask(notification) => {
                            notification_handler.async_task_update(notification).await
                        }
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                // Second, enum-typed handler for whatever the
                // `session/update`-typed handler doesn't match — in practice
                // the bridge's `_claude/sdkMessage` ExtNotification frames,
                // which would otherwise be actively rejected with a
                // method_not_found error frame back to the agent.
                // Registration order matters: `AgentNotification`'s
                // `matches_method` accepts every method and handlers run in
                // registration order, so this must come after the
                // `session/update`-typed handler above to avoid stealing its
                // frames.
                .on_receive_notification(
                    async move |notification: AgentNotification, _connection| {
                        match notification {
                            AgentNotification::ExtNotification(ext) => {
                                ext_notification_handler.ext_notification(ext).await
                            }
                            // `session/update` never lands here (consumed by
                            // the handler above); drop any other known-but-
                            // unused notification kind.
                            other => {
                                log::debug!(
                                    "Ignoring unhandled ACP notification '{}'",
                                    other.method()
                                );
                                Ok(())
                            }
                        }
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(transport, async |connection| {
                    run_acp_session(
                        &connection,
                        &acp_working_dir,
                        &store,
                        &our_session_id,
                        acp_session_id.as_deref(),
                        &config_options,
                        &handler,
                        &mcp_servers,
                        &agent_label,
                        &cancel_token,
                        background_hold.as_ref(),
                        background_hold_observer.as_ref(),
                        &child_exited,
                        &mut prompt_rx,
                        &mut hold_control_rx,
                        &hold_active_for_task,
                        &pending_reply,
                    )
                    .await
                    .map_err(agent_client_protocol::util::internal_error)
                })
                .await
                .map_err(|e| format!("ACP protocol failed: {e:?}"));

            writer.finalize().await;
            graceful_stop(&mut child, is_remote).await;

            let (result, settle_reason) = match protocol_result {
                Ok((outcome, reason)) => (Ok(outcome), reason),
                Err(e) if cancel_token.is_cancelled() => {
                    log::info!("Session {our_session_id} cancelled during ACP teardown: {e}");
                    (
                        Ok(AgentRunOutcome::Cancelled),
                        SessionSettleReason::Cancelled,
                    )
                }
                Err(e) => (Err(e), SessionSettleReason::Failed),
            };

            // The settled event marks actual teardown: finalize() and
            // graceful_stop() have run, so post-completion work keyed off it
            // cannot race still-live background continuations.
            let _ = settled_tx.send(SessionSettled {
                outcome: result.clone(),
                reason: settle_reason,
            });

            if let Some(reply) = pending_reply.borrow_mut().take() {
                // The caller may have dropped its prompt() future; the result
                // still comes back through the task handle. (Under a
                // background hold a completed turn's reply was already
                // resolved at hold entry, leaving nothing stashed here.)
                let _ = reply.send(result.clone());
            }

            result
        });

        Ok(SessionConnection {
            prompt_tx,
            hold_control_tx,
            hold_active,
            connection_task: Some(connection_task),
            exit_result: None,
            settled_rx: Some(settled_rx),
        })
    }
}

#[async_trait(?Send)]
impl AgentDriver for AcpDriver {
    async fn run(
        &self,
        session_id: &str,
        prompt: &str,
        images: &[(String, String)],
        working_dir: &Path,
        store: &Arc<dyn Store>,
        writer: &Arc<dyn MessageWriter>,
        cancel_token: &CancellationToken,
        agent_session_id: Option<&str>,
        config_options: &[AcpSessionConfigOptionSelection],
    ) -> Result<AgentRunOutcome, String> {
        let mut connection = self
            .connect(
                session_id,
                working_dir,
                store,
                writer,
                cancel_token,
                agent_session_id,
                config_options,
            )
            .await?;
        let settled_rx = connection.take_settled_receiver();
        let turn_result = connection.prompt(prompt, images).await;
        // One-shot semantics: wait for the connection to settle (a no-op
        // without a background hold, where the settled event fires with the
        // turn's own outcome right before the prompt resolves) so the agent
        // is fully torn down when run() returns.
        match settled_rx {
            Some(rx) => match rx.await {
                Ok(settled) => settled.fold_turn_result(turn_result),
                // The connection task died without settling; the turn result
                // is the best information available.
                Err(_) => turn_result,
            },
            None => turn_result,
        }
    }
}

/// Gracefully stop the ACP child process.
///
/// For remote proxies (spawned with `process_group(0)`), sends SIGINT to the
/// process group so the proxy and its children can run cleanup. Falls back to
/// SIGKILL after a 5-second timeout.
///
/// For local agents (no separate process group), kills immediately.
async fn graceful_stop(child: &mut tokio::process::Child, is_remote: bool) {
    #[cfg(unix)]
    if is_remote {
        let Some(pid) = child.id() else {
            return;
        };
        let Ok(pid) = i32::try_from(pid) else {
            let _ = child.kill().await;
            return;
        };
        // Send SIGINT to the process group (negative PID) so both `sq`
        // and its child processes (the blox acp proxy) receive the signal.
        if signal::kill(Pid::from_raw(-pid), Signal::SIGINT).is_ok() {
            if let Ok(Ok(_status)) =
                tokio::time::timeout(Duration::from_secs(5), child.wait()).await
            {
                return;
            }
        }
    }
    let _ = child.kill().await;
}

#[derive(Debug, PartialEq, Eq)]
enum RemoteLineOutcome {
    Emit(String),
    Pending,
    Dropped,
}

fn sanitize_remote_acp_chunk(chunk: &str) -> String {
    chunk
        .chars()
        .filter(|ch| *ch != '\0' && *ch != '\u{1e}')
        .collect()
}

fn decode_remote_acp_line(raw_line: &[u8]) -> (String, bool) {
    let mut decoded = String::with_capacity(raw_line.len());
    let mut had_invalid_utf8 = false;
    let mut cursor = raw_line;

    while !cursor.is_empty() {
        match std::str::from_utf8(cursor) {
            Ok(valid) => {
                decoded.push_str(valid);
                break;
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                if valid_up_to > 0 {
                    if let Ok(valid) = std::str::from_utf8(&cursor[..valid_up_to]) {
                        decoded.push_str(valid);
                    }
                }

                had_invalid_utf8 = true;
                cursor = if let Some(invalid_len) = error.error_len() {
                    &cursor[valid_up_to + invalid_len..]
                } else {
                    // Incomplete sequence at EOF, which cannot be recovered.
                    break;
                };
            }
        }
    }

    (decoded, had_invalid_utf8)
}

fn consume_remote_acp_line(pending: &mut String, raw_line: &str) -> RemoteLineOutcome {
    let line = raw_line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return RemoteLineOutcome::Pending;
    }

    let chunk = sanitize_remote_acp_chunk(line);
    if chunk.is_empty() {
        return RemoteLineOutcome::Pending;
    }

    pending.push_str(&chunk);

    match serde_json::from_str::<serde_json::Value>(pending) {
        Ok(_) => RemoteLineOutcome::Emit(std::mem::take(pending)),
        Err(error) if error.is_eof() => {
            if pending.len() > REMOTE_ACP_MAX_PENDING_LINE_BYTES {
                pending.clear();
                RemoteLineOutcome::Dropped
            } else {
                RemoteLineOutcome::Pending
            }
        }
        Err(_) => {
            // Recovery path: pending may contain stale/corrupted bytes. If the
            // current chunk is a standalone JSON payload, emit it and reset.
            match serde_json::from_str::<serde_json::Value>(&chunk) {
                Ok(_) => {
                    pending.clear();
                    RemoteLineOutcome::Emit(chunk)
                }
                Err(chunk_error) if chunk_error.is_eof() => {
                    pending.clear();
                    pending.push_str(&chunk);
                    if pending.len() > REMOTE_ACP_MAX_PENDING_LINE_BYTES {
                        pending.clear();
                        RemoteLineOutcome::Dropped
                    } else {
                        RemoteLineOutcome::Pending
                    }
                }
                Err(_) => {
                    pending.clear();
                    RemoteLineOutcome::Dropped
                }
            }
        }
    }
}

fn remote_acp_segments(decoded_line: &str) -> impl Iterator<Item = &str> {
    // `sq blox acp` can emit JSON Text Sequences where records are delimited by
    // U+001E (record separator). Keep line-based handling for normal JSON-RPC
    // output, but split RS-delimited frames so concatenated messages are not
    // treated as malformed JSON.
    decoded_line
        .split('\u{1e}')
        .filter(|segment| !segment.trim().is_empty())
}

async fn normalize_remote_acp_stdout<R: tokio::io::AsyncRead + Unpin>(
    stdout: R,
    mut writer: tokio::io::DuplexStream,
) -> Result<(), std::io::Error> {
    let mut reader = BufReader::new(stdout);
    let mut raw_line = Vec::new();
    let mut pending = String::new();

    loop {
        raw_line.clear();
        let bytes_read = reader.read_until(b'\n', &mut raw_line).await?;
        if bytes_read == 0 {
            break;
        }

        let (decoded_line, had_invalid_utf8) = decode_remote_acp_line(&raw_line);
        if had_invalid_utf8 {
            log::warn!("Dropped invalid UTF-8 bytes from remote ACP stdout");
        }

        for segment in remote_acp_segments(&decoded_line) {
            match consume_remote_acp_line(&mut pending, segment) {
                RemoteLineOutcome::Emit(line) => {
                    writer.write_all(line.as_bytes()).await?;
                    writer.write_all(b"\n").await?;
                }
                RemoteLineOutcome::Pending => {}
                RemoteLineOutcome::Dropped => {
                    if !segment.trim().is_empty() {
                        log::warn!("Dropped malformed ACP proxy output line");
                    }
                }
            }
        }
    }

    if !pending.is_empty() {
        log::warn!("Dropped incomplete ACP proxy output at EOF");
    }

    writer.shutdown().await
}

/// Filter local ACP stdout, forwarding only valid JSON lines.
///
/// Local shell initialization (`.zshrc`, Hermit activation, plugin banners)
/// may write non-JSON text to stdout before `exec` replaces the shell with
/// the agent binary. This function reads lines from the child's stdout and
/// only forwards those that parse as valid JSON, discarding everything else.
async fn normalize_local_acp_stdout<R: tokio::io::AsyncRead + Unpin>(
    stdout: R,
    mut writer: tokio::io::DuplexStream,
) -> Result<(), std::io::Error> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).await?;
        if bytes_read == 0 {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
            writer.write_all(trimmed.as_bytes()).await?;
            writer.write_all(b"\n").await?;
        } else {
            log::debug!("Dropped non-JSON line from local ACP stdout: {trimmed}");
        }
    }

    writer.shutdown().await
}

// =============================================================================
// ACP notification handler — phase-based replay-sync state machine
// =============================================================================

/// The current phase of the notification handler during session resumption.
enum HandlerPhase {
    /// Accumulating replay notifications and matching against DB messages.
    Replaying(ReplayBuffer),
    /// Replay detected as complete; waiting for prompt to be sent.
    /// All notifications are dropped; tool-call IDs are recorded.
    WaitingForPrompt {
        replayed_tool_call_ids: HashSet<String>,
    },
    /// Prompt has been sent; forwarding live notifications to the writer.
    Live {
        replayed_tool_call_ids: HashSet<String>,
        /// ACP message id of the text this turn is streaming, kept so a
        /// following continuation can be forced onto a *different* boundary.
        message_id: Option<String>,
        /// A background continuation was still open when this turn started
        /// (the user prompted during the hold). The first thing this turn
        /// records must close that record first, or the continuation's text
        /// would be absorbed into this turn's message.
        close_open_record: bool,
    },
    /// The prompt resolved but the connection is held open for background work
    /// ([`SessionLifetime::BackgroundHolding`]). Notifications still arrive —
    /// out-of-turn continuations stream as ordinary `session/update`s — and are
    /// recorded as a separate, attributed continuation record.
    BackgroundHolding {
        replayed_tool_call_ids: HashSet<String>,
        /// ACP message id the finished turn last streamed into.
        turn_message_id: Option<String>,
        /// The continuation record, opened by the first out-of-turn update.
        continuation: Option<ContinuationRecord>,
    },
}

/// One background continuation: everything an out-of-turn burst records
/// between the turn that preceded it and whatever comes next.
///
/// Its whole job is to be a boundary. Text is stamped with a message id that
/// is *not* the finished turn's, and every row carries the continuation origin
/// tag, so neither the replay projection nor the UI can fold a continuation
/// into a turn it does not belong to.
struct ContinuationRecord {
    /// Origin tag persisted on every row of this record.
    origin: String,
    /// Message-id boundary for this record's text, decided by the first
    /// out-of-turn text chunk (see [`ContinuationRecord::text_message_id`]).
    message_id: Option<String>,
    /// Whether [`ContinuationRecord::message_id`] was synthesized because the
    /// provider's chunks carried no usable id of their own. A synthesized id
    /// is kept for the record's lifetime: rows are already persisted under it,
    /// so switching mid-record would split the record in two.
    synthesized_message_id: bool,
}

impl ContinuationRecord {
    fn new(origin: String) -> Self {
        Self {
            origin,
            message_id: None,
            synthesized_message_id: false,
        }
    }

    /// The ACP message id to stamp on an out-of-turn text chunk.
    ///
    /// The provider's own id is preferred — it keeps resume-time id matching
    /// working — but only when it differs from the finished turn's id. A
    /// missing (or turn-colliding) id is replaced with a synthesized one,
    /// because chunks with no id coalesce into a single boundary and would
    /// merge the continuation into the turn.
    fn text_message_id(&mut self, chunk_id: Option<&str>, turn_message_id: Option<&str>) -> String {
        if self.message_id.is_none() {
            match chunk_id {
                Some(id) if Some(id) != turn_message_id => {
                    self.message_id = Some(id.to_string());
                    self.synthesized_message_id = false;
                }
                _ => {
                    self.message_id = Some(synthesized_continuation_message_id());
                    self.synthesized_message_id = true;
                }
            }
        }
        self.message_id.clone().expect("message_id set above")
    }

    /// Whether `chunk_id` belongs to a *different* message than this record is
    /// already streaming — a second autonomous cycle during the same hold,
    /// which gets its own record rather than extending this one.
    fn is_foreign_text_chunk(&self, chunk_id: Option<&str>) -> bool {
        match (chunk_id, self.message_id.as_deref()) {
            (Some(chunk_id), Some(record_id)) => {
                !self.synthesized_message_id && chunk_id != record_id
            }
            _ => false,
        }
    }
}

/// Synthesized ACP message ids for continuations whose chunks carry none.
/// Prefixed so a boundary that Staged invented is recognizable in the DB, and
/// counted (rather than random) so it stays deterministic within a process.
const CONTINUATION_MESSAGE_ID_PREFIX: &str = "background-continuation-";
static CONTINUATION_MESSAGE_COUNTER: AtomicU64 = AtomicU64::new(1);

fn synthesized_continuation_message_id() -> String {
    let seq = CONTINUATION_MESSAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{CONTINUATION_MESSAGE_ID_PREFIX}{seq}")
}

/// Accumulates replay notifications and matches them against DB messages.
struct ReplayBuffer {
    /// Persisted replay boundaries from the DB, in order.
    db_messages: Vec<ReplayBoundary>,
    /// Index into `db_messages` of the next boundary to match.
    match_cursor: usize,
    /// Index of the last non-user message in `db_messages`.
    /// When the cursor passes this, replay is considered complete.
    target_index: Option<usize>,
    /// Text accumulated for the current streaming message.
    current_text: String,
    /// Role of the current streaming message (`"user"` or `"assistant"`).
    current_role: Option<String>,
    /// ACP message ID for the current streaming message, when the provider
    /// includes one in message chunks.
    current_message_id: Option<String>,
    /// Tool-call IDs observed during replay (used as a safety-net later).
    replayed_tool_call_ids: HashSet<String>,
    /// Timestamp of the last notification received during replay.
    last_notification_at: Instant,
    /// Whether at least one notification has been received.
    received_any: bool,
}

impl ReplayBuffer {
    fn new(db_messages: Vec<ReplayBoundary>) -> Self {
        // Find index of last non-user message.
        let target_index = db_messages
            .iter()
            .enumerate()
            .rev()
            .find(|(_, message)| message.role != "user")
            .map(|(i, _)| i);

        Self {
            db_messages,
            match_cursor: 0,
            target_index,
            current_text: String::new(),
            current_role: None,
            current_message_id: None,
            replayed_tool_call_ids: HashSet::new(),
            last_notification_at: Instant::now(),
            received_any: false,
        }
    }

    /// Finalize the current streaming text and try to match it against DB.
    /// Called when the role transitions (e.g. from assistant text to tool call).
    /// Returns `true` if replay is now considered complete.
    fn finalize_current(&mut self) -> bool {
        if let Some(role) = self.current_role.take() {
            if !self.current_text.is_empty() {
                let event = ReplayEvent {
                    role,
                    content: std::mem::take(&mut self.current_text),
                    acp_message_id: self.current_message_id.take(),
                    acp_tool_call_id: None,
                };
                return self.try_match(&event);
            }
        }
        self.current_message_id = None;
        false
    }

    /// Try to match a replay event against persisted replay boundaries.
    /// Returns `true` if replay is now considered complete.
    fn try_match(&mut self, event: &ReplayEvent) -> bool {
        if self.match_cursor >= self.db_messages.len() {
            return self.is_complete();
        }

        if let Some(idx) = self.find_id_match(event) {
            self.match_cursor = idx + 1;
            return self.is_complete();
        }

        let boundary = &self.db_messages[self.match_cursor];
        if boundary.matches_fallback(event) {
            self.match_cursor += 1;
        }

        self.is_complete()
    }

    fn push_text_chunk(&mut self, role: &str, message_id: Option<&str>, text: &str) -> bool {
        let role_changed = self.current_role.as_deref() != Some(role);
        let message_id_changed = !role_changed
            && matches!(
                (self.current_message_id.as_deref(), message_id),
                (Some(current), Some(next)) if current != next
            );

        let mut done = false;
        if role_changed || message_id_changed {
            done = self.finalize_current();
            self.current_role = Some(role.to_string());
            self.current_message_id = message_id.map(str::to_string);
        } else if self.current_message_id.is_none() {
            self.current_message_id = message_id.map(str::to_string);
        }

        self.current_text.push_str(text);
        done
    }

    fn find_id_match(&self, event: &ReplayEvent) -> Option<usize> {
        self.db_messages
            .iter()
            .enumerate()
            .skip(self.match_cursor)
            .find(|(_, boundary)| boundary.matches_id(event))
            .map(|(idx, _)| idx)
    }

    /// Returns `true` if the match cursor has passed the target index.
    fn is_complete(&self) -> bool {
        match self.target_index {
            Some(target) => self.match_cursor > target,
            None => true, // No non-user messages → complete immediately
        }
    }
}

struct ReplayEvent {
    role: String,
    content: String,
    acp_message_id: Option<String>,
    acp_tool_call_id: Option<String>,
}

impl ReplayBoundary {
    fn matches_id(&self, event: &ReplayEvent) -> bool {
        match (
            self.acp_message_id.as_deref(),
            event.acp_message_id.as_deref(),
        ) {
            (Some(boundary_id), Some(event_id)) if boundary_id == event_id => return true,
            _ => {}
        }

        matches!(
            (
                self.acp_tool_call_id.as_deref(),
                event.acp_tool_call_id.as_deref()
            ),
            (Some(boundary_id), Some(event_id)) if boundary_id == event_id
        )
    }

    fn matches_fallback(&self, event: &ReplayEvent) -> bool {
        if self.role != event.role {
            return false;
        }

        if self.content.is_empty() || event.content.is_empty() {
            return true;
        }

        self.content == event.content
    }
}

/// Method of the Claude bridge's raw SDK message forwarding, as delivered to
/// our notification handler. On the wire the method is `_claude/sdkMessage`;
/// `AgentNotification::parse_message` strips the leading `_` from extension
/// methods on receive (the `_` must be re-added if we ever send an extension
/// frame ourselves).
const CLAUDE_SDK_MESSAGE_METHOD: &str = "claude/sdkMessage";

/// Raw SDK `system` frame subtypes that track background tasks. Requested
/// from the bridge via the `_meta.claudeCode.emitRawSDKMessages` filters on
/// `session/new` and `session/load`, and consumed by
/// [`BackgroundTaskSet::apply_sdk_message`].
const BACKGROUND_TASK_SUBTYPES: [&str; 4] = [
    "task_started",
    "task_updated",
    TASK_NOTIFICATION_SUBTYPE,
    "background_tasks_changed",
];

/// Raw SDK `system` frame subtype announcing that a background task settled.
/// It is also what *wakes* the model for an autonomous continuation cycle, so
/// it stands in for the [`TASK_NOTIFICATION_ORIGIN`] attribution on frames
/// that carry no `origin` of their own.
const TASK_NOTIFICATION_SUBTYPE: &str = "task_notification";

/// The bridge's `origin.kind` for an autonomous cycle the model ran off a
/// settled background task — the precise attribution for a continuation
/// record (see [`background_continuation_origin`]).
const TASK_NOTIFICATION_ORIGIN: &str = "task-notification";

/// Raw SDK `system` frame subtype requested purely as an availability probe:
/// `init` re-emits at the start of every prompt turn, so its arrival proves
/// the raw-SDK stream works on this connection even when no background task
/// ever starts. The post-turn holding wait refuses to trust an empty task set
/// (and its debounce) until at least one raw frame has arrived — without the
/// probe, every task-less turn would be indistinguishable from a bridge that
/// can't emit frames, and would hold to the cap.
const AVAILABILITY_PROBE_SUBTYPE: &str = "init";

/// Raw SDK `system` frame subtype carrying the SDK's own turn-liveness
/// signal (`state`: `idle` / `running` / `requires_action`). Requested so
/// the post-turn holding wait can refuse quiescence while the model is
/// mid-cycle: the live-task set empties on `task_notification` at the very
/// instant a continuation cycle *starts*, so without this latch only the
/// debounce protects a running continuation from teardown.
const SESSION_STATE_SUBTYPE: &str = "session_state_changed";

/// The SDK's session state as the holding wait consumes it: `idle` releases
/// quiescence, everything else blocks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SdkSessionState {
    Idle,
    Busy,
}

/// Reads a [`SESSION_STATE_SUBTYPE`] frame's state, folding `running`,
/// `requires_action`, and anything the SDK adds later into
/// [`SdkSessionState::Busy`] — unknown liveness is pessimistic on purpose,
/// since a later `idle` clears it and the hard cap bounds the pessimism.
/// `None` for every other frame.
fn sdk_message_session_state(message: &serde_json::Value) -> Option<SdkSessionState> {
    if message.get("type").and_then(serde_json::Value::as_str) != Some("system")
        || message.get("subtype").and_then(serde_json::Value::as_str) != Some(SESSION_STATE_SUBTYPE)
    {
        return None;
    }
    match message.get("state").and_then(serde_json::Value::as_str)? {
        "idle" => Some(SdkSessionState::Idle),
        _ => Some(SdkSessionState::Busy),
    }
}

/// Live background-task set for one ACP session, maintained from the raw
/// Claude Agent SDK `system` frames the bridge forwards as
/// `_claude/sdkMessage` extension notifications.
///
/// The post-turn holding wait keys quiescence off "set is empty" (through the
/// handler's background-activity watch); membership changes are also logged
/// so the wire format stays observable against the real bridge.
#[derive(Debug, Default)]
struct BackgroundTaskSet {
    task_ids: HashSet<String>,
}

impl BackgroundTaskSet {
    /// Apply one raw SDK message, returning `true` when membership changed.
    ///
    /// - `background_tasks_changed` carries the full live set with REPLACE
    ///   semantics — the authoritative snapshot.
    /// - `task_started` inserts `task_id`.
    /// - `task_updated` removes `task_id` on a terminal `patch.status`;
    ///   non-terminal patches (`pending` / `running` / `paused`) are ignored.
    ///   The terminal set matches upstream's own `taskState` mapping, which
    ///   folds `killed`, `cancelled` and `stopped` together — wider than the
    ///   bridge's `liveBackgroundTasks` pruning, which only checks
    ///   `completed` / `failed` / `killed`. Taking the wider set means a task
    ///   the agent reports as cancelled or stopped leaves this set on the
    ///   patch, rather than lingering until a `task_notification` or
    ///   `background_tasks_changed` snapshot reconciles it.
    /// - `task_notification` means the task settled — removes `task_id`.
    fn apply_sdk_message(&mut self, message: &serde_json::Value) -> bool {
        if message.get("type").and_then(serde_json::Value::as_str) != Some("system") {
            return false;
        }
        let task_id = || message.get("task_id").and_then(serde_json::Value::as_str);
        match message.get("subtype").and_then(serde_json::Value::as_str) {
            Some("background_tasks_changed") => {
                let snapshot: HashSet<String> = message
                    .get("tasks")
                    .and_then(serde_json::Value::as_array)
                    .map(|tasks| {
                        tasks
                            .iter()
                            .filter_map(|task| {
                                task.get("task_id")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_string)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if snapshot == self.task_ids {
                    false
                } else {
                    self.task_ids = snapshot;
                    true
                }
            }
            Some("task_started") => match task_id() {
                Some(id) => self.task_ids.insert(id.to_string()),
                None => false,
            },
            Some("task_updated") => {
                let terminal = matches!(
                    message
                        .pointer("/patch/status")
                        .and_then(serde_json::Value::as_str),
                    Some("completed" | "failed" | "killed" | "cancelled" | "stopped")
                );
                terminal && task_id().is_some_and(|id| self.task_ids.remove(id))
            }
            Some("task_notification") => task_id().is_some_and(|id| self.task_ids.remove(id)),
            _ => false,
        }
    }

    /// Sorted task ids, for deterministic logging and assertions.
    fn sorted_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.task_ids.iter().map(String::as_str).collect();
        ids.sort_unstable();
        ids
    }
}

// =============================================================================
// Typed asyncTasks extension (claude-agent-acp#1017)
// =============================================================================

/// How a connection tracks the agent's background tasks, decided once per
/// connection by the initialize-time asyncTasks negotiation.
///
/// The client advertises the AIR `asyncTasks` capability on every
/// `initialize` (see [`air_client_capabilities_meta`]); an agent that has the
/// extension mirrors its own capability list in the initialize *response*'s
/// `_meta`, and that mirror is the version probe
/// ([`task_tracking_mode_from_initialize`]). Both task-set sources feed the
/// same holding-wait decision core — the mode only selects which one is
/// published and which quiescence predicate applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum TaskTrackingMode {
    /// Raw `_claude/sdkMessage` `task_*` frames maintain
    /// [`BackgroundTaskSet`] — the shipped path, and the only one an agent
    /// that does not mirror `asyncTasks` (bridge ≤0.70.0, non-Claude agents)
    /// can serve.
    #[default]
    Raw,
    /// The typed asyncTasks extension: `async_task_spawned` /
    /// `async_task_state_update` session updates maintain
    /// [`TypedAsyncTaskSet`], and the mirrored capability replaces the raw
    /// path's stream proof.
    Typed,
}

/// `_meta` keys of the AIR extension namespace: capability negotiation for
/// typed asyncTasks lives under `_meta.jetbrains.air` on both sides of
/// `initialize`, as `{version, capabilities}`.
const AIR_META_NAMESPACE: &str = "jetbrains";
const AIR_META_KEY: &str = "air";
/// AIR extension version this client speaks. The bridge requires an integer
/// peer version `>= 1` before it treats any advertised capability as real.
const AIR_EXTENSION_VERSION: u64 = 1;
/// The one AIR capability this client advertises. Deliberately *not*
/// `nativeSubagentSessions`: a negotiating client receives `session/update`s
/// whose session id is a virtual child id the recording layer can't route
/// yet, and background subagents were never this feature's gap (the bridge
/// has held the prompt open for them since v0.59.0).
const AIR_ASYNC_TASKS_CAPABILITY: &str = "asyncTasks";

/// The `clientCapabilities._meta` advertising this client's AIR capability
/// list. Sent unconditionally on every `initialize`: agents MUST NOT make
/// assumptions about unrecognized `_meta` keys per the ACP spec — the same
/// contract `emitRawSDKMessages` already rides — and only an agent that
/// mirrors the capability back ever sends the typed updates.
fn air_client_capabilities_meta() -> Meta {
    let mut meta = Meta::new();
    meta.insert(
        AIR_META_NAMESPACE.to_string(),
        serde_json::json!({
            AIR_META_KEY: {
                "version": AIR_EXTENSION_VERSION,
                "capabilities": [AIR_ASYNC_TASKS_CAPABILITY],
            }
        }),
    );
    meta
}

/// Decide a connection's [`TaskTrackingMode`] from the initialize response's
/// `_meta`.
///
/// Mirrors the bridge's own peer check (`clientSupportsAirCapability`): an
/// integer `version >= 1` plus membership in the `capabilities` array. A
/// missing or malformed mirror is simply an agent without the extension —
/// bridge 0.70.0 and older, or any non-Claude agent — and reads
/// [`TaskTrackingMode::Raw`], today's path unchanged.
fn task_tracking_mode_from_initialize(meta: Option<&Meta>) -> TaskTrackingMode {
    let Some(air) = meta
        .and_then(|meta| meta.get(AIR_META_NAMESPACE))
        .and_then(|jetbrains| jetbrains.get(AIR_META_KEY))
    else {
        return TaskTrackingMode::Raw;
    };
    let version_ok = air
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|version| version >= AIR_EXTENSION_VERSION);
    let mirrored = air
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|capabilities| capabilities.iter().any(|c| c == AIR_ASYNC_TASKS_CAPABILITY));
    if version_ok && mirrored {
        TaskTrackingMode::Typed
    } else {
        TaskTrackingMode::Raw
    }
}

/// Method name shared by every `session/update` notification, typed or not.
const SESSION_UPDATE_METHOD: &str = "session/update";

/// Extension request that stops one background task without cancelling the
/// parent session: `{sessionId, asyncTaskId}` → `{stopped: boolean}`. Part of
/// the typed asyncTasks extension (claude-agent-acp#1017). Sent verbatim —
/// unlike received extension methods, whose leading `_` the dispatch layer
/// strips (see [`CLAUDE_SDK_MESSAGE_METHOD`]), an outgoing method keeps it.
const ASYNC_TASK_STOP_METHOD: &str = "_session/async_task/stop";

/// The `_session/async_task/stop` request frame for one task.
fn async_task_stop_message(
    agent_session_id: &str,
    task_id: &str,
) -> Result<UntypedMessage, agent_client_protocol::Error> {
    UntypedMessage::new(
        ASYNC_TASK_STOP_METHOD,
        serde_json::json!({
            "sessionId": agent_session_id,
            "asyncTaskId": task_id,
        }),
    )
}

/// Fold a `_session/async_task/stop` response into the caller's answer: the
/// agent's `{stopped}` boolean, with a malformed response reading `false`
/// (the agent did not say it stopped anything) and a transport or agent error
/// surfaced as such — an agent without the extension answers method-not-found
/// here, not silence.
fn async_task_stop_outcome(
    result: Result<serde_json::Value, agent_client_protocol::Error>,
) -> Result<bool, String> {
    result
        .map(|response| {
            response
                .get("stopped")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .map_err(|e| format!("Async task stop request failed: {e:?}"))
}

/// The three `sessionUpdate` discriminators of the typed asyncTasks
/// extension.
const ASYNC_TASK_SPAWNED_KIND: &str = "async_task_spawned";
const ASYNC_TASK_PROGRESS_KIND: &str = "async_task_progress";
const ASYNC_TASK_STATE_UPDATE_KIND: &str = "async_task_state_update";

/// Lifecycle states of a typed async task:
/// `running | paused | completed | failed | stopped`, with anything the
/// extension adds later folded to [`AsyncTaskState::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AsyncTaskState {
    Running,
    Paused,
    Completed,
    Failed,
    Stopped,
    /// A state this client doesn't know. Counts as live — pessimistic on
    /// purpose, matching the idle latch's posture on unknown session states:
    /// tearing down under a task that is actually alive loses its
    /// continuation, while holding a finished one is bounded by the cap.
    Unknown,
}

impl AsyncTaskState {
    fn parse(state: &str) -> Self {
        match state {
            "running" => Self::Running,
            "paused" => Self::Paused,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "stopped" => Self::Stopped,
            _ => Self::Unknown,
        }
    }

    /// Whether a task in this state keeps the post-turn hold open. `paused`
    /// is live — a state the raw set never surfaced; only the hard cap
    /// bounds a parked task.
    fn is_live(self) -> bool {
        matches!(self, Self::Running | Self::Paused | Self::Unknown)
    }
}

/// One typed asyncTasks `session/update`, parsed structurally: the v1 schema
/// has no types for these discriminators yet (spec PR #1992), so this
/// deliberately mirrors the bridge's own temporary cast — minimal, hand-rolled
/// structs that parse only what the task set (and, later, the UI) needs and
/// ignore the rest. Swap for crate types when a released
/// `agent-client-protocol` ships them.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AsyncTaskUpdate {
    /// `async_task_spawned` — a background task was announced. The
    /// announcement rides the tool result mid-turn, so a task the turn
    /// launched is on the wire before the prompt resolves.
    Spawned {
        task_id: String,
        name: Option<String>,
        description: Option<String>,
        output_file_path: Option<String>,
        tool_call_id: Option<String>,
    },
    /// `async_task_progress` — a presence signal, deliberately not a
    /// lifecycle edge (it never touches the task set).
    Progress { task_id: String },
    /// `async_task_state_update` — the task moved through the typed
    /// lifecycle. Terminal states are guaranteed even against a dying bridge
    /// (`finishAll` publishes one for every announced task at stream end).
    StateUpdate {
        task_id: String,
        state: AsyncTaskState,
    },
}

impl AsyncTaskUpdate {
    /// Structural parse of a `session/update`'s `update` object. `None` when
    /// the discriminator is not one of the three `async_task_*` kinds or a
    /// required field is missing — the caller then reports the typed enum's
    /// own parse error.
    fn from_update_value(update: &serde_json::Value) -> Option<Self> {
        let string_field = |name: &str| {
            update
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        let task_id = || string_field("asyncTaskId");
        match update
            .get("sessionUpdate")
            .and_then(serde_json::Value::as_str)?
        {
            ASYNC_TASK_SPAWNED_KIND => Some(Self::Spawned {
                task_id: task_id()?,
                name: string_field("name"),
                description: string_field("description"),
                output_file_path: string_field("outputFilePath"),
                tool_call_id: string_field("toolCallId"),
            }),
            ASYNC_TASK_PROGRESS_KIND => Some(Self::Progress {
                task_id: task_id()?,
            }),
            ASYNC_TASK_STATE_UPDATE_KIND => Some(Self::StateUpdate {
                task_id: task_id()?,
                state: AsyncTaskState::parse(
                    update.get("state").and_then(serde_json::Value::as_str)?,
                ),
            }),
            _ => None,
        }
    }
}

/// A `session/update` notification carrying an [`AsyncTaskUpdate`].
#[derive(Debug, Clone)]
struct AsyncTaskNotification {
    session_id: String,
    update: AsyncTaskUpdate,
    /// The `update` object as received, kept so
    /// [`IncomingSessionUpdate::to_untyped_message`] stays faithful (this
    /// client never sends an async task update itself).
    raw_update: serde_json::Value,
}

impl AsyncTaskNotification {
    /// Structural parse of `session/update` params whose `update` carries one
    /// of the three `async_task_*` discriminators; `None` for anything else.
    fn from_params(params: &serde_json::Value) -> Option<Self> {
        let session_id = params
            .get("sessionId")
            .and_then(serde_json::Value::as_str)?
            .to_string();
        let raw_update = params.get("update")?;
        let update = AsyncTaskUpdate::from_update_value(raw_update)?;
        Some(Self {
            session_id,
            update,
            raw_update: raw_update.clone(),
        })
    }
}

/// The `session/update` notification as this client receives it: either a
/// standard typed [`SessionNotification`], or one of the typed asyncTasks
/// extension's updates the v1 [`SessionUpdate`] enum cannot represent.
///
/// This wrapper exists because of a hard constraint in the dispatch layer:
/// the v1 `SessionUpdate` enum has no unknown-discriminator fallback, and
/// `Dispatch::into_notification` hard-errors when a handler's
/// `matches_method` matches but the parse fails — it does not fall through
/// to the next handler. The moment this client advertises `asyncTasks`, a
/// mirroring bridge sends `async_task_*` updates that would fail the plain
/// `SessionNotification` parse and error the connection mid-hold. The
/// advertisement and this parse path must therefore always land together.
// Not boxed: the wrapper lives only from parse to the routing match in the
// notification handler, and boxing the common variant would cost a heap
// allocation on every streamed session/update.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
enum IncomingSessionUpdate {
    Standard(SessionNotification),
    AsyncTask(AsyncTaskNotification),
}

impl JsonRpcMessage for IncomingSessionUpdate {
    fn matches_method(method: &str) -> bool {
        method == SESSION_UPDATE_METHOD
    }

    fn method(&self) -> &str {
        SESSION_UPDATE_METHOD
    }

    fn to_untyped_message(&self) -> Result<UntypedMessage, agent_client_protocol::Error> {
        match self {
            Self::Standard(notification) => notification.to_untyped_message(),
            Self::AsyncTask(notification) => UntypedMessage::new(
                SESSION_UPDATE_METHOD,
                serde_json::json!({
                    "sessionId": notification.session_id,
                    "update": notification.raw_update,
                }),
            ),
        }
    }

    fn parse_message(
        method: &str,
        params: &impl serde::Serialize,
    ) -> Result<Self, agent_client_protocol::Error> {
        let typed_error = match SessionNotification::parse_message(method, params) {
            Ok(notification) => return Ok(Self::Standard(notification)),
            Err(error) => error,
        };
        if method != SESSION_UPDATE_METHOD {
            return Err(typed_error);
        }
        let params = serde_json::to_value(params)?;
        // Everything that is neither the typed enum nor an async_task_*
        // update keeps the typed parse's error (and its diagnostics) — an
        // unknown discriminator still fails loudly rather than vanishing.
        AsyncTaskNotification::from_params(&params)
            .map(Self::AsyncTask)
            .ok_or(typed_error)
    }
}

impl agent_client_protocol::JsonRpcNotification for IncomingSessionUpdate {}

/// One typed async task's tracked state: its lifecycle position plus the
/// presentational metadata its spawn announced, kept so the hold can show a
/// *named* wait row (and label the continuation the task's settling wakes).
#[derive(Debug, Clone, PartialEq, Eq)]
struct TypedAsyncTask {
    state: AsyncTaskState,
    name: Option<String>,
    description: Option<String>,
    output_file_path: Option<String>,
}

impl TypedAsyncTask {
    /// A task known only from a lifecycle edge — its spawn was never seen, so
    /// it has no metadata to present.
    fn unannounced(state: AsyncTaskState) -> Self {
        Self {
            state,
            name: None,
            description: None,
            output_file_path: None,
        }
    }
}

/// Live background-task set for one ACP session in typed mode, maintained
/// from [`AsyncTaskUpdate`]s.
///
/// `spawned` inserts as running — unless the id is already tracked as
/// terminal, in which case the spawn is a late replay of a finished task and
/// is ignored (its terminal state was already published, and no further edge
/// is guaranteed; resurrecting it would hold to the cap). `state_update`
/// moves the task through the typed lifecycle, and is authoritative even for
/// an id whose spawn was never seen. Live means [`AsyncTaskState::is_live`].
/// Terminal entries are retained rather than removed: bounded by the tasks a
/// session actually ran, and a set that remembers its history cannot mistake
/// a late frame for a brand-new task.
#[derive(Debug, Default)]
struct TypedAsyncTaskSet {
    tasks: HashMap<String, TypedAsyncTask>,
}

impl TypedAsyncTaskSet {
    /// Apply one typed update, returning `true` when the *live* membership
    /// changed (a move between two live states — running to paused — is not
    /// a membership change).
    fn apply(&mut self, update: &AsyncTaskUpdate) -> bool {
        match update {
            AsyncTaskUpdate::Spawned {
                task_id,
                name,
                description,
                output_file_path,
                ..
            } => {
                if self
                    .tasks
                    .get(task_id)
                    .is_some_and(|task| !task.state.is_live())
                {
                    return false;
                }
                let was_live = self
                    .tasks
                    .insert(
                        task_id.clone(),
                        TypedAsyncTask {
                            state: AsyncTaskState::Running,
                            name: name.clone(),
                            description: description.clone(),
                            output_file_path: output_file_path.clone(),
                        },
                    )
                    .is_some_and(|task| task.state.is_live());
                !was_live
            }
            AsyncTaskUpdate::StateUpdate { task_id, state } => {
                // A task not in the set (spawn never seen) was not live.
                let was_live = self
                    .tasks
                    .get(task_id)
                    .is_some_and(|task| task.state.is_live());
                self.tasks
                    .entry(task_id.clone())
                    .and_modify(|task| task.state = *state)
                    .or_insert_with(|| TypedAsyncTask::unannounced(*state));
                was_live != state.is_live()
            }
            AsyncTaskUpdate::Progress { .. } => false,
        }
    }

    fn live_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|task| task.state.is_live())
            .count()
    }

    /// Sorted live task ids, for deterministic logging and assertions.
    fn live_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self
            .tasks
            .iter()
            .filter(|(_, task)| task.state.is_live())
            .map(|(id, _)| id.as_str())
            .collect();
        ids.sort_unstable();
        ids
    }

    /// The live tasks as a client presents them, sorted by id so repeated
    /// snapshots of the same set compare (and render) identically.
    fn live_snapshot(&self) -> Vec<BackgroundHoldTask> {
        let mut tasks: Vec<BackgroundHoldTask> = self
            .tasks
            .iter()
            .filter(|(_, task)| task.state.is_live())
            .map(|(id, task)| BackgroundHoldTask {
                id: id.clone(),
                name: task.name.clone(),
                description: task.description.clone(),
                output_file_path: task.output_file_path.clone(),
            })
            .collect();
        tasks.sort_unstable_by(|a, b| a.id.cmp(&b.id));
        tasks
    }

    /// The name a task announced at spawn, if it did.
    fn task_name(&self, task_id: &str) -> Option<String> {
        self.tasks.get(task_id).and_then(|task| task.name.clone())
    }
}

/// Whether a raw SDK frame refers to a background task at all — a `task_*`
/// frame carrying a task id, or a non-empty `background_tasks_changed`
/// snapshot. Feeds [`BackgroundActivity::ever_started_task`]: it separates
/// "this turn started no background work" from a set that merely happens to
/// be empty right now (a `task_notification` whose start frame was missed
/// still proves a task existed).
fn sdk_message_mentions_task(message: &serde_json::Value) -> bool {
    if message.get("type").and_then(serde_json::Value::as_str) != Some("system") {
        return false;
    }
    match message.get("subtype").and_then(serde_json::Value::as_str) {
        Some("task_started" | "task_updated" | "task_notification") => {
            message.get("task_id").is_some()
        }
        Some("background_tasks_changed") => message
            .get("tasks")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|tasks| !tasks.is_empty()),
        _ => false,
    }
}

/// Snapshot of a connection's background-task signal, published by the
/// notification handler over a `watch` channel and consumed by the post-turn
/// holding wait.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct BackgroundActivity {
    /// Number of live background tasks (see [`BackgroundTaskSet`]).
    live_tasks: usize,
    /// The live tasks by name, published alongside the count so the wait can
    /// be presented as named rows. Only typed mode can name tasks (spawn
    /// announcements carry the metadata); raw mode leaves this empty and the
    /// count stands alone. Published in the same snapshot as `live_tasks` so
    /// the two can never disagree mid-render.
    tasks: Vec<BackgroundHoldTask>,
    /// Whether any `_claude/sdkMessage` frame has arrived on this connection.
    /// Until this latches true the (vacuously empty) task set is
    /// uninformative — the holding wait then never declares quiescence and
    /// relies on the hard cap (the fallback rule).
    sdk_frames_seen: bool,
    /// Whether any raw frame has ever mentioned a background task on this
    /// connection. While false, an empty task set means "this turn started no
    /// background work", and the holding wait confirms quiescence with the
    /// short [`BackgroundHoldConfig::taskless_debounce`] instead of the full
    /// drain debounce.
    ever_started_task: bool,
    /// The idle latch: the last [`SESSION_STATE_SUBTYPE`] state seen on the
    /// raw stream. `Busy` blocks quiescence — it is the direct signal that a
    /// continuation cycle is running after the task set has already emptied.
    /// Advisory by construction: a bridge that never emits the frame leaves
    /// it `None`, which blocks nothing. A busy reading is also stale-bounded
    /// per hold (see [`BackgroundHoldConfig::idle_latch_staleness`]), so a
    /// lost trailing `idle` cannot cap-condemn every later hold.
    session_state: Option<SdkSessionState>,
    /// Monotonic counter bumped on every received notification; the holding
    /// debounce resets whenever it changes.
    activity_seq: u64,
}

struct AcpNotificationHandler {
    writer: Arc<dyn MessageWriter>,
    phase: Mutex<HandlerPhase>,
    /// Signalled when replay matching determines all DB messages have been replayed.
    replay_done: tokio::sync::Notify,
    permission_cancel_token: CancellationToken,
    /// This connection's task-tracking mode, fixed by [`setup_acp_session`]
    /// right after the initialize response — race-free, because `session/new`
    /// (and hence any update) only happens after initialize resolves. Unset
    /// reads [`TaskTrackingMode::Raw`].
    task_tracking_mode: OnceLock<TaskTrackingMode>,
    /// Live background tasks reported over `_claude/sdkMessage` frames.
    background_tasks: Mutex<BackgroundTaskSet>,
    /// Live typed asyncTasks — the task-set source in typed mode, maintained
    /// alongside the raw set (the mode selects which one is published).
    typed_tasks: Mutex<TypedAsyncTaskSet>,
    /// Publishes [`BackgroundActivity`] to the post-turn holding wait.
    background_activity_tx: watch::Sender<BackgroundActivity>,
    /// Most recent autonomous-cycle `origin.kind` seen on a raw
    /// `_claude/sdkMessage` frame, cleared on entry to each hold so a
    /// continuation record is only ever tagged from *its own* evidence.
    autonomous_origin: Mutex<Option<String>>,
    /// Name of the typed async task that most recently settled (left the live
    /// set), cleared on entry to each hold like [`Self::autonomous_origin`]:
    /// a settled task is what wakes a `task-notification` cycle, so its name
    /// labels the continuation record with what woke it. Only typed mode ever
    /// sets it — raw frames carry no task names.
    woke_task_name: Mutex<Option<String>>,
    /// Tool-call ids announced by `session/update` on this connection — the
    /// only state an incoming permission request can be correlated against
    /// (see [`AcpNotificationHandler::request_permission`]).
    announced_tool_calls: Mutex<HashSet<String>>,
    /// Notifies permission requests waiting for their tool call to be
    /// announced.
    tool_call_announced: tokio::sync::Notify,
    /// Tool-call ids with a permission request already awaiting a decision.
    /// A second, concurrent request for the same id is a desync, not a queue.
    pending_permissions: Mutex<HashSet<String>>,
    /// How to resolve permission requests that arrive out-of-turn.
    out_of_turn_permissions: OutOfTurnPermissionPolicy,
}

impl AcpNotificationHandler {
    fn new(
        writer: Arc<dyn MessageWriter>,
        replaying: bool,
        db_messages: Vec<ReplayBoundary>,
        permission_cancel_token: CancellationToken,
    ) -> Self {
        let phase = if replaying {
            HandlerPhase::Replaying(ReplayBuffer::new(db_messages))
        } else {
            HandlerPhase::Live {
                replayed_tool_call_ids: HashSet::new(),
                message_id: None,
                close_open_record: false,
            }
        };

        Self {
            writer,
            phase: Mutex::new(phase),
            replay_done: tokio::sync::Notify::new(),
            permission_cancel_token,
            task_tracking_mode: OnceLock::new(),
            background_tasks: Mutex::new(BackgroundTaskSet::default()),
            typed_tasks: Mutex::new(TypedAsyncTaskSet::default()),
            background_activity_tx: watch::Sender::new(BackgroundActivity::default()),
            autonomous_origin: Mutex::new(None),
            woke_task_name: Mutex::new(None),
            announced_tool_calls: Mutex::new(HashSet::new()),
            tool_call_announced: tokio::sync::Notify::new(),
            pending_permissions: Mutex::new(HashSet::new()),
            out_of_turn_permissions: OutOfTurnPermissionPolicy::default(),
        }
    }

    /// Set the policy for permission requests arriving during a background
    /// hold. Only meaningful with a [`BackgroundHoldConfig`] on the driver.
    fn with_out_of_turn_permissions(mut self, policy: OutOfTurnPermissionPolicy) -> Self {
        self.out_of_turn_permissions = policy;
        self
    }

    /// Fix this connection's task-tracking mode, decided by the initialize
    /// response's capability mirror. Called exactly once, before
    /// `session/new` is sent, so no `session/update` can race the decision.
    fn set_task_tracking_mode(&self, mode: TaskTrackingMode) {
        let was_unset = self.task_tracking_mode.set(mode).is_ok();
        debug_assert!(was_unset, "task tracking mode is set once per connection");
    }

    fn task_tracking_mode(&self) -> TaskTrackingMode {
        self.task_tracking_mode.get().copied().unwrap_or_default()
    }

    /// The live-task signal the holding wait should see — the count plus the
    /// named snapshot, from whichever set this connection's mode selects. Raw
    /// frames and typed updates both publish through this, so neither source
    /// can clobber the other's count; only typed mode can name its tasks, so
    /// raw mode pairs its count with an empty list.
    async fn mode_selected_task_snapshot(&self) -> (usize, Vec<BackgroundHoldTask>) {
        match self.task_tracking_mode() {
            TaskTrackingMode::Raw => (
                self.background_tasks.lock().await.task_ids.len(),
                Vec::new(),
            ),
            TaskTrackingMode::Typed => {
                let tasks = self.typed_tasks.lock().await;
                (tasks.live_count(), tasks.live_snapshot())
            }
        }
    }

    /// Subscribe to the connection's background-activity signal.
    fn subscribe_background_activity(&self) -> watch::Receiver<BackgroundActivity> {
        self.background_activity_tx.subscribe()
    }

    /// Count a received notification as activity for the holding debounce.
    fn note_activity(&self) {
        self.background_activity_tx
            .send_modify(|activity| activity.activity_seq = activity.activity_seq.wrapping_add(1));
    }

    async fn finalize_replay_if_idle(&self, timeout: Duration) -> bool {
        let mut phase = self.phase.lock().await;
        if let HandlerPhase::Replaying(buf) = &mut *phase {
            if buf.received_any && buf.last_notification_at.elapsed() >= timeout {
                let completed = buf.finalize_current();
                if completed {
                    self.replay_done.notify_one();
                }
                return true;
            }
        }
        false
    }

    /// Transition from Replaying to WaitingForPrompt.
    /// Extracts the replayed_tool_call_ids from the ReplayBuffer.
    async fn transition_to_waiting_for_prompt(&self) {
        let mut phase = self.phase.lock().await;
        let ids = match &mut *phase {
            HandlerPhase::Replaying(buf) => {
                let completed = buf.finalize_current();
                if completed {
                    self.replay_done.notify_one();
                }
                std::mem::take(&mut buf.replayed_tool_call_ids)
            }
            HandlerPhase::WaitingForPrompt { .. }
            | HandlerPhase::Live { .. }
            | HandlerPhase::BackgroundHolding { .. } => return,
        };
        *phase = HandlerPhase::WaitingForPrompt {
            replayed_tool_call_ids: ids,
        };
    }

    /// Transition from WaitingForPrompt, Replaying, or BackgroundHolding to
    /// Live — a prompt is about to be sent.
    ///
    /// Coming out of a hold, an open continuation record is handed to the new
    /// turn as `close_open_record`: the turn's first write closes it, so the
    /// continuation's text cannot bleed into the new turn's message.
    async fn transition_to_live(&self) {
        let mut phase = self.phase.lock().await;
        let (ids, close_open_record) = match &mut *phase {
            HandlerPhase::WaitingForPrompt {
                replayed_tool_call_ids,
            } => (std::mem::take(replayed_tool_call_ids), false),
            HandlerPhase::Replaying(buf) => {
                (std::mem::take(&mut buf.replayed_tool_call_ids), false)
            }
            HandlerPhase::BackgroundHolding {
                replayed_tool_call_ids,
                continuation,
                ..
            } => (
                std::mem::take(replayed_tool_call_ids),
                continuation.is_some(),
            ),
            HandlerPhase::Live { .. } => return,
        };
        *phase = HandlerPhase::Live {
            replayed_tool_call_ids: ids,
            message_id: None,
            close_open_record,
        };
    }

    /// Transition from Live to BackgroundHolding — the prompt resolved, but the
    /// connection stays up for background work, so everything that arrives from
    /// here on is an out-of-turn continuation.
    async fn transition_to_background_holding(&self) {
        // Only this hold's own evidence may name its continuation: neither a
        // raw frame's origin kind nor a settled task from before the hold.
        *self.autonomous_origin.lock().await = None;
        *self.woke_task_name.lock().await = None;
        let mut phase = self.phase.lock().await;
        let (ids, turn_message_id) = match &mut *phase {
            HandlerPhase::Live {
                replayed_tool_call_ids,
                message_id,
                ..
            } => (std::mem::take(replayed_tool_call_ids), message_id.take()),
            // A hold is only ever entered from a resolved live turn.
            HandlerPhase::Replaying(_)
            | HandlerPhase::WaitingForPrompt { .. }
            | HandlerPhase::BackgroundHolding { .. } => return,
        };
        *phase = HandlerPhase::BackgroundHolding {
            replayed_tool_call_ids: ids,
            turn_message_id,
            continuation: None,
        };
    }

    fn cancel_pending_permissions(&self) {
        self.permission_cancel_token.cancel();
    }

    async fn is_replay_complete(&self) -> bool {
        let phase = self.phase.lock().await;
        match &*phase {
            HandlerPhase::Replaying(buf) => buf.is_complete(),
            HandlerPhase::WaitingForPrompt { .. }
            | HandlerPhase::Live { .. }
            | HandlerPhase::BackgroundHolding { .. } => true,
        }
    }
}

impl AcpNotificationHandler {
    /// Handle a `session/request_permission`.
    ///
    /// Two things beyond the in-turn path:
    ///
    /// - **Attribution.** A request that arrives while the connection is
    ///   holding for background work belongs to a background continuation, not
    ///   to the turn that already finished, and is tagged
    ///   [`AcpPermissionRequest::origin`] so clients present it that way.
    ///   [`OutOfTurnPermissionPolicy`] can resolve such a request without
    ///   prompting at all, so an unattended session can't wedge on a decision
    ///   nobody is there to make.
    /// - **Desync defense** ([claude-agent-acp#851]). An incoming permission id
    ///   is never assumed to map to state this client knows: a duplicate id, or
    ///   an out-of-turn id for a tool call that was never announced, is
    ///   answered defensively (reject, or `Cancelled` when the request offers
    ///   no rejection option) with a logged warning — rather than dispatched
    ///   and blocked on a correlation that may never resolve. This extends the
    ///   `permission_cancel_token` path, which already models responding
    ///   `Cancelled` as the safe default.
    ///
    /// [claude-agent-acp#851]: https://github.com/agentclientprotocol/claude-agent-acp/issues/851
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        if self.permission_cancel_token.is_cancelled() {
            return Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ));
        }

        let mut request = acp_permission_request_from_args(&args);
        request.origin = self.out_of_turn_origin().await;

        // Registered before anything can await a decision, so a duplicate is
        // recognized while the first request is still outstanding.
        if !self
            .pending_permissions
            .lock()
            .await
            .insert(request.tool_call_id.clone())
        {
            log::warn!(
                "ACP session {}: duplicate permission request for tool call {} while one is \
                 still pending — answering defensively (claude-agent-acp#851)",
                request.session_id,
                request.tool_call_id
            );
            return Ok(permission_response_for_decision(
                defensive_permission_decision(&request),
            ));
        }

        let response = self.resolve_permission(request.clone()).await;
        self.pending_permissions
            .lock()
            .await
            .remove(&request.tool_call_id);
        Ok(response)
    }

    async fn resolve_permission(&self, request: AcpPermissionRequest) -> RequestPermissionResponse {
        if let Some(origin) = request.origin.clone() {
            // Out-of-turn: the tool call must be one this connection actually
            // saw announced, or there is nothing to correlate the request to.
            if !self
                .await_tool_call_announcement(&request.tool_call_id, PERMISSION_ANNOUNCEMENT_GRACE)
                .await
            {
                log::warn!(
                    "ACP session {}: out-of-turn permission request for unannounced tool call \
                     {} ({origin}) — answering defensively (claude-agent-acp#851)",
                    request.session_id,
                    request.tool_call_id
                );
                return permission_response_for_decision(defensive_permission_decision(&request));
            }

            let auto_decision = match self.out_of_turn_permissions {
                OutOfTurnPermissionPolicy::Prompt => None,
                OutOfTurnPermissionPolicy::AutoAllow => {
                    Some(autoapprove_permission_decision(&request))
                }
                OutOfTurnPermissionPolicy::AutoDeny => {
                    Some(defensive_permission_decision(&request))
                }
            };
            if let Some(decision) = auto_decision {
                log::info!(
                    "ACP session {}: auto-resolving {origin} permission request for tool call {} \
                     as {decision:?} ({:?} policy)",
                    request.session_id,
                    request.tool_call_id,
                    self.out_of_turn_permissions
                );
                return permission_response_for_decision(decision);
            }
            log::info!(
                "ACP session {}: presenting {origin} permission request for tool call {}",
                request.session_id,
                request.tool_call_id
            );
        }

        let decision = self
            .writer
            .request_permission(request, self.permission_cancel_token.clone())
            .await;
        permission_response_for_decision(decision)
    }

    /// The continuation origin tag when the handler is holding for background
    /// work, `None` during a live turn.
    async fn out_of_turn_origin(&self) -> Option<String> {
        let origin_kind = self.autonomous_origin.lock().await.clone();
        let woke_task_name = self.woke_task_name.lock().await.clone();
        let phase = self.phase.lock().await;
        matches!(&*phase, HandlerPhase::BackgroundHolding { .. }).then(|| {
            labeled_background_continuation_origin(
                origin_kind.as_deref(),
                woke_task_name.as_deref(),
            )
        })
    }

    /// Whether `tool_call_id` was announced by a `session/update` on this
    /// connection, waiting up to `grace` for an announcement still in flight.
    ///
    /// The announcement and the permission request are separate frames, so a
    /// legitimate request can beat its own `tool_call` update through the
    /// dispatcher; without the grace window that race would look exactly like
    /// the #851 desync and get a legitimate tool call denied.
    async fn await_tool_call_announcement(&self, tool_call_id: &str, grace: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            let announced = self.tool_call_announced.notified();
            // Register interest *before* the check so an announcement landing
            // between them still wakes this wait.
            tokio::pin!(announced);
            announced.as_mut().enable();
            if self
                .announced_tool_calls
                .lock()
                .await
                .contains(tool_call_id)
            {
                return true;
            }
            tokio::select! {
                _ = announced => {}
                _ = tokio::time::sleep_until(deadline) => {
                    return self.announced_tool_calls.lock().await.contains(tool_call_id);
                }
            }
        }
    }

    /// Record a tool-call id announced by a `session/update`, waking any
    /// permission request waiting to correlate against it.
    async fn announce_tool_call(&self, tool_call_id: &str) {
        if self
            .announced_tool_calls
            .lock()
            .await
            .insert(tool_call_id.to_string())
        {
            self.tool_call_announced.notify_waiters();
        }
    }

    /// Handles extension notifications, i.e. the `_claude/sdkMessage` frames
    /// carrying raw Claude Agent SDK messages (params `{ sessionId, message }`)
    /// requested via [`background_task_tracking_meta`]. Maintains the live
    /// background-task set and publishes it — with the frames-seen
    /// availability latch — to the post-turn holding wait.
    async fn ext_notification(
        &self,
        notification: ExtNotification,
    ) -> agent_client_protocol::Result<()> {
        if notification.method.as_ref() != CLAUDE_SDK_MESSAGE_METHOD {
            log::debug!(
                "Ignoring unrecognized ACP extension notification '{}'",
                notification.method
            );
            return Ok(());
        }
        let params: serde_json::Value = match serde_json::from_str(notification.params.get()) {
            Ok(params) => params,
            Err(e) => {
                log::warn!("Failed to parse {CLAUDE_SDK_MESSAGE_METHOD} params: {e}");
                return Ok(());
            }
        };
        let Some(message) = params.get("message") else {
            log::warn!("{CLAUDE_SDK_MESSAGE_METHOD} params carried no message field");
            return Ok(());
        };

        {
            let mut tasks = self.background_tasks.lock().await;
            if tasks.apply_sdk_message(message) {
                let session_id = params
                    .get("sessionId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("<unknown>");
                log::info!(
                    "ACP session {session_id}: live background tasks now [{}]",
                    tasks.sorted_ids().join(", ")
                );
            }
        }
        // In typed mode the raw stream still feeds the idle latch and the
        // attribution origin, but the task set the holding wait sees is the
        // typed one — a raw frame must not clobber its count.
        let (live_tasks, tasks) = self.mode_selected_task_snapshot().await;
        // Attribution source for continuation records: the raw stream is the
        // only place the bridge names the *kind* of cycle a set of out-of-turn
        // `session/update`s belongs to.
        if let Some(kind) = sdk_message_origin_kind(message) {
            *self.autonomous_origin.lock().await = Some(kind);
        }
        // Any raw frame — the `init` availability probe included — both
        // proves the raw-SDK stream works on this connection and counts as
        // activity for the holding debounce. A frame that mentions a task
        // additionally latches `ever_started_task`, switching the holding
        // wait from the short taskless confirmation to the full drain
        // debounce for the rest of the connection; a `session_state_changed`
        // frame moves the idle latch.
        let mentions_task = sdk_message_mentions_task(message);
        let session_state = sdk_message_session_state(message);
        self.background_activity_tx.send_modify(|activity| {
            activity.sdk_frames_seen = true;
            activity.ever_started_task |= mentions_task || live_tasks > 0;
            activity.live_tasks = live_tasks;
            activity.tasks = tasks;
            if session_state.is_some() {
                activity.session_state = session_state;
            }
            activity.activity_seq = activity.activity_seq.wrapping_add(1);
        });
        Ok(())
    }

    /// Handle one typed asyncTasks `session/update` (the
    /// [`IncomingSessionUpdate::AsyncTask`] arm). Maintains the typed task
    /// set and publishes the mode-selected live count to the post-turn
    /// holding wait; counts as activity for the holding debounce, like every
    /// other notification on the connection.
    async fn async_task_update(
        &self,
        notification: AsyncTaskNotification,
    ) -> agent_client_protocol::Result<()> {
        if let AsyncTaskUpdate::Spawned {
            task_id,
            name,
            description,
            output_file_path,
            tool_call_id,
        } = &notification.update
        {
            log::info!(
                "ACP session {}: async task {task_id} spawned (name: {name:?}, description: \
                 {description:?}, output: {output_file_path:?}, tool call: {tool_call_id:?})",
                notification.session_id
            );
        }
        {
            let mut tasks = self.typed_tasks.lock().await;
            let membership_changed = tasks.apply(&notification.update);
            if membership_changed {
                log::info!(
                    "ACP session {}: live async tasks now [{}]",
                    notification.session_id,
                    tasks.live_ids().join(", ")
                );
            }
            // A task leaving the live set is what wakes a `task-notification`
            // continuation cycle, so remember its announced name (or that it
            // had none — a later unnamed settle must not inherit an earlier
            // task's name) to label the continuation with what woke it.
            if let AsyncTaskUpdate::StateUpdate { task_id, state } = &notification.update {
                if membership_changed && !state.is_live() {
                    *self.woke_task_name.lock().await = tasks.task_name(task_id);
                }
            }
        }
        let (live_tasks, tasks) = self.mode_selected_task_snapshot().await;
        self.background_activity_tx.send_modify(|activity| {
            activity.ever_started_task |= live_tasks > 0;
            activity.live_tasks = live_tasks;
            activity.tasks = tasks;
            activity.activity_seq = activity.activity_seq.wrapping_add(1);
        });
        Ok(())
    }

    async fn session_notification(
        &self,
        notification: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        // Every received update counts as activity for the post-turn holding
        // debounce — out-of-turn continuations stream as ordinary
        // session/updates, and the hold must not declare quiescence mid-burst.
        self.note_activity();

        // Session state updates are forwarded regardless of phase.
        match &notification.update {
            SessionUpdate::SessionInfoUpdate(info) => {
                self.writer.on_session_info_update(info).await;
                return Ok(());
            }
            SessionUpdate::ConfigOptionUpdate(update) => {
                self.writer
                    .on_config_option_update(&update.config_options)
                    .await;
                return Ok(());
            }
            SessionUpdate::CurrentModeUpdate(update) => {
                self.writer
                    .record_acp_event_metadata(AcpEventMetadata {
                        event_kind: Some("current_mode_update".to_string()),
                        content: serialize_value(update),
                        ..Default::default()
                    })
                    .await;
                return Ok(());
            }
            SessionUpdate::AvailableCommandsUpdate(update) => {
                self.writer
                    .record_acp_event_metadata(AcpEventMetadata {
                        event_kind: Some("available_commands_update".to_string()),
                        content: serialize_value(update),
                        ..Default::default()
                    })
                    .await;
                return Ok(());
            }
            _ => {}
        }

        // Any tool call announced on this connection is the only state a later
        // `session/request_permission` can be correlated against (#851).
        if let Some(id) = notification_tool_call_id(&notification.update) {
            self.announce_tool_call(&id).await;
        }

        // Attribution evidence, read before the phase lock: a continuation
        // record is tagged with whatever the raw-SDK stream last named,
        // labeled with the typed task whose settling woke it when known.
        let origin_kind = self.autonomous_origin.lock().await.clone();
        let woke_task_name = self.woke_task_name.lock().await.clone();

        // Determine the action to take under the lock, then drop the lock
        // before calling into the writer to avoid holding it across await points.
        let decision = {
            let mut phase = self.phase.lock().await;

            match &mut *phase {
                // ── Replaying phase: accumulate chunks, match against DB ──
                HandlerPhase::Replaying(buf) => {
                    buf.last_notification_at = Instant::now();
                    buf.received_any = true;

                    // Record tool-call IDs for the safety-net.
                    if let Some(id) = notification_tool_call_id(&notification.update) {
                        buf.replayed_tool_call_ids.insert(id);
                    }

                    let completed = match &notification.update {
                        SessionUpdate::AgentMessageChunk(chunk) => {
                            if let AcpContentBlock::Text(text) = &chunk.content {
                                buf.push_text_chunk(
                                    "assistant",
                                    chunk.message_id.as_ref().map(|id| id.0.as_ref()),
                                    &text.text,
                                )
                            } else {
                                false
                            }
                        }
                        SessionUpdate::UserMessageChunk(chunk) => {
                            if let AcpContentBlock::Text(text) = &chunk.content {
                                buf.push_text_chunk(
                                    "user",
                                    chunk.message_id.as_ref().map(|id| id.0.as_ref()),
                                    &text.text,
                                )
                            } else {
                                false
                            }
                        }
                        SessionUpdate::ToolCall(tc) => {
                            buf.finalize_current();
                            buf.try_match(&ReplayEvent {
                                role: "tool_call".to_string(),
                                content: tc.title.clone(),
                                acp_message_id: None,
                                acp_tool_call_id: Some(tc.tool_call_id.0.to_string()),
                            })
                        }
                        SessionUpdate::ToolCallUpdate(update)
                            if update.fields.content.is_some() =>
                        {
                            buf.finalize_current();
                            buf.try_match(&ReplayEvent {
                                role: "tool_result".to_string(),
                                content: update
                                    .fields
                                    .content
                                    .as_ref()
                                    .and_then(|content| extract_content_preview(content))
                                    .unwrap_or_default(),
                                acp_message_id: None,
                                acp_tool_call_id: Some(update.tool_call_id.0.to_string()),
                            })
                        }
                        SessionUpdate::AgentThoughtChunk(_) => {
                            // Thinking is not persisted — ignore.
                            false
                        }
                        _ => false,
                    };

                    if completed {
                        self.replay_done.notify_one();
                    }
                    return Ok(());
                }

                // ── WaitingForPrompt phase: drop everything, record tool-call IDs ──
                HandlerPhase::WaitingForPrompt {
                    replayed_tool_call_ids,
                } => {
                    if let Some(id) = notification_tool_call_id(&notification.update) {
                        replayed_tool_call_ids.insert(id);
                    }
                    return Ok(());
                }

                // ── Live phase: determine action, then release lock ──
                HandlerPhase::Live {
                    replayed_tool_call_ids,
                    message_id,
                    close_open_record,
                } => {
                    // Safety net: drop notifications for tool-call IDs seen during replay.
                    if let Some(id) = notification_tool_call_id(&notification.update) {
                        if replayed_tool_call_ids.contains(&id) {
                            return Ok(());
                        }
                    }

                    let action = live_action_for_update(&notification.update);
                    // Remember the id this turn is streaming into so a hold
                    // entered after it can force its continuation elsewhere.
                    if let Some(Some(chunk_id)) = text_chunk_message_id(&notification.update) {
                        *message_id = Some(chunk_id.to_string());
                    }
                    // A continuation was open when this turn started: close it
                    // before the turn records anything, or the turn's message
                    // would absorb the continuation's text.
                    let close_record = *close_open_record && action.records_something();
                    if close_record {
                        *close_open_record = false;
                    }
                    LiveDecision {
                        close_open_record: close_record,
                        action,
                    }
                }

                // ── BackgroundHolding phase: out-of-turn continuation ──
                HandlerPhase::BackgroundHolding {
                    replayed_tool_call_ids,
                    turn_message_id,
                    continuation,
                } => {
                    if let Some(id) = notification_tool_call_id(&notification.update) {
                        if replayed_tool_call_ids.contains(&id) {
                            return Ok(());
                        }
                    }

                    let mut action = live_action_for_update(&notification.update);
                    if !action.records_something() {
                        LiveDecision {
                            close_open_record: false,
                            action,
                        }
                    } else {
                        // Text from a different message than the open record is
                        // a second autonomous cycle: its own record, not an
                        // extension of the first.
                        let text_chunk = text_chunk_message_id(&notification.update);
                        let foreign_chunk = text_chunk.is_some_and(|chunk_id| {
                            continuation
                                .as_ref()
                                .is_some_and(|record| record.is_foreign_text_chunk(chunk_id))
                        });
                        let open_record = continuation.is_none() || foreign_chunk;
                        if open_record {
                            *continuation = Some(ContinuationRecord::new(
                                labeled_background_continuation_origin(
                                    origin_kind.as_deref(),
                                    woke_task_name.as_deref(),
                                ),
                            ));
                        }
                        let record = continuation.as_mut().expect("record opened above");
                        let boundary_id = text_chunk.map(|chunk_id| {
                            record.text_message_id(chunk_id, turn_message_id.as_deref())
                        });
                        action.attribute_to(&record.origin, boundary_id.as_deref());
                        LiveDecision {
                            close_open_record: open_record,
                            action,
                        }
                    }
                }
            }
            // phase lock is dropped here
        };

        // A new record starts by closing whatever message was open, so the
        // writer's next append lands in a row of its own.
        if decision.close_open_record {
            self.writer.finalize().await;
        }

        // Execute the live action without holding the phase lock.
        match decision.action {
            LiveAction::AppendText { text, metadata } => {
                self.writer.append_text(&text).await;
                self.writer.record_acp_event_metadata(metadata).await;
            }
            LiveAction::RecordAcpEvent(metadata) => {
                self.writer.record_acp_event_metadata(metadata).await;
            }
            LiveAction::RecordToolCall {
                id,
                title,
                raw_input,
                metadata,
            } => {
                self.writer
                    .record_tool_call(&id, &title, raw_input.as_ref())
                    .await;
                self.writer.record_tool_call_metadata(metadata).await;
            }
            LiveAction::ToolCallUpdate {
                id,
                title,
                raw_input,
                result,
                metadata,
            } => {
                if title.is_some() || raw_input.is_some() {
                    self.writer
                        .update_tool_call_title(&id, title.as_deref(), raw_input.as_ref())
                        .await;
                }
                self.writer.record_tool_call_metadata(metadata).await;
                if let Some(preview) = result {
                    self.writer.record_tool_result(&id, &preview).await;
                }
            }
            LiveAction::Ignore => {
                log::debug!("Ignoring session update: {:?}", notification.update);
            }
            LiveAction::Drop => {}
        }
        Ok(())
    }
}

/// What one `session/update` records, decided under the handler's phase lock
/// and executed once the lock is dropped (the writer must never be called with
/// it held).
enum LiveAction {
    AppendText {
        text: String,
        metadata: AcpEventMetadata,
    },
    RecordAcpEvent(AcpEventMetadata),
    RecordToolCall {
        id: String,
        title: String,
        raw_input: Option<serde_json::Value>,
        metadata: AcpToolCallMetadata,
    },
    ToolCallUpdate {
        id: String,
        title: Option<String>,
        raw_input: Option<serde_json::Value>,
        result: Option<String>,
        metadata: AcpToolCallMetadata,
    },
    Ignore,
    Drop,
}

impl LiveAction {
    /// Whether this action writes anything at all — an update that records
    /// nothing needs neither a continuation record nor a message boundary.
    fn records_something(&self) -> bool {
        !matches!(self, LiveAction::Ignore | LiveAction::Drop)
    }

    /// Attribute what this action records to a background continuation: the
    /// origin tag on every row, plus the record's own ACP message id on
    /// streamed text so the continuation is a distinct message boundary.
    fn attribute_to(&mut self, origin: &str, message_id: Option<&str>) {
        match self {
            LiveAction::AppendText { metadata, .. } | LiveAction::RecordAcpEvent(metadata) => {
                metadata.origin = Some(origin.to_string());
                if let Some(id) = message_id {
                    metadata.message_id = Some(id.to_string());
                }
            }
            LiveAction::RecordToolCall { metadata, .. }
            | LiveAction::ToolCallUpdate { metadata, .. } => {
                metadata.origin = Some(origin.to_string());
            }
            LiveAction::Ignore | LiveAction::Drop => {}
        }
    }
}

/// The handler's decision for one update: what to record, and whether to close
/// the currently-open message record first (a message-boundary change).
struct LiveDecision {
    close_open_record: bool,
    action: LiveAction,
}

/// Map a `session/update` to what it records. Shared by the `Live` and
/// `BackgroundHolding` phases — an out-of-turn continuation streams the same
/// update kinds as a live turn, and only its attribution differs.
fn live_action_for_update(update: &SessionUpdate) -> LiveAction {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            let metadata = message_chunk_metadata("agent_message_chunk", chunk);
            if let AcpContentBlock::Text(text) = &chunk.content {
                LiveAction::AppendText {
                    text: text.text.clone(),
                    metadata,
                }
            } else {
                LiveAction::RecordAcpEvent(metadata)
            }
        }
        SessionUpdate::UserMessageChunk(chunk) => {
            LiveAction::RecordAcpEvent(message_chunk_metadata("user_message_chunk", chunk))
        }
        SessionUpdate::ToolCall(tool_call) => {
            let id = tool_call.tool_call_id.0.to_string();
            LiveAction::RecordToolCall {
                id: id.clone(),
                title: tool_call.title.clone(),
                raw_input: tool_call.raw_input.clone(),
                metadata: AcpToolCallMetadata {
                    event_kind: Some("tool_call".to_string()),
                    tool_call_id: Some(id),
                    tool_kind: serialize_as_string(&tool_call.kind),
                    tool_status: serialize_as_string(&tool_call.status),
                    raw_input: tool_call.raw_input.clone(),
                    raw_output: tool_call.raw_output.clone(),
                    content: serialize_non_empty(&tool_call.content),
                    locations: serialize_non_empty(&tool_call.locations),
                    ..Default::default()
                },
            }
        }
        SessionUpdate::ToolCallUpdate(update) => {
            let tc_id = update.tool_call_id.0.to_string();
            let title = update.fields.title.clone();
            let raw_input = update.fields.raw_input.clone();
            let metadata = AcpToolCallMetadata {
                event_kind: Some("tool_call_update".to_string()),
                tool_call_id: Some(tc_id.clone()),
                tool_kind: update.fields.kind.as_ref().and_then(serialize_as_string),
                tool_status: update.fields.status.as_ref().and_then(serialize_as_string),
                raw_input: raw_input.clone(),
                raw_output: update.fields.raw_output.clone(),
                content: update
                    .fields
                    .content
                    .as_ref()
                    .and_then(|content| serialize_non_empty(content)),
                locations: update
                    .fields
                    .locations
                    .as_ref()
                    .and_then(|locations| serialize_non_empty(locations)),
                ..Default::default()
            };
            let result = update
                .fields
                .content
                .as_ref()
                .and_then(|c| extract_content_preview(c));
            if title.is_some()
                || raw_input.is_some()
                || result.is_some()
                || metadata.has_update_fields()
            {
                LiveAction::ToolCallUpdate {
                    id: tc_id,
                    title,
                    raw_input,
                    result,
                    metadata,
                }
            } else {
                LiveAction::Drop
            }
        }
        SessionUpdate::UsageUpdate(update) => {
            LiveAction::RecordAcpEvent(usage_update_metadata(update))
        }
        SessionUpdate::Plan(plan) => LiveAction::RecordAcpEvent(AcpEventMetadata {
            event_kind: Some("plan_update".to_string()),
            content: serialize_value(plan),
            ..Default::default()
        }),
        _ => LiveAction::Ignore,
    }
}

/// Whether an update streams text into a message record — and the ACP message
/// id the provider gave it, if any.
///
/// `None` for updates that stream no text: only text chunks define a message
/// boundary, because only they are appended into a message row.
fn text_chunk_message_id(update: &SessionUpdate) -> Option<Option<&str>> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            matches!(chunk.content, AcpContentBlock::Text(_))
                .then(|| chunk.message_id.as_ref().map(|id| id.0.as_ref()))
        }
        _ => None,
    }
}

/// The autonomous-cycle `origin.kind` a raw `_claude/sdkMessage` frame reveals,
/// if any.
///
/// The bridge stamps `origin` on the frames of a cycle the model ran on its own
/// (`assistant`/`result`), which is the precise attribution for a continuation.
/// A `task_notification` system frame carries no `origin` of its own, but a
/// settled background task is exactly what wakes a `task-notification` cycle,
/// so it stands in for one.
fn sdk_message_origin_kind(message: &serde_json::Value) -> Option<String> {
    if let Some(kind) = message
        .pointer("/origin/kind")
        .and_then(serde_json::Value::as_str)
    {
        return Some(kind.to_string());
    }
    let is_task_notification = message.get("type").and_then(serde_json::Value::as_str)
        == Some("system")
        && message.get("subtype").and_then(serde_json::Value::as_str)
            == Some(TASK_NOTIFICATION_SUBTYPE);
    is_task_notification.then(|| TASK_NOTIFICATION_ORIGIN.to_string())
}

#[cfg(test)]
fn permission_decision_for_options(
    options: &[SchemaPermissionOption],
    cancelled: bool,
) -> AcpPermissionDecision {
    if cancelled {
        return AcpPermissionDecision::Cancelled;
    }

    let request = AcpPermissionRequest {
        request_id: "test-request".to_string(),
        session_id: "test-session".to_string(),
        tool_call_id: "test-tool-call".to_string(),
        tool_title: None,
        tool_kind: None,
        tool_status: None,
        raw_input: None,
        raw_output: None,
        content: None,
        locations: None,
        options: options
            .iter()
            .map(acp_permission_option_from_schema)
            .collect(),
        raw_request: None,
        origin: None,
    };

    autoapprove_permission_decision(&request)
}

#[cfg(test)]
fn permission_response_for_options(
    options: &[agent_client_protocol::schema::v1::PermissionOption],
    cancelled: bool,
) -> RequestPermissionResponse {
    permission_response_for_decision(permission_decision_for_options(options, cancelled))
}

fn permission_response_for_decision(decision: AcpPermissionDecision) -> RequestPermissionResponse {
    match decision {
        AcpPermissionDecision::Cancelled => {
            RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
        }
        AcpPermissionDecision::Selected { option_id } => {
            RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                SelectedPermissionOutcome::new(PermissionOptionId::new(option_id)),
            ))
        }
    }
}

fn acp_permission_option_from_schema(option: &SchemaPermissionOption) -> AcpPermissionOption {
    AcpPermissionOption {
        option_id: option.option_id.0.as_ref().to_string(),
        name: option.name.clone(),
        kind: option.kind.into(),
    }
}

fn acp_permission_request_from_args(args: &RequestPermissionRequest) -> AcpPermissionRequest {
    let tool = &args.tool_call;
    let tool_call_id = tool.tool_call_id.0.as_ref().to_string();
    let request_counter = PERMISSION_REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    AcpPermissionRequest {
        request_id: format!(
            "{}:{tool_call_id}:{request_counter}",
            args.session_id.0.as_ref()
        ),
        session_id: args.session_id.0.as_ref().to_string(),
        tool_call_id,
        tool_title: tool.fields.title.clone(),
        tool_kind: tool.fields.kind.as_ref().and_then(serialize_as_string),
        tool_status: tool.fields.status.as_ref().and_then(serialize_as_string),
        raw_input: tool.fields.raw_input.clone(),
        raw_output: tool.fields.raw_output.clone(),
        content: tool
            .fields
            .content
            .as_ref()
            .and_then(|content| serialize_non_empty(content)),
        locations: tool
            .fields
            .locations
            .as_ref()
            .and_then(|locations| serialize_non_empty(locations)),
        options: args
            .options
            .iter()
            .map(acp_permission_option_from_schema)
            .collect(),
        raw_request: serialize_value(args),
        // Set by the caller from the handler phase: the wire frame says
        // nothing about whether a turn is still live.
        origin: None,
    }
}

/// Extract the tool-call ID from a session update, if it carries one.
fn notification_tool_call_id(update: &SessionUpdate) -> Option<String> {
    match update {
        SessionUpdate::ToolCall(tc) => Some(tc.tool_call_id.0.to_string()),
        SessionUpdate::ToolCallUpdate(tcu) => Some(tcu.tool_call_id.0.to_string()),
        _ => None,
    }
}

fn message_chunk_metadata(event_kind: &str, chunk: &ContentChunk) -> AcpEventMetadata {
    AcpEventMetadata {
        event_kind: Some(event_kind.to_string()),
        message_id: chunk.message_id.as_ref().map(|id| id.0.to_string()),
        content: serialize_value(chunk),
        ..Default::default()
    }
}

fn usage_update_metadata<T: serde::Serialize>(update: &T) -> AcpEventMetadata {
    AcpEventMetadata {
        event_kind: Some("usage_update".to_string()),
        usage: serialize_value(update),
        content: serialize_value(update),
        ..Default::default()
    }
}

fn prompt_response_metadata(response: &PromptResponse) -> Option<AcpEventMetadata> {
    let usage = response.usage.as_ref().and_then(serialize_value);
    usage.as_ref()?;

    Some(AcpEventMetadata {
        event_kind: Some("prompt_response".to_string()),
        message_id: None,
        usage,
        content: serialize_value(response),
        origin: None,
    })
}

fn send_session_cancel(
    connection: &ConnectionTo<Agent>,
    acp_session_id: &str,
) -> Result<(), String> {
    connection
        .send_notification(CancelNotification::new(acp_session_id.to_string()))
        .map_err(|e| format!("Failed to send ACP session/cancel: {e:?}"))
}

/// Serve one queued per-task stop: send `_session/async_task/stop` for the
/// task and schedule the agent's `{stopped}` answer onto the request's reply
/// channel. Never awaits the answer inline — the holding wait dispatches this
/// from its select loop, and a slow bridge must not stall the hold's other
/// exits. A caller whose reply channel drops unanswered (connection teardown
/// discards the scheduled callback) reads that as the connection closing.
fn serve_async_task_stop(
    connection: &ConnectionTo<Agent>,
    setup: &AcpSessionSetup,
    our_session_id: &str,
    request: StopAsyncTaskRequest,
) {
    let StopAsyncTaskRequest { task_id, reply } = request;
    log::info!(
        "ACP session {our_session_id}: stopping async task {task_id} (session/cancel untouched)"
    );
    let message = match async_task_stop_message(&setup.agent_session_id, &task_id) {
        Ok(message) => message,
        Err(e) => {
            let _ = reply.send(Err(format!(
                "Failed to build async task stop request: {e:?}"
            )));
            return;
        }
    };
    let session_id = our_session_id.to_string();
    let scheduled =
        connection
            .send_request(message)
            .on_receiving_result(move |result| async move {
                let outcome = async_task_stop_outcome(result);
                match &outcome {
                    Ok(stopped) => log::info!(
                        "ACP session {session_id}: async task {task_id} stop answered \
                     (stopped: {stopped})"
                    ),
                    Err(e) => {
                        log::warn!("ACP session {session_id}: async task {task_id} stop: {e}")
                    }
                }
                let _ = reply.send(outcome);
                Ok(())
            });
    if let Err(e) = scheduled {
        // The reply sender moved into the callback and drops with it, which
        // already answers the caller; the send failure is only loggable here.
        log::warn!("ACP session {our_session_id}: failed to send async task stop: {e:?}");
    }
}

// =============================================================================
// Protocol helpers
// =============================================================================

/// How long the holding wait gives the bridge to process a `session/cancel`
/// before teardown. Out-of-turn there is no prompt response to await, so this
/// bounded courtesy drain mirrors the in-turn cancel's bounded response wait
/// (which the bridge answers by aborting the turn).
const HOLDING_CANCEL_DRAIN: Duration = Duration::from_millis(500);

/// Terminal decisions of the holding wait's timer core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HoldSettle {
    Quiescent,
    HeldUntilCap,
}

/// How a post-turn holding wait ended.
enum HoldOutcome {
    /// The background task set drained and the debounce elapsed — safe to
    /// tear down.
    Quiescent,
    /// The hard cap expired before quiescence could be confirmed.
    HeldUntilCap,
    /// A new prompt arrived; re-enter a live turn on this connection.
    NewTurn(QueuedSessionTurn),
    /// The user cancelled the session.
    Cancelled,
    /// The agent process exited (stderr EOF) while holding.
    ChildExited,
}

// Hand-written so the queued turn (which carries a reply channel, not a
// `Debug` type) doesn't have to be printable.
impl std::fmt::Debug for HoldOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            HoldOutcome::Quiescent => "Quiescent",
            HoldOutcome::HeldUntilCap => "HeldUntilCap",
            HoldOutcome::NewTurn(_) => "NewTurn",
            HoldOutcome::Cancelled => "Cancelled",
            HoldOutcome::ChildExited => "ChildExited",
        })
    }
}

/// Pure decision core for the post-turn holding wait
/// ([`hold_for_background_quiescence`] drives it; unit tests exercise it
/// directly).
///
/// Tracks the two teardown clocks against the live background-task signal:
///
/// - **Quiescence** requires the mode-selected task set to be empty *and* the
///   debounce window to pass with no activity. The debounce is only ever a
///   secondary confirmation on an already-empty set — never the sole gate.
///   Any activity resets it; the set repopulating (a task re-start) disarms
///   it entirely, re-arming the hold. In raw mode the empty set is only
///   trusted once the raw-SDK stream has proven itself (at least one
///   `_claude/sdkMessage` frame this connection), and a set that actually
///   drained confirms with the full debounce — a connection that never
///   mentioned a task confirms with the short taskless one. In typed mode
///   the mirrored asyncTasks capability replaces the stream proof — it is
///   the contract that the lifecycle will be published, with terminal states
///   guaranteed even against a dying bridge — and the spawn announcement
///   rides the tool result mid-turn, so anything the turn launched was
///   announced before the prompt resolved: the short debounce always
///   suffices, absorbing only frame-ordering slop.
/// - **The idle latch** additionally refuses quiescence while the last
///   `session_state_changed` frame said busy: the task set empties on the
///   terminal edge at the very instant a continuation cycle starts, so the
///   SDK's own liveness signal — not the debounce — is what protects a
///   running continuation. Advisory in both modes: a stream that never
///   carries the frame (a non-Claude asyncTasks agent has no raw stream at
///   all) leaves it `None`, which blocks nothing. And stale-bounded: the
///   busy reading's release rides a single trailing `idle` frame, so
///   instead of blocking outright it stretches the required quiet window to
///   [`BackgroundHoldConfig::idle_latch_staleness`] — one lost release
///   costs each hold at most that window, not the cap, while a live
///   continuation keeps resetting the window with its own traffic.
/// - **The hard cap** bounds every hold, and in typed mode also bounds a
///   task parked in `paused`. In raw mode, when no raw SDK frame has arrived
///   the empty set is uninformative (an older bridge or a rejected filter
///   looks identical to "no background work ever started"), so quiescence is
///   never declared and the cap is the only clock — the fallback rule: never
///   trust debounce alone. [`run_acp_session`] never enters the hold in that
///   state (an unproven stream at prompt-resolve means no frame will ever
///   come, so it skips straight to teardown); the rule remains here as
///   defense in depth.
struct HoldingState {
    config: BackgroundHoldConfig,
    /// How this connection tracks tasks — decides the quiescence predicate.
    mode: TaskTrackingMode,
    cap_deadline: tokio::time::Instant,
    /// When the connection last showed activity (any notification).
    quiet_since: tokio::time::Instant,
    live_tasks: usize,
    sdk_frames_seen: bool,
    ever_started_task: bool,
    session_state: Option<SdkSessionState>,
}

impl HoldingState {
    fn new(
        config: BackgroundHoldConfig,
        mode: TaskTrackingMode,
        now: tokio::time::Instant,
        initial: &BackgroundActivity,
    ) -> Self {
        Self {
            mode,
            cap_deadline: now + config.hold_cap,
            quiet_since: now,
            live_tasks: initial.live_tasks,
            sdk_frames_seen: initial.sdk_frames_seen,
            ever_started_task: initial.ever_started_task || initial.live_tasks > 0,
            session_state: initial.session_state,
            config,
        }
    }

    /// Fold in a new activity observation: resets the debounce clock and
    /// re-arms/disarms quiescence from the task set and the idle latch.
    fn observe(&mut self, now: tokio::time::Instant, activity: &BackgroundActivity) {
        self.quiet_since = now;
        self.live_tasks = activity.live_tasks;
        self.session_state = activity.session_state;
        // Availability and task history latch: once frames have arrived on
        // this connection the stream is proven — and once a task has been
        // seen the taskless fast path is off — whatever later snapshots say.
        self.sdk_frames_seen |= activity.sdk_frames_seen;
        self.ever_started_task |= activity.ever_started_task || activity.live_tasks > 0;
    }

    /// Deadline at which quiescence is declared, when eligible. `None` while
    /// tasks are live or (raw mode only) the raw-SDK stream is unproven
    /// (hold to cap); a busy idle latch pushes the deadline out to its
    /// staleness bound rather than withholding one.
    fn quiescence_deadline(&self) -> Option<tokio::time::Instant> {
        let (set_trusted, debounce) = match self.mode {
            // Raw mode infers the set from unordered frames: the stream must
            // have proven itself, and a set that actually drained gets the
            // full debounce in case a re-spawn frame is still in flight.
            TaskTrackingMode::Raw => (
                self.sdk_frames_seen,
                if self.ever_started_task {
                    self.config.debounce
                } else {
                    self.config.taskless_debounce
                },
            ),
            // Typed mode's set is the agent's own lifecycle ledger — the
            // mirrored capability is the availability proof, and spawns are
            // announced before the prompt resolves, so an empty set needs
            // only the short frame-ordering-slop debounce.
            TaskTrackingMode::Typed => (true, self.config.taskless_debounce),
        };
        if !set_trusted || self.live_tasks != 0 {
            return None;
        }
        // A busy idle latch stretches the quiet window instead of blocking
        // outright: with no activity at all for the staleness bound, the
        // reading is stale (its release rode a single frame that may be
        // lost), while a live continuation's own traffic keeps resetting
        // `quiet_since` and with it the window.
        let quiet_window = if self.session_state == Some(SdkSessionState::Busy) {
            debounce.max(self.config.idle_latch_staleness)
        } else {
            debounce
        };
        Some(self.quiet_since + quiet_window)
    }

    /// The next instant at which [`HoldingState::poll_settle`] could decide.
    fn next_deadline(&self) -> tokio::time::Instant {
        match self.quiescence_deadline() {
            Some(deadline) => deadline.min(self.cap_deadline),
            None => self.cap_deadline,
        }
    }

    /// The settle decision at `now`, if a deadline has passed. Quiescence is
    /// preferred over the cap when both have (the set was confirmed empty).
    fn poll_settle(&self, now: tokio::time::Instant) -> Option<HoldSettle> {
        match self.quiescence_deadline() {
            Some(deadline) if now >= deadline => Some(HoldSettle::Quiescent),
            _ if now >= self.cap_deadline => Some(HoldSettle::HeldUntilCap),
            _ => None,
        }
    }
}

/// Post-turn holding wait: keep the connection alive after a completed prompt
/// until it is safe to tear the agent down — or a reason to stop holding
/// arrives (new prompt, user cancel, child exit, hard cap).
///
/// The wait also serves the connection's hold-control channel: a per-task
/// stop ([`SessionConnection::stop_async_task`]) is only meaningful against
/// the hold's live task set, so requests are dispatched here — anything
/// queued from before this hold, or still queued when it ends, is answered
/// with an error rather than left to act on a set it wasn't aimed at.
/// `hold_active` mirrors this window to [`AsyncTaskStopHandle`], which
/// rejects a stop outright while it reads false instead of queueing a
/// request nothing is serving.
#[allow(clippy::too_many_arguments)]
async fn hold_for_background_quiescence(
    config: &BackgroundHoldConfig,
    mode: TaskTrackingMode,
    handler: &Arc<AcpNotificationHandler>,
    cancel_token: &CancellationToken,
    child_exited: &CancellationToken,
    prompt_rx: &mut mpsc::UnboundedReceiver<QueuedSessionTurn>,
    hold_control_rx: &mut mpsc::UnboundedReceiver<StopAsyncTaskRequest>,
    hold_active: &AtomicBool,
    serve_stop: &dyn Fn(StopAsyncTaskRequest),
    our_session_id: &str,
    observer: Option<&BackgroundHoldObserver>,
) -> HoldOutcome {
    reject_queued_stop_requests(
        hold_control_rx,
        "the request predates the current background hold",
    );
    hold_active.store(true, Ordering::Relaxed);

    let mut activity_rx = handler.subscribe_background_activity();
    let initial = activity_rx.borrow_and_update().clone();
    let mut state = HoldingState::new(config.clone(), mode, tokio::time::Instant::now(), &initial);

    // Report the wait for as long as it lasts: entering the hold, every change
    // to the live-task signal while in it, and the cleared status on the way
    // out (whatever ends it). The session stays `running` throughout — holding
    // is a sub-state, not a terminal status.
    let mut reported = BackgroundHoldStatus::default();
    let mut report = |status: BackgroundHoldStatus| {
        if status != reported {
            if let Some(observer) = observer {
                observer(status.clone());
            }
            reported = status;
        }
    };
    report(BackgroundHoldStatus::for_lifetime(
        SessionLifetime::BackgroundHolding,
        state.live_tasks,
        initial.tasks,
    ));

    match mode {
        TaskTrackingMode::Typed => log::info!(
            "ACP session {our_session_id}: holding after turn ({} live async task(s), typed \
             lifecycle)",
            initial.live_tasks
        ),
        TaskTrackingMode::Raw if initial.sdk_frames_seen => log::info!(
            "ACP session {our_session_id}: holding after turn ({} live background task(s))",
            initial.live_tasks
        ),
        TaskTrackingMode::Raw => log::warn!(
            "ACP session {our_session_id}: holding after turn, but no raw SDK frames have \
             arrived — can't confirm background state, holding up to the {}s cap",
            config.hold_cap.as_secs()
        ),
    }

    // Closed channels disable their branch instead of busy-looping the select.
    let mut prompts_open = true;
    let mut control_open = true;
    let mut activity_open = true;
    let outcome = loop {
        if let Some(settle) = state.poll_settle(tokio::time::Instant::now()) {
            break match settle {
                HoldSettle::Quiescent => HoldOutcome::Quiescent,
                HoldSettle::HeldUntilCap => HoldOutcome::HeldUntilCap,
            };
        }
        tokio::select! {
            _ = cancel_token.cancelled() => break HoldOutcome::Cancelled,
            _ = child_exited.cancelled() => break HoldOutcome::ChildExited,
            turn = prompt_rx.recv(), if prompts_open => match turn {
                Some(turn) => break HoldOutcome::NewTurn(turn),
                // All connection handles dropped: no further prompts can
                // arrive, but the hold itself continues — background work is
                // still worth draining before teardown.
                None => prompts_open = false,
            },
            request = hold_control_rx.recv(), if control_open => match request {
                // Dispatched without awaiting the agent's answer, so a slow
                // (or dead) bridge can't stall the hold's other exits.
                Some(request) => serve_stop(request),
                None => control_open = false,
            },
            changed = activity_rx.changed(), if activity_open => match changed {
                Ok(()) => {
                    let activity = activity_rx.borrow_and_update().clone();
                    state.observe(tokio::time::Instant::now(), &activity);
                    report(BackgroundHoldStatus::for_lifetime(
                        SessionLifetime::BackgroundHolding,
                        state.live_tasks,
                        activity.tasks,
                    ));
                }
                Err(_) => activity_open = false,
            },
            _ = tokio::time::sleep_until(state.next_deadline()) => {}
        }
    };

    // Whatever ended the hold, the wait is over: the next state carries its own
    // presentation (a live turn, or the session's terminal status), and a stop
    // still queued has no hold left to serve it. The flag drops before the
    // drain so a stop racing this boundary is either answered here or
    // rejected at the handle — never left queued unanswered.
    hold_active.store(false, Ordering::Relaxed);
    reject_queued_stop_requests(
        hold_control_rx,
        "the background hold ended before it was served",
    );
    report(BackgroundHoldStatus::default());
    outcome
}

/// Answer every currently-queued per-task stop request with an error. A stop
/// is only meaningful against the hold it was aimed at, so requests found
/// queued at a hold boundary are refused instead of being carried across it.
fn reject_queued_stop_requests(
    hold_control_rx: &mut mpsc::UnboundedReceiver<StopAsyncTaskRequest>,
    reason: &str,
) {
    while let Ok(request) = hold_control_rx.try_recv() {
        let _ = request
            .reply
            .send(Err(format!("Async task stop not served: {reason}")));
    }
}

/// Session-scoped connection loop: set up the ACP session once, then serve
/// prompt turns received over `prompt_rx`.
///
/// Returning from this function ends the connection — the connection task in
/// [`AcpDriver::connect`] then finalizes the writer, gracefully stops the
/// child process, and emits the [`SessionSettled`] event with the returned
/// [`SessionSettleReason`].
#[allow(clippy::too_many_arguments)]
async fn run_acp_session(
    connection: &ConnectionTo<Agent>,
    working_dir: &Path,
    store: &Arc<dyn Store>,
    our_session_id: &str,
    acp_session_id: Option<&str>,
    config_options: &[AcpSessionConfigOptionSelection],
    handler: &Arc<AcpNotificationHandler>,
    mcp_servers: &[McpServer],
    agent_label: &str,
    cancel_token: &CancellationToken,
    background_hold: Option<&BackgroundHoldConfig>,
    background_hold_observer: Option<&BackgroundHoldObserver>,
    child_exited: &CancellationToken,
    prompt_rx: &mut mpsc::UnboundedReceiver<QueuedSessionTurn>,
    hold_control_rx: &mut mpsc::UnboundedReceiver<StopAsyncTaskRequest>,
    hold_active: &AtomicBool,
    pending_reply: &RefCell<Option<oneshot::Sender<SessionTurnResult>>>,
) -> Result<(AgentRunOutcome, SessionSettleReason), String> {
    let setup_task = tokio::time::timeout(
        ACP_SETUP_TIMEOUT,
        setup_acp_session(AcpSessionSetupContext {
            connection,
            working_dir,
            store,
            handler,
            our_session_id,
            acp_session_id,
            config_options,
            mcp_servers,
            agent_label,
        }),
    );
    let setup = tokio::select! {
        _ = cancel_token.cancelled() => {
            handler.cancel_pending_permissions();
            return Ok((AgentRunOutcome::Cancelled, SessionSettleReason::Cancelled));
        }
        result = setup_task => {
            result
                .map_err(|_| {
                    format!(
                        "Timed out waiting for ACP protocol startup after {}s",
                        ACP_SETUP_TIMEOUT.as_secs()
                    )
                })??
        }
    };

    // If resuming, wait for replay to complete (content match OR idle timeout).
    // An absolute 10s timeout prevents a hang if the server sends zero replay
    // notifications (e.g. the remote session was garbage-collected).
    if acp_session_id.is_some() && !handler.is_replay_complete().await {
        let absolute_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    handler.cancel_pending_permissions();
                    if let Err(e) = send_session_cancel(connection, &setup.agent_session_id) {
                        log::warn!("{e}");
                    }
                    return Ok((AgentRunOutcome::Cancelled, SessionSettleReason::Cancelled));
                }
                _ = handler.replay_done.notified() => {
                    break;
                }
                _ = tokio::time::sleep_until(absolute_deadline) => {
                    log::warn!("Replay-wait absolute timeout reached (10s) — proceeding");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if handler.finalize_replay_if_idle(Duration::from_secs(1)).await {
                        break;
                    }
                }
            }
        }
    }
    if acp_session_id.is_some() {
        handler.transition_to_waiting_for_prompt().await;
    }

    // Prompt loop: the connection lifetime is hoisted above the turn
    // lifetime, so turns arrive over a channel instead of being bound to the
    // process spawn. With a background hold configured, the loop re-enters a
    // live turn when a prompt arrives while holding.
    let mut lifetime: Option<SessionLifetime> = None;
    let mut next_turn: Option<QueuedSessionTurn> = None;
    loop {
        let turn = match next_turn.take() {
            // A prompt that arrived during the holding wait re-enters a live
            // turn on the already-established session.
            Some(turn) => turn,
            None => tokio::select! {
                _ = cancel_token.cancelled() => {
                    handler.cancel_pending_permissions();
                    return Ok((AgentRunOutcome::Cancelled, SessionSettleReason::Cancelled));
                }
                turn = prompt_rx.recv() => match turn {
                    Some(turn) => turn,
                    // Every connection handle was dropped without sending a
                    // prompt — nothing to serve, tear down.
                    None => return Ok((AgentRunOutcome::Cancelled, SessionSettleReason::Cancelled)),
                },
            },
        };
        transition_lifetime(our_session_id, &mut lifetime, SessionLifetime::TurnLive);

        // Stash the reply sender: unless the holding path below resolves it
        // early, the connection task resolves it after teardown, so prompt()
        // callers resume with the child already stopped, exactly like the
        // old per-turn run().
        *pending_reply.borrow_mut() = Some(turn.reply);

        let outcome = run_prompt_turn(
            connection,
            &setup,
            handler,
            our_session_id,
            cancel_token,
            &turn.prompt,
            &turn.images,
        )
        .await;

        // Feature off: preserve the old behavior byte-for-byte — finalize()
        // + graceful_stop() fire immediately after each prompt resolves, and
        // the stashed reply resolves after that teardown.
        let Some(hold_config) = background_hold else {
            let (settled_lifetime, mapped) = match outcome {
                Ok(AgentRunOutcome::Completed) => (
                    SessionLifetime::TornDown,
                    Ok((AgentRunOutcome::Completed, SessionSettleReason::Immediate)),
                ),
                Ok(AgentRunOutcome::Cancelled) => (
                    SessionLifetime::Cancelled,
                    Ok((AgentRunOutcome::Cancelled, SessionSettleReason::Cancelled)),
                ),
                Err(e) => (SessionLifetime::Failed, Err(e)),
            };
            transition_lifetime(our_session_id, &mut lifetime, settled_lifetime);
            return mapped;
        };

        // Only a successfully completed turn enters the holding wait; a
        // cancelled or failed turn tears down immediately, as before.
        match outcome {
            Ok(AgentRunOutcome::Completed) => {}
            Ok(AgentRunOutcome::Cancelled) => {
                transition_lifetime(our_session_id, &mut lifetime, SessionLifetime::Cancelled);
                return Ok((AgentRunOutcome::Cancelled, SessionSettleReason::Cancelled));
            }
            Err(e) => {
                transition_lifetime(our_session_id, &mut lifetime, SessionLifetime::Failed);
                return Err(e);
            }
        }

        // Only hold when there is something to wait for. In raw mode the
        // `init` availability probe re-emits at the start of every prompt
        // turn, so a raw-SDK stream that works has already proven itself by
        // the time the prompt resolves: no frame yet is proof the stream will
        // never come — an older bridge, a rejected filter, or a non-Claude
        // agent — not uncertainty. Background work is unobservable on such a
        // connection, so there is nothing a hold could drain; tear down
        // immediately instead of holding to the cap. Typed mode never skips:
        // the mirrored asyncTasks capability is the availability proof, and
        // there may be no raw stream at all (a non-Claude asyncTasks agent).
        if setup.task_tracking_mode == TaskTrackingMode::Raw
            && !handler
                .subscribe_background_activity()
                .borrow()
                .sdk_frames_seen
        {
            log::info!(
                "ACP session {our_session_id}: no raw SDK frames arrived this turn — \
                 background work is unobservable on this connection; skipping the \
                 post-turn hold"
            );
            transition_lifetime(our_session_id, &mut lifetime, SessionLifetime::TornDown);
            return Ok((AgentRunOutcome::Completed, SessionSettleReason::Immediate));
        }

        // The turn itself is done: resolve its reply now — do NOT
        // finalize/kill and do NOT let the caller run post-completion hooks
        // (those gate on the settled event) — and hold the connection open
        // for background work.
        if let Some(reply) = pending_reply.borrow_mut().take() {
            let _ = reply.send(Ok(AgentRunOutcome::Completed));
        }
        transition_lifetime(
            our_session_id,
            &mut lifetime,
            SessionLifetime::BackgroundHolding,
        );
        // Everything the handler sees from here on is out-of-turn: recorded as
        // an attributed continuation on its own message boundary, never folded
        // into the turn that just finished (or the one that may follow).
        handler.transition_to_background_holding().await;

        match hold_for_background_quiescence(
            hold_config,
            setup.task_tracking_mode,
            handler,
            cancel_token,
            child_exited,
            prompt_rx,
            hold_control_rx,
            hold_active,
            &|request| serve_async_task_stop(connection, &setup, our_session_id, request),
            our_session_id,
            background_hold_observer,
        )
        .await
        {
            HoldOutcome::NewTurn(turn) => {
                next_turn = Some(turn);
            }
            HoldOutcome::Quiescent => {
                transition_lifetime(our_session_id, &mut lifetime, SessionLifetime::Quiescent);
                // Quiescent tears down immediately: returning runs finalize()
                // + graceful_stop() in the connection task, whose settled
                // event then lets the caller run post-completion hooks.
                transition_lifetime(our_session_id, &mut lifetime, SessionLifetime::TornDown);
                return Ok((AgentRunOutcome::Completed, SessionSettleReason::Quiescent));
            }
            HoldOutcome::HeldUntilCap => {
                log::warn!(
                    "ACP session {our_session_id}: hold cap ({}s) expired with background \
                     work still unconfirmed — tearing down",
                    hold_config.hold_cap.as_secs()
                );
                transition_lifetime(our_session_id, &mut lifetime, SessionLifetime::TornDown);
                return Ok((
                    AgentRunOutcome::Completed,
                    SessionSettleReason::HeldUntilCap,
                ));
            }
            HoldOutcome::Cancelled => {
                // User-initiated close during the hold: reuse the in-turn
                // cancel ordering — session/cancel first (which also kills
                // the agent's background tasks), then a bounded drain, then
                // teardown (finalize + graceful_stop in the connection task).
                //
                // The turn itself completed before the hold began, so the
                // stop cancels only the *wait*, not the work: the session
                // settles Completed with the truncated-wait reason, and
                // callers still run post-completion hooks — a commit the turn
                // made is detected instead of being erased by the stop.
                handler.cancel_pending_permissions();
                if let Err(e) = send_session_cancel(connection, &setup.agent_session_id) {
                    log::warn!("{e}");
                }
                tokio::time::sleep(HOLDING_CANCEL_DRAIN).await;
                transition_lifetime(our_session_id, &mut lifetime, SessionLifetime::TornDown);
                return Ok((AgentRunOutcome::Completed, SessionSettleReason::HoldStopped));
            }
            HoldOutcome::ChildExited => {
                // The turn's result was known good before the child died;
                // only the wait for its background work was cut short. Settle
                // the same truncated-wait completion instead of failing a
                // finished turn — with a warning, because whatever background
                // work was still live (and the continuation it would have
                // triggered) is lost.
                log::warn!(
                    "ACP agent process for session {our_session_id} exited while holding \
                     for background tasks — settling the completed turn with its wait \
                     truncated"
                );
                transition_lifetime(our_session_id, &mut lifetime, SessionLifetime::Failed);
                return Ok((AgentRunOutcome::Completed, SessionSettleReason::HoldStopped));
            }
        }
    }
}

/// Send one prompt turn over an established ACP session and map its stop
/// reason to an [`AgentRunOutcome`].
async fn run_prompt_turn(
    connection: &ConnectionTo<Agent>,
    setup: &AcpSessionSetup,
    handler: &Arc<AcpNotificationHandler>,
    our_session_id: &str,
    cancel_token: &CancellationToken,
    prompt: &str,
    images: &[(String, String)],
) -> Result<AgentRunOutcome, String> {
    let supports_images = setup.agent_capabilities.prompt_capabilities.image;
    if !images.is_empty() && !supports_images {
        log::warn!(
            "ACP agent for session {our_session_id} does not advertise promptCapabilities.image; omitting {} image attachment(s)",
            images.len()
        );
    }
    let content_blocks = build_prompt_content_blocks(prompt, images, supports_images);
    let prompt_request = PromptRequest::new(setup.agent_session_id.clone(), content_blocks);

    handler.transition_to_live().await;

    let prompt_task = connection.send_request(prompt_request).block_task();
    tokio::pin!(prompt_task);

    let prompt_response = tokio::select! {
        result = &mut prompt_task => {
            result.map_err(|e| format!("Prompt failed: {e:?}"))?
        }
        _ = cancel_token.cancelled() => {
            handler.cancel_pending_permissions();
            if let Err(e) = send_session_cancel(connection, &setup.agent_session_id) {
                log::warn!("{e}");
            }

            match tokio::time::timeout(Duration::from_secs(5), &mut prompt_task).await {
                Ok(result) => result.map_err(|e| format!("Prompt failed after cancellation: {e:?}"))?,
                Err(_) => {
                    log::warn!(
                        "Timed out waiting for ACP prompt response after session/cancel for session {our_session_id}"
                    );
                    return Ok(AgentRunOutcome::Cancelled);
                }
            }
        }
    };

    if let Some(metadata) = prompt_response_metadata(&prompt_response) {
        handler.writer.record_acp_event_metadata(metadata).await;
    }

    Ok(AgentRunOutcome::from_stop_reason(
        prompt_response.stop_reason,
    ))
}

/// Whether the agent advertises support for the transport an MCP server needs.
/// Stdio is always supported per the ACP spec.
fn mcp_server_transport_supported(server: &McpServer, caps: &McpCapabilities) -> bool {
    match server {
        McpServer::Http(_) => caps.http,
        McpServer::Sse(_) => caps.sse,
        McpServer::Stdio(_) => true,
        // `McpServer` is non_exhaustive. A transport we don't recognize can't be
        // reasoned about, so we conservatively treat it as unsupported, which
        // fails the session rather than silently shipping an unvalidated server.
        _ => false,
    }
}

// Fragments shared by the config-selection error producers below and
// [`is_config_selection_unavailable_error`]. Consumers persist selections and
// clear them when the matcher fires, so producer and matcher must stay on the
// same strings — reword these constants, never the format! call sites.
const CONFIG_SELECTION_MISSING_OPTIONS_PREFIX: &str =
    "Agent did not return ACP config options needed to apply selected";
const CONFIG_SELECTION_STALE_PREFIX: &str = "Selected ACP ";
const CONFIG_SELECTION_STALE_UNAVAILABLE: &str = " is no longer available";
const CONFIG_SELECTION_STALE_NOT_SELECT: &str = " is not a select option";

/// Whether an error from applying stored session config selections means the
/// selection itself is stale or unavailable, rather than a transport or
/// protocol failure. Callers that persist selections use this to clear the
/// stored value so the next run falls back to provider defaults.
pub fn is_config_selection_unavailable_error(error: &str) -> bool {
    error.contains(CONFIG_SELECTION_MISSING_OPTIONS_PREFIX)
        || (error.contains(CONFIG_SELECTION_STALE_PREFIX)
            && (error.contains(CONFIG_SELECTION_STALE_UNAVAILABLE)
                || error.contains(CONFIG_SELECTION_STALE_NOT_SELECT)))
}

// Fragment shared by `setup_acp_session`'s required-transport check and
// [`is_missing_mcp_transport_error`] — the same producer/matcher contract as
// the config-selection constants above.
const MISSING_MCP_TRANSPORT_PREFIX: &str = "Agent does not support required MCP transports";

/// Whether a session-establishment error means the agent lacks a transport
/// required by the session's MCP servers (e.g. no HTTP MCP support), rather
/// than the run failing for its own reasons. Callers that pin work to a
/// specific agent use this to reroute to one that can host the servers.
pub fn is_missing_mcp_transport_error(error: &str) -> bool {
    error.contains(MISSING_MCP_TRANSPORT_PREFIX)
}

async fn apply_or_record_session_config_options(
    connection: &ConnectionTo<Agent>,
    agent_session_id: &str,
    initial_options: Option<&[SessionConfigOption]>,
    selections: &[AcpSessionConfigOptionSelection],
    writer: &Arc<dyn MessageWriter>,
    resuming: bool,
) -> Result<(), String> {
    if selections.is_empty() {
        if let Some(options) = initial_options {
            writer.on_config_option_update(options).await;
        }
        return Ok(());
    }

    let Some(initial_options) = initial_options else {
        let labels = selections
            .iter()
            .map(|selection| config_selection_label(&selection.category))
            .collect::<Vec<_>>()
            .join("/");
        if resuming {
            // Some agents advertise config options on session/new but omit
            // them from session/load. The config applied when the session was
            // created lives in the agent's own session state, so skip the
            // re-apply rather than failing the first follow-up after an app
            // restart. A selection changed since creation is skipped too —
            // the warning is the only trace of that.
            log::warn!(
                "Agent returned no ACP config options on session/load; skipping selected {labels}"
            );
            return Ok(());
        }
        return Err(format!(
            "{CONFIG_SELECTION_MISSING_OPTIONS_PREFIX} {labels} before prompting"
        ));
    };
    let mut latest_options = initial_options.to_vec();

    let mut model_selection_applied = false;
    for selection in selections {
        let config_id = match resolve_session_config_option_selection(&latest_options, selection) {
            Ok(config_id) => config_id,
            Err(e)
                if model_selection_applied
                    && selection.category == SessionConfigOptionCategory::ThoughtLevel =>
            {
                log::warn!("Skipping stale ACP effort selection after model change: {e}");
                continue;
            }
            Err(e) => return Err(e),
        };
        let response = connection
            .send_request(SetSessionConfigOptionRequest::new(
                agent_session_id.to_string(),
                config_id,
                selection.value_id.as_str(),
            ))
            .block_task()
            .await
            .map_err(|e| {
                format!(
                    "Failed to set selected ACP {} config option: {e:?}",
                    config_selection_label(&selection.category)
                )
            })?;
        latest_options = response.config_options;
        if selection.category == SessionConfigOptionCategory::Model {
            model_selection_applied = true;
        }
    }

    writer.on_config_option_update(&latest_options).await;
    Ok(())
}

fn resolve_session_config_option_selection(
    options: &[SessionConfigOption],
    selection: &AcpSessionConfigOptionSelection,
) -> Result<String, String> {
    let label = config_selection_label(&selection.category);

    if let Some(option) = options
        .iter()
        .find(|option| option.id.to_string() == selection.config_id)
    {
        ensure_config_option_has_value(option, &selection.value_id, label)?;
        return Ok(option.id.to_string());
    }

    let Some(option) = options.iter().find(|option| {
        option.category.as_ref() == Some(&selection.category) && is_select_option(option)
    }) else {
        return Err(format!(
            "{CONFIG_SELECTION_STALE_PREFIX}{label} config option '{}'{CONFIG_SELECTION_STALE_UNAVAILABLE} for this provider",
            selection.config_id
        ));
    };

    ensure_config_option_has_value(option, &selection.value_id, label)?;
    Ok(option.id.to_string())
}

fn ensure_config_option_has_value(
    option: &SessionConfigOption,
    value_id: &str,
    label: &str,
) -> Result<(), String> {
    match select_option_has_value(option, value_id) {
        Some(true) => Ok(()),
        Some(false) => Err(format!(
            "{CONFIG_SELECTION_STALE_PREFIX}{label} value '{value_id}'{CONFIG_SELECTION_STALE_UNAVAILABLE} for config option '{}'",
            option.id
        )),
        None => Err(format!(
            "{CONFIG_SELECTION_STALE_PREFIX}{label} config option '{}'{CONFIG_SELECTION_STALE_NOT_SELECT}",
            option.id
        )),
    }
}

fn is_select_option(option: &SessionConfigOption) -> bool {
    matches!(&option.kind, SessionConfigKind::Select(_))
}

fn select_option_has_value(option: &SessionConfigOption, value_id: &str) -> Option<bool> {
    let SessionConfigKind::Select(select) = &option.kind else {
        return None;
    };

    Some(match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .any(|option| option.value.to_string() == value_id),
        SessionConfigSelectOptions::Grouped(groups) => groups.iter().any(|group| {
            group
                .options
                .iter()
                .any(|option| option.value.to_string() == value_id)
        }),
        _ => false,
    })
}

fn config_selection_label(category: &SessionConfigOptionCategory) -> &'static str {
    match category {
        SessionConfigOptionCategory::Model => "model",
        SessionConfigOptionCategory::ThoughtLevel => "effort",
        _ => "configuration",
    }
}

struct AcpSessionSetup {
    agent_session_id: String,
    agent_capabilities: AgentCapabilities,
    /// How this connection tracks background tasks, negotiated at initialize
    /// (see [`task_tracking_mode_from_initialize`]).
    task_tracking_mode: TaskTrackingMode,
}

struct AcpSessionSetupContext<'a> {
    connection: &'a ConnectionTo<Agent>,
    working_dir: &'a Path,
    store: &'a Arc<dyn Store>,
    /// Setup owns mode selection: the negotiated tracking mode must be set on
    /// the handler after `initialize` resolves but before `session/new` or
    /// `session/load` is sent, so no `session/update` can race it.
    handler: &'a Arc<AcpNotificationHandler>,
    our_session_id: &'a str,
    acp_session_id: Option<&'a str>,
    config_options: &'a [AcpSessionConfigOptionSelection],
    mcp_servers: &'a [McpServer],
    agent_label: &'a str,
}

/// `_meta` for `session/new` and `session/load` asking the Claude bridge to
/// forward raw SDK frames (`_meta.claudeCode.emitRawSDKMessages`).
///
/// In raw mode, the filters carry the whole task-tracking burden: one filter
/// per subtype in [`BACKGROUND_TASK_SUBTYPES`], plus the
/// [`AVAILABILITY_PROBE_SUBTYPE`] `init` frame that proves the stream works
/// even on task-less turns, plus the [`SESSION_STATE_SUBTYPE`] frame feeding
/// the holding wait's idle latch, and the `assistant` frames of an autonomous
/// cycle, matched on the bridge's own `origin.kind`, which name what a
/// background continuation belongs to (see [`sdk_message_origin_kind`]). The
/// origin filter only ever matches autonomous cycles, so it costs nothing on
/// an ordinary turn.
///
/// In typed mode the `async_task_*` session updates replace the `task_*`
/// frames and the `init` availability probe (the mirrored capability is the
/// availability proof), so only the idle-latch frame and the continuation
/// origin filter remain requested.
///
/// Agents MUST NOT make assumptions about unrecognized `_meta` keys per the
/// ACP spec, so this is safe to send regardless of provider — non-Claude
/// agents ignore it.
fn background_task_tracking_meta(mode: TaskTrackingMode) -> Meta {
    let task_subtypes: &[&str] = match mode {
        TaskTrackingMode::Raw => &BACKGROUND_TASK_SUBTYPES,
        TaskTrackingMode::Typed => &[],
    };
    let system_subtypes: &[&str] = match mode {
        TaskTrackingMode::Raw => &[AVAILABILITY_PROBE_SUBTYPE, SESSION_STATE_SUBTYPE],
        TaskTrackingMode::Typed => &[SESSION_STATE_SUBTYPE],
    };
    let filters: Vec<serde_json::Value> = task_subtypes
        .iter()
        .chain(system_subtypes)
        .map(|subtype| serde_json::json!({ "type": "system", "subtype": subtype }))
        .chain(std::iter::once(serde_json::json!({
            "type": "assistant",
            "origin": TASK_NOTIFICATION_ORIGIN,
        })))
        .collect();
    let mut meta = Meta::new();
    meta.insert(
        "claudeCode".to_string(),
        serde_json::json!({ "emitRawSDKMessages": filters }),
    );
    meta
}

async fn setup_acp_session(context: AcpSessionSetupContext<'_>) -> Result<AcpSessionSetup, String> {
    let AcpSessionSetupContext {
        connection,
        working_dir,
        store,
        handler,
        our_session_id,
        acp_session_id,
        config_options,
        mcp_servers,
        agent_label,
    } = context;
    let writer = &handler.writer;

    let client_info = Implementation::new("acp-client", env!("CARGO_PKG_VERSION"));
    // Advertise the AIR asyncTasks capability. `_meta` is the spec's
    // extension point — agents that don't recognize the namespace ignore it —
    // and an agent that does support it mirrors the accepted list back in the
    // initialize response's `_meta`, which selects typed tracking below.
    let init_request = InitializeRequest::new(ProtocolVersion::V1)
        .client_info(client_info)
        .client_capabilities(ClientCapabilities::new().meta(air_client_capabilities_meta()));

    let init_response = connection
        .send_request(init_request)
        .block_task()
        .await
        .map_err(|e| format!("ACP init failed: {e:?}"))?;

    log_initialize_response(agent_label, &init_response);
    writer
        .on_initialize(&initialize_metadata(&init_response))
        .await;

    if init_response.protocol_version != ProtocolVersion::V1 {
        return Err(format!(
            "Agent negotiated unsupported ACP protocol version {} (expected {})",
            init_response.protocol_version,
            ProtocolVersion::V1
        ));
    }

    authenticate_if_advertised(connection, &init_response.auth_methods).await?;

    // Required servers must all have a supported transport, or the session
    // fails. Route the decision through mcp_server_transport_supported so the
    // transport->capability mapping lives in exactly one place: a newly added
    // transport stays validated here instead of silently slipping through.
    let mcp_caps = &init_response.agent_capabilities.mcp_capabilities;
    if mcp_servers
        .iter()
        .any(|server| !mcp_server_transport_supported(server, mcp_caps))
    {
        let requires_http = mcp_servers
            .iter()
            .any(|server| matches!(server, McpServer::Http(_)));
        let requires_sse = mcp_servers
            .iter()
            .any(|server| matches!(server, McpServer::Sse(_)));

        return Err(format!(
            "{MISSING_MCP_TRANSPORT_PREFIX} (required: http={}, sse={}; agent: http={}, sse={}). Select a provider that supports MCP over HTTP/SSE.",
            requires_http, requires_sse, mcp_caps.http, mcp_caps.sse
        ));
    }

    let agent_capabilities = init_response.agent_capabilities.clone();

    // Negotiation resolves here, before session/new or session/load is sent:
    // any `session/update` this connection ever delivers is dispatched with
    // the mode already set, so a typed async-task update can't race the
    // selection.
    let task_tracking_mode = task_tracking_mode_from_initialize(init_response.meta.as_ref());
    log::debug!(
        "ACP session {our_session_id}: task tracking mode {task_tracking_mode:?} \
         (asyncTasks mirrored: {})",
        task_tracking_mode == TaskTrackingMode::Typed
    );
    handler.set_task_tracking_mode(task_tracking_mode);

    match acp_session_id {
        Some(existing_id) => {
            if !agent_capabilities.load_session {
                return Err(
                    "Agent does not support load_session — cannot resume conversation".to_string(),
                );
            }

            log::info!(
                "Resuming ACP session {existing_id} via load_session for session {our_session_id}"
            );

            let load_response = connection
                .send_request(
                    LoadSessionRequest::new(existing_id.to_string(), working_dir.to_path_buf())
                        .mcp_servers(mcp_servers.to_vec())
                        // The Claude bridge recreates the session on load and
                        // reads `_meta.claudeCode.emitRawSDKMessages` there
                        // too, so resumed sessions keep the raw
                        // background-task frames flowing.
                        .meta(background_task_tracking_meta(task_tracking_mode)),
                )
                .block_task()
                .await
                .map_err(|e| format!("Failed to load ACP session: {e:?}"))?;

            if let Some(ref modes) = load_response.modes {
                writer.on_model_state_update(modes).await;
            }
            apply_or_record_session_config_options(
                connection,
                existing_id,
                load_response.config_options.as_deref(),
                config_options,
                writer,
                true,
            )
            .await?;

            Ok(AcpSessionSetup {
                agent_session_id: existing_id.to_string(),
                agent_capabilities,
                task_tracking_mode,
            })
        }
        None => {
            let new_session_request = NewSessionRequest::new(working_dir.to_path_buf())
                .mcp_servers(mcp_servers.to_vec())
                .meta(background_task_tracking_meta(task_tracking_mode));
            let session_response = connection
                .send_request(new_session_request)
                .block_task()
                .await
                .map_err(|e| format!("Failed to create ACP session: {e:?}"))?;

            let new_id = session_response.session_id.to_string();
            store
                .set_agent_session_id(our_session_id, &new_id)
                .map_err(|e| format!("Failed to save agent session ID: {e}"))?;

            if let Some(ref modes) = session_response.modes {
                writer.on_model_state_update(modes).await;
            }
            apply_or_record_session_config_options(
                connection,
                &new_id,
                session_response.config_options.as_deref(),
                config_options,
                writer,
                false,
            )
            .await?;

            Ok(AcpSessionSetup {
                agent_session_id: new_id,
                agent_capabilities,
                task_tracking_mode,
            })
        }
    }
}

fn initialize_metadata(init_response: &InitializeResponse) -> AcpInitializeMetadata {
    AcpInitializeMetadata {
        protocol_version: init_response.protocol_version.to_string(),
        agent_capabilities: serde_json::to_value(&init_response.agent_capabilities).ok(),
        auth_methods: serde_json::to_value(&init_response.auth_methods).ok(),
        agent_info: init_response
            .agent_info
            .as_ref()
            .and_then(|info| serde_json::to_value(info).ok()),
    }
}

fn log_initialize_response(agent_label: &str, init_response: &InitializeResponse) {
    let agent_name = init_response
        .agent_info
        .as_ref()
        .map(|info| info.name.as_str())
        .unwrap_or(agent_label);
    let agent_version = init_response
        .agent_info
        .as_ref()
        .map(|info| info.version.as_str())
        .unwrap_or("unknown");
    let capabilities = serde_json::to_string(&init_response.agent_capabilities)
        .unwrap_or_else(|_| "<unserializable>".to_string());
    let auth_methods = describe_auth_methods(&init_response.auth_methods);

    log::debug!(
        "ACP initialized provider={agent_label} agent={agent_name} version={agent_version} protocol={} capabilities={} auth_methods=[{}]",
        init_response.protocol_version,
        capabilities,
        auth_methods
    );
}

fn describe_auth_methods(auth_methods: &[AuthMethod]) -> String {
    auth_methods
        .iter()
        .map(|method| format!("{} ({})", method.name(), method.id()))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn authenticate_if_advertised(
    connection: &ConnectionTo<Agent>,
    auth_methods: &[AuthMethod],
) -> Result<(), String> {
    let Some(method) = auth_methods.first() else {
        return Ok(());
    };

    log::debug!(
        "ACP agent advertised authentication methods; selecting {} ({})",
        method.name(),
        method.id()
    );

    connection
        .send_request(AuthenticateRequest::new(method.id().clone()))
        .block_task()
        .await
        .map_err(|e| {
            format!(
                "ACP authentication failed with method {} ({}): {e:?}",
                method.name(),
                method.id()
            )
        })?;

    Ok(())
}

fn build_prompt_content_blocks(
    prompt: &str,
    images: &[(String, String)],
    supports_images: bool,
) -> Vec<AcpContentBlock> {
    let mut content_blocks = vec![AcpContentBlock::Text(TextContent::new(prompt))];

    if supports_images {
        for (data, mime_type) in images {
            content_blocks.push(AcpContentBlock::Image(ImageContent::new(
                data.as_str(),
                mime_type.as_str(),
            )));
        }
    } else if !images.is_empty() {
        content_blocks.push(AcpContentBlock::Text(TextContent::new(format!(
            "[{} image attachment(s) omitted because this ACP provider does not advertise promptCapabilities.image]",
            images.len()
        ))));
    }

    content_blocks
}

/// Strip outer markdown code fences from tool-result content.
/// Agents often wrap results in ``` fences which are redundant in our `<pre>` display.
/// The closing fence may be absent when content was truncated by the preview limit.
pub fn strip_code_fences(content: &str) -> String {
    let trimmed = content.trim();
    if let Some(after_open) = trimmed.strip_prefix("```") {
        if let Some(nl) = after_open.find('\n') {
            let body = after_open[nl + 1..].trim_end();
            return body
                .strip_suffix("```")
                .unwrap_or(body)
                .trim_end()
                .to_string();
        }
    }
    content.to_string()
}

fn extract_content_preview(content: &[ToolCallContent]) -> Option<String> {
    for item in content {
        match item {
            ToolCallContent::Content(c) => {
                if let AcpContentBlock::Text(text) = &c.content {
                    let preview: String = text.text.chars().take(500).collect();
                    return Some(if text.text.len() > 500 {
                        format!("{preview}…")
                    } else {
                        preview
                    });
                }
            }
            ToolCallContent::Diff(d) => {
                return Some(format!(
                    "{}{}",
                    d.path.display(),
                    if d.old_text.is_some() {
                        " (modified)"
                    } else {
                        " (new)"
                    }
                ));
            }
            ToolCallContent::Terminal(t) => {
                return Some(format!("Terminal: {}", t.terminal_id.0));
            }
            _ => {}
        }
    }
    None
}

// =============================================================================
// Basic MessageWriter implementation
// =============================================================================

/// Simple in-memory message writer for basic usage.
///
/// Only agent *text* is recorded. Tool calls and tool results are deliberately
/// dropped: this writer backs one-shot prompting ([`crate::run_acp_prompt`]),
/// whose callers parse the accumulated text as machine-readable output (JSON).
/// Interleaving tool markers would corrupt that payload.
pub struct BasicMessageWriter {
    text: Mutex<String>,
    last_flush_at: Mutex<Instant>,
}

impl BasicMessageWriter {
    pub fn new() -> Self {
        Self {
            text: Mutex::new(String::new()),
            last_flush_at: Mutex::new(Instant::now()),
        }
    }

    pub async fn get_text(&self) -> String {
        self.text.lock().await.clone()
    }
}

impl Default for BasicMessageWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MessageWriter for BasicMessageWriter {
    async fn append_text(&self, text: &str) {
        let mut current = self.text.lock().await;
        current.push_str(text);
        *self.last_flush_at.lock().await = Instant::now();
    }

    async fn finalize(&self) {
        // Nothing to do for basic implementation
    }

    async fn record_tool_call(
        &self,
        _tool_call_id: &str,
        _title: &str,
        _raw_input: Option<&serde_json::Value>,
    ) {
        // Intentionally ignored: see the type-level docs. Tool activity must not
        // land in the buffer that one-shot callers parse.
    }

    async fn update_tool_call_title(
        &self,
        _tool_call_id: &str,
        _title: Option<&str>,
        _raw_input: Option<&serde_json::Value>,
    ) {
        // Nothing to do for basic implementation
    }

    async fn record_tool_result(&self, _tool_call_id: &str, _content: &str) {
        // Intentionally ignored: see the type-level docs.
    }
}

#[cfg(test)]
mod tests {
    use super::{
        acp_spawn_command, air_client_capabilities_meta, apply_or_record_session_config_options,
        async_task_stop_message, async_task_stop_outcome, autoapprove_permission_decision,
        background_continuation_origin, background_task_tracking_meta, build_prompt_content_blocks,
        consume_remote_acp_line, decode_remote_acp_line, defensive_permission_decision,
        hold_for_background_quiescence, is_config_selection_unavailable_error,
        is_missing_mcp_transport_error, labeled_background_continuation_origin,
        mcp_server_transport_supported, origin_task_name_label, permission_response_for_decision,
        permission_response_for_options, reject_queued_stop_requests, remote_acp_segments,
        resolve_acp_working_dir, resolve_session_config_option_selection,
        resolve_spawn_working_dir, sanitize_remote_acp_chunk, sdk_message_mentions_task,
        sdk_message_origin_kind, sdk_message_session_state, shell_exec_line, shell_quote,
        task_tracking_mode_from_initialize, AcpDriver, AcpEventMetadata, AcpNotificationHandler,
        AcpPermissionDecision, AcpPermissionOption, AcpPermissionOptionKind, AcpPermissionRequest,
        AcpSessionConfigOptionSelection, AcpToolCallMetadata, AgentRunOutcome,
        AsyncTaskNotification, AsyncTaskState, AsyncTaskStopHandle, AsyncTaskUpdate,
        BackgroundActivity, BackgroundHoldConfig, BackgroundHoldObserver, BackgroundHoldStatus,
        BackgroundHoldTask, BackgroundTaskSet, BasicMessageWriter, HoldOutcome, HoldSettle,
        HoldingState, IncomingSessionUpdate, MessageWriter, OutOfTurnPermissionPolicy,
        QueuedSessionTurn, RemoteLineOutcome, ReplayBoundary, ReplayBuffer, ReplayEvent,
        SdkSessionState, SessionLifetime, SessionSettleReason, SessionSettled,
        StopAsyncTaskRequest, TaskTrackingMode, TypedAsyncTaskSet, ASYNC_TASK_STOP_METHOD,
        AVAILABILITY_PROBE_SUBTYPE, BACKGROUND_CONTINUATION_ORIGIN, BACKGROUND_TASK_SUBTYPES,
        CLAUDE_SDK_MESSAGE_METHOD, CONTINUATION_MESSAGE_ID_PREFIX, ORIGIN_TASK_NAME_MAX_CHARS,
        PERMISSION_ANNOUNCEMENT_GRACE, SESSION_STATE_SUBTYPE, TASK_NOTIFICATION_ORIGIN,
    };
    use agent_client_protocol::schema::v1::{
        ContentBlock as AcpContentBlock, ContentChunk, ExtNotification, McpCapabilities, McpServer,
        McpServerHttp, McpServerSse, McpServerStdio, PermissionOption, PermissionOptionKind, Plan,
        PlanEntry, PlanEntryPriority, PlanEntryStatus, RequestPermissionOutcome,
        RequestPermissionRequest, SessionConfigOption, SessionConfigOptionCategory,
        SessionConfigSelectOption, SessionNotification, SessionUpdate,
        SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason, TextContent,
        ToolCall, ToolCallUpdate, ToolCallUpdateFields,
    };
    use agent_client_protocol::JsonRpcMessage;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::sync::{mpsc, oneshot};
    use tokio_util::sync::CancellationToken;

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[test]
    fn resolves_stale_config_id_by_category() {
        let options = vec![SessionConfigOption::select(
            "model-v2",
            "Model",
            "sonnet",
            vec![
                SessionConfigSelectOption::new("sonnet", "Sonnet"),
                SessionConfigSelectOption::new("opus", "Opus"),
            ],
        )
        .category(SessionConfigOptionCategory::Model)];
        let selection = AcpSessionConfigOptionSelection {
            category: SessionConfigOptionCategory::Model,
            config_id: "model-v1".to_string(),
            value_id: "opus".to_string(),
        };

        let resolved =
            resolve_session_config_option_selection(&options, &selection).expect("resolved config");

        assert_eq!(resolved, "model-v2");
    }

    #[test]
    fn errors_when_selected_config_value_is_missing() {
        let options = vec![SessionConfigOption::select(
            "reasoning",
            "Reasoning",
            "medium",
            vec![
                SessionConfigSelectOption::new("low", "Low"),
                SessionConfigSelectOption::new("medium", "Medium"),
            ],
        )
        .category(SessionConfigOptionCategory::ThoughtLevel)];
        let selection = AcpSessionConfigOptionSelection {
            category: SessionConfigOptionCategory::ThoughtLevel,
            config_id: "reasoning".to_string(),
            value_id: "high".to_string(),
        };

        let error = resolve_session_config_option_selection(&options, &selection)
            .expect_err("missing value should fail");

        assert!(error.contains("Selected ACP effort value 'high' is no longer available"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn skips_stale_effort_after_applying_selected_model() {
        let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let calls_for_handler = Arc::clone(&calls);
        let agent = agent_client_protocol::Agent.builder().on_receive_request(
            async move |request: SetSessionConfigOptionRequest, responder, _cx| {
                calls_for_handler.lock().unwrap().push((
                    request.config_id.to_string(),
                    request
                        .value
                        .as_value_id()
                        .expect("selected config value should be a value ID")
                        .to_string(),
                ));
                responder.respond(SetSessionConfigOptionResponse::new(vec![
                    SessionConfigOption::select(
                        "model",
                        "Model",
                        "opus",
                        vec![
                            SessionConfigSelectOption::new("sonnet", "Sonnet"),
                            SessionConfigSelectOption::new("opus", "Opus"),
                        ],
                    )
                    .category(SessionConfigOptionCategory::Model),
                    SessionConfigOption::select(
                        "reasoning",
                        "Reasoning",
                        "low",
                        vec![SessionConfigSelectOption::new("low", "Low")],
                    )
                    .category(SessionConfigOptionCategory::ThoughtLevel),
                ]))
            },
            agent_client_protocol::on_receive_request!(),
        );
        let writer = Arc::new(RecordingMessageWriter::default());
        let message_writer: Arc<dyn MessageWriter> = writer.clone();
        let initial_options = vec![
            SessionConfigOption::select(
                "model",
                "Model",
                "sonnet",
                vec![
                    SessionConfigSelectOption::new("sonnet", "Sonnet"),
                    SessionConfigSelectOption::new("opus", "Opus"),
                ],
            )
            .category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select(
                "reasoning",
                "Reasoning",
                "high",
                vec![
                    SessionConfigSelectOption::new("low", "Low"),
                    SessionConfigSelectOption::new("high", "High"),
                ],
            )
            .category(SessionConfigOptionCategory::ThoughtLevel),
        ];
        let selections = vec![
            AcpSessionConfigOptionSelection {
                category: SessionConfigOptionCategory::Model,
                config_id: "model".to_string(),
                value_id: "opus".to_string(),
            },
            AcpSessionConfigOptionSelection {
                category: SessionConfigOptionCategory::ThoughtLevel,
                config_id: "reasoning".to_string(),
                value_id: "high".to_string(),
            },
        ];

        agent_client_protocol::Client
            .builder()
            .name("acp-config-test")
            .connect_with(agent, async |connection| {
                apply_or_record_session_config_options(
                    &connection,
                    "session-1",
                    Some(&initial_options),
                    &selections,
                    &message_writer,
                    false,
                )
                .await
                .map_err(agent_client_protocol::util::internal_error)
            })
            .await
            .expect("protocol should succeed");

        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[(String::from("model"), String::from("opus"))]
        );
        let updates = writer.config_option_updates.lock().unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(
            resolve_session_config_option_selection(&updates[0], &selections[1])
                .expect_err("stale effort should remain unavailable"),
            "Selected ACP effort value 'high' is no longer available for config option 'reasoning'"
        );
    }

    #[test]
    fn detects_unavailable_config_selection_errors() {
        assert!(is_config_selection_unavailable_error(
            "Selected ACP model value 'sonnet' is no longer available for config option 'model'"
        ));
        assert!(is_config_selection_unavailable_error(
            "Agent did not return ACP config options needed to apply selected model before prompting"
        ));
        assert!(is_config_selection_unavailable_error(
            "Selected ACP effort config option 'reasoning' is not a select option"
        ));
        assert!(!is_config_selection_unavailable_error(
            "Failed to create ACP session: transport closed"
        ));
    }

    #[test]
    fn detects_missing_mcp_transport_errors() {
        assert!(is_missing_mcp_transport_error(
            "Agent does not support required MCP transports (required: http=true, sse=false; \
agent: http=false, sse=false). Select a provider that supports MCP over HTTP/SSE."
        ));
        assert!(!is_missing_mcp_transport_error(
            "Failed to create ACP session: transport closed"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_config_options_skip_apply_on_resume_but_fail_new_sessions() {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let calls_for_handler = Arc::clone(&calls);
        let agent = agent_client_protocol::Agent.builder().on_receive_request(
            async move |request: SetSessionConfigOptionRequest, responder, _cx| {
                calls_for_handler
                    .lock()
                    .unwrap()
                    .push(request.config_id.to_string());
                responder.respond(SetSessionConfigOptionResponse::new(vec![]))
            },
            agent_client_protocol::on_receive_request!(),
        );
        let writer = Arc::new(RecordingMessageWriter::default());
        let message_writer: Arc<dyn MessageWriter> = writer.clone();
        let selections = vec![AcpSessionConfigOptionSelection {
            category: SessionConfigOptionCategory::Model,
            config_id: "model".to_string(),
            value_id: "opus".to_string(),
        }];

        agent_client_protocol::Client
            .builder()
            .name("acp-config-test")
            .connect_with(agent, async |connection| {
                apply_or_record_session_config_options(
                    &connection,
                    "session-1",
                    None,
                    &selections,
                    &message_writer,
                    true,
                )
                .await
                .map_err(agent_client_protocol::util::internal_error)?;

                let error = apply_or_record_session_config_options(
                    &connection,
                    "session-1",
                    None,
                    &selections,
                    &message_writer,
                    false,
                )
                .await
                .expect_err("missing config options must fail new sessions");
                assert!(is_config_selection_unavailable_error(&error));
                Ok(())
            })
            .await
            .expect("protocol should succeed");

        assert!(calls.lock().unwrap().is_empty());
        assert!(writer.config_option_updates.lock().unwrap().is_empty());
    }

    fn write_executable(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create executable parent");
        }
        std::fs::write(path, content).expect("write executable");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).expect("chmod executable");
        }
    }

    fn join_path_entries(entries: &[PathBuf]) -> String {
        std::env::join_paths(entries)
            .expect("join path entries")
            .into_string()
            .expect("path entries should be utf8")
    }

    fn acp_permission_request(options: Vec<AcpPermissionOption>) -> AcpPermissionRequest {
        AcpPermissionRequest {
            request_id: "test-request".to_string(),
            session_id: "test-session".to_string(),
            tool_call_id: "test-tool-call".to_string(),
            tool_title: None,
            tool_kind: None,
            tool_status: None,
            raw_input: None,
            raw_output: None,
            content: None,
            locations: None,
            options,
            raw_request: None,
            origin: None,
        }
    }

    fn acp_permission_option(
        option_id: &str,
        name: &str,
        kind: AcpPermissionOptionKind,
    ) -> AcpPermissionOption {
        AcpPermissionOption {
            option_id: option_id.to_string(),
            name: name.to_string(),
            kind,
        }
    }

    #[derive(Default)]
    struct RecordingMessageWriter {
        events: Mutex<Vec<AcpEventMetadata>>,
        config_option_updates: Mutex<Vec<Vec<SessionConfigOption>>>,
    }

    #[async_trait::async_trait]
    impl MessageWriter for RecordingMessageWriter {
        async fn append_text(&self, _text: &str) {}

        async fn finalize(&self) {}

        async fn record_tool_call(
            &self,
            _tool_call_id: &str,
            _title: &str,
            _raw_input: Option<&serde_json::Value>,
        ) {
        }

        async fn update_tool_call_title(
            &self,
            _tool_call_id: &str,
            _title: Option<&str>,
            _raw_input: Option<&serde_json::Value>,
        ) {
        }

        async fn record_tool_result(&self, _tool_call_id: &str, _content: &str) {}

        async fn record_acp_event_metadata(&self, metadata: AcpEventMetadata) {
            self.events.lock().unwrap().push(metadata);
        }

        async fn on_config_option_update(&self, options: &[SessionConfigOption]) {
            self.config_option_updates
                .lock()
                .unwrap()
                .push(options.to_vec());
        }
    }

    fn test_plan() -> Plan {
        Plan::new(vec![PlanEntry::new(
            "Inspect current state",
            PlanEntryPriority::Medium,
            PlanEntryStatus::Pending,
        )])
    }

    #[test]
    fn consumes_wrapped_json_line_across_multiple_chunks() {
        let mut pending = String::new();
        let first = r#"{"jsonrpc":"2.0","id":1,"result":{"text":"Bypass all permiss"#;
        let second = r#"ion checks"}}"#;

        assert_eq!(
            consume_remote_acp_line(&mut pending, first),
            RemoteLineOutcome::Pending
        );

        assert_eq!(
            consume_remote_acp_line(&mut pending, second),
            RemoteLineOutcome::Emit(format!("{first}{second}"))
        );
    }

    #[test]
    fn strips_record_separator_and_nul_bytes() {
        let chunk = "\u{1e}{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\0";
        assert_eq!(
            sanitize_remote_acp_chunk(chunk),
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}"
        );
    }

    #[test]
    fn drops_noise_and_recovers_with_next_valid_json_message() {
        let mut pending = String::new();
        assert_eq!(
            consume_remote_acp_line(&mut pending, "this is not json"),
            RemoteLineOutcome::Dropped
        );

        assert_eq!(
            consume_remote_acp_line(
                &mut pending,
                "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}"
            ),
            RemoteLineOutcome::Emit("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}".to_string())
        );
    }

    #[test]
    fn splits_record_separator_delimited_messages_in_one_stdout_line() {
        let mut pending = String::new();
        let line = "\u{1e}{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\u{1e}{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":null}\n";

        let outcomes: Vec<RemoteLineOutcome> = remote_acp_segments(line)
            .map(|segment| consume_remote_acp_line(&mut pending, segment))
            .collect();

        assert_eq!(
            outcomes,
            vec![
                RemoteLineOutcome::Emit(
                    "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}".to_string()
                ),
                RemoteLineOutcome::Emit(
                    "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":null}".to_string()
                ),
            ]
        );
    }

    #[test]
    fn remote_utf8_decoder_strips_invalid_bytes() {
        let (decoded, had_invalid_utf8) =
            decode_remote_acp_line(b"\xff{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\n");
        assert!(had_invalid_utf8);
        assert_eq!(decoded, "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\n");
    }

    #[test]
    fn remote_utf8_decoder_preserves_valid_replacement_character() {
        let (decoded, had_invalid_utf8) =
            decode_remote_acp_line("\u{FFFD}{\"jsonrpc\":\"2.0\",\"id\":1}\n".as_bytes());
        assert!(!had_invalid_utf8);
        assert_eq!(decoded, "\u{FFFD}{\"jsonrpc\":\"2.0\",\"id\":1}\n");
    }

    #[test]
    fn remote_spawn_dir_falls_back_when_working_dir_is_missing() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_nanos();
        let missing_path =
            std::env::temp_dir().join(format!("acp-client-missing-{}-{nonce}", std::process::id()));
        assert!(!missing_path.exists());

        assert_eq!(
            resolve_spawn_working_dir(&missing_path, true),
            std::env::temp_dir()
        );
        assert_eq!(
            resolve_spawn_working_dir(&missing_path, false),
            missing_path
        );
    }

    #[test]
    fn remote_spawn_dir_uses_existing_working_dir() {
        let existing = std::env::temp_dir();
        assert_eq!(resolve_spawn_working_dir(&existing, true), existing);
    }

    #[test]
    fn env_snapshot_is_ignored_for_remote_drivers() {
        let snapshot = vec![(String::from("PATH"), String::from("/snapshot/bin"))];
        let local = AcpDriver {
            binary_path: PathBuf::from("/usr/local/bin/codex-acp"),
            acp_args: vec![],
            agent_label: String::from("Codex"),
            is_remote: false,
            extra_env: vec![],
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: vec![],
            remote_working_dir: None,
            background_hold: None,
            background_hold_observer: None,
        }
        .with_env_snapshot(snapshot.clone());
        let remote = AcpDriver {
            binary_path: PathBuf::from("/usr/local/bin/sq"),
            acp_args: vec![String::from("blox"), String::from("acp")],
            agent_label: String::from("Blox"),
            is_remote: true,
            extra_env: vec![],
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: vec![],
            remote_working_dir: None,
            background_hold: None,
            background_hold_observer: None,
        }
        .with_env_snapshot(snapshot);

        assert!(local.env_snapshot.is_some());
        assert!(
            remote.env_snapshot.is_none(),
            "remote sq proxy launches must keep their inherited environment"
        );
    }

    #[test]
    fn interpreter_env_snapshot_is_ignored_for_remote_drivers() {
        let snapshot = vec![(String::from("PATH"), String::from("/home/bin"))];
        let local = AcpDriver {
            binary_path: PathBuf::from("/usr/local/bin/codex-acp"),
            acp_args: vec![],
            agent_label: String::from("Codex"),
            is_remote: false,
            extra_env: vec![],
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: vec![],
            remote_working_dir: None,
            background_hold: None,
            background_hold_observer: None,
        }
        .with_interpreter_env_snapshot(snapshot.clone());
        let remote = AcpDriver {
            binary_path: PathBuf::from("/usr/local/bin/sq"),
            acp_args: vec![String::from("blox"), String::from("acp")],
            agent_label: String::from("Blox"),
            is_remote: true,
            extra_env: vec![],
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: vec![],
            remote_working_dir: None,
            background_hold: None,
            background_hold_observer: None,
        }
        .with_interpreter_env_snapshot(snapshot);

        assert!(local.interpreter_env_snapshot.is_some());
        assert!(
            remote.interpreter_env_snapshot.is_none(),
            "remote sq proxy launches must keep their inherited environment"
        );
    }

    #[test]
    fn remote_acp_working_dir_requires_remote_path() {
        let error = resolve_acp_working_dir(Path::new("/tmp/local"), true, None)
            .expect_err("remote ACP cwd should be required");
        assert!(error.contains("absolute remote working directory"));
    }

    #[test]
    fn remote_acp_working_dir_rejects_relative_path() {
        let error = resolve_acp_working_dir(Path::new("/tmp/local"), true, Some(Path::new("repo")))
            .expect_err("relative remote ACP cwd should be rejected");
        assert!(error.contains("must be absolute"));
    }

    #[test]
    fn remote_acp_working_dir_uses_absolute_remote_path() {
        let remote = Path::new("/home/bloxer/repo");
        assert_eq!(
            resolve_acp_working_dir(Path::new("/tmp/local"), true, Some(remote)).unwrap(),
            remote
        );
    }

    #[test]
    fn local_acp_working_dir_is_absolute() {
        let resolved = resolve_acp_working_dir(Path::new("."), false, None).unwrap();
        assert!(resolved.is_absolute());
    }

    #[test]
    fn image_prompt_blocks_are_omitted_when_unsupported() {
        let images = vec![("abcd".to_string(), "image/png".to_string())];
        let blocks = build_prompt_content_blocks("inspect this", &images, false);

        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], AcpContentBlock::Text(_)));
        match &blocks[1] {
            AcpContentBlock::Text(text) => {
                assert!(text.text.contains("image attachment(s) omitted"));
            }
            other => panic!("expected omission notice text block, got {other:?}"),
        }
    }

    #[test]
    fn image_prompt_blocks_are_sent_when_supported() {
        let images = vec![("abcd".to_string(), "image/png".to_string())];
        let blocks = build_prompt_content_blocks("inspect this", &images, true);

        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], AcpContentBlock::Text(_)));
        assert!(matches!(blocks[1], AcpContentBlock::Image(_)));
    }

    #[test]
    fn shell_quote_simple_value() {
        assert_eq!(
            shell_quote("/usr/local/bin/goose"),
            "'/usr/local/bin/goose'"
        );
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("it's here"), "'it'\\''s here'");
    }

    #[test]
    fn shell_quote_preserves_spaces() {
        assert_eq!(shell_quote("/path/with space"), "'/path/with space'");
    }

    #[test]
    fn spawn_command_uses_home_interpreter_for_env_shebang_bridge() {
        let dir = unique_test_dir("acp-home-interpreter");
        let home_bin = dir.join("home-bin");
        let project_bin = dir.join("project-bin");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("claude-agent-acp");
        let home_node = home_bin.join("node");
        write_executable(&home_node, "#!/bin/sh\n");
        write_executable(&project_bin.join("node"), "#!/bin/sh\n");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let home_snapshot = vec![(
            String::from("PATH"),
            join_path_entries(std::slice::from_ref(&home_bin)),
        )];
        let command = acp_spawn_command(
            &launcher,
            &[String::from("--stdio")],
            Some(home_snapshot.as_slice()),
        );

        assert_eq!(command.program, home_node);
        assert_eq!(
            command.args,
            vec![
                launcher.as_os_str().to_os_string(),
                OsString::from("--stdio")
            ]
        );
        assert!(command.uses_explicit_interpreter);

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }

    #[test]
    fn spawn_command_falls_back_to_launcher_without_interpreter_snapshot() {
        let dir = unique_test_dir("acp-no-home-interpreter");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("codex-acp");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let command = acp_spawn_command(&launcher, &[String::from("--stdio")], None);

        assert_eq!(command.program, launcher);
        assert_eq!(command.args, vec![OsString::from("--stdio")]);
        assert!(!command.uses_explicit_interpreter);

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }

    #[test]
    fn shell_exec_line_guards_interpreter_before_mutating_path() {
        let dir = unique_test_dir("acp-shell-guard");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("amp-acp");
        write_executable(&agent_bin.join("node"), "#!/bin/sh\n");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let line = shell_exec_line(&launcher, &[String::from("--stdio")], None);

        assert!(
            line.starts_with(
                "command -v 'node' >/dev/null 2>&1 || { if [ -n \"$PATH\" ]; then PATH='"
            ),
            "shell fallback should check the initialized PATH before adding the agent dir: {line}"
        );
        assert!(
            line.contains(":\"$PATH\"; else PATH='"),
            "private agent dir should be prepended only inside the missing-interpreter guard: {line}"
        );
        assert!(
            line.contains("; fi; export PATH; }; exec '"),
            "shell fallback should export the guarded PATH before exec: {line}"
        );
        assert!(
            line.ends_with("' '--stdio'\n"),
            "exec should preserve the binary and args after the guard: {line}"
        );

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }

    #[test]
    fn shell_exec_line_uses_home_interpreter_when_available() {
        let dir = unique_test_dir("acp-shell-home-interpreter");
        let home_bin = dir.join("home-bin");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("claude-agent-acp");
        let home_node = home_bin.join("node");
        write_executable(&home_node, "#!/bin/sh\n");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let home_snapshot = vec![(
            String::from("PATH"),
            join_path_entries(std::slice::from_ref(&home_bin)),
        )];

        let line = shell_exec_line(
            &launcher,
            &[String::from("--stdio")],
            Some(home_snapshot.as_slice()),
        );

        assert_eq!(
            line,
            format!(
                "exec '{}' '{}' '--stdio'\n",
                home_node.display(),
                launcher.display()
            )
        );

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }

    #[test]
    fn mcp_transport_support_maps_each_transport_to_its_capability() {
        let stdio = McpServer::Stdio(McpServerStdio::new("local", "/usr/bin/server"));
        let http = McpServer::Http(McpServerHttp::new("remote", "https://example.com"));
        let sse = McpServer::Sse(McpServerSse::new("remote", "https://example.com/events"));

        // Stdio needs no capability — it is always supported, even with all caps off.
        let none = McpCapabilities::new();
        assert!(mcp_server_transport_supported(&stdio, &none));
        assert!(!mcp_server_transport_supported(&http, &none));
        assert!(!mcp_server_transport_supported(&sse, &none));

        // Each remote transport is gated on its own capability.
        let http_only = McpCapabilities::new().http(true);
        assert!(mcp_server_transport_supported(&http, &http_only));
        assert!(!mcp_server_transport_supported(&sse, &http_only));

        let sse_only = McpCapabilities::new().sse(true);
        assert!(mcp_server_transport_supported(&sse, &sse_only));
        assert!(!mcp_server_transport_supported(&http, &sse_only));
    }

    #[tokio::test]
    async fn local_stdout_normalization_filters_non_json() {
        use super::normalize_local_acp_stdout;
        use tokio::io::AsyncReadExt;

        let input = b"Hermit environment /home/user/.hermit activated\n\
                       {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\n\
                       some banner text\n\
                       {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":null}\n";

        let (writer, mut reader) = tokio::io::duplex(64 * 1024);
        let input_reader = &input[..];

        normalize_local_acp_stdout(input_reader, writer)
            .await
            .expect("normalization should succeed");

        let mut output = String::new();
        reader
            .read_to_string(&mut output)
            .await
            .expect("read should succeed");

        assert_eq!(
            output,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}\n\
             {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":null}\n"
        );
    }

    #[tokio::test]
    async fn local_stdout_normalization_passes_empty_input() {
        use super::normalize_local_acp_stdout;
        use tokio::io::AsyncReadExt;

        let input = b"";
        let (writer, mut reader) = tokio::io::duplex(64 * 1024);

        normalize_local_acp_stdout(&input[..], writer)
            .await
            .expect("normalization should succeed");

        let mut output = String::new();
        reader
            .read_to_string(&mut output)
            .await
            .expect("read should succeed");

        assert!(output.is_empty());
    }

    #[test]
    fn replay_boundary_prefers_acp_message_id_over_content() {
        let mut buffer = ReplayBuffer::new(vec![ReplayBoundary {
            role: "assistant".to_string(),
            content: "persisted text".to_string(),
            acp_message_id: Some("msg-1".to_string()),
            acp_tool_call_id: None,
        }]);

        let completed = buffer.try_match(&ReplayEvent {
            role: "assistant".to_string(),
            content: "provider replayed different text".to_string(),
            acp_message_id: Some("msg-1".to_string()),
            acp_tool_call_id: None,
        });

        assert!(completed);
        assert_eq!(buffer.match_cursor, 1);
    }

    #[test]
    fn replay_buffer_splits_assistant_chunks_when_message_id_changes() {
        let mut buffer = ReplayBuffer::new(vec![
            ReplayBoundary {
                role: "assistant".to_string(),
                content: "first".to_string(),
                acp_message_id: Some("msg-1".to_string()),
                acp_tool_call_id: None,
            },
            ReplayBoundary {
                role: "assistant".to_string(),
                content: "second".to_string(),
                acp_message_id: Some("msg-2".to_string()),
                acp_tool_call_id: None,
            },
        ]);

        let completed = buffer.push_text_chunk("assistant", Some("msg-1"), "first");

        assert!(!completed);
        assert_eq!(buffer.match_cursor, 0);

        let completed = buffer.push_text_chunk("assistant", Some("msg-2"), "second");

        assert!(!completed);
        assert_eq!(buffer.match_cursor, 1);
        assert_eq!(buffer.current_message_id.as_deref(), Some("msg-2"));
        assert_eq!(buffer.current_text, "second");

        let completed = buffer.finalize_current();

        assert!(completed);
        assert_eq!(buffer.match_cursor, 2);
    }

    #[tokio::test]
    async fn basic_writer_records_only_agent_text() {
        let writer = BasicMessageWriter::new();

        writer.append_text("[").await;
        writer.record_tool_call("call-1", "Terminal", None).await;
        writer.record_tool_result("call-1", "total 42").await;
        writer.append_text("]").await;

        // One-shot callers parse this buffer as JSON, so tool activity must not
        // leak into it.
        assert_eq!(writer.get_text().await, "[]");
    }

    #[tokio::test]
    async fn replay_handler_treats_user_only_boundaries_as_complete() {
        let writer: Arc<dyn MessageWriter> = Arc::new(BasicMessageWriter::new());
        let handler = AcpNotificationHandler::new(
            writer,
            true,
            vec![ReplayBoundary::legacy(
                "user".to_string(),
                "previous prompt".to_string(),
            )],
            CancellationToken::new(),
        );

        assert!(handler.is_replay_complete().await);
    }

    #[tokio::test]
    async fn replay_handler_drops_replayed_plan_updates() {
        let writer = Arc::new(RecordingMessageWriter::default());
        let handler = AcpNotificationHandler::new(
            writer.clone(),
            true,
            vec![ReplayBoundary::legacy(
                "assistant".to_string(),
                "previous response".to_string(),
            )],
            CancellationToken::new(),
        );

        handler
            .session_notification(SessionNotification::new(
                "session-1",
                SessionUpdate::Plan(test_plan()),
            ))
            .await
            .expect("replayed plan should be accepted");

        assert!(
            writer.events.lock().unwrap().is_empty(),
            "plan updates replayed by session/load must not be persisted again"
        );

        handler.transition_to_live().await;
        handler
            .session_notification(SessionNotification::new(
                "session-1",
                SessionUpdate::Plan(test_plan()),
            ))
            .await
            .expect("live plan should be accepted");

        let events = writer.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind.as_deref(), Some("plan_update"));
    }

    #[test]
    fn replay_boundary_falls_back_to_role_and_content_without_ids() {
        let mut buffer = ReplayBuffer::new(vec![ReplayBoundary::legacy(
            "assistant".to_string(),
            "persisted text".to_string(),
        )]);

        let mismatch = buffer.try_match(&ReplayEvent {
            role: "assistant".to_string(),
            content: "different text".to_string(),
            acp_message_id: None,
            acp_tool_call_id: None,
        });
        assert!(!mismatch);
        assert_eq!(buffer.match_cursor, 0);

        let completed = buffer.try_match(&ReplayEvent {
            role: "assistant".to_string(),
            content: "persisted text".to_string(),
            acp_message_id: None,
            acp_tool_call_id: None,
        });
        assert!(completed);
        assert_eq!(buffer.match_cursor, 1);
    }

    #[test]
    fn cancelled_permission_response_uses_acp_cancelled_outcome() {
        let options = vec![PermissionOption::new(
            "approve",
            "Approve",
            PermissionOptionKind::AllowOnce,
        )];

        let response = permission_response_for_options(&options, true);

        assert!(matches!(
            response.outcome,
            RequestPermissionOutcome::Cancelled
        ));
    }

    #[test]
    fn permission_response_autoapproves_allow_option_before_cancellation() {
        let options = vec![
            PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
            PermissionOption::new("approve", "Approve", PermissionOptionKind::AllowOnce),
        ];

        let response = permission_response_for_options(&options, false);

        match response.outcome {
            RequestPermissionOutcome::Selected(outcome) => {
                assert_eq!(outcome.option_id.0.as_ref(), "approve");
            }
            RequestPermissionOutcome::Cancelled => panic!("permission should be selected"),
            _ => panic!("unexpected permission outcome"),
        }
    }

    #[test]
    fn autoapproval_ignores_reject_kind_named_dont_allow() {
        let request = acp_permission_request(vec![
            acp_permission_option("reject", "Don't allow", AcpPermissionOptionKind::RejectOnce),
            acp_permission_option("approve", "Approve", AcpPermissionOptionKind::AllowOnce),
        ]);

        let decision = autoapprove_permission_decision(&request);

        assert_eq!(
            decision,
            super::AcpPermissionDecision::Selected {
                option_id: "approve".to_string()
            }
        );
    }

    #[test]
    fn autoapproval_ignores_reject_kind_named_disallow() {
        let request = acp_permission_request(vec![
            acp_permission_option("reject", "Disallow", AcpPermissionOptionKind::RejectAlways),
            acp_permission_option("allow", "Allow", AcpPermissionOptionKind::AllowAlways),
        ]);

        let decision = autoapprove_permission_decision(&request);

        assert_eq!(
            decision,
            super::AcpPermissionDecision::Selected {
                option_id: "allow".to_string()
            }
        );
    }

    #[test]
    fn autoapproval_falls_back_to_legacy_matching_for_unknown_kind() {
        let request = acp_permission_request(vec![
            acp_permission_option("reject", "Reject", AcpPermissionOptionKind::RejectOnce),
            acp_permission_option(
                "approve-custom",
                "Proceed",
                AcpPermissionOptionKind::Unknown,
            ),
        ]);

        let decision = autoapprove_permission_decision(&request);

        assert_eq!(
            decision,
            super::AcpPermissionDecision::Selected {
                option_id: "approve-custom".to_string()
            }
        );
    }

    #[test]
    fn stop_reason_cancelled_maps_to_cancelled_outcome() {
        assert_eq!(
            AgentRunOutcome::from_stop_reason(StopReason::Cancelled),
            AgentRunOutcome::Cancelled
        );
        assert_eq!(
            AgentRunOutcome::from_stop_reason(StopReason::EndTurn),
            AgentRunOutcome::Completed
        );
    }

    fn task_started(task_id: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "system",
            "subtype": "task_started",
            "task_id": task_id,
            "tool_use_id": format!("toolu_{task_id}"),
            "description": "sleep 10 && echo done",
        })
    }

    #[test]
    fn background_task_set_inserts_on_task_started() {
        let mut set = BackgroundTaskSet::default();

        assert!(set.apply_sdk_message(&task_started("task-1")));
        assert!(set.apply_sdk_message(&task_started("task-2")));
        assert_eq!(set.sorted_ids(), ["task-1", "task-2"]);

        // Re-announcing a live task is not a membership change.
        assert!(!set.apply_sdk_message(&task_started("task-1")));
    }

    #[test]
    fn background_tasks_changed_replaces_the_set() {
        let mut set = BackgroundTaskSet::default();
        set.apply_sdk_message(&task_started("task-1"));
        set.apply_sdk_message(&task_started("task-2"));

        // The snapshot is authoritative: task-1 vanished without a terminal
        // bookend and task-3 appears without a task_started.
        let snapshot = serde_json::json!({
            "type": "system",
            "subtype": "background_tasks_changed",
            "tasks": [
                { "task_id": "task-2", "task_type": "local_bash", "description": "a" },
                { "task_id": "task-3", "task_type": "local_bash", "description": "b" },
            ],
        });
        assert!(set.apply_sdk_message(&snapshot));
        assert_eq!(set.sorted_ids(), ["task-2", "task-3"]);

        // An identical snapshot is not a membership change.
        assert!(!set.apply_sdk_message(&snapshot));

        // An empty snapshot drains the set.
        assert!(set.apply_sdk_message(&serde_json::json!({
            "type": "system",
            "subtype": "background_tasks_changed",
            "tasks": [],
        })));
        assert!(set.sorted_ids().is_empty());
    }

    #[test]
    fn terminal_task_updated_removes_but_running_does_not() {
        let task_updated = |task_id: &str, status: &str| {
            serde_json::json!({
                "type": "system",
                "subtype": "task_updated",
                "task_id": task_id,
                "patch": { "status": status },
            })
        };
        let mut set = BackgroundTaskSet::default();
        for id in ["task-1", "task-2", "task-3", "task-4", "task-5", "task-6"] {
            set.apply_sdk_message(&task_started(id));
        }

        // Non-terminal patches keep the task live. `pending` and `paused` are
        // the other two states upstream's mapping treats as non-terminal.
        for status in ["running", "pending", "paused"] {
            assert!(!set.apply_sdk_message(&task_updated("task-1", status)));
        }
        assert_eq!(set.sorted_ids().len(), 6);

        // The terminal set matches upstream's `taskState` mapping, which folds
        // `killed`, `cancelled` and `stopped` together: a task the agent
        // reports as cancelled or stopped is dead and must not keep the hold
        // open waiting for a reconciling snapshot.
        assert!(set.apply_sdk_message(&task_updated("task-1", "completed")));
        assert!(set.apply_sdk_message(&task_updated("task-2", "failed")));
        assert!(set.apply_sdk_message(&task_updated("task-3", "killed")));
        assert!(set.apply_sdk_message(&task_updated("task-4", "cancelled")));
        assert!(set.apply_sdk_message(&task_updated("task-5", "stopped")));
        assert_eq!(set.sorted_ids(), ["task-6"]);

        // A status this client doesn't know keeps the task live — pessimistic,
        // like the typed set's `Unknown`: the cap bounds a stale entry, while
        // dropping a live task loses its continuation.
        assert!(!set.apply_sdk_message(&task_updated("task-6", "reticulating")));
        assert_eq!(set.sorted_ids(), ["task-6"]);

        // Terminal update for an unknown task is a no-op.
        assert!(!set.apply_sdk_message(&task_updated("task-9", "completed")));
    }

    #[test]
    fn task_notification_removes_the_settled_task() {
        let mut set = BackgroundTaskSet::default();
        set.apply_sdk_message(&task_started("task-1"));

        assert!(set.apply_sdk_message(&serde_json::json!({
            "type": "system",
            "subtype": "task_notification",
            "task_id": "task-1",
            "status": "completed",
            "summary": "done",
        })));
        assert!(set.sorted_ids().is_empty());
    }

    #[test]
    fn background_task_set_ignores_unrelated_frames() {
        let mut set = BackgroundTaskSet::default();
        set.apply_sdk_message(&task_started("task-1"));

        // Non-system frames never touch the set, even with a task-ish shape.
        assert!(!set.apply_sdk_message(&serde_json::json!({
            "type": "assistant",
            "subtype": "task_notification",
            "task_id": "task-1",
        })));
        // Unknown system subtypes are ignored.
        assert!(!set.apply_sdk_message(&serde_json::json!({
            "type": "system",
            "subtype": "task_progress",
            "task_id": "task-1",
        })));
        // A task_started missing its task_id is ignored.
        assert!(!set.apply_sdk_message(&serde_json::json!({
            "type": "system",
            "subtype": "task_started",
        })));
        assert_eq!(set.sorted_ids(), ["task-1"]);
    }

    #[test]
    fn session_new_meta_requests_background_task_frames() {
        let meta = background_task_tracking_meta(TaskTrackingMode::Raw);

        let filters = meta["claudeCode"]["emitRawSDKMessages"]
            .as_array()
            .expect("emitRawSDKMessages should be a filter array");
        let (system_filters, other_filters): (Vec<_>, Vec<_>) = filters
            .iter()
            .partition(|filter| filter["type"] == "system");
        let subtypes: Vec<&str> = system_filters
            .iter()
            .map(|filter| {
                filter["subtype"]
                    .as_str()
                    .expect("subtype should be a string")
            })
            .collect();

        let expected: Vec<&str> = BACKGROUND_TASK_SUBTYPES
            .iter()
            .copied()
            // The probe subtype rides along so the raw-SDK stream proves
            // itself even on a turn that starts no background tasks, and the
            // session-state subtype feeds the holding wait's idle latch.
            .chain([AVAILABILITY_PROBE_SUBTYPE, SESSION_STATE_SUBTYPE])
            .collect();
        assert_eq!(subtypes, expected);

        // Plus the origin-matched assistant filter that names the autonomous
        // cycle a continuation record belongs to.
        assert_eq!(other_filters.len(), 1);
        assert_eq!(other_filters[0]["type"], "assistant");
        assert_eq!(other_filters[0]["origin"], TASK_NOTIFICATION_ORIGIN);
    }

    #[test]
    fn typed_mode_meta_drops_task_frames_but_keeps_the_idle_latch() {
        let meta = background_task_tracking_meta(TaskTrackingMode::Typed);

        let filters = meta["claudeCode"]["emitRawSDKMessages"]
            .as_array()
            .expect("emitRawSDKMessages should be a filter array");

        // The typed lifecycle replaces the task_* subtypes and the init
        // availability probe; only the idle-latch frame and the continuation
        // origin filter remain requested.
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0]["type"], "system");
        assert_eq!(filters[0]["subtype"], SESSION_STATE_SUBTYPE);
        assert_eq!(filters[1]["type"], "assistant");
        assert_eq!(filters[1]["origin"], TASK_NOTIFICATION_ORIGIN);
    }

    #[test]
    fn the_advertisement_and_the_mirror_check_agree() {
        let meta = air_client_capabilities_meta();
        assert_eq!(
            serde_json::to_value(&meta).expect("meta should serialize"),
            serde_json::json!({
                "jetbrains": { "air": { "version": 1, "capabilities": ["asyncTasks"] } }
            })
        );
        // Reciprocity: an agent that mirrors the advertisement verbatim must
        // read typed, or the two sides could never negotiate.
        assert_eq!(
            task_tracking_mode_from_initialize(Some(&meta)),
            TaskTrackingMode::Typed
        );
    }

    #[test]
    fn mirror_detection_requires_version_and_capability_membership() {
        let mode = |meta: serde_json::Value| {
            let serde_json::Value::Object(meta) = meta else {
                panic!("test meta must be an object");
            };
            task_tracking_mode_from_initialize(Some(&meta))
        };

        // A mirroring bridge lists everything it accepted — sibling
        // capabilities and a newer version both still count.
        assert_eq!(
            mode(serde_json::json!({
                "jetbrains": { "air": { "version": 1, "capabilities": ["asyncTasks", "steering"] } }
            })),
            TaskTrackingMode::Typed
        );
        assert_eq!(
            mode(serde_json::json!({
                "jetbrains": { "air": { "version": 2, "capabilities": ["asyncTasks"] } }
            })),
            TaskTrackingMode::Typed
        );

        // No mirror is simply an agent without the extension: bridge 0.70.0
        // and older returns `_meta` without the namespace (or none at all),
        // and any malformed or insufficient mirror reads the same.
        assert_eq!(
            task_tracking_mode_from_initialize(None),
            TaskTrackingMode::Raw
        );
        for (label, meta) in [
            (
                "no jetbrains namespace",
                serde_json::json!({ "claudeCode": { "version": "0.70.0" } }),
            ),
            (
                "missing version",
                serde_json::json!({ "jetbrains": { "air": { "capabilities": ["asyncTasks"] } } }),
            ),
            (
                "version below 1",
                serde_json::json!({
                    "jetbrains": { "air": { "version": 0, "capabilities": ["asyncTasks"] } }
                }),
            ),
            (
                "non-integer version",
                serde_json::json!({
                    "jetbrains": { "air": { "version": "1", "capabilities": ["asyncTasks"] } }
                }),
            ),
            (
                "asyncTasks not mirrored",
                serde_json::json!({
                    "jetbrains": { "air": { "version": 1, "capabilities": ["nativeSubagentSessions"] } }
                }),
            ),
        ] {
            assert_eq!(mode(meta), TaskTrackingMode::Raw, "case: {label}");
        }
    }

    #[test]
    fn incoming_session_update_parses_standard_and_all_three_async_task_kinds() {
        let parse = |params: serde_json::Value| {
            IncomingSessionUpdate::parse_message("session/update", &params)
        };
        let async_params = |update: serde_json::Value| serde_json::json!({ "sessionId": "sess-1", "update": update });

        // The plain typed enum still takes the standard arm.
        let standard = parse(serde_json::json!({
            "sessionId": "sess-1",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": "hi" },
            },
        }))
        .expect("standard update should parse");
        assert!(matches!(
            standard,
            IncomingSessionUpdate::Standard(SessionNotification {
                update: SessionUpdate::AgentMessageChunk(_),
                ..
            })
        ));

        // `async_task_spawned` as the bridge publishes it, extension-only
        // fields included.
        let spawned = parse(async_params(serde_json::json!({
            "sessionUpdate": "async_task_spawned",
            "asyncTaskId": "task-1",
            "name": "Investigate flaky test",
            "taskType": "local_agent",
            "description": "Bisecting the failure",
            "showInTranscript": false,
            "canStop": true,
            "outputFilePath": "/tmp/task-1.md",
            "toolCallId": "call-9",
        })))
        .expect("async_task_spawned should parse");
        match spawned {
            IncomingSessionUpdate::AsyncTask(notification) => {
                assert_eq!(notification.session_id, "sess-1");
                assert_eq!(
                    notification.update,
                    AsyncTaskUpdate::Spawned {
                        task_id: "task-1".into(),
                        name: Some("Investigate flaky test".into()),
                        description: Some("Bisecting the failure".into()),
                        output_file_path: Some("/tmp/task-1.md".into()),
                        tool_call_id: Some("call-9".into()),
                    }
                );
            }
            other => panic!("expected the async-task arm, got {other:?}"),
        }

        // `async_task_progress` is a presence signal; extra summary fields
        // are tolerated and ignored.
        let progress = parse(async_params(serde_json::json!({
            "sessionUpdate": "async_task_progress",
            "asyncTaskId": "task-1",
            "recentActivity": "compiling",
        })))
        .expect("async_task_progress should parse");
        assert!(matches!(
            progress,
            IncomingSessionUpdate::AsyncTask(AsyncTaskNotification {
                update: AsyncTaskUpdate::Progress { .. },
                ..
            })
        ));

        // `async_task_state_update` across the whole published lifecycle —
        // `paused` and `stopped` are states the raw path never surfaced.
        for (wire, state) in [
            ("running", AsyncTaskState::Running),
            ("paused", AsyncTaskState::Paused),
            ("completed", AsyncTaskState::Completed),
            ("failed", AsyncTaskState::Failed),
            ("stopped", AsyncTaskState::Stopped),
        ] {
            let update = parse(async_params(serde_json::json!({
                "sessionUpdate": "async_task_state_update",
                "asyncTaskId": "task-1",
                "state": wire,
            })))
            .unwrap_or_else(|e| panic!("state {wire:?} should parse: {e:?}"));
            match update {
                IncomingSessionUpdate::AsyncTask(notification) => assert_eq!(
                    notification.update,
                    AsyncTaskUpdate::StateUpdate {
                        task_id: "task-1".into(),
                        state,
                    }
                ),
                other => panic!("expected the async-task arm for {wire:?}, got {other:?}"),
            }
        }

        // Only `session/update` matches this wrapper at all.
        assert!(IncomingSessionUpdate::parse_message(
            "session/request_permission",
            &async_params(serde_json::json!({
                "sessionUpdate": "async_task_progress",
                "asyncTaskId": "task-1",
            })),
        )
        .is_err());
        // An unknown discriminator keeps the typed parse's error: the
        // dispatch layer hard-errors rather than silently dropping it — the
        // same loud failure an unadvertised extension update gets today.
        assert!(parse(async_params(serde_json::json!({
            "sessionUpdate": "subagent_spawned",
            "subagentSessionId": "child-1",
        })))
        .is_err());
        // As does an async_task_* update missing its required fields.
        assert!(parse(async_params(serde_json::json!({
            "sessionUpdate": "async_task_spawned",
            "name": "no id",
        })))
        .is_err());
        assert!(parse(async_params(serde_json::json!({
            "sessionUpdate": "async_task_state_update",
            "asyncTaskId": "task-1",
        })))
        .is_err());
    }

    #[test]
    fn async_task_states_parse_and_classify_liveness() {
        for (wire, state) in [
            ("running", AsyncTaskState::Running),
            ("paused", AsyncTaskState::Paused),
            ("completed", AsyncTaskState::Completed),
            ("failed", AsyncTaskState::Failed),
            ("stopped", AsyncTaskState::Stopped),
            ("hibernating", AsyncTaskState::Unknown),
        ] {
            assert_eq!(AsyncTaskState::parse(wire), state, "state: {wire:?}");
        }
        // `paused` parks the task without ending it, and an unknown state
        // folds to live — pessimistic, bounded by the cap.
        for live in [
            AsyncTaskState::Running,
            AsyncTaskState::Paused,
            AsyncTaskState::Unknown,
        ] {
            assert!(live.is_live(), "{live:?} should hold the wait open");
        }
        for terminal in [
            AsyncTaskState::Completed,
            AsyncTaskState::Failed,
            AsyncTaskState::Stopped,
        ] {
            assert!(!terminal.is_live(), "{terminal:?} should release the wait");
        }
    }

    #[test]
    fn typed_task_set_tracks_live_membership() {
        let spawned = |id: &str| AsyncTaskUpdate::Spawned {
            task_id: id.to_string(),
            name: None,
            description: None,
            output_file_path: None,
            tool_call_id: None,
        };
        let moved = |id: &str, state: AsyncTaskState| AsyncTaskUpdate::StateUpdate {
            task_id: id.to_string(),
            state,
        };
        let mut set = TypedAsyncTaskSet::default();

        assert!(set.apply(&spawned("task-1")));
        assert!(
            !set.apply(&spawned("task-1")),
            "a duplicate spawn is not a membership change"
        );
        // Progress is presence, deliberately not a lifecycle edge.
        assert!(!set.apply(&AsyncTaskUpdate::Progress {
            task_id: "task-1".to_string(),
        }));
        assert_eq!(set.live_count(), 1);

        // Pausing parks the task but keeps it live: a move between two live
        // states is not a membership change.
        assert!(!set.apply(&moved("task-1", AsyncTaskState::Paused)));
        assert_eq!(set.live_ids(), ["task-1"]);

        // A state update is authoritative even when the spawn was never seen.
        assert!(set.apply(&moved("task-2", AsyncTaskState::Running)));
        assert_eq!(set.live_ids(), ["task-1", "task-2"]);

        // An unknown state keeps the task live rather than draining early.
        assert!(!set.apply(&moved("task-2", AsyncTaskState::Unknown)));
        assert_eq!(set.live_count(), 2);

        // Every terminal kind drains, `stopped` and `failed` included.
        assert!(set.apply(&moved("task-1", AsyncTaskState::Completed)));
        assert!(set.apply(&moved("task-2", AsyncTaskState::Stopped)));
        assert!(set.apply(&spawned("task-3")));
        assert!(set.apply(&moved("task-3", AsyncTaskState::Failed)));
        assert_eq!(set.live_count(), 0);
        assert!(set.live_ids().is_empty());
    }

    #[test]
    fn typed_task_set_ignores_a_spawn_for_a_terminal_id() {
        let spawned = |id: &str| AsyncTaskUpdate::Spawned {
            task_id: id.to_string(),
            name: None,
            description: None,
            output_file_path: None,
            tool_call_id: None,
        };
        let moved = |id: &str, state: AsyncTaskState| AsyncTaskUpdate::StateUpdate {
            task_id: id.to_string(),
            state,
        };
        let mut set = TypedAsyncTaskSet::default();

        assert!(set.apply(&spawned("task-1")));
        assert!(set.apply(&moved("task-1", AsyncTaskState::Completed)));

        // A replayed announcement of the finished task must not resurrect
        // it: its terminal state was already published, no further edge is
        // guaranteed, and a resurrected entry would hold to the cap.
        assert!(!set.apply(&spawned("task-1")));
        assert_eq!(set.live_count(), 0);
        assert_eq!(
            set.tasks.get("task-1").map(|task| task.state),
            Some(AsyncTaskState::Completed),
            "the terminal state stands"
        );

        // Every terminal kind is guarded, and a paused (live) task is not:
        // re-announcing it keeps it live rather than dropping it.
        assert!(set.apply(&spawned("task-2")));
        assert!(set.apply(&moved("task-2", AsyncTaskState::Stopped)));
        assert!(!set.apply(&spawned("task-2")));
        assert!(set.apply(&spawned("task-3")));
        assert!(!set.apply(&moved("task-3", AsyncTaskState::Paused)));
        assert!(!set.apply(&spawned("task-3")), "still live, not a change");
        assert_eq!(set.live_ids(), ["task-3"]);

        // A fresh id is a brand-new task as usual.
        assert!(set.apply(&spawned("task-4")));
        assert_eq!(set.live_ids(), ["task-3", "task-4"]);
    }

    #[test]
    fn typed_task_set_snapshots_live_tasks_with_their_metadata() {
        let mut set = TypedAsyncTaskSet::default();
        assert!(set.apply(&AsyncTaskUpdate::Spawned {
            task_id: "task-b".to_string(),
            name: Some("Run the tests".to_string()),
            description: Some("cargo test in the background".to_string()),
            output_file_path: Some("/tmp/tests.log".to_string()),
            tool_call_id: None,
        }));
        // A task known only from a lifecycle edge has no metadata to present.
        assert!(set.apply(&AsyncTaskUpdate::StateUpdate {
            task_id: "task-a".to_string(),
            state: AsyncTaskState::Running,
        }));

        // Sorted by id, so repeated snapshots of one set compare (and render)
        // identically.
        assert_eq!(
            set.live_snapshot(),
            [
                BackgroundHoldTask {
                    id: "task-a".to_string(),
                    name: None,
                    description: None,
                    output_file_path: None,
                },
                BackgroundHoldTask {
                    id: "task-b".to_string(),
                    name: Some("Run the tests".to_string()),
                    description: Some("cargo test in the background".to_string()),
                    output_file_path: Some("/tmp/tests.log".to_string()),
                },
            ]
        );
        assert_eq!(set.task_name("task-b").as_deref(), Some("Run the tests"));
        assert_eq!(set.task_name("task-a"), None);

        // A move between live states keeps the spawn's metadata; a terminal
        // one leaves the snapshot but keeps its name for the wake label.
        assert!(!set.apply(&AsyncTaskUpdate::StateUpdate {
            task_id: "task-b".to_string(),
            state: AsyncTaskState::Paused,
        }));
        assert_eq!(
            set.live_snapshot()[1].name.as_deref(),
            Some("Run the tests")
        );
        assert!(set.apply(&AsyncTaskUpdate::StateUpdate {
            task_id: "task-b".to_string(),
            state: AsyncTaskState::Completed,
        }));
        assert_eq!(set.live_snapshot().len(), 1);
        assert_eq!(set.task_name("task-b").as_deref(), Some("Run the tests"));
    }

    #[test]
    fn async_task_stop_frames_match_the_extension_contract() {
        let message = async_task_stop_message("acp-sess", "task-1").expect("message should build");
        assert_eq!(message.method(), ASYNC_TASK_STOP_METHOD);
        assert_eq!(
            message.params(),
            &serde_json::json!({ "sessionId": "acp-sess", "asyncTaskId": "task-1" })
        );

        // The agent's answer is its own `{stopped}` verdict...
        assert_eq!(
            async_task_stop_outcome(Ok(serde_json::json!({ "stopped": true }))),
            Ok(true)
        );
        // ...`false` for a task it did not stop (unknown id, already
        // terminal), and a malformed response reads the same way.
        assert_eq!(
            async_task_stop_outcome(Ok(serde_json::json!({ "stopped": false }))),
            Ok(false)
        );
        assert_eq!(
            async_task_stop_outcome(Ok(serde_json::json!({}))),
            Ok(false)
        );
        // An agent without the extension refuses the method rather than
        // staying silent, and that refusal must reach the caller as an error.
        let refused =
            async_task_stop_outcome(Err(agent_client_protocol::Error::method_not_found()))
                .expect_err("an agent error must surface");
        assert!(refused.contains("stop request failed"), "got: {refused}");
    }

    #[test]
    fn labeled_origin_extends_only_the_task_notification_kind() {
        assert_eq!(
            labeled_background_continuation_origin(
                Some(TASK_NOTIFICATION_ORIGIN),
                Some("Run the tests"),
            ),
            "background-continuation:task-notification:Run the tests"
        );
        // Without a name (raw mode, or a task that never announced one) the
        // kind-only tag stands.
        assert_eq!(
            labeled_background_continuation_origin(Some(TASK_NOTIFICATION_ORIGIN), None),
            background_continuation_origin(Some(TASK_NOTIFICATION_ORIGIN))
        );
        // No other cycle kind is woken by a settled task, so no other kind
        // gets the label.
        assert_eq!(
            labeled_background_continuation_origin(Some("peer"), Some("Run the tests")),
            "background-continuation:peer"
        );
        assert_eq!(
            labeled_background_continuation_origin(None, Some("Run the tests")),
            BACKGROUND_CONTINUATION_ORIGIN
        );
    }

    #[test]
    fn a_background_shells_command_name_is_bounded_before_it_is_persisted() {
        // A background shell's task name is the command itself: the bridge
        // falls back to the spawn description, which is `input.command`. The
        // origin is persisted as an attribution value, so a multi-line script
        // must not land in it verbatim.
        let command = "sleep 20 && echo done\n  > marker.txt\t&& \\\n  echo 'and a tail long \
                       enough to be clipped by the bound'";
        let origin =
            labeled_background_continuation_origin(Some(TASK_NOTIFICATION_ORIGIN), Some(command));

        assert!(!origin.contains('\n'), "got: {origin}");
        assert!(!origin.contains('\t'), "got: {origin}");
        assert!(
            origin.ends_with('…'),
            "a clipped label reads as clipped: {origin}"
        );
        // Consumers prefix-match, so the bound must not disturb the prefix.
        assert!(origin.starts_with(&format!(
            "{BACKGROUND_CONTINUATION_ORIGIN}:{TASK_NOTIFICATION_ORIGIN}:"
        )));
        assert!(
            origin.chars().count()
                <= BACKGROUND_CONTINUATION_ORIGIN.chars().count()
                    + TASK_NOTIFICATION_ORIGIN.chars().count()
                    + ORIGIN_TASK_NAME_MAX_CHARS
                    + "::…".chars().count()
        );
    }

    #[test]
    fn a_task_name_is_collapsed_and_clipped_but_a_short_one_is_untouched() {
        assert_eq!(
            origin_task_name_label("Run the tests"),
            Some("Run the tests".to_string())
        );
        // Newlines, tabs and runs of spaces all collapse to single spaces.
        assert_eq!(
            origin_task_name_label("  Run\n\tthe   tests  "),
            Some("Run the tests".to_string())
        );
        // Exactly at the bound is not clipped; one char past it is.
        let at_bound = "x".repeat(ORIGIN_TASK_NAME_MAX_CHARS);
        assert_eq!(origin_task_name_label(&at_bound), Some(at_bound.clone()));
        assert_eq!(
            origin_task_name_label(&format!("{at_bound}y")),
            Some(format!("{at_bound}…"))
        );
        // Multi-byte names are clipped on a char boundary, not a byte one.
        let multibyte = "é".repeat(ORIGIN_TASK_NAME_MAX_CHARS + 10);
        let clipped = origin_task_name_label(&multibyte).expect("a non-blank name labels");
        assert_eq!(clipped.chars().count(), ORIGIN_TASK_NAME_MAX_CHARS + 1);
        // A name that is only whitespace labels nothing, so the tag stays at
        // its unlabeled form rather than ending in a bare separator.
        assert_eq!(origin_task_name_label("   \n\t "), None);
        assert_eq!(origin_task_name_label(""), None);
        assert_eq!(
            labeled_background_continuation_origin(Some(TASK_NOTIFICATION_ORIGIN), Some("  \n ")),
            background_continuation_origin(Some(TASK_NOTIFICATION_ORIGIN))
        );
    }

    #[test]
    fn sdk_message_origin_kind_reads_the_frames_own_origin_first() {
        // An autonomous cycle's frame names its own origin.
        assert_eq!(
            sdk_message_origin_kind(&serde_json::json!({
                "type": "assistant",
                "origin": { "kind": TASK_NOTIFICATION_ORIGIN },
            })),
            Some(TASK_NOTIFICATION_ORIGIN.to_string())
        );
        // A non-task origin is carried through verbatim, not normalized.
        assert_eq!(
            sdk_message_origin_kind(&serde_json::json!({
                "type": "assistant",
                "origin": { "kind": "peer" },
            })),
            Some("peer".to_string())
        );
        // A settled background task carries no origin, but it is exactly what
        // wakes a task-notification cycle.
        assert_eq!(
            sdk_message_origin_kind(&serde_json::json!({
                "type": "system",
                "subtype": "task_notification",
                "task_id": "task-1",
            })),
            Some(TASK_NOTIFICATION_ORIGIN.to_string())
        );
        // Everything else says nothing about attribution.
        for frame in [
            serde_json::json!({ "type": "system", "subtype": "init" }),
            serde_json::json!({ "type": "system", "subtype": "task_started", "task_id": "t" }),
            serde_json::json!({ "type": "assistant" }),
        ] {
            assert_eq!(sdk_message_origin_kind(&frame), None, "frame: {frame}");
        }
    }

    #[tokio::test]
    async fn ext_notification_maintains_background_task_set() {
        let handler = AcpNotificationHandler::new(
            Arc::new(BasicMessageWriter::new()),
            false,
            vec![],
            CancellationToken::new(),
        );

        let sdk_message = |message: serde_json::Value| {
            let params = serde_json::value::to_raw_value(&serde_json::json!({
                "sessionId": "sess-1",
                "message": message,
            }))
            .expect("params should serialize");
            ExtNotification::new(CLAUDE_SDK_MESSAGE_METHOD, params.into())
        };

        handler
            .ext_notification(sdk_message(task_started("task-1")))
            .await
            .expect("sdkMessage frame should be accepted");
        assert_eq!(
            handler.background_tasks.lock().await.sorted_ids(),
            ["task-1"]
        );

        // Frames for other extension methods are ignored, not errors.
        let unrelated = serde_json::value::to_raw_value(&serde_json::json!({}))
            .expect("params should serialize");
        handler
            .ext_notification(ExtNotification::new("claude/other", unrelated.into()))
            .await
            .expect("unknown ext methods should be dropped without error");
        assert_eq!(
            handler.background_tasks.lock().await.sorted_ids(),
            ["task-1"]
        );

        // Malformed params (no message field) are dropped without error.
        let no_message = serde_json::value::to_raw_value(&serde_json::json!({
            "sessionId": "sess-1",
        }))
        .expect("params should serialize");
        handler
            .ext_notification(ExtNotification::new(
                CLAUDE_SDK_MESSAGE_METHOD,
                no_message.into(),
            ))
            .await
            .expect("malformed params should be dropped without error");
        assert_eq!(
            handler.background_tasks.lock().await.sorted_ids(),
            ["task-1"]
        );
    }

    #[tokio::test]
    async fn ext_notification_publishes_background_activity() {
        let handler = AcpNotificationHandler::new(
            Arc::new(BasicMessageWriter::new()),
            false,
            vec![],
            CancellationToken::new(),
        );
        let mut activity_rx = handler.subscribe_background_activity();
        assert_eq!(
            *activity_rx.borrow_and_update(),
            BackgroundActivity::default()
        );

        let sdk_message = |message: serde_json::Value| {
            let params = serde_json::value::to_raw_value(&serde_json::json!({
                "sessionId": "sess-1",
                "message": message,
            }))
            .expect("params should serialize");
            ExtNotification::new(CLAUDE_SDK_MESSAGE_METHOD, params.into())
        };

        // An `init` frame carries no task at all, but it proves the raw-SDK
        // stream is live — the fallback rule's availability probe.
        handler
            .ext_notification(sdk_message(serde_json::json!({
                "type": "system",
                "subtype": "init",
            })))
            .await
            .expect("init frame should be accepted");
        let after_init = activity_rx.borrow_and_update().clone();
        assert!(after_init.sdk_frames_seen);
        assert_eq!(after_init.live_tasks, 0);
        assert!(
            !after_init.ever_started_task,
            "the probe mentions no task, so the taskless fast path stays available"
        );

        handler
            .ext_notification(sdk_message(task_started("task-1")))
            .await
            .expect("task_started frame should be accepted");
        let after_start = activity_rx.borrow_and_update().clone();
        assert_eq!(after_start.live_tasks, 1);
        assert!(after_start.ever_started_task);
        assert!(
            after_start.activity_seq > after_init.activity_seq,
            "every frame must bump the sequence so the debounce clock resets"
        );

        // Draining the set does not forget that a task existed: the drained
        // connection keeps the full debounce.
        handler
            .ext_notification(sdk_message(serde_json::json!({
                "type": "system",
                "subtype": "background_tasks_changed",
                "tasks": [],
            })))
            .await
            .expect("drain frame should be accepted");
        let after_drain = activity_rx.borrow_and_update().clone();
        assert_eq!(after_drain.live_tasks, 0);
        assert!(after_drain.ever_started_task);
        assert_eq!(
            after_drain.session_state, None,
            "no session_state_changed frame has arrived yet"
        );

        // The idle latch tracks the last session_state_changed frame, and
        // frames that carry no state leave it where it was.
        handler
            .ext_notification(sdk_message(serde_json::json!({
                "type": "system",
                "subtype": "session_state_changed",
                "state": "running",
            })))
            .await
            .expect("state frame should be accepted");
        assert_eq!(
            activity_rx.borrow_and_update().session_state,
            Some(SdkSessionState::Busy)
        );
        handler
            .ext_notification(sdk_message(serde_json::json!({
                "type": "system",
                "subtype": "init",
            })))
            .await
            .expect("init frame should be accepted");
        assert_eq!(
            activity_rx.borrow_and_update().session_state,
            Some(SdkSessionState::Busy),
            "a state-less frame must not clear the latch"
        );
        handler
            .ext_notification(sdk_message(serde_json::json!({
                "type": "system",
                "subtype": "session_state_changed",
                "state": "idle",
            })))
            .await
            .expect("state frame should be accepted");
        assert_eq!(
            activity_rx.borrow_and_update().session_state,
            Some(SdkSessionState::Idle)
        );
    }

    #[test]
    fn the_task_tracking_mode_cell_defaults_to_raw_until_negotiated() {
        let handler = AcpNotificationHandler::new(
            Arc::new(BasicMessageWriter::new()),
            false,
            vec![],
            CancellationToken::new(),
        );
        // Raw until initialize resolves — and initialize precedes session/new,
        // so no session/update is ever dispatched against the default.
        assert_eq!(handler.task_tracking_mode(), TaskTrackingMode::Raw);
        handler.set_task_tracking_mode(TaskTrackingMode::Typed);
        assert_eq!(handler.task_tracking_mode(), TaskTrackingMode::Typed);
    }

    /// Feed the handler one typed async-task update through the real parse
    /// path — the same [`IncomingSessionUpdate`] wrapper the connection's
    /// dispatch uses.
    async fn feed_async_task_update(handler: &AcpNotificationHandler, update: serde_json::Value) {
        let params = serde_json::json!({ "sessionId": "sess-1", "update": update });
        match IncomingSessionUpdate::parse_message("session/update", &params)
            .expect("async task update should parse")
        {
            IncomingSessionUpdate::AsyncTask(notification) => handler
                .async_task_update(notification)
                .await
                .expect("async task update should be accepted"),
            IncomingSessionUpdate::Standard(other) => {
                panic!("expected the async-task arm, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn typed_updates_publish_the_mode_selected_task_count() {
        let handler = AcpNotificationHandler::new(
            Arc::new(BasicMessageWriter::new()),
            false,
            vec![],
            CancellationToken::new(),
        );
        handler.set_task_tracking_mode(TaskTrackingMode::Typed);
        let mut activity_rx = handler.subscribe_background_activity();

        feed_async_task_update(
            &handler,
            serde_json::json!({
                "sessionUpdate": "async_task_spawned",
                "asyncTaskId": "task-1",
                "taskType": "local_agent",
                "showInTranscript": false,
                "canStop": true,
            }),
        )
        .await;
        let after_spawn = activity_rx.borrow_and_update().clone();
        assert_eq!(after_spawn.live_tasks, 1);
        assert!(after_spawn.ever_started_task);
        assert!(
            !after_spawn.sdk_frames_seen,
            "typed updates are not raw-stream proof — typed quiescence never consults it"
        );

        // The raw stream still feeds the idle latch in typed mode, but its
        // own (empty) task set must not clobber the typed count.
        let params = serde_json::value::to_raw_value(&serde_json::json!({
            "sessionId": "sess-1",
            "message": {
                "type": "system",
                "subtype": "session_state_changed",
                "state": "idle",
            },
        }))
        .expect("params should serialize");
        handler
            .ext_notification(ExtNotification::new(
                CLAUDE_SDK_MESSAGE_METHOD,
                params.into(),
            ))
            .await
            .expect("state frame should be accepted");
        let after_raw = activity_rx.borrow_and_update().clone();
        assert_eq!(
            after_raw.live_tasks, 1,
            "a raw frame must not clobber the typed count"
        );
        assert_eq!(after_raw.session_state, Some(SdkSessionState::Idle));

        // The terminal edge drains the published count.
        feed_async_task_update(
            &handler,
            serde_json::json!({
                "sessionUpdate": "async_task_state_update",
                "asyncTaskId": "task-1",
                "state": "completed",
            }),
        )
        .await;
        let after_done = activity_rx.borrow_and_update().clone();
        assert_eq!(after_done.live_tasks, 0);
        assert!(after_done.ever_started_task);
        assert!(
            after_done.activity_seq > after_spawn.activity_seq,
            "every typed update must bump the sequence so the debounce clock resets"
        );
    }

    #[tokio::test]
    async fn typed_updates_publish_named_rows_and_raw_mode_keeps_the_bare_count() {
        let handler = AcpNotificationHandler::new(
            Arc::new(BasicMessageWriter::new()),
            false,
            vec![],
            CancellationToken::new(),
        );
        handler.set_task_tracking_mode(TaskTrackingMode::Typed);
        let mut activity_rx = handler.subscribe_background_activity();

        feed_async_task_update(
            &handler,
            serde_json::json!({
                "sessionUpdate": "async_task_spawned",
                "asyncTaskId": "task-1",
                "name": "Run the tests",
                "description": "cargo test in the background",
                "outputFilePath": "/tmp/tests.log",
            }),
        )
        .await;
        assert_eq!(
            activity_rx.borrow_and_update().tasks,
            [BackgroundHoldTask {
                id: "task-1".to_string(),
                name: Some("Run the tests".to_string()),
                description: Some("cargo test in the background".to_string()),
                output_file_path: Some("/tmp/tests.log".to_string()),
            }]
        );

        feed_async_task_update(
            &handler,
            serde_json::json!({
                "sessionUpdate": "async_task_state_update",
                "asyncTaskId": "task-1",
                "state": "stopped",
            }),
        )
        .await;
        assert!(activity_rx.borrow_and_update().tasks.is_empty());

        // Raw mode only ever knows opaque task ids: the count is published,
        // the named rows stay empty.
        let raw = hold_test_handler();
        let mut raw_activity_rx = raw.subscribe_background_activity();
        feed_sdk_frame(&raw, task_started("task-1")).await;
        let activity = raw_activity_rx.borrow_and_update().clone();
        assert_eq!(activity.live_tasks, 1);
        assert!(activity.tasks.is_empty());
    }

    #[test]
    fn a_task_notification_alone_counts_as_task_history() {
        // A `task_notification` whose start frame was missed leaves the set
        // empty but still proves a task existed — the taskless fast path must
        // not fire on the very cycle a settled task is about to wake.
        assert!(sdk_message_mentions_task(&serde_json::json!({
            "type": "system",
            "subtype": "task_notification",
            "task_id": "task-1",
        })));
        // The probe and an empty authoritative snapshot mention no task.
        assert!(!sdk_message_mentions_task(&serde_json::json!({
            "type": "system",
            "subtype": "init",
        })));
        assert!(!sdk_message_mentions_task(&serde_json::json!({
            "type": "system",
            "subtype": "background_tasks_changed",
            "tasks": [],
        })));
        // Non-system frames say nothing about tasks, whatever their shape.
        assert!(!sdk_message_mentions_task(&serde_json::json!({
            "type": "assistant",
            "subtype": "task_notification",
            "task_id": "task-1",
        })));
    }

    fn test_hold_config() -> BackgroundHoldConfig {
        BackgroundHoldConfig {
            hold_cap: Duration::from_secs(600),
            debounce: Duration::from_secs(10),
            taskless_debounce: Duration::from_secs(1),
            idle_latch_staleness: Duration::from_secs(120),
            out_of_turn_permissions: OutOfTurnPermissionPolicy::Prompt,
        }
    }

    fn seen_with(live_tasks: usize) -> BackgroundActivity {
        BackgroundActivity {
            live_tasks,
            sdk_frames_seen: true,
            ever_started_task: live_tasks > 0,
            activity_seq: 1,
            ..BackgroundActivity::default()
        }
    }

    /// A connection whose task set drained: frames seen, a task was live at
    /// some point, none is now.
    fn drained() -> BackgroundActivity {
        BackgroundActivity {
            live_tasks: 0,
            sdk_frames_seen: true,
            ever_started_task: true,
            activity_seq: 1,
            ..BackgroundActivity::default()
        }
    }

    /// [`drained`], with the idle latch reading `state`.
    fn drained_in_state(state: SdkSessionState) -> BackgroundActivity {
        BackgroundActivity {
            session_state: Some(state),
            ..drained()
        }
    }

    #[test]
    fn hold_declares_quiescence_once_the_drained_set_survives_the_debounce() {
        let start = tokio::time::Instant::now();
        let state = HoldingState::new(test_hold_config(), TaskTrackingMode::Raw, start, &drained());

        // The empty set alone is not enough: the debounce must elapse too.
        assert_eq!(state.poll_settle(start), None);
        assert_eq!(state.poll_settle(start + Duration::from_secs(9)), None);
        assert_eq!(
            state.poll_settle(start + Duration::from_secs(10)),
            Some(HoldSettle::Quiescent)
        );
        // Waking at the debounce is cheaper than polling: the next deadline is
        // the debounce, not the far-off cap.
        assert_eq!(state.next_deadline(), start + Duration::from_secs(10));
    }

    #[test]
    fn a_turn_that_never_started_a_task_confirms_with_the_short_debounce() {
        let start = tokio::time::Instant::now();
        // Frames proven (the init probe arrived), but nothing ever mentioned
        // a task: there is nothing to drain, so quiescence needs only the
        // short taskless confirmation instead of the full 10s debounce.
        let mut state = HoldingState::new(
            test_hold_config(),
            TaskTrackingMode::Raw,
            start,
            &seen_with(0),
        );

        assert_eq!(state.poll_settle(start), None);
        assert_eq!(state.next_deadline(), start + Duration::from_secs(1));
        assert_eq!(
            state.poll_settle(start + Duration::from_secs(1)),
            Some(HoldSettle::Quiescent)
        );

        // The first task ends the fast path: the hold re-arms while the task
        // lives, and the eventual drain confirms with the full debounce.
        let task = start + Duration::from_millis(500);
        state.observe(task, &seen_with(1));
        assert_eq!(state.quiescence_deadline(), None);
        let drain = start + Duration::from_secs(2);
        state.observe(drain, &seen_with(0));
        assert_eq!(state.poll_settle(drain + Duration::from_secs(9)), None);
        assert_eq!(
            state.poll_settle(drain + Duration::from_secs(10)),
            Some(HoldSettle::Quiescent)
        );
    }

    #[test]
    fn hold_waits_for_the_cap_while_a_task_is_live() {
        let start = tokio::time::Instant::now();
        let state = HoldingState::new(
            test_hold_config(),
            TaskTrackingMode::Raw,
            start,
            &seen_with(1),
        );

        assert_eq!(state.quiescence_deadline(), None);
        assert_eq!(state.next_deadline(), start + Duration::from_secs(600));
        assert_eq!(state.poll_settle(start + Duration::from_secs(300)), None);
        assert_eq!(
            state.poll_settle(start + Duration::from_secs(600)),
            Some(HoldSettle::HeldUntilCap)
        );
    }

    #[test]
    fn the_idle_latch_blocks_quiescence_while_busy() {
        let start = tokio::time::Instant::now();
        // Set drained, but the SDK says a cycle is running — exactly the
        // continuation window after `task_notification` empties the set.
        let mut state = HoldingState::new(
            test_hold_config(),
            TaskTrackingMode::Raw,
            start,
            &drained_in_state(SdkSessionState::Busy),
        );

        // Busy stretches the quiet window from the 10s debounce to the 120s
        // staleness bound — it does not withhold the deadline outright.
        assert_eq!(
            state.quiescence_deadline(),
            Some(start + Duration::from_secs(120))
        );
        assert_eq!(state.next_deadline(), start + Duration::from_secs(120));
        assert_eq!(state.poll_settle(start + Duration::from_secs(60)), None);

        // The trailing idle releases it: quiescence confirms one debounce
        // after the release, not retroactively.
        let idle = start + Duration::from_secs(90);
        state.observe(idle, &drained_in_state(SdkSessionState::Idle));
        assert_eq!(state.poll_settle(idle + Duration::from_secs(9)), None);
        assert_eq!(
            state.poll_settle(idle + Duration::from_secs(10)),
            Some(HoldSettle::Quiescent)
        );
    }

    #[test]
    fn a_busy_latch_never_released_goes_stale_after_the_bound() {
        let start = tokio::time::Instant::now();
        // The poisoned-connection case: a busy frame's trailing idle was
        // lost, so the latch reads busy at hold entry and nothing will ever
        // release it. Without a staleness bound this hold — and every later
        // hold on the connection — would run to the 10-minute cap.
        let mut state = HoldingState::new(
            test_hold_config(),
            TaskTrackingMode::Raw,
            start,
            &drained_in_state(SdkSessionState::Busy),
        );

        // With no activity at all for the full bound, the busy reading is
        // stale and quiescence proceeds — well before the cap.
        assert_eq!(state.poll_settle(start + Duration::from_secs(119)), None);
        assert_eq!(
            state.poll_settle(start + Duration::from_secs(120)),
            Some(HoldSettle::Quiescent)
        );

        // Any traffic while busy re-arms the full staleness window, not just
        // the debounce: a live continuation keeps pushing the window with
        // its own frames, which is what makes going stale safe.
        let bump = start + Duration::from_secs(60);
        state.observe(bump, &drained_in_state(SdkSessionState::Busy));
        assert_eq!(state.poll_settle(bump + Duration::from_secs(119)), None);
        assert_eq!(
            state.poll_settle(bump + Duration::from_secs(120)),
            Some(HoldSettle::Quiescent)
        );

        // The bound never shortens the active debounce: with a staleness
        // window inside the drain debounce, the debounce still governs.
        let short_staleness = BackgroundHoldConfig {
            idle_latch_staleness: Duration::from_secs(2),
            ..test_hold_config()
        };
        let state = HoldingState::new(
            short_staleness,
            TaskTrackingMode::Raw,
            start,
            &drained_in_state(SdkSessionState::Busy),
        );
        assert_eq!(state.poll_settle(start + Duration::from_secs(9)), None);
        assert_eq!(
            state.poll_settle(start + Duration::from_secs(10)),
            Some(HoldSettle::Quiescent)
        );
    }

    #[test]
    fn a_missing_idle_latch_is_neutral() {
        let start = tokio::time::Instant::now();
        // A bridge that never emits `session_state_changed` leaves the latch
        // `None`: the hold must behave exactly as it did without the latch,
        // or every turn on such a bridge would run to the cap.
        let state = HoldingState::new(test_hold_config(), TaskTrackingMode::Raw, start, &drained());
        assert_eq!(
            state.poll_settle(start + Duration::from_secs(10)),
            Some(HoldSettle::Quiescent)
        );
    }

    #[test]
    fn session_state_frames_parse_idle_and_fold_the_rest_to_busy() {
        let state_frame = |state: &str| {
            serde_json::json!({
                "type": "system",
                "subtype": "session_state_changed",
                "state": state,
            })
        };
        assert_eq!(
            sdk_message_session_state(&state_frame("idle")),
            Some(SdkSessionState::Idle)
        );
        // `running`, `requires_action`, and anything the SDK adds later all
        // read busy — pessimistic, cleared by the next idle, capped anyway.
        for busy in ["running", "requires_action", "some_future_state"] {
            assert_eq!(
                sdk_message_session_state(&state_frame(busy)),
                Some(SdkSessionState::Busy),
                "state {busy:?} should read busy"
            );
        }
        // Every other frame says nothing about the latch, including a
        // state-less or non-system frame with the right subtype.
        for frame in [
            serde_json::json!({ "type": "system", "subtype": "session_state_changed" }),
            serde_json::json!({ "type": "assistant", "subtype": "session_state_changed", "state": "idle" }),
            serde_json::json!({ "type": "system", "subtype": "init" }),
        ] {
            assert_eq!(sdk_message_session_state(&frame), None, "frame: {frame}");
        }
    }

    #[test]
    fn activity_resets_the_debounce_clock() {
        let start = tokio::time::Instant::now();
        let mut state =
            HoldingState::new(test_hold_config(), TaskTrackingMode::Raw, start, &drained());

        // A frame at t+9s (set still empty) pushes quiescence out to t+19s.
        let bump = start + Duration::from_secs(9);
        state.observe(bump, &drained());
        assert_eq!(state.poll_settle(start + Duration::from_secs(10)), None);
        assert_eq!(
            state.poll_settle(start + Duration::from_secs(19)),
            Some(HoldSettle::Quiescent)
        );
        // The cap is absolute: `observe` never extends it.
        assert_eq!(state.cap_deadline, start + Duration::from_secs(600));
    }

    #[test]
    fn a_restarted_task_re_arms_the_hold() {
        let start = tokio::time::Instant::now();
        let mut state = HoldingState::new(
            test_hold_config(),
            TaskTrackingMode::Raw,
            start,
            &seen_with(0),
        );
        assert!(state.quiescence_deadline().is_some());

        // A fresh task_started after we were about to declare quiescence
        // disarms it entirely — not just delays it.
        let restart = start + Duration::from_secs(9);
        state.observe(restart, &seen_with(1));
        assert_eq!(state.quiescence_deadline(), None);
        assert_eq!(state.poll_settle(start + Duration::from_secs(30)), None);

        // Draining again re-arms the debounce from the drain, not the restart.
        let drain = start + Duration::from_secs(20);
        state.observe(drain, &seen_with(0));
        assert_eq!(state.poll_settle(start + Duration::from_secs(29)), None);
        assert_eq!(
            state.poll_settle(drain + Duration::from_secs(10)),
            Some(HoldSettle::Quiescent)
        );
    }

    #[test]
    fn without_raw_sdk_frames_the_hold_runs_to_the_cap() {
        let start = tokio::time::Instant::now();
        // No frame has ever arrived, so an empty task set is uninformative:
        // an older bridge (or a rejected filter) is indistinguishable from a
        // session that started no background work. Never trust the debounce
        // alone — hold to the cap.
        let mut state = HoldingState::new(
            test_hold_config(),
            TaskTrackingMode::Raw,
            start,
            &BackgroundActivity::default(),
        );

        assert_eq!(state.quiescence_deadline(), None);
        assert_eq!(state.poll_settle(start + Duration::from_secs(10)), None);
        assert_eq!(
            state.poll_settle(start + Duration::from_secs(600)),
            Some(HoldSettle::HeldUntilCap)
        );

        // A `session/update` with no raw frame behind it resets the debounce
        // but still cannot unlock quiescence.
        state.observe(
            start + Duration::from_secs(5),
            &BackgroundActivity::default(),
        );
        assert_eq!(state.quiescence_deadline(), None);

        // The first raw frame proves the stream and unlocks it — and since
        // nothing ever mentioned a task, the short confirmation suffices.
        let proven = start + Duration::from_secs(30);
        state.observe(proven, &seen_with(0));
        assert_eq!(state.poll_settle(proven), None);
        assert_eq!(
            state.poll_settle(proven + Duration::from_secs(1)),
            Some(HoldSettle::Quiescent)
        );
    }

    #[test]
    fn raw_sdk_availability_latches_once_proven() {
        let start = tokio::time::Instant::now();
        let mut state = HoldingState::new(
            test_hold_config(),
            TaskTrackingMode::Raw,
            start,
            &seen_with(0),
        );

        // A later snapshot that happens to report no frames must not un-prove
        // a stream that already delivered one.
        state.observe(
            start + Duration::from_secs(1),
            &BackgroundActivity {
                activity_seq: 7,
                ..BackgroundActivity::default()
            },
        );
        assert!(state.sdk_frames_seen);
        assert!(state.quiescence_deadline().is_some());
    }

    #[test]
    fn quiescence_wins_when_both_deadlines_have_passed() {
        let start = tokio::time::Instant::now();
        let config = BackgroundHoldConfig {
            hold_cap: Duration::from_secs(10),
            debounce: Duration::from_secs(10),
            taskless_debounce: Duration::from_secs(1),
            idle_latch_staleness: Duration::from_secs(120),
            out_of_turn_permissions: OutOfTurnPermissionPolicy::Prompt,
        };
        let state = HoldingState::new(config, TaskTrackingMode::Raw, start, &drained());

        // A confirmed-empty set is the more informative outcome, so it should
        // not be reported as a truncated wait.
        assert_eq!(
            state.poll_settle(start + Duration::from_secs(10)),
            Some(HoldSettle::Quiescent)
        );
    }

    #[test]
    fn typed_mode_declares_quiescence_without_raw_stream_proof() {
        let start = tokio::time::Instant::now();
        // No raw SDK frame ever arrives — a non-Claude asyncTasks agent has
        // no raw stream at all. The mirrored capability replaces the stream
        // proof, so the empty typed set confirms with the short debounce
        // instead of raw mode's hold-to-cap fallback.
        let state = HoldingState::new(
            test_hold_config(),
            TaskTrackingMode::Typed,
            start,
            &BackgroundActivity::default(),
        );

        assert_eq!(state.next_deadline(), start + Duration::from_secs(1));
        assert_eq!(state.poll_settle(start), None);
        assert_eq!(
            state.poll_settle(start + Duration::from_secs(1)),
            Some(HoldSettle::Quiescent)
        );
    }

    #[test]
    fn typed_mode_keeps_the_short_debounce_even_after_a_drain() {
        let start = tokio::time::Instant::now();
        let typed_activity = |live_tasks: usize| BackgroundActivity {
            live_tasks,
            ever_started_task: true,
            activity_seq: 1,
            ..BackgroundActivity::default()
        };
        // A live typed task holds, exactly like a raw one.
        let mut state = HoldingState::new(
            test_hold_config(),
            TaskTrackingMode::Typed,
            start,
            &typed_activity(1),
        );
        assert_eq!(state.quiescence_deadline(), None);

        // The terminal edge is authoritative — there is no in-flight re-spawn
        // frame to absorb, so the drain confirms with the short debounce, not
        // raw mode's full 10s drain debounce.
        let drain = start + Duration::from_secs(5);
        state.observe(drain, &typed_activity(0));
        assert_eq!(state.poll_settle(drain + Duration::from_millis(999)), None);
        assert_eq!(
            state.poll_settle(drain + Duration::from_secs(1)),
            Some(HoldSettle::Quiescent)
        );
    }

    #[test]
    fn typed_mode_still_defers_to_the_idle_latch() {
        let start = tokio::time::Instant::now();
        // The typed set is empty, but the SDK says a cycle is running — the
        // continuation window the latch exists for is mode-independent.
        let mut state = HoldingState::new(
            test_hold_config(),
            TaskTrackingMode::Typed,
            start,
            &drained_in_state(SdkSessionState::Busy),
        );

        // Busy stretches typed mode's short debounce to the staleness bound
        // just as it does raw mode's drain debounce.
        assert_eq!(
            state.quiescence_deadline(),
            Some(start + Duration::from_secs(120))
        );
        assert_eq!(state.poll_settle(start + Duration::from_secs(60)), None);

        // The trailing idle releases it, confirmed one short debounce later.
        let idle = start + Duration::from_secs(90);
        state.observe(idle, &drained_in_state(SdkSessionState::Idle));
        assert_eq!(state.poll_settle(idle + Duration::from_millis(999)), None);
        assert_eq!(
            state.poll_settle(idle + Duration::from_secs(1)),
            Some(HoldSettle::Quiescent)
        );
    }

    #[test]
    fn lifetime_transition_table_matches_the_state_machine() {
        use SessionLifetime::*;

        // A resolved prompt holds; teardown, cancel, and failure are the ways
        // out of a live turn.
        assert!(TurnLive.can_transition_to(BackgroundHolding));
        assert!(TurnLive.can_transition_to(TornDown));
        assert!(TurnLive.can_transition_to(Cancelled));
        assert!(TurnLive.can_transition_to(Failed));
        assert!(!TurnLive.can_transition_to(Quiescent));

        // Holding can go back to a live turn (a new prompt arrived) or settle.
        assert!(BackgroundHolding.can_transition_to(TurnLive));
        assert!(BackgroundHolding.can_transition_to(Quiescent));
        assert!(BackgroundHolding.can_transition_to(TornDown));
        assert!(BackgroundHolding.can_transition_to(Cancelled));
        assert!(BackgroundHolding.can_transition_to(Failed));

        // Quiescent tears down, unless a task_started re-arms the hold.
        assert!(Quiescent.can_transition_to(TornDown));
        assert!(Quiescent.can_transition_to(BackgroundHolding));
        assert!(!Quiescent.can_transition_to(TurnLive));

        for terminal in [TornDown, Cancelled, Failed] {
            for next in [
                TurnLive,
                BackgroundHolding,
                Quiescent,
                TornDown,
                Cancelled,
                Failed,
            ] {
                assert!(
                    !terminal.can_transition_to(next),
                    "{terminal:?} is terminal but allowed {next:?}"
                );
            }
        }
    }

    #[test]
    fn settled_outcome_overrides_a_completed_turn_but_never_a_failed_one() {
        let settled =
            |outcome: Result<AgentRunOutcome, String>, reason| SessionSettled { outcome, reason };

        // A hold that was cancelled or failed after the turn completed is the
        // session's real outcome — the completed turn does not mask it.
        assert_eq!(
            settled(
                Ok(AgentRunOutcome::Cancelled),
                SessionSettleReason::Cancelled
            )
            .fold_turn_result(Ok(AgentRunOutcome::Completed)),
            Ok(AgentRunOutcome::Cancelled)
        );
        assert_eq!(
            settled(Err("child exited".into()), SessionSettleReason::Failed)
                .fold_turn_result(Ok(AgentRunOutcome::Completed)),
            Err("child exited".into())
        );

        // A clean settle keeps the turn's own outcome.
        assert_eq!(
            settled(
                Ok(AgentRunOutcome::Completed),
                SessionSettleReason::Quiescent
            )
            .fold_turn_result(Ok(AgentRunOutcome::Completed)),
            Ok(AgentRunOutcome::Completed)
        );
        // A truncated wait still reports the turn as completed: the hooks run
        // best-effort, the turn itself did finish. This covers the cap
        // expiring, a stop during the hold, and the child dying under it.
        for reason in [
            SessionSettleReason::HeldUntilCap,
            SessionSettleReason::HoldStopped,
        ] {
            assert_eq!(
                settled(Ok(AgentRunOutcome::Completed), reason)
                    .fold_turn_result(Ok(AgentRunOutcome::Completed)),
                Ok(AgentRunOutcome::Completed),
                "{reason:?} must not erase a completed turn"
            );
        }

        // The turn's own failure or cancellation is never upgraded by a clean
        // settle.
        assert_eq!(
            settled(
                Ok(AgentRunOutcome::Completed),
                SessionSettleReason::Quiescent
            )
            .fold_turn_result(Err("prompt failed".into())),
            Err("prompt failed".into())
        );
        assert_eq!(
            settled(
                Ok(AgentRunOutcome::Completed),
                SessionSettleReason::Quiescent
            )
            .fold_turn_result(Ok(AgentRunOutcome::Cancelled)),
            Ok(AgentRunOutcome::Cancelled)
        );
    }

    /// Handler wired for holding-wait tests, plus a helper that feeds it raw
    /// SDK frames the way the bridge would.
    fn hold_test_handler() -> Arc<AcpNotificationHandler> {
        Arc::new(AcpNotificationHandler::new(
            Arc::new(BasicMessageWriter::new()),
            false,
            vec![],
            CancellationToken::new(),
        ))
    }

    async fn feed_sdk_frame(handler: &Arc<AcpNotificationHandler>, message: serde_json::Value) {
        let params = serde_json::value::to_raw_value(&serde_json::json!({
            "sessionId": "sess-1",
            "message": message,
        }))
        .expect("params should serialize");
        handler
            .ext_notification(ExtNotification::new(
                CLAUDE_SDK_MESSAGE_METHOD,
                params.into(),
            ))
            .await
            .expect("sdkMessage frame should be accepted");
    }

    fn empty_tasks_frame() -> serde_json::Value {
        serde_json::json!({
            "type": "system",
            "subtype": "background_tasks_changed",
            "tasks": [],
        })
    }

    /// Cap far enough out that only quiescence can end the hold, debounce
    /// short enough to observe. `HoldingState`'s own tests pin the exact
    /// deadline arithmetic; these loop tests only assert which branch wins, so
    /// they stay robust with real (short) durations.
    const QUIESCENCE_PROBE: BackgroundHoldConfig = BackgroundHoldConfig {
        hold_cap: Duration::from_secs(120),
        debounce: Duration::from_millis(50),
        taskless_debounce: Duration::from_millis(50),
        idle_latch_staleness: Duration::from_secs(60),
        out_of_turn_permissions: OutOfTurnPermissionPolicy::Prompt,
    };
    /// Long enough that a slow machine can't mistake scheduling delay for a
    /// missing settle, short enough not to slow the suite down.
    const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
    /// A window many debounces wide: if the hold were going to end early it
    /// would have by now.
    const HELD_OPEN_PROBE: Duration = Duration::from_millis(500);

    #[tokio::test]
    async fn holding_wait_settles_quiescent_after_tasks_drain() {
        let handler = hold_test_handler();
        let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let child_exited = CancellationToken::new();
        let hold_active = AtomicBool::new(false);

        feed_sdk_frame(&handler, task_started("task-1")).await;

        let mut hold = std::pin::pin!(hold_for_background_quiescence(
            &QUIESCENCE_PROBE,
            TaskTrackingMode::Raw,
            &handler,
            &cancel,
            &child_exited,
            &mut prompt_rx,
            &mut hold_control_rx,
            &hold_active,
            &|_| {},
            "sess-1",
            None,
        ));

        // The live task keeps the hold open however long the debounce is.
        tokio::select! {
            _ = &mut hold => panic!("must not settle while a task is live"),
            _ = tokio::time::sleep(HELD_OPEN_PROBE) => {}
        }

        feed_sdk_frame(&handler, empty_tasks_frame()).await;
        let outcome = tokio::time::timeout(SETTLE_TIMEOUT, hold)
            .await
            .expect("drained set plus debounce should settle before the cap");
        assert!(matches!(outcome, HoldOutcome::Quiescent));
    }

    #[tokio::test]
    async fn holding_wait_does_not_trust_debounce_without_raw_sdk_frames() {
        let handler = hold_test_handler();
        let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let child_exited = CancellationToken::new();
        let hold_active = AtomicBool::new(false);

        // No `_claude/sdkMessage` frame ever arrives (older bridge, or the
        // filter was rejected), so the empty task set proves nothing and the
        // debounce must not buy an early teardown.
        let mut hold = std::pin::pin!(hold_for_background_quiescence(
            &QUIESCENCE_PROBE,
            TaskTrackingMode::Raw,
            &handler,
            &cancel,
            &child_exited,
            &mut prompt_rx,
            &mut hold_control_rx,
            &hold_active,
            &|_| {},
            "sess-1",
            None,
        ));
        tokio::select! {
            _ = &mut hold => panic!("debounce alone must not end the hold"),
            _ = tokio::time::sleep(HELD_OPEN_PROBE) => {}
        }
    }

    #[tokio::test]
    async fn holding_wait_ends_at_the_cap_with_a_task_still_live() {
        let handler = hold_test_handler();
        let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let child_exited = CancellationToken::new();
        let hold_active = AtomicBool::new(false);

        // A task that never reports a terminal status — the "shell hangs or
        // gets left behind" case the cap exists for.
        feed_sdk_frame(&handler, task_started("task-1")).await;

        let config = BackgroundHoldConfig {
            hold_cap: Duration::from_millis(100),
            debounce: Duration::from_secs(60),
            taskless_debounce: Duration::from_secs(60),
            idle_latch_staleness: Duration::from_secs(60),
            out_of_turn_permissions: OutOfTurnPermissionPolicy::Prompt,
        };
        let outcome = tokio::time::timeout(
            SETTLE_TIMEOUT,
            hold_for_background_quiescence(
                &config,
                TaskTrackingMode::Raw,
                &handler,
                &cancel,
                &child_exited,
                &mut prompt_rx,
                &mut hold_control_rx,
                &hold_active,
                &|_| {},
                "sess-1",
                None,
            ),
        )
        .await
        .expect("the cap must bound every hold");
        assert!(matches!(outcome, HoldOutcome::HeldUntilCap));
    }

    #[tokio::test]
    async fn holding_wait_yields_to_a_new_prompt() {
        let handler = hold_test_handler();
        let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let child_exited = CancellationToken::new();
        let hold_active = AtomicBool::new(false);

        let (reply_tx, _reply_rx) = oneshot::channel();
        prompt_tx
            .send(QueuedSessionTurn {
                prompt: String::from("follow-up"),
                images: vec![],
                reply: reply_tx,
            })
            .expect("queued turn should send");

        let outcome = tokio::time::timeout(
            SETTLE_TIMEOUT,
            hold_for_background_quiescence(
                &QUIESCENCE_PROBE,
                TaskTrackingMode::Raw,
                &handler,
                &cancel,
                &child_exited,
                &mut prompt_rx,
                &mut hold_control_rx,
                &hold_active,
                &|_| {},
                "sess-1",
                None,
            ),
        )
        .await
        .expect("a queued prompt should end the hold immediately");
        match outcome {
            HoldOutcome::NewTurn(turn) => assert_eq!(turn.prompt, "follow-up"),
            other => panic!("a queued prompt must re-enter a live turn, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn holding_wait_reports_cancel_and_child_exit() {
        for (label, trip_cancel) in [("cancel", true), ("child exit", false)] {
            let handler = hold_test_handler();
            let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
            let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
            let cancel = CancellationToken::new();
            let child_exited = CancellationToken::new();
            let hold_active = AtomicBool::new(false);
            if trip_cancel {
                cancel.cancel();
            } else {
                child_exited.cancel();
            }

            let outcome = tokio::time::timeout(
                SETTLE_TIMEOUT,
                hold_for_background_quiescence(
                    &QUIESCENCE_PROBE,
                    TaskTrackingMode::Raw,
                    &handler,
                    &cancel,
                    &child_exited,
                    &mut prompt_rx,
                    &mut hold_control_rx,
                    &hold_active,
                    &|_| {},
                    "sess-1",
                    None,
                ),
            )
            .await
            .unwrap_or_else(|_| panic!("{label} should end the hold promptly"));
            match (trip_cancel, outcome) {
                (true, HoldOutcome::Cancelled) | (false, HoldOutcome::ChildExited) => {}
                (_, other) => panic!("{label} should end the hold, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn holding_wait_keeps_draining_after_the_last_handle_drops() {
        let handler = hold_test_handler();
        let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel::<QueuedSessionTurn>();
        let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let child_exited = CancellationToken::new();
        let hold_active = AtomicBool::new(false);

        feed_sdk_frame(&handler, task_started("task-1")).await;
        // No caller can send another prompt, but the live background task is
        // still worth draining before teardown.
        drop(prompt_tx);

        let mut hold = std::pin::pin!(hold_for_background_quiescence(
            &QUIESCENCE_PROBE,
            TaskTrackingMode::Raw,
            &handler,
            &cancel,
            &child_exited,
            &mut prompt_rx,
            &mut hold_control_rx,
            &hold_active,
            &|_| {},
            "sess-1",
            None,
        ));
        tokio::select! {
            _ = &mut hold => panic!("a closed prompt channel must not end the hold early"),
            _ = tokio::time::sleep(HELD_OPEN_PROBE) => {}
        }

        feed_sdk_frame(&handler, empty_tasks_frame()).await;
        let outcome = tokio::time::timeout(SETTLE_TIMEOUT, hold)
            .await
            .expect("the drained set should still settle quiescent");
        assert!(matches!(outcome, HoldOutcome::Quiescent));
    }

    /// Collects everything a [`BackgroundHoldObserver`] is told, so tests can
    /// assert what a client would have rendered.
    #[allow(clippy::type_complexity)]
    fn recording_hold_observer() -> (
        BackgroundHoldObserver,
        Arc<std::sync::Mutex<Vec<BackgroundHoldStatus>>>,
    ) {
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&recorded);
        // The observer is called synchronously from the connection task, so a
        // std mutex (never held across an await) is the right sink here.
        let observer: BackgroundHoldObserver = Arc::new(move |status| {
            sink.lock()
                .expect("recorder should not be poisoned")
                .push(status);
        });
        (observer, recorded)
    }

    #[test]
    fn only_background_holding_has_a_presentational_sub_state() {
        let task = BackgroundHoldTask {
            id: "task-1".to_string(),
            name: Some("Run the tests".to_string()),
            description: None,
            output_file_path: None,
        };
        assert_eq!(
            BackgroundHoldStatus::for_lifetime(
                SessionLifetime::BackgroundHolding,
                2,
                vec![task.clone()],
            ),
            BackgroundHoldStatus {
                holding: true,
                live_tasks: 2,
                tasks: vec![task.clone()],
            }
        );
        // Every other state is covered by the session's own status —
        // TurnLive is running, Quiescent/TornDown completed, Cancelled
        // cancelled, Failed an error — so none of them shows the wait.
        for lifetime in [
            SessionLifetime::TurnLive,
            SessionLifetime::Quiescent,
            SessionLifetime::TornDown,
            SessionLifetime::Cancelled,
            SessionLifetime::Failed,
        ] {
            assert_eq!(
                BackgroundHoldStatus::for_lifetime(lifetime, 2, vec![task.clone()]),
                BackgroundHoldStatus::default(),
                "{lifetime:?} must not present the background wait"
            );
        }
    }

    #[tokio::test]
    async fn holding_wait_reports_the_live_task_count_then_clears() {
        let handler = hold_test_handler();
        let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let child_exited = CancellationToken::new();
        let hold_active = AtomicBool::new(false);
        let (observer, recorded) = recording_hold_observer();

        feed_sdk_frame(&handler, task_started("task-1")).await;

        let mut hold = std::pin::pin!(hold_for_background_quiescence(
            &QUIESCENCE_PROBE,
            TaskTrackingMode::Raw,
            &handler,
            &cancel,
            &child_exited,
            &mut prompt_rx,
            &mut hold_control_rx,
            &hold_active,
            &|_| {},
            "sess-1",
            Some(&observer),
        ));
        tokio::select! {
            _ = &mut hold => panic!("must not settle while a task is live"),
            _ = tokio::time::sleep(HELD_OPEN_PROBE) => {}
        }
        assert_eq!(
            recorded.lock().expect("recorder").as_slice(),
            [BackgroundHoldStatus {
                holding: true,
                live_tasks: 1,
                tasks: vec![],
            }],
            "entering the hold should report the wait and its task count"
        );

        // A second task starting is a fresh count for the client to show. The
        // activity signal is a `watch`, so let the hold observe this snapshot
        // before the next one replaces it.
        feed_sdk_frame(&handler, task_started("task-2")).await;
        tokio::select! {
            _ = &mut hold => panic!("must not settle while tasks are live"),
            _ = tokio::time::sleep(HELD_OPEN_PROBE) => {}
        }

        feed_sdk_frame(&handler, empty_tasks_frame()).await;
        tokio::time::timeout(SETTLE_TIMEOUT, hold)
            .await
            .expect("drained set plus debounce should settle before the cap");

        assert_eq!(
            recorded.lock().expect("recorder").as_slice(),
            [
                BackgroundHoldStatus {
                    holding: true,
                    live_tasks: 1,
                    tasks: vec![],
                },
                BackgroundHoldStatus {
                    holding: true,
                    live_tasks: 2,
                    tasks: vec![],
                },
                // The drained set is still the hold (the debounce has to pass),
                // then teardown clears the wait entirely.
                BackgroundHoldStatus {
                    holding: true,
                    live_tasks: 0,
                    tasks: vec![],
                },
                BackgroundHoldStatus::default(),
            ]
        );
    }

    #[tokio::test]
    async fn holding_wait_clears_its_report_when_a_new_prompt_takes_over() {
        let handler = hold_test_handler();
        let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let child_exited = CancellationToken::new();
        let hold_active = AtomicBool::new(false);
        let (observer, recorded) = recording_hold_observer();

        feed_sdk_frame(&handler, task_started("task-1")).await;
        let (reply_tx, _reply_rx) = oneshot::channel();
        prompt_tx
            .send(QueuedSessionTurn {
                prompt: String::from("follow-up"),
                images: vec![],
                reply: reply_tx,
            })
            .expect("queued turn should send");

        let outcome = tokio::time::timeout(
            SETTLE_TIMEOUT,
            hold_for_background_quiescence(
                &QUIESCENCE_PROBE,
                TaskTrackingMode::Raw,
                &handler,
                &cancel,
                &child_exited,
                &mut prompt_rx,
                &mut hold_control_rx,
                &hold_active,
                &|_| {},
                "sess-1",
                Some(&observer),
            ),
        )
        .await
        .expect("a queued prompt should end the hold immediately");
        assert!(matches!(outcome, HoldOutcome::NewTurn(_)));

        // Re-entering a live turn is plain "running" again, so the wait must be
        // withdrawn even though the background task is still live.
        assert_eq!(
            recorded.lock().expect("recorder").last().cloned(),
            Some(BackgroundHoldStatus::default())
        );
    }

    #[tokio::test]
    async fn holding_wait_reports_named_task_rows_in_typed_mode() {
        let handler = hold_test_handler();
        handler.set_task_tracking_mode(TaskTrackingMode::Typed);
        let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let child_exited = CancellationToken::new();
        let hold_active = AtomicBool::new(false);
        let (observer, recorded) = recording_hold_observer();

        feed_async_task_update(
            &handler,
            serde_json::json!({
                "sessionUpdate": "async_task_spawned",
                "asyncTaskId": "task-1",
                "name": "Run the tests",
            }),
        )
        .await;

        let mut hold = std::pin::pin!(hold_for_background_quiescence(
            &QUIESCENCE_PROBE,
            TaskTrackingMode::Typed,
            &handler,
            &cancel,
            &child_exited,
            &mut prompt_rx,
            &mut hold_control_rx,
            &hold_active,
            &|_| {},
            "sess-1",
            Some(&observer),
        ));
        tokio::select! {
            _ = &mut hold => panic!("must not settle while a task is live"),
            _ = tokio::time::sleep(HELD_OPEN_PROBE) => {}
        }
        assert_eq!(
            recorded.lock().expect("recorder").as_slice(),
            [BackgroundHoldStatus {
                holding: true,
                live_tasks: 1,
                tasks: vec![BackgroundHoldTask {
                    id: "task-1".to_string(),
                    name: Some("Run the tests".to_string()),
                    description: None,
                    output_file_path: None,
                }],
            }],
            "typed mode names the wait's rows"
        );

        feed_async_task_update(
            &handler,
            serde_json::json!({
                "sessionUpdate": "async_task_state_update",
                "asyncTaskId": "task-1",
                "state": "completed",
            }),
        )
        .await;
        let outcome = tokio::time::timeout(SETTLE_TIMEOUT, hold)
            .await
            .expect("drained set plus debounce should settle before the cap");
        assert!(matches!(outcome, HoldOutcome::Quiescent));
        assert_eq!(
            recorded.lock().expect("recorder").last(),
            Some(&BackgroundHoldStatus::default())
        );
    }

    #[tokio::test]
    async fn holding_wait_serves_per_task_stops_and_rejects_stale_ones() {
        let handler = hold_test_handler();
        let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let child_exited = CancellationToken::new();
        // Shared with the hold: it flips true at entry, which is what lets
        // the handle's stop through at all.
        let hold_active = Arc::new(AtomicBool::new(false));
        let handle = AsyncTaskStopHandle {
            hold_control_tx: hold_control_tx.clone(),
            hold_active: Arc::clone(&hold_active),
        };

        feed_sdk_frame(&handler, task_started("task-1")).await;

        // Queued before the hold exists — aimed at no hold's task set.
        let (stale_reply_tx, stale_reply_rx) = oneshot::channel();
        hold_control_tx
            .send(StopAsyncTaskRequest {
                task_id: "task-0".to_string(),
                reply: stale_reply_tx,
            })
            .expect("queue should accept");

        let served = std::sync::Mutex::new(Vec::new());
        let serve = |request: StopAsyncTaskRequest| {
            served.lock().expect("served").push(request.task_id.clone());
            let _ = request.reply.send(Ok(true));
        };
        let mut hold = std::pin::pin!(hold_for_background_quiescence(
            &QUIESCENCE_PROBE,
            TaskTrackingMode::Raw,
            &handler,
            &cancel,
            &child_exited,
            &mut prompt_rx,
            &mut hold_control_rx,
            &hold_active,
            &serve,
            "sess-1",
            None,
        ));

        // Let the hold enter: the stale request is rejected at the boundary,
        // never dispatched.
        tokio::select! {
            _ = &mut hold => panic!("must not settle while a task is live"),
            _ = tokio::time::sleep(HELD_OPEN_PROBE) => {}
        }
        let stale = stale_reply_rx.await.expect("entry rejection must answer");
        assert!(
            stale.as_ref().is_err_and(|e| e.contains("predates")),
            "got: {stale:?}"
        );
        assert!(served.lock().expect("served").is_empty());

        // A stop sent during the hold is dispatched (here, to the recording
        // stand-in) and the agent's answer routed back to the caller — while
        // the hold itself keeps waiting.
        let stopped = tokio::select! {
            _ = &mut hold => panic!("serving a stop must not end the hold"),
            stopped = handle.stop("task-1") => stopped,
        };
        assert_eq!(stopped, Ok(true));
        assert_eq!(*served.lock().expect("served"), ["task-1".to_string()]);

        feed_sdk_frame(&handler, empty_tasks_frame()).await;
        let outcome = tokio::time::timeout(SETTLE_TIMEOUT, hold)
            .await
            .expect("drained set plus debounce should settle before the cap");
        assert!(matches!(outcome, HoldOutcome::Quiescent));
    }

    #[tokio::test]
    async fn a_stop_with_no_hold_active_is_rejected_immediately() {
        let handler = hold_test_handler();
        let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let child_exited = CancellationToken::new();
        let hold_active = Arc::new(AtomicBool::new(false));
        let handle = AsyncTaskStopHandle {
            hold_control_tx,
            hold_active: Arc::clone(&hold_active),
        };

        // Before any hold: nothing serves the channel, so queueing would
        // leave the caller pending — unanswered — until the next hold entry
        // or teardown. Reject at the handle instead.
        let error = handle
            .stop("task-1")
            .await
            .expect_err("no hold to serve the stop");
        assert!(
            error.contains("no background hold is active"),
            "got: {error}"
        );
        assert!(
            hold_control_rx.try_recv().is_err(),
            "an immediate rejection must not queue the request"
        );

        // Run one hold to quiescence: the flag clears on the way out, so a
        // stop clicked just after the hold ends is rejected as immediately as
        // one clicked before it began — not parked until the next boundary.
        feed_sdk_frame(&handler, empty_tasks_frame()).await;
        let outcome = tokio::time::timeout(
            SETTLE_TIMEOUT,
            hold_for_background_quiescence(
                &QUIESCENCE_PROBE,
                TaskTrackingMode::Raw,
                &handler,
                &cancel,
                &child_exited,
                &mut prompt_rx,
                &mut hold_control_rx,
                &hold_active,
                &|_| {},
                "sess-1",
                None,
            ),
        )
        .await
        .expect("an empty proven set should settle quiescent");
        assert!(matches!(outcome, HoldOutcome::Quiescent));

        let error = handle
            .stop("task-1")
            .await
            .expect_err("the hold ended; nothing serves the stop");
        assert!(
            error.contains("no background hold is active"),
            "got: {error}"
        );
        assert!(hold_control_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn boundary_rejection_answers_every_queued_stop() {
        let (hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        let mut replies = Vec::new();
        for task_id in ["task-1", "task-2"] {
            let (reply_tx, reply_rx) = oneshot::channel();
            hold_control_tx
                .send(StopAsyncTaskRequest {
                    task_id: task_id.to_string(),
                    reply: reply_tx,
                })
                .expect("queue should accept");
            replies.push(reply_rx);
        }

        reject_queued_stop_requests(
            &mut hold_control_rx,
            "the background hold ended before it was served",
        );
        for reply in replies {
            let answer = reply.await.expect("every queued request must be answered");
            assert!(
                answer.as_ref().is_err_and(|e| e.contains("hold ended")),
                "got: {answer:?}"
            );
        }
        assert!(
            hold_control_rx.try_recv().is_err(),
            "the queue must be drained"
        );
    }

    #[tokio::test]
    async fn a_stop_handle_maps_channel_failures_to_connection_state() {
        let (hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
        // Reads as if a hold were live, so the requests reach the channel and
        // exercise the failure mapping past the immediate-rejection gate.
        let handle = AsyncTaskStopHandle {
            hold_control_tx,
            hold_active: Arc::new(AtomicBool::new(true)),
        };

        // A reply dropped unanswered (teardown discarded the scheduled
        // callback) reads as the connection closing mid-request.
        let drop_unanswered = async {
            let request = hold_control_rx.recv().await.expect("request should arrive");
            drop(request);
        };
        let (stopped, ()) = tokio::join!(handle.stop("task-1"), drop_unanswered);
        let error = stopped.expect_err("a dropped reply is not an answer");
        assert!(error.contains("closed before"), "got: {error}");

        // With the receiver gone the connection itself is gone.
        drop(hold_control_rx);
        let error = handle
            .stop("task-2")
            .await
            .expect_err("no connection to serve the stop");
        assert!(error.contains("no longer running"), "got: {error}");
    }

    // =========================================================================
    // Out-of-turn attribution: continuation records
    // =========================================================================

    /// Metadata fields the attribution tests care about.
    #[derive(Debug, Clone, PartialEq)]
    struct RecordedMetadata {
        event_kind: Option<String>,
        message_id: Option<String>,
        origin: Option<String>,
    }

    /// One call the handler made on the writer. The *order* is the point: a
    /// `Finalize` between two `Append`s is a message-record boundary.
    #[derive(Debug, Clone, PartialEq)]
    enum WriterCall {
        Append(String),
        Finalize,
        Event(RecordedMetadata),
        ToolCall(String),
        ToolCallMetadata(RecordedMetadata),
    }

    #[derive(Default)]
    struct TranscriptWriter {
        calls: Mutex<Vec<WriterCall>>,
    }

    impl TranscriptWriter {
        fn calls(&self) -> Vec<WriterCall> {
            self.calls.lock().unwrap().clone()
        }

        /// Appends and boundaries only — the transcript shape, without the
        /// metadata rows.
        fn text_flow(&self) -> Vec<WriterCall> {
            self.calls()
                .into_iter()
                .filter(|call| matches!(call, WriterCall::Append(_) | WriterCall::Finalize))
                .collect()
        }

        fn chunk_metadata(&self) -> Vec<RecordedMetadata> {
            self.calls()
                .into_iter()
                .filter_map(|call| match call {
                    WriterCall::Event(metadata)
                        if metadata.event_kind.as_deref() == Some("agent_message_chunk") =>
                    {
                        Some(metadata)
                    }
                    _ => None,
                })
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl MessageWriter for TranscriptWriter {
        async fn append_text(&self, text: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(WriterCall::Append(text.to_string()));
        }

        async fn finalize(&self) {
            self.calls.lock().unwrap().push(WriterCall::Finalize);
        }

        async fn record_tool_call(
            &self,
            tool_call_id: &str,
            _title: &str,
            _raw_input: Option<&serde_json::Value>,
        ) {
            self.calls
                .lock()
                .unwrap()
                .push(WriterCall::ToolCall(tool_call_id.to_string()));
        }

        async fn update_tool_call_title(
            &self,
            _tool_call_id: &str,
            _title: Option<&str>,
            _raw_input: Option<&serde_json::Value>,
        ) {
        }

        async fn record_tool_result(&self, _tool_call_id: &str, _content: &str) {}

        async fn record_acp_event_metadata(&self, metadata: AcpEventMetadata) {
            self.calls
                .lock()
                .unwrap()
                .push(WriterCall::Event(RecordedMetadata {
                    event_kind: metadata.event_kind,
                    message_id: metadata.message_id,
                    origin: metadata.origin,
                }));
        }

        async fn record_tool_call_metadata(&self, metadata: AcpToolCallMetadata) {
            self.calls
                .lock()
                .unwrap()
                .push(WriterCall::ToolCallMetadata(RecordedMetadata {
                    event_kind: metadata.event_kind,
                    message_id: metadata.tool_call_id,
                    origin: metadata.origin,
                }));
        }
    }

    fn transcript_handler(writer: &Arc<TranscriptWriter>) -> AcpNotificationHandler {
        AcpNotificationHandler::new(writer.clone(), false, vec![], CancellationToken::new())
    }

    fn agent_text(message_id: Option<&str>, text: &str) -> SessionNotification {
        let mut chunk = ContentChunk::new(AcpContentBlock::Text(TextContent::new(text)));
        chunk.message_id = message_id.map(|id| id.to_string().into());
        SessionNotification::new("sess-1", SessionUpdate::AgentMessageChunk(chunk))
    }

    fn tool_call_notification(tool_call_id: &str) -> SessionNotification {
        SessionNotification::new(
            "sess-1",
            SessionUpdate::ToolCall(ToolCall::new(tool_call_id.to_string(), "Run tests")),
        )
    }

    async fn feed(handler: &AcpNotificationHandler, notification: SessionNotification) {
        handler
            .session_notification(notification)
            .await
            .expect("update should be accepted");
    }

    #[tokio::test]
    async fn continuation_is_a_separate_record_attributed_to_its_autonomous_cycle() {
        let writer = Arc::new(TranscriptWriter::default());
        let handler = Arc::new(transcript_handler(&writer));

        feed(&handler, agent_text(Some("msg-1"), "starting the build")).await;
        handler.transition_to_background_holding().await;
        // The task that settled is what woke the model — the bridge's own
        // attribution for the cycle about to stream.
        feed_sdk_frame(
            &handler,
            serde_json::json!({
                "type": "system",
                "subtype": "task_notification",
                "task_id": "task-1",
                "status": "completed",
            }),
        )
        .await;
        feed(&handler, agent_text(Some("msg-2"), "the build passed")).await;

        // The turn's text is closed before the continuation's begins: two
        // records, never one.
        assert_eq!(
            writer.text_flow(),
            vec![
                WriterCall::Append("starting the build".to_string()),
                WriterCall::Finalize,
                WriterCall::Append("the build passed".to_string()),
            ]
        );

        let chunks = writer.chunk_metadata();
        assert_eq!(chunks.len(), 2);
        // The live turn is untagged and keeps its own id.
        assert_eq!(chunks[0].origin, None);
        assert_eq!(chunks[0].message_id.as_deref(), Some("msg-1"));
        // The continuation is tagged with the precise cycle kind and sits on
        // its own id boundary.
        assert_eq!(
            chunks[1].origin.as_deref(),
            Some(background_continuation_origin(Some(TASK_NOTIFICATION_ORIGIN)).as_str())
        );
        assert_eq!(chunks[1].message_id.as_deref(), Some("msg-2"));
    }

    #[tokio::test]
    async fn continuation_without_a_usable_chunk_id_gets_a_synthesized_boundary() {
        let writer = Arc::new(TranscriptWriter::default());
        let handler = Arc::new(transcript_handler(&writer));

        feed(&handler, agent_text(Some("msg-1"), "kicked off the tests")).await;
        handler.transition_to_background_holding().await;
        // No raw frame named the cycle, and the provider sends no message id:
        // the phase alone is the attribution, and the boundary must be
        // invented — id-less chunks would otherwise coalesce into the turn's
        // own message.
        feed(&handler, agent_text(None, "tests are green")).await;
        feed(&handler, agent_text(None, " — 412 passed")).await;

        let chunks = writer.chunk_metadata();
        assert_eq!(chunks.len(), 3);
        assert_eq!(
            chunks[1].origin.as_deref(),
            Some(BACKGROUND_CONTINUATION_ORIGIN)
        );
        let synthesized = chunks[1]
            .message_id
            .clone()
            .expect("continuation text must carry a boundary id");
        assert!(
            synthesized.starts_with(CONTINUATION_MESSAGE_ID_PREFIX),
            "expected a synthesized boundary, got {synthesized}"
        );
        // Both chunks of the same continuation share the record's boundary.
        assert_eq!(chunks[2].message_id.as_deref(), Some(synthesized.as_str()));
        // One record: the boundary is closed once, not per chunk.
        assert_eq!(
            writer
                .text_flow()
                .iter()
                .filter(|call| **call == WriterCall::Finalize)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn a_continuation_chunk_reusing_the_turns_message_id_is_still_split_off() {
        let writer = Arc::new(TranscriptWriter::default());
        let handler = Arc::new(transcript_handler(&writer));

        feed(&handler, agent_text(Some("msg-1"), "waiting on the shell")).await;
        handler.transition_to_background_holding().await;
        // A provider that keeps streaming under the finished turn's id must not
        // be allowed to extend that message.
        feed(&handler, agent_text(Some("msg-1"), "the shell finished")).await;

        let chunks = writer.chunk_metadata();
        let continuation_id = chunks[1]
            .message_id
            .clone()
            .expect("continuation text must carry a boundary id");
        assert_ne!(continuation_id, "msg-1");
        assert!(continuation_id.starts_with(CONTINUATION_MESSAGE_ID_PREFIX));
    }

    #[tokio::test]
    async fn a_new_prompt_during_the_hold_does_not_absorb_continuation_text() {
        let writer = Arc::new(TranscriptWriter::default());
        let handler = Arc::new(transcript_handler(&writer));

        feed(&handler, agent_text(Some("msg-1"), "build running")).await;
        handler.transition_to_background_holding().await;
        feed(&handler, agent_text(Some("msg-2"), "build finished")).await;

        // The user prompts while the connection is still holding: the
        // continuation is closed before the new turn records anything.
        handler.transition_to_live().await;
        feed(&handler, agent_text(Some("msg-3"), "on it")).await;

        assert_eq!(
            writer.text_flow(),
            vec![
                WriterCall::Append("build running".to_string()),
                WriterCall::Finalize,
                WriterCall::Append("build finished".to_string()),
                WriterCall::Finalize,
                WriterCall::Append("on it".to_string()),
            ]
        );

        let chunks = writer.chunk_metadata();
        assert_eq!(chunks.len(), 3);
        assert_eq!(
            chunks[1].origin.as_deref(),
            Some(BACKGROUND_CONTINUATION_ORIGIN)
        );
        // The new turn is a live turn again: untagged, and on its own id.
        assert_eq!(chunks[2].origin, None);
        assert_eq!(chunks[2].message_id.as_deref(), Some("msg-3"));
    }

    #[tokio::test]
    async fn a_second_autonomous_cycle_gets_its_own_continuation_record() {
        let writer = Arc::new(TranscriptWriter::default());
        let handler = Arc::new(transcript_handler(&writer));

        feed(&handler, agent_text(Some("msg-1"), "two shells running")).await;
        handler.transition_to_background_holding().await;
        feed(&handler, agent_text(Some("msg-2"), "shell A done")).await;
        feed(&handler, agent_text(Some("msg-3"), "shell B done")).await;

        assert_eq!(
            writer.text_flow(),
            vec![
                WriterCall::Append("two shells running".to_string()),
                WriterCall::Finalize,
                WriterCall::Append("shell A done".to_string()),
                WriterCall::Finalize,
                WriterCall::Append("shell B done".to_string()),
            ]
        );
        let chunks = writer.chunk_metadata();
        assert_eq!(chunks[1].message_id.as_deref(), Some("msg-2"));
        assert_eq!(chunks[2].message_id.as_deref(), Some("msg-3"));
        for chunk in &chunks[1..] {
            assert_eq!(
                chunk.origin.as_deref(),
                Some(BACKGROUND_CONTINUATION_ORIGIN)
            );
        }
    }

    #[tokio::test]
    async fn a_settled_typed_task_names_the_continuation_it_wakes() {
        let writer = Arc::new(TranscriptWriter::default());
        let handler = Arc::new(transcript_handler(&writer));
        handler.set_task_tracking_mode(TaskTrackingMode::Typed);

        feed(&handler, agent_text(Some("msg-1"), "starting the tests")).await;
        handler.transition_to_background_holding().await;
        feed_async_task_update(
            &handler,
            serde_json::json!({
                "sessionUpdate": "async_task_spawned",
                "asyncTaskId": "task-1",
                "name": "Run the tests",
            }),
        )
        .await;
        // The settle is the wake: the typed edge names it, the raw frame
        // kinds the cycle that follows.
        feed_async_task_update(
            &handler,
            serde_json::json!({
                "sessionUpdate": "async_task_state_update",
                "asyncTaskId": "task-1",
                "state": "completed",
            }),
        )
        .await;
        feed_sdk_frame(
            &handler,
            serde_json::json!({
                "type": "system",
                "subtype": "task_notification",
                "task_id": "task-1",
                "status": "completed",
            }),
        )
        .await;
        feed(&handler, agent_text(Some("msg-2"), "the tests passed")).await;

        let chunks = writer.chunk_metadata();
        assert_eq!(
            chunks.last().and_then(|chunk| chunk.origin.as_deref()),
            Some("background-continuation:task-notification:Run the tests")
        );
    }

    #[tokio::test]
    async fn an_unnamed_settle_does_not_inherit_an_earlier_tasks_name() {
        let writer = Arc::new(TranscriptWriter::default());
        let handler = Arc::new(transcript_handler(&writer));
        handler.set_task_tracking_mode(TaskTrackingMode::Typed);

        feed(&handler, agent_text(Some("msg-1"), "two tasks running")).await;
        handler.transition_to_background_holding().await;
        for update in [
            serde_json::json!({
                "sessionUpdate": "async_task_spawned",
                "asyncTaskId": "task-1",
                "name": "Run the tests",
            }),
            serde_json::json!({
                "sessionUpdate": "async_task_spawned",
                "asyncTaskId": "task-2",
            }),
            serde_json::json!({
                "sessionUpdate": "async_task_state_update",
                "asyncTaskId": "task-1",
                "state": "completed",
            }),
            // The nameless task settles last: its (absent) name is what the
            // wake reports on now.
            serde_json::json!({
                "sessionUpdate": "async_task_state_update",
                "asyncTaskId": "task-2",
                "state": "completed",
            }),
        ] {
            feed_async_task_update(&handler, update).await;
        }
        feed_sdk_frame(
            &handler,
            serde_json::json!({
                "type": "system",
                "subtype": "task_notification",
                "task_id": "task-2",
                "status": "completed",
            }),
        )
        .await;
        feed(&handler, agent_text(Some("msg-2"), "both settled")).await;

        let chunks = writer.chunk_metadata();
        assert_eq!(
            chunks.last().and_then(|chunk| chunk.origin.as_deref()),
            Some(background_continuation_origin(Some(TASK_NOTIFICATION_ORIGIN)).as_str())
        );
    }

    #[tokio::test]
    async fn a_new_hold_does_not_inherit_the_previous_wake_name() {
        let writer = Arc::new(TranscriptWriter::default());
        let handler = Arc::new(transcript_handler(&writer));
        handler.set_task_tracking_mode(TaskTrackingMode::Typed);

        feed(&handler, agent_text(Some("msg-1"), "first turn")).await;
        handler.transition_to_background_holding().await;
        // A named task settles during the first hold...
        feed_async_task_update(
            &handler,
            serde_json::json!({
                "sessionUpdate": "async_task_spawned",
                "asyncTaskId": "task-1",
                "name": "Run the tests",
            }),
        )
        .await;
        feed_async_task_update(
            &handler,
            serde_json::json!({
                "sessionUpdate": "async_task_state_update",
                "asyncTaskId": "task-1",
                "state": "completed",
            }),
        )
        .await;

        // ...but the next hold starts with fresh evidence: a wake with no
        // settle of its own is kind-only, not the earlier task's name.
        handler.transition_to_live().await;
        feed(&handler, agent_text(Some("msg-2"), "second turn")).await;
        handler.transition_to_background_holding().await;
        feed_sdk_frame(
            &handler,
            serde_json::json!({
                "type": "system",
                "subtype": "task_notification",
                "task_id": "task-1",
                "status": "completed",
            }),
        )
        .await;
        feed(&handler, agent_text(Some("msg-3"), "woke again")).await;

        let chunks = writer.chunk_metadata();
        assert_eq!(
            chunks.last().and_then(|chunk| chunk.origin.as_deref()),
            Some(background_continuation_origin(Some(TASK_NOTIFICATION_ORIGIN)).as_str())
        );
    }

    #[tokio::test]
    async fn out_of_turn_tool_calls_are_attributed_to_the_continuation() {
        let writer = Arc::new(TranscriptWriter::default());
        let handler = Arc::new(transcript_handler(&writer));

        feed(&handler, tool_call_notification("tc-live")).await;
        handler.transition_to_background_holding().await;
        feed(&handler, tool_call_notification("tc-continuation")).await;

        let tool_metadata: Vec<RecordedMetadata> = writer
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                WriterCall::ToolCallMetadata(metadata) => Some(metadata),
                _ => None,
            })
            .collect();
        assert_eq!(tool_metadata.len(), 2);
        assert_eq!(tool_metadata[0].origin, None);
        assert_eq!(
            tool_metadata[1].origin.as_deref(),
            Some(BACKGROUND_CONTINUATION_ORIGIN)
        );
    }

    #[tokio::test]
    async fn origin_evidence_does_not_leak_across_holds() {
        let writer = Arc::new(TranscriptWriter::default());
        let handler = Arc::new(transcript_handler(&writer));

        handler.transition_to_background_holding().await;
        feed_sdk_frame(
            &handler,
            serde_json::json!({
                "type": "assistant",
                "origin": { "kind": TASK_NOTIFICATION_ORIGIN },
            }),
        )
        .await;
        feed(&handler, agent_text(Some("msg-1"), "first cycle")).await;

        // A later hold must not inherit the previous one's evidence.
        handler.transition_to_live().await;
        feed(&handler, agent_text(Some("msg-2"), "next turn")).await;
        handler.transition_to_background_holding().await;
        feed(&handler, agent_text(Some("msg-3"), "second cycle")).await;

        let chunks = writer.chunk_metadata();
        assert_eq!(
            chunks[0].origin.as_deref(),
            Some(background_continuation_origin(Some(TASK_NOTIFICATION_ORIGIN)).as_str())
        );
        assert_eq!(
            chunks[2].origin.as_deref(),
            Some(BACKGROUND_CONTINUATION_ORIGIN)
        );
    }

    #[test]
    fn replay_keeps_a_continuation_separate_from_the_turn_it_followed() {
        // What the previous session persisted: the turn's message, then the
        // continuation on its own (here synthesized) boundary.
        let continuation_id = format!("{CONTINUATION_MESSAGE_ID_PREFIX}7");
        let mut buffer = ReplayBuffer::new(vec![
            ReplayBoundary {
                role: "assistant".to_string(),
                content: "build running".to_string(),
                acp_message_id: Some("msg-1".to_string()),
                acp_tool_call_id: None,
            },
            ReplayBoundary {
                role: "assistant".to_string(),
                content: "build finished".to_string(),
                acp_message_id: Some(continuation_id.clone()),
                acp_tool_call_id: None,
            },
        ]);

        // On resume the agent replays both messages. The distinct ids keep them
        // matched one-for-one instead of collapsing into a single boundary.
        assert!(!buffer.push_text_chunk("assistant", Some("msg-1"), "build running"));
        assert_eq!(buffer.match_cursor, 0);
        assert!(!buffer.push_text_chunk("assistant", Some(&continuation_id), "build finished"));
        assert_eq!(
            buffer.match_cursor, 1,
            "the turn's message must match on its own before the continuation's"
        );
        assert!(buffer.finalize_current());
        assert_eq!(buffer.match_cursor, 2);
    }

    // =========================================================================
    // Out-of-turn permission requests (claude-agent-acp#851)
    // =========================================================================

    /// Writer that captures the permission requests it is asked to resolve, and
    /// answers them however the test says — including "never", to prove nothing
    /// blocks on it.
    struct PermissionWriter {
        requests: Mutex<Vec<AcpPermissionRequest>>,
        behavior: PermissionWriterBehavior,
    }

    enum PermissionWriterBehavior {
        /// Answer immediately with this decision.
        Answer(AcpPermissionDecision),
        /// Never answer — a client that cannot resolve the request (nobody is
        /// watching, or the id means nothing to its UI).
        Hang,
        /// Fail the test if asked at all.
        Unreachable,
    }

    impl PermissionWriter {
        fn new(behavior: PermissionWriterBehavior) -> Arc<Self> {
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                behavior,
            })
        }

        fn requests(&self) -> Vec<AcpPermissionRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl MessageWriter for PermissionWriter {
        async fn append_text(&self, _text: &str) {}

        async fn finalize(&self) {}

        async fn record_tool_call(
            &self,
            _tool_call_id: &str,
            _title: &str,
            _raw_input: Option<&serde_json::Value>,
        ) {
        }

        async fn update_tool_call_title(
            &self,
            _tool_call_id: &str,
            _title: Option<&str>,
            _raw_input: Option<&serde_json::Value>,
        ) {
        }

        async fn record_tool_result(&self, _tool_call_id: &str, _content: &str) {}

        async fn request_permission(
            &self,
            request: AcpPermissionRequest,
            _cancel_token: CancellationToken,
        ) -> AcpPermissionDecision {
            self.requests.lock().unwrap().push(request);
            match &self.behavior {
                PermissionWriterBehavior::Answer(decision) => decision.clone(),
                PermissionWriterBehavior::Hang => std::future::pending().await,
                PermissionWriterBehavior::Unreachable => {
                    panic!("the writer must not be asked to resolve this request")
                }
            }
        }
    }

    fn permission_request(tool_call_id: &str) -> RequestPermissionRequest {
        RequestPermissionRequest::new(
            "sess-1",
            ToolCallUpdate::new(
                tool_call_id.to_string(),
                ToolCallUpdateFields::new().title("Run tests"),
            ),
            vec![
                PermissionOption::new("allow-once", "Allow once", PermissionOptionKind::AllowOnce),
                PermissionOption::new("reject-once", "Deny", PermissionOptionKind::RejectOnce),
            ],
        )
    }

    /// Room for the announcement grace window plus scheduling slack, without
    /// letting a wedged request hang the suite.
    const PERMISSION_TIMEOUT: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn out_of_turn_permission_request_is_attributed_to_the_continuation() {
        let writer = PermissionWriter::new(PermissionWriterBehavior::Answer(
            AcpPermissionDecision::Selected {
                option_id: "allow-once".to_string(),
            },
        ));
        let handler = Arc::new(AcpNotificationHandler::new(
            writer.clone(),
            false,
            vec![],
            CancellationToken::new(),
        ));

        // A live turn's request carries no attribution.
        feed(&handler, tool_call_notification("tc-live")).await;
        handler
            .request_permission(permission_request("tc-live"))
            .await
            .expect("in-turn request should be answered");

        handler.transition_to_background_holding().await;
        feed_sdk_frame(
            &handler,
            serde_json::json!({
                "type": "system",
                "subtype": "task_notification",
                "task_id": "task-1",
            }),
        )
        .await;
        feed(&handler, tool_call_notification("tc-continuation")).await;
        handler
            .request_permission(permission_request("tc-continuation"))
            .await
            .expect("out-of-turn request should be answered");

        let requests = writer.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].origin, None);
        assert_eq!(
            requests[1].origin.as_deref(),
            Some(background_continuation_origin(Some(TASK_NOTIFICATION_ORIGIN)).as_str()),
            "a request during the hold belongs to the continuation, not the finished turn"
        );
    }

    #[tokio::test]
    async fn unannounced_out_of_turn_permission_id_is_answered_defensively() {
        // The client can never resolve this one — exactly the #851 deadlock, if
        // the request were dispatched and awaited.
        let writer = PermissionWriter::new(PermissionWriterBehavior::Hang);
        let handler = Arc::new(AcpNotificationHandler::new(
            writer.clone(),
            false,
            vec![],
            CancellationToken::new(),
        ));
        handler.transition_to_background_holding().await;

        let response = tokio::time::timeout(
            PERMISSION_TIMEOUT,
            handler.request_permission(permission_request("tc-never-announced")),
        )
        .await
        .expect("an uncorrelatable id must never block the agent")
        .expect("the request should be answered");

        assert_eq!(
            response,
            permission_response_for_decision(AcpPermissionDecision::Selected {
                option_id: "reject-once".to_string()
            })
        );
        assert!(
            writer.requests().is_empty(),
            "an uncorrelatable id must not be dispatched to the client at all"
        );
    }

    #[tokio::test]
    async fn an_announced_out_of_turn_id_is_still_presented() {
        // Same shape as the desync test, but the tool call *was* announced: the
        // defense must not swallow legitimate continuation requests.
        let writer = PermissionWriter::new(PermissionWriterBehavior::Answer(
            AcpPermissionDecision::Selected {
                option_id: "allow-once".to_string(),
            },
        ));
        let handler = Arc::new(AcpNotificationHandler::new(
            writer.clone(),
            false,
            vec![],
            CancellationToken::new(),
        ));
        handler.transition_to_background_holding().await;
        feed(&handler, tool_call_notification("tc-announced")).await;

        let response = tokio::time::timeout(
            PERMISSION_TIMEOUT,
            handler.request_permission(permission_request("tc-announced")),
        )
        .await
        .expect("an announced request should resolve promptly")
        .expect("the request should be answered");

        assert_eq!(
            response,
            permission_response_for_decision(AcpPermissionDecision::Selected {
                option_id: "allow-once".to_string()
            })
        );
        assert_eq!(writer.requests().len(), 1);
    }

    #[tokio::test]
    async fn an_announcement_still_in_flight_is_waited_out() {
        // The `tool_call` update and the permission request are separate frames;
        // a reordered announcement must not be mistaken for the #851 desync.
        let writer = PermissionWriter::new(PermissionWriterBehavior::Answer(
            AcpPermissionDecision::Selected {
                option_id: "allow-once".to_string(),
            },
        ));
        let handler = Arc::new(AcpNotificationHandler::new(
            writer.clone(),
            false,
            vec![],
            CancellationToken::new(),
        ));
        handler.transition_to_background_holding().await;

        let announcing = {
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                tokio::time::sleep(PERMISSION_ANNOUNCEMENT_GRACE / 5).await;
                feed(&handler, tool_call_notification("tc-late")).await;
            })
        };
        let response = tokio::time::timeout(
            PERMISSION_TIMEOUT,
            handler.request_permission(permission_request("tc-late")),
        )
        .await
        .expect("the grace window should cover a late announcement")
        .expect("the request should be answered");
        announcing.await.expect("announcement task should finish");

        assert_eq!(
            response,
            permission_response_for_decision(AcpPermissionDecision::Selected {
                option_id: "allow-once".to_string()
            })
        );
    }

    #[tokio::test]
    async fn a_duplicate_permission_id_is_answered_defensively_while_the_first_is_pending() {
        let writer = PermissionWriter::new(PermissionWriterBehavior::Hang);
        let handler = Arc::new(AcpNotificationHandler::new(
            writer.clone(),
            false,
            vec![],
            CancellationToken::new(),
        ));
        feed(&handler, tool_call_notification("tc-dup")).await;

        // The first request is outstanding (the client is still deciding)...
        let first = {
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                handler
                    .request_permission(permission_request("tc-dup"))
                    .await
            })
        };
        // ...so a second request for the same id cannot be a queue: it is a
        // desync, and answering it is the only way not to wedge the agent.
        let mut second = None;
        for _ in 0..50 {
            if writer.requests().len() == 1 {
                second = Some(
                    tokio::time::timeout(
                        PERMISSION_TIMEOUT,
                        handler.request_permission(permission_request("tc-dup")),
                    )
                    .await
                    .expect("a duplicate id must never block")
                    .expect("the duplicate should be answered"),
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let second = second.expect("the first request should have reached the writer");

        assert_eq!(
            second,
            permission_response_for_decision(AcpPermissionDecision::Selected {
                option_id: "reject-once".to_string()
            })
        );
        assert_eq!(
            writer.requests().len(),
            1,
            "the duplicate must not be dispatched on top of the pending request"
        );
        first.abort();
    }

    #[tokio::test]
    async fn out_of_turn_policies_resolve_without_prompting() {
        for (policy, expected) in [
            (
                OutOfTurnPermissionPolicy::AutoAllow,
                AcpPermissionDecision::Selected {
                    option_id: "allow-once".to_string(),
                },
            ),
            (
                OutOfTurnPermissionPolicy::AutoDeny,
                AcpPermissionDecision::Selected {
                    option_id: "reject-once".to_string(),
                },
            ),
        ] {
            // An unattended session must never wait on a decision nobody is
            // there to make, so the writer is never asked.
            let writer = PermissionWriter::new(PermissionWriterBehavior::Unreachable);
            let handler = Arc::new(
                AcpNotificationHandler::new(
                    writer.clone(),
                    false,
                    vec![],
                    CancellationToken::new(),
                )
                .with_out_of_turn_permissions(policy),
            );
            handler.transition_to_background_holding().await;
            feed(&handler, tool_call_notification("tc-auto")).await;

            let response = tokio::time::timeout(
                PERMISSION_TIMEOUT,
                handler.request_permission(permission_request("tc-auto")),
            )
            .await
            .unwrap_or_else(|_| panic!("{policy:?} should resolve immediately"))
            .expect("the request should be answered");

            assert_eq!(response, permission_response_for_decision(expected));
            assert!(writer.requests().is_empty());
        }
    }

    #[tokio::test]
    async fn in_turn_permission_requests_keep_their_existing_path() {
        // The desync defense is scoped to the out-of-turn window: an in-turn
        // request for a tool call we never saw announced is still presented,
        // exactly as before this change.
        let writer = PermissionWriter::new(PermissionWriterBehavior::Answer(
            AcpPermissionDecision::Selected {
                option_id: "allow-once".to_string(),
            },
        ));
        let handler = Arc::new(AcpNotificationHandler::new(
            writer.clone(),
            false,
            vec![],
            CancellationToken::new(),
        ));

        let response = tokio::time::timeout(
            PERMISSION_TIMEOUT,
            handler.request_permission(permission_request("tc-unannounced")),
        )
        .await
        .expect("an in-turn request should not be delayed")
        .expect("the request should be answered");

        assert_eq!(
            response,
            permission_response_for_decision(AcpPermissionDecision::Selected {
                option_id: "allow-once".to_string()
            })
        );
        let requests = writer.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].origin, None);
    }

    #[test]
    fn the_defensive_decision_rejects_and_never_approves() {
        // Prefers an explicit rejection option…
        assert_eq!(
            defensive_permission_decision(&acp_permission_request(vec![
                acp_permission_option(
                    "allow-once",
                    "Allow once",
                    AcpPermissionOptionKind::AllowOnce
                ),
                acp_permission_option("reject-once", "Deny", AcpPermissionOptionKind::RejectOnce),
            ])),
            AcpPermissionDecision::Selected {
                option_id: "reject-once".to_string()
            }
        );
        // …and falls back to cancelling rather than picking an unknown option,
        // which could be an approval in disguise.
        assert_eq!(
            defensive_permission_decision(&acp_permission_request(vec![
                acp_permission_option(
                    "allow-once",
                    "Allow once",
                    AcpPermissionOptionKind::AllowOnce
                ),
                acp_permission_option("proceed", "Proceed", AcpPermissionOptionKind::Unknown),
            ])),
            AcpPermissionDecision::Cancelled
        );
        assert_eq!(
            defensive_permission_decision(&acp_permission_request(vec![])),
            AcpPermissionDecision::Cancelled
        );
    }
}
