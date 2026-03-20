//! Full-featured ACP driver for session management and streaming.
//!
//! This module provides the complete ACP integration including:
//! - Session initialization and resumption
//! - Streaming text and tool calls
//! - Permission handling
//! - Remote workspace support via Blox
//! - Cancellation support

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_client_protocol::{
    Agent, ClientSideConnection, ContentBlock as AcpContentBlock, ImageContent, Implementation,
    InitializeRequest, LoadSessionRequest, McpServer, NewSessionRequest, PermissionOptionId,
    PromptRequest, ProtocolVersion, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
    TextContent,
};
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;

use crate::types::blox_acp_command;

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

    /// Record a tool call with its ID and title.
    async fn record_tool_call(&self, tool_call_id: &str, title: &str);

    /// Update a previously recorded tool call's title.
    async fn update_tool_call_title(&self, tool_call_id: &str, title: &str);

    /// Record the result/output of a tool call.
    async fn record_tool_result(&self, content: &str);
}

/// Storage interface for persisting agent session data.
///
/// This trait abstracts the storage backend, allowing different implementations
/// (SQLite, PostgreSQL, in-memory, etc.) without changing the driver logic.
#[async_trait]
pub trait Store: Send + Sync {
    /// Save the agent's session ID for resumption.
    fn set_agent_session_id(&self, session_id: &str, agent_session_id: &str) -> Result<(), String>;
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
    ) -> Result<(), String>;
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
    /// MCP servers to inject into the session via NewSessionRequest.
    mcp_servers: Vec<McpServer>,
    /// Override the working directory sent to the remote agent.
    /// When set, this path is used in the `NewSessionRequest` instead of the
    /// local `working_dir` passed to `run()`. This is needed because the
    /// local `working_dir` is a fallback path on the host machine, while the
    /// remote agent needs the actual workspace path (e.g. `/home/bloxer/cash-server`).
    remote_working_dir: Option<PathBuf>,
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
                mcp_servers: Vec::new(),
                remote_working_dir: None,
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
                mcp_servers: Vec::new(),
                remote_working_dir: None,
            })
            .ok_or_else(|| {
                "No ACP agent found. Install Goose, Claude Code, Codex, Pi, or Amp and ensure it's on your PATH."
                    .to_string()
            })
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
            mcp_servers: Vec::new(),
            remote_working_dir: None,
        })
    }

    /// Set extra environment variables to pass to the agent process.
    pub fn with_extra_env(mut self, vars: Vec<(String, String)>) -> Self {
        self.extra_env = vars;
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
}

/// Shell-escape a value by wrapping it in single quotes with interior quotes
/// escaped via the standard `'\''` trick.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
    ) -> Result<(), String> {
        let spawn_working_dir = resolve_spawn_working_dir(working_dir, self.is_remote);
        if self.is_remote && spawn_working_dir.as_path() != working_dir {
            log::warn!(
                "Remote ACP spawn cwd missing ({}); falling back to {}",
                working_dir.display(),
                spawn_working_dir.display()
            );
        }

        // For local sessions we need Hermit (and similar directory-based shell
        // hooks) to activate before the agent binary runs. We match the
        // approach used by the actions executor: spawn an interactive login
        // shell with `-s` (stdin mode) in the working directory with a clean
        // environment. The shell initialises fully (`.zshrc` installs hooks),
        // `precmd` fires in the working directory (activating Hermit), then we
        // write an `exec <binary>` command to stdin. `exec` replaces the shell
        // with the agent binary so all subsequent stdin/stdout traffic is the
        // JSON-RPC protocol.
        let is_local_shell = !self.is_remote;

        let mut cmd = if is_local_shell {
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
            let mut c = Command::new(&self.binary_path);
            c.args(&self.acp_args).current_dir(&spawn_working_dir);
            c
        };

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // NOTE: stderr is discarded for both local and remote spawns. For local
            // shells this means shell init errors (e.g. Hermit activation failures,
            // .zshrc syntax errors) are silently swallowed. The agent will still run
            // but without the hermit-managed toolchain. Consider piping stderr (as
            // the actions executor does) and logging it to aid debugging.
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // For local shells extra_env is set on the clean environment; for
        // remote spawns it augments the inherited environment.
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

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to get stdin".to_string())?;

        // For local shells, write the exec command to stdin. By the time the
        // shell reads from stdin, init is complete and `precmd` has fired in
        // the working directory (activating Hermit). `exec` replaces the shell
        // with the agent binary — from this point on, stdin belongs to the
        // agent's JSON-RPC transport.
        if is_local_shell {
            let exec_line = format!(
                "exec {} {}\n",
                shell_quote(&self.binary_path.to_string_lossy()),
                self.acp_args
                    .iter()
                    .map(|a| shell_quote(a))
                    .collect::<Vec<_>>()
                    .join(" ")
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
        let incoming_reader: Box<dyn tokio::io::AsyncRead + Unpin> = if self.is_remote {
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
            // Local shell init (.zshrc, plugin banners, Hermit activation) may
            // write to stdout before `exec` replaces the shell. Filter out any
            // non-JSON lines so they don't reach the JSON-RPC parser.
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
        let handler = Arc::new(AcpNotificationHandler::new(Arc::clone(writer), is_resuming));
        let handler_for_conn = Arc::clone(&handler);

        let (connection, io_future) =
            ClientSideConnection::new(handler_for_conn, stdin_compat, stdout_compat, |fut| {
                tokio::task::spawn_local(fut);
            });

        tokio::task::spawn_local(async move {
            if let Err(e) = io_future.await {
                log::error!("ACP IO error: {e:?}");
            }
        });

        let acp_working_dir = if let Some(ref remote_dir) = self.remote_working_dir {
            remote_dir.clone()
        } else if self.is_remote {
            PathBuf::from(".")
        } else {
            working_dir.to_path_buf()
        };

        let protocol_result = tokio::select! {
            _ = cancel_token.cancelled() => {
                log::info!("Session {session_id} cancelled");
                writer.finalize().await;
                return Ok(());
            }
            result = run_acp_protocol(
                &connection, &acp_working_dir, prompt, images, store,
                session_id, agent_session_id, &handler, &self.mcp_servers,
            ) => result,
        };

        writer.finalize().await;
        let _ = child.kill().await;

        protocol_result
    }
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
// ACP notification handler
// =============================================================================

struct AcpNotificationHandler {
    writer: Arc<dyn MessageWriter>,
    replaying: AtomicBool,
}

impl AcpNotificationHandler {
    fn new(writer: Arc<dyn MessageWriter>, replaying: bool) -> Self {
        Self {
            writer,
            replaying: AtomicBool::new(replaying),
        }
    }

    fn set_live(&self) {
        log::debug!("[replay-guard] set_live() called — transitioning from replaying to live");
        self.replaying.store(false, Ordering::Release);
    }
}

#[async_trait(?Send)]
impl agent_client_protocol::Client for AcpNotificationHandler {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        let option_id = args
            .options
            .first()
            .map(|opt| opt.option_id.clone())
            .unwrap_or_else(|| PermissionOptionId::new("approve"));

        Ok(RequestPermissionResponse::new(
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id)),
        ))
    }

    async fn session_notification(
        &self,
        notification: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        if self.replaying.load(Ordering::Acquire) {
            log::debug!(
                "[replay-guard] Dropping replayed notification: {:?}",
                notification.update
            );
            return Ok(());
        }

        log::debug!(
            "[replay-guard] Processing LIVE notification: {:?}",
            notification.update
        );

        match &notification.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let AcpContentBlock::Text(text) = &chunk.content {
                    self.writer.append_text(&text.text).await;
                }
            }
            SessionUpdate::ToolCall(tool_call) => {
                self.writer
                    .record_tool_call(tool_call.tool_call_id.0.as_ref(), &tool_call.title)
                    .await;
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let tc_id = update.tool_call_id.0.to_string();

                if let Some(ref title) = update.fields.title {
                    self.writer.update_tool_call_title(&tc_id, title).await;
                }

                if let Some(ref content) = update.fields.content {
                    if let Some(preview) = extract_content_preview(content) {
                        self.writer.record_tool_result(&preview).await;
                    }
                }
            }
            _ => {
                log::debug!("Ignoring session update: {:?}", notification.update);
            }
        }
        Ok(())
    }
}

// =============================================================================
// Protocol helpers
// =============================================================================

#[allow(clippy::too_many_arguments)]
async fn run_acp_protocol(
    connection: &ClientSideConnection,
    working_dir: &Path,
    prompt: &str,
    images: &[(String, String)],
    store: &Arc<dyn Store>,
    our_session_id: &str,
    acp_session_id: Option<&str>,
    handler: &Arc<AcpNotificationHandler>,
    mcp_servers: &[McpServer],
) -> Result<(), String> {
    let agent_session_id = tokio::time::timeout(
        ACP_SETUP_TIMEOUT,
        setup_acp_session(
            connection,
            working_dir,
            store,
            our_session_id,
            acp_session_id,
            mcp_servers,
        ),
    )
    .await
    .map_err(|_| {
        format!(
            "Timed out waiting for ACP protocol startup after {}s",
            ACP_SETUP_TIMEOUT.as_secs()
        )
    })??;

    handler.set_live();
    log::debug!(
        "[replay-guard] setup_acp_session complete, handler is now live — about to send prompt"
    );

    let mut content_blocks = vec![AcpContentBlock::Text(TextContent::new(prompt))];
    for (data, mime_type) in images {
        content_blocks.push(AcpContentBlock::Image(ImageContent::new(
            data.as_str(),
            mime_type.as_str(),
        )));
    }
    let prompt_request = PromptRequest::new(agent_session_id, content_blocks);

    connection
        .prompt(prompt_request)
        .await
        .map_err(|e| format!("Prompt failed: {e:?}"))?;

    Ok(())
}

async fn setup_acp_session(
    connection: &ClientSideConnection,
    working_dir: &Path,
    store: &Arc<dyn Store>,
    our_session_id: &str,
    acp_session_id: Option<&str>,
    mcp_servers: &[McpServer],
) -> Result<String, String> {
    let client_info = Implementation::new("acp-client", env!("CARGO_PKG_VERSION"));
    let init_request = InitializeRequest::new(ProtocolVersion::LATEST).client_info(client_info);

    let init_response = connection
        .initialize(init_request)
        .await
        .map_err(|e| format!("ACP init failed: {e:?}"))?;

    if !mcp_servers.is_empty() {
        let mcp_caps = &init_response.agent_capabilities.mcp_capabilities;
        let requires_http = mcp_servers
            .iter()
            .any(|server| matches!(server, McpServer::Http(_)));
        let requires_sse = mcp_servers
            .iter()
            .any(|server| matches!(server, McpServer::Sse(_)));

        if (requires_http && !mcp_caps.http) || (requires_sse && !mcp_caps.sse) {
            return Err(format!(
                "Agent does not support required MCP transports (required: http={}, sse={}; agent: http={}, sse={}). Select a provider with MCP support for project tools.",
                requires_http,
                requires_sse,
                mcp_caps.http,
                mcp_caps.sse
            ));
        }
    }

    match acp_session_id {
        Some(existing_id) => {
            if !init_response.agent_capabilities.load_session {
                return Err(
                    "Agent does not support load_session — cannot resume conversation".to_string(),
                );
            }

            log::info!(
                "Resuming ACP session {existing_id} via load_session for session {our_session_id}"
            );

            connection
                .load_session(
                    LoadSessionRequest::new(existing_id.to_string(), working_dir.to_path_buf())
                        .mcp_servers(mcp_servers.to_vec()),
                )
                .await
                .map_err(|e| format!("Failed to load ACP session: {e:?}"))?;

            Ok(existing_id.to_string())
        }
        None => {
            let new_session_request =
                NewSessionRequest::new(working_dir.to_path_buf()).mcp_servers(mcp_servers.to_vec());
            let session_response = connection
                .new_session(new_session_request)
                .await
                .map_err(|e| format!("Failed to create ACP session: {e:?}"))?;

            let new_id = session_response.session_id.to_string();
            store
                .set_agent_session_id(our_session_id, &new_id)
                .map_err(|e| format!("Failed to save agent session ID: {e}"))?;
            Ok(new_id)
        }
    }
}

fn extract_content_preview(content: &[agent_client_protocol::ToolCallContent]) -> Option<String> {
    for item in content {
        match item {
            agent_client_protocol::ToolCallContent::Content(c) => {
                if let AcpContentBlock::Text(text) = &c.content {
                    let preview: String = text.text.chars().take(500).collect();
                    return Some(if text.text.len() > 500 {
                        format!("{preview}…")
                    } else {
                        preview
                    });
                }
            }
            agent_client_protocol::ToolCallContent::Diff(d) => {
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
            agent_client_protocol::ToolCallContent::Terminal(t) => {
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

    async fn record_tool_call(&self, _tool_call_id: &str, title: &str) {
        let mut current = self.text.lock().await;
        current.push_str(&format!("\n[Tool: {}]\n", title));
    }

    async fn update_tool_call_title(&self, _tool_call_id: &str, _title: &str) {
        // Nothing to do for basic implementation
    }

    async fn record_tool_result(&self, content: &str) {
        let mut current = self.text.lock().await;
        current.push_str(&format!("\n[Result: {}]\n", content));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        consume_remote_acp_line, decode_remote_acp_line, remote_acp_segments,
        resolve_spawn_working_dir, sanitize_remote_acp_chunk, shell_quote, RemoteLineOutcome,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

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
}
