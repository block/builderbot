//! ACP Client - handles communication with AI agents via Agent Client Protocol
//!
//! This module spawns agent processes and communicates with them using ACP,
//! a JSON-RPC based protocol over stdio. Supports both one-shot requests
//! (for diff analysis) and persistent sessions (for interactive chat).
//!
//! For streaming sessions, emits Tauri events with SDK types directly:
//! - "session-update": SessionNotification from the SDK
//! - "session-complete": Custom event with finalized transcript

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use agent_client_protocol::{
    Agent, ClientSideConnection, ContentBlock as AcpContentBlock, Implementation,
    InitializeRequest, LoadSessionRequest, NewSessionRequest, PermissionOptionId, PromptRequest,
    ProtocolVersion, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    Result as AcpResult, SelectedPermissionOutcome, SessionId, SessionNotification, SessionUpdate,
    TextContent, ToolCall,
};
use async_trait::async_trait;

use tauri::Emitter;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// System context prepended to the first message in new sessions.
/// This guides the agent's behavior for Staged's code review use case.
const STAGED_SYSTEM_CONTEXT: &str = r#"[System Context for Staged - Code Review Assistant]

You are helping with code review in Staged, a diff viewer application. Your role is to help users understand, plan changes to, and research code in their changesets.

Output Guidelines:
- When asked to create a PLAN: produce a structured markdown document with clear objectives, step-by-step tasks, and file references
- When asked to do RESEARCH: produce a research document with summary of findings, relevant code references, and recommendations
- When answering QUESTIONS: be concise and focused on the code changes

The user is viewing a diff. Context tags like [Changeset: ...], [Viewing: ...], and [Original task: ...] provide information about what they're looking at.

---

"#;

/// Supported ACP-compatible AI agents
#[derive(Debug, Clone)]
pub enum AcpAgent {
    Goose(PathBuf),
    Claude(PathBuf),
    Codex(PathBuf),
}

impl AcpAgent {
    pub fn name(&self) -> &'static str {
        match self {
            AcpAgent::Goose(_) => "goose",
            AcpAgent::Claude(_) => "claude",
            AcpAgent::Codex(_) => "codex",
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            AcpAgent::Goose(p) => p,
            AcpAgent::Claude(p) => p,
            AcpAgent::Codex(p) => p,
        }
    }

    /// Get the arguments to start ACP mode
    pub fn acp_args(&self) -> Vec<&str> {
        match self {
            // Include developer extension for file/shell access, and extensionmanager
            // to allow discovering/enabling additional extensions as needed
            AcpAgent::Goose(_) => vec!["acp", "--with-builtin", "developer,extensionmanager"],
            AcpAgent::Claude(_) => vec![], // claude-code-acp runs in ACP mode by default
            AcpAgent::Codex(_) => vec![],  // codex-acp runs in ACP mode by default
        }
    }
}

/// Common paths where CLIs might be installed (for GUI apps that don't inherit shell PATH)
const COMMON_PATHS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/home/linuxbrew/.linuxbrew/bin",
];

/// Find goose CLI using login shell (to get user's PATH)
fn find_via_login_shell(cmd: &str) -> Option<PathBuf> {
    let which_cmd = format!("which {cmd}");

    // Try zsh first (default on macOS)
    if let Ok(output) = std::process::Command::new("/bin/zsh")
        .args(["-l", "-c", &which_cmd])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(path_str) = stdout.lines().rfind(|l| !l.is_empty()) {
                let path_str = path_str.trim();
                if !path_str.is_empty() && path_str.starts_with('/') {
                    return Some(PathBuf::from(path_str));
                }
            }
        }
    }

    // Fallback to bash
    if let Ok(output) = std::process::Command::new("/bin/bash")
        .args(["-l", "-c", &which_cmd])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(path_str) = stdout.lines().rfind(|l| !l.is_empty()) {
                let path_str = path_str.trim();
                if !path_str.is_empty() && path_str.starts_with('/') {
                    return Some(PathBuf::from(path_str));
                }
            }
        }
    }

    None
}

/// Verify a command works by checking if it exists and is executable
fn verify_command(path: &Path) -> bool {
    // First check if file exists and is executable
    if !path.exists() {
        return false;
    }

    // Try --version first (works for most tools)
    if let Ok(output) = std::process::Command::new(path).arg("--version").output() {
        if output.status.success() {
            return true;
        }
    }

    // If --version fails, try --help.
    // codex-acp doesn't implement --version but does respond to --help,
    // so this avoids false negatives when checking availability.
    if let Ok(output) = std::process::Command::new(path).arg("--help").output() {
        if output.status.success() {
            return true;
        }
    }

    // If both fail, assume it's not a valid command
    false
}

/// Information about an available ACP provider
#[derive(Debug, Clone, serde::Serialize)]
pub struct AcpProviderInfo {
    pub id: String,
    pub label: String,
}

/// Discover all available ACP providers on the system
pub fn discover_acp_providers() -> Vec<AcpProviderInfo> {
    let mut providers = Vec::new();

    if find_agent("goose", AcpAgent::Goose).is_some() {
        providers.push(AcpProviderInfo {
            id: "goose".to_string(),
            label: "Goose".to_string(),
        });
    }

    if find_agent("claude-code-acp", AcpAgent::Claude).is_some() {
        providers.push(AcpProviderInfo {
            id: "claude".to_string(),
            label: "Claude Code".to_string(),
        });
    }

    if find_agent("codex-acp", AcpAgent::Codex).is_some() {
        providers.push(AcpProviderInfo {
            id: "codex".to_string(),
            label: "Codex".to_string(),
        });
    }

    providers
}

/// Find a specific ACP agent by provider ID
pub fn find_acp_agent_by_id(provider_id: &str) -> Option<AcpAgent> {
    match provider_id {
        "goose" => find_agent("goose", AcpAgent::Goose),
        "claude" => find_agent("claude-code-acp", AcpAgent::Claude),
        "codex" => find_agent("codex-acp", AcpAgent::Codex),
        _ => None,
    }
}

/// Find an ACP-compatible AI agent
/// Prefers Goose if available, falls back to Claude
pub fn find_acp_agent() -> Option<AcpAgent> {
    // Try Goose first (default)
    if let Some(agent) = find_agent("goose", AcpAgent::Goose) {
        return Some(agent);
    }

    // Fall back to Claude (claude-code-acp)
    find_agent("claude-code-acp", AcpAgent::Claude)
}

/// Find a specific agent by command name
fn find_agent<F>(cmd: &str, constructor: F) -> Option<AcpAgent>
where
    F: Fn(PathBuf) -> AcpAgent,
{
    // Strategy 1: Login shell which
    if let Some(path) = find_via_login_shell(cmd) {
        if verify_command(&path) {
            return Some(constructor(path));
        }
    }

    // Strategy 2: Direct command
    let direct_path = PathBuf::from(cmd);
    if verify_command(&direct_path) {
        return Some(constructor(direct_path));
    }

    // Strategy 3: Common paths
    for dir in COMMON_PATHS {
        let path = PathBuf::from(dir).join(cmd);
        if path.exists() && verify_command(&path) {
            return Some(constructor(path));
        }
    }

    None
}

// =============================================================================
// Finalized Message Types (for database storage)
// =============================================================================
// Streaming Client Implementation
// =============================================================================

/// Internal state for tracking a tool call during streaming
#[derive(Debug, Clone)]
struct ToolCallState {
    id: String,
    title: String,
    status: String,
    locations: Vec<String>,
    result_preview: Option<String>,
}

impl From<&ToolCall> for ToolCallState {
    fn from(tc: &ToolCall) -> Self {
        Self {
            id: tc.tool_call_id.0.to_string(),
            title: tc.title.clone(),
            status: format!("{:?}", tc.status).to_lowercase(),
            locations: tc
                .locations
                .iter()
                .map(|l| l.path.display().to_string())
                .collect(),
            result_preview: None,
        }
    }
}

/// A segment of content in order of arrival
#[derive(Debug, Clone)]
enum ContentSegment {
    Text(String),
    ToolCall(ToolCallState),
}

/// Client implementation for handling agent notifications with streaming support
struct StreamingAcpClient {
    /// Tauri app handle for emitting events (None for non-streaming mode)
    app_handle: Option<tauri::AppHandle>,
    /// Internal session ID (our DB key) — used to replace the ACP session ID
    /// in emitted events so the frontend always sees our internal IDs.
    internal_session_id: String,
    /// Content segments in arrival order (text chunks get merged, tool calls break the sequence)
    segments: Mutex<Vec<ContentSegment>>,
    /// Tool call index by ID (for updates)
    tool_call_indices: Mutex<HashMap<String, usize>>,
    /// Whether to suppress emitting events (used during session load replay)
    suppress_emit: Mutex<bool>,
    /// Optional callback for buffer updates (called whenever segments change during streaming).
    ///
    /// This callback allows the SessionManager to maintain an in-memory buffer of streaming
    /// segments before they are persisted to the database. It enables the UI to show immediate
    /// progress when opening a session viewer during active streaming.
    ///
    /// The callback is invoked asynchronously in a spawned task to:
    /// - Prevent blocking the streaming process
    /// - Isolate potential panics or errors in the callback
    ///
    /// Thread-safety notes:
    /// - The callback is Arc<dyn Fn + Send + Sync> so it can be called from async contexts
    /// - The callback should NOT acquire locks that the calling code might hold (risk of deadlock)
    /// - The callback should be fast and non-blocking (it runs in a spawned task but still)
    ///
    /// When to use:
    /// - Set this callback when streaming to a session that might be viewed live (e.g., SessionManager)
    /// - Leave as None for fire-and-forget prompts or internal operations (e.g., legacy paths)
    buffer_update_callback: Option<Arc<dyn Fn(Vec<crate::store::ContentSegment>) + Send + Sync>>,
}

impl StreamingAcpClient {
    fn new(app_handle: Option<tauri::AppHandle>, internal_session_id: String) -> Self {
        Self {
            app_handle,
            internal_session_id,
            segments: Mutex::new(Vec::new()),
            tool_call_indices: Mutex::new(HashMap::new()),
            suppress_emit: Mutex::new(false),
            buffer_update_callback: None,
        }
    }

    fn with_buffer_callback(
        app_handle: Option<tauri::AppHandle>,
        internal_session_id: String,
        callback: Arc<dyn Fn(Vec<crate::store::ContentSegment>) + Send + Sync>,
    ) -> Self {
        Self {
            app_handle,
            internal_session_id,
            segments: Mutex::new(Vec::new()),
            tool_call_indices: Mutex::new(HashMap::new()),
            suppress_emit: Mutex::new(false),
            buffer_update_callback: Some(callback),
        }
    }

    /// Set whether to suppress emitting events to frontend
    async fn set_suppress_emit(&self, suppress: bool) {
        *self.suppress_emit.lock().await = suppress;
    }

    /// Emit a session update event to the frontend (unless suppressed).
    /// Replaces the ACP session ID with our internal session ID so the
    /// frontend can correlate updates with the correct session.
    async fn emit_update(&self, notification: &SessionNotification) {
        if *self.suppress_emit.lock().await {
            return;
        }
        if let Some(ref app_handle) = self.app_handle {
            let mut patched = notification.clone();
            patched.session_id = SessionId::new(&*self.internal_session_id);
            if let Err(e) = app_handle.emit("session-update", &patched) {
                log::warn!("Failed to emit session-update event: {e}");
            }
        }
    }

    /// Get the segments in order for storage
    async fn get_segments(&self) -> Vec<crate::store::ContentSegment> {
        let segments = self.segments.lock().await;
        segments
            .iter()
            .map(|seg| match seg {
                ContentSegment::Text(text) => {
                    crate::store::ContentSegment::Text { text: text.clone() }
                }
                ContentSegment::ToolCall(tc) => crate::store::ContentSegment::ToolCall {
                    id: tc.id.clone(),
                    title: tc.title.clone(),
                    status: tc.status.clone(),
                    locations: tc.locations.clone(),
                },
            })
            .collect()
    }

    /// Get the accumulated response text (for non-streaming callers)
    async fn get_response(&self) -> String {
        let segments = self.segments.lock().await;
        segments
            .iter()
            .filter_map(|seg| match seg {
                ContentSegment::Text(text) => Some(text.as_str()),
                ContentSegment::ToolCall(_) => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Clear accumulated state (used after loading session history)
    async fn clear(&self) {
        self.segments.lock().await.clear();
        self.tool_call_indices.lock().await.clear();
    }

    /// Notify buffer callback with current segments
    async fn notify_buffer_update(&self) {
        if let Some(ref callback) = self.buffer_update_callback {
            let segments = self.get_segments().await;
            let callback = Arc::clone(callback);
            // Spawn task to prevent blocking and isolate potential panics/errors
            tokio::spawn(async move {
                callback(segments);
            });
        }
    }
}

#[async_trait(?Send)]
impl agent_client_protocol::Client for StreamingAcpClient {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> AcpResult<RequestPermissionResponse> {
        // Auto-approve permissions (Staged doesn't use tools that need approval)
        log::debug!("Permission requested: {args:?}");

        let option_id = args
            .options
            .first()
            .map(|opt| opt.option_id.clone())
            .unwrap_or_else(|| PermissionOptionId::new("approve"));

        Ok(RequestPermissionResponse::new(
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id)),
        ))
    }

    async fn session_notification(&self, notification: SessionNotification) -> AcpResult<()> {
        // 1. Emit the raw SDK notification to frontend (if streaming, and not suppressed)
        self.emit_update(&notification).await;

        // 2. Update internal state - track segments in order
        let mut should_notify_buffer = false;
        match &notification.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let AcpContentBlock::Text(text) = &chunk.content {
                    let mut segments = self.segments.lock().await;
                    // Append to last text segment, or create new one
                    if let Some(ContentSegment::Text(last_text)) = segments.last_mut() {
                        last_text.push_str(&text.text);
                    } else {
                        segments.push(ContentSegment::Text(text.text.clone()));
                    }
                    should_notify_buffer = true;
                }
            }
            SessionUpdate::ToolCall(tool_call) => {
                let state = ToolCallState::from(tool_call);
                let mut segments = self.segments.lock().await;
                let mut indices = self.tool_call_indices.lock().await;
                let idx = segments.len();
                indices.insert(state.id.clone(), idx);
                segments.push(ContentSegment::ToolCall(state));
                should_notify_buffer = true;
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let indices = self.tool_call_indices.lock().await;
                if let Some(&idx) = indices.get(&update.tool_call_id.0.to_string()) {
                    let mut segments = self.segments.lock().await;
                    if let Some(ContentSegment::ToolCall(tc)) = segments.get_mut(idx) {
                        if let Some(ref status) = update.fields.status {
                            tc.status = format!("{status:?}").to_lowercase();
                        }
                        if let Some(ref title) = update.fields.title {
                            tc.title = title.clone();
                        }
                        if let Some(ref content) = update.fields.content {
                            tc.result_preview = extract_content_preview(content);
                        }
                        should_notify_buffer = true;
                    }
                }
            }
            _ => {
                log::debug!("Ignoring session update: {:?}", notification.update);
            }
        }

        // 3. Notify buffer callback if segments were updated
        if should_notify_buffer {
            self.notify_buffer_update().await;
        }

        Ok(())
    }
}

/// Extract a preview string from tool call content
fn extract_content_preview(content: &[agent_client_protocol::ToolCallContent]) -> Option<String> {
    for item in content {
        match item {
            agent_client_protocol::ToolCallContent::Content(c) => {
                if let AcpContentBlock::Text(text) = &c.content {
                    let preview: String = text.text.chars().take(200).collect();
                    return Some(if text.text.len() > 200 {
                        format!("{preview}...")
                    } else {
                        preview
                    });
                }
            }
            agent_client_protocol::ToolCallContent::Diff(d) => {
                // Show a preview of the diff (old_text -> new_text)
                let preview = format!(
                    "{}{}",
                    d.path.display(),
                    if d.old_text.is_some() {
                        " (modified)"
                    } else {
                        " (new)"
                    }
                );
                return Some(preview);
            }
            agent_client_protocol::ToolCallContent::Terminal(t) => {
                return Some(format!("Terminal: {}", t.terminal_id.0));
            }
            _ => {}
        }
    }
    None
}

// =============================================================================
// Public API
// =============================================================================

/// Result of running an ACP prompt with session support
pub struct AcpPromptResult {
    /// The agent's response text (all text segments concatenated)
    pub response: String,
    /// The session ID (can be used to resume this session later)
    pub session_id: String,
    /// Content segments in order (for storage)
    pub segments: Vec<crate::store::ContentSegment>,
}

/// Run a one-shot prompt through ACP and return the response (no streaming)
///
/// This spawns the agent, initializes ACP, sends the prompt, collects the
/// response, and shuts down. Designed for Staged's single-request use case
/// (e.g., diff analysis).
///
/// Note: This prepends `STAGED_SYSTEM_CONTEXT` to guide the agent for code review.
/// Use `run_acp_prompt_raw` if you need to provide your own system instructions.
pub async fn run_acp_prompt(
    agent: &AcpAgent,
    working_dir: &Path,
    prompt: &str,
) -> Result<String, String> {
    // No streaming, no events emitted — internal_session_id is unused
    let result = run_acp_prompt_internal(
        agent,
        working_dir,
        prompt,
        None,
        None,
        None,
        "",
        true,
        None,
        None,
    )
    .await?;
    Ok(result.response)
}

/// Run a one-shot prompt through ACP without prepending system context.
///
/// Use this when you need full control over the prompt, such as when providing
/// your own system instructions (e.g., PR description generation with JSON output).
pub async fn run_acp_prompt_raw(
    agent: &AcpAgent,
    working_dir: &Path,
    prompt: &str,
) -> Result<String, String> {
    let result = run_acp_prompt_internal(
        agent,
        working_dir,
        prompt,
        None,
        None,
        None,
        "",
        false,
        None,
        None,
    )
    .await?;
    Ok(result.response)
}

/// Run a prompt through ACP with optional session resumption (no streaming)
///
/// If `session_id` is provided, attempts to load and resume that session.
/// Otherwise, creates a new session. Returns both the response and the
/// session ID for future resumption.
pub async fn run_acp_prompt_with_session(
    agent: &AcpAgent,
    working_dir: &Path,
    prompt: &str,
    session_id: Option<&str>,
) -> Result<AcpPromptResult, String> {
    // No streaming, no events emitted — internal_session_id is unused
    run_acp_prompt_internal(
        agent,
        working_dir,
        prompt,
        None,
        session_id,
        None,
        "",
        true,
        None,
        None,
    )
    .await
}

use super::session::CancellationHandle;

/// Run a prompt through ACP with streaming events emitted to frontend
///
/// Emits "session-update" events with SessionNotification payloads during execution.
/// The `internal_session_id` is stamped onto all emitted events so the frontend
/// can correlate them (the ACP protocol uses its own opaque session IDs internally).
///
/// If `cancellation` is provided, the PID of the spawned agent process will be
/// registered with it, allowing external cancellation via process kill.
#[allow(clippy::too_many_arguments)]
pub async fn run_acp_prompt_streaming(
    agent: &AcpAgent,
    working_dir: &Path,
    prompt: &str,
    acp_session_id: Option<&str>,
    internal_session_id: &str,
    app_handle: tauri::AppHandle,
    buffer_callback: Option<Arc<dyn Fn(Vec<crate::store::ContentSegment>) + Send + Sync>>,
    cancellation: Option<Arc<CancellationHandle>>,
) -> Result<AcpPromptResult, String> {
    run_acp_prompt_internal(
        agent,
        working_dir,
        prompt,
        None, // No images
        acp_session_id,
        Some(app_handle),
        internal_session_id,
        true,
        buffer_callback,
        cancellation,
    )
    .await
}

/// Run a prompt with images through ACP with streaming events emitted to frontend
///
/// Same as `run_acp_prompt_streaming` but accepts optional image attachments.
/// Images are sent as ContentBlock::Image in the prompt request.
#[allow(clippy::too_many_arguments)]
pub async fn run_acp_prompt_streaming_with_images(
    agent: &AcpAgent,
    working_dir: &Path,
    prompt: &str,
    images: Option<&[crate::ImageAttachment]>,
    acp_session_id: Option<&str>,
    internal_session_id: &str,
    app_handle: tauri::AppHandle,
    buffer_callback: Option<Arc<dyn Fn(Vec<crate::store::ContentSegment>) + Send + Sync>>,
    cancellation: Option<Arc<CancellationHandle>>,
) -> Result<AcpPromptResult, String> {
    run_acp_prompt_internal(
        agent,
        working_dir,
        prompt,
        images,
        acp_session_id,
        Some(app_handle),
        internal_session_id,
        true,
        buffer_callback,
        cancellation,
    )
    .await
}

/// Internal implementation that handles both streaming and non-streaming modes
#[allow(clippy::too_many_arguments)]
async fn run_acp_prompt_internal(
    agent: &AcpAgent,
    working_dir: &Path,
    prompt: &str,
    images: Option<&[crate::ImageAttachment]>,
    acp_session_id: Option<&str>,
    app_handle: Option<tauri::AppHandle>,
    internal_session_id: &str,
    prepend_system_context: bool,
    buffer_callback: Option<Arc<dyn Fn(Vec<crate::store::ContentSegment>) + Send + Sync>>,
    cancellation: Option<Arc<CancellationHandle>>,
) -> Result<AcpPromptResult, String> {
    let agent_path = agent.path().to_path_buf();
    let agent_name = agent.name().to_string();
    let agent_args: Vec<String> = agent.acp_args().iter().map(|s| s.to_string()).collect();
    let working_dir = working_dir.to_path_buf();
    let prompt = prompt.to_string();
    let images_owned: Option<Vec<crate::ImageAttachment>> = images.map(|imgs| imgs.to_vec());
    let acp_session_id = acp_session_id.map(|s| s.to_string());
    let internal_session_id = internal_session_id.to_string();

    // Run the ACP session in a blocking task with its own runtime
    // This is needed because ACP uses !Send futures (LocalSet)
    tokio::task::spawn_blocking(move || {
        // Create a new runtime for this thread
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("Failed to create runtime: {e}"))?;

        // Run the ACP session on a LocalSet
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            run_acp_session_inner(
                &agent_path,
                &agent_name,
                &agent_args,
                &working_dir,
                &prompt,
                images_owned.as_deref(),
                acp_session_id.as_deref(),
                app_handle,
                &internal_session_id,
                prepend_system_context,
                buffer_callback,
                cancellation,
            )
            .await
        })
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Internal function to run the ACP session (runs on LocalSet)
#[allow(clippy::too_many_arguments)]
async fn run_acp_session_inner(
    agent_path: &Path,
    agent_name: &str,
    agent_args: &[String],
    working_dir: &Path,
    prompt: &str,
    images: Option<&[crate::ImageAttachment]>,
    existing_session_id: Option<&str>,
    app_handle: Option<tauri::AppHandle>,
    internal_session_id: &str,
    prepend_system_context: bool,
    buffer_callback: Option<Arc<dyn Fn(Vec<crate::store::ContentSegment>) + Send + Sync>>,
    cancellation: Option<Arc<CancellationHandle>>,
) -> Result<AcpPromptResult, String> {
    // Spawn the agent process with ACP mode
    let mut cmd = Command::new(agent_path);
    cmd.args(agent_args)
        .current_dir(working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true); // Ensure child is killed if we exit early

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn {agent_name}: {e}"))?;

    // Register the PID with the cancellation handle so it can be killed externally
    if let Some(ref cancel) = cancellation {
        if let Some(pid) = child.id() {
            log::debug!("Registering agent PID {pid} for cancellation");
            cancel.set_pid(pid);
        }
    }

    // Get stdin/stdout
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to get stdin from agent process".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to get stdout from agent process".to_string())?;

    // Convert to futures-compatible async read/write
    let stdin_compat = stdin.compat_write();
    let stdout_compat = stdout.compat();

    // Create streaming client with our internal session ID for event correlation
    let client = Arc::new(if let Some(callback) = buffer_callback {
        StreamingAcpClient::with_buffer_callback(
            app_handle.clone(),
            internal_session_id.to_string(),
            callback,
        )
    } else {
        StreamingAcpClient::new(app_handle.clone(), internal_session_id.to_string())
    });
    let client_for_connection = Arc::clone(&client);

    // Create the ACP connection
    let (connection, io_future) =
        ClientSideConnection::new(client_for_connection, stdin_compat, stdout_compat, |fut| {
            tokio::task::spawn_local(fut);
        });

    // Spawn the IO task
    tokio::task::spawn_local(async move {
        if let Err(e) = io_future.await {
            log::error!("ACP IO error: {e:?}");
        }
    });

    // Initialize the connection
    let client_info = Implementation::new("staged", env!("CARGO_PKG_VERSION"));
    let init_request = InitializeRequest::new(ProtocolVersion::LATEST).client_info(client_info);

    let init_response = connection
        .initialize(init_request)
        .await
        .map_err(|e| format!("Failed to initialize ACP connection: {e:?}"))?;

    if let Some(agent_info) = &init_response.agent_info {
        log::info!(
            "Connected to agent: {} v{}",
            agent_info.name,
            agent_info.version
        );
    }

    // Get or create session, track if this is a new session
    let (session_id, is_new_session): (SessionId, bool) =
        if let Some(existing_id) = existing_session_id {
            // Try to load existing session
            // Suppress emit during load to avoid replaying history to frontend
            client.set_suppress_emit(true).await;

            log::info!("Attempting to load session: {existing_id}");
            let load_request =
                LoadSessionRequest::new(SessionId::new(existing_id), working_dir.to_path_buf());

            let result = match connection.load_session(load_request).await {
                Ok(_) => {
                    log::info!("Resumed session: {existing_id}");
                    (SessionId::new(existing_id), false)
                }
                Err(e) => {
                    // Session not found or error - create a new one
                    log::warn!("Failed to load session {existing_id}: {e:?}, creating new session");
                    let session_response = connection
                        .new_session(NewSessionRequest::new(working_dir.to_path_buf()))
                        .await
                        .map_err(|e| format!("Failed to create ACP session: {e:?}"))?;
                    (session_response.session_id, true)
                }
            };

            // Re-enable emit after session load (replay is done)
            client.set_suppress_emit(false).await;

            result
        } else {
            // Create new session
            let session_response = connection
                .new_session(NewSessionRequest::new(working_dir.to_path_buf()))
                .await
                .map_err(|e| format!("Failed to create ACP session: {e:?}"))?;
            log::info!("Created new session: {}", session_response.session_id.0);
            (session_response.session_id, true)
        };

    // Clear any accumulated content from loading session history
    // (load_session may replay old messages as AgentMessageChunk notifications)
    client.clear().await;

    // For new sessions, optionally prepend system context to guide the agent's behavior
    let full_prompt = if is_new_session && prepend_system_context {
        format!("{STAGED_SYSTEM_CONTEXT}{prompt}")
    } else {
        prompt.to_string()
    };

    // Build content blocks: text prompt + optional images
    let mut content_blocks = vec![AcpContentBlock::Text(TextContent::new(full_prompt))];

    // Add image blocks if provided
    if let Some(imgs) = images {
        for img in imgs {
            content_blocks.push(AcpContentBlock::Image(
                agent_client_protocol::ImageContent::new(img.data.clone(), img.mime_type.clone()),
            ));
        }
    }

    // Send the prompt with content blocks
    let prompt_request = PromptRequest::new(session_id.clone(), content_blocks);

    let prompt_result = connection.prompt(prompt_request).await;

    // Clean up the child process
    let _ = child.kill().await;

    // Handle result
    let session_id_str = session_id.0.to_string();

    match prompt_result {
        Ok(_) => {
            let response = client.get_response().await;
            let segments = client.get_segments().await;

            Ok(AcpPromptResult {
                response,
                session_id: session_id_str,
                segments,
            })
        }
        Err(e) => Err(format!("Failed to send prompt: {e:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_acp_agent() {
        // This test just verifies the function doesn't panic
        // Actual availability depends on the system
        let _ = find_acp_agent();
    }
}
