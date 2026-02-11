//! ACP (Agent Client Protocol) driver — spawns an ACP-compatible agent
//! and communicates via the ACP JSON-RPC protocol over stdio.
//!
//! This is the **only** file that imports `agent_client_protocol`. To
//! switch to a different agent backend, create a new module implementing
//! [`AgentDriver`] and leave this file untouched (or remove it).
//!
//! ## Supported agents
//!
//! | ID        | Command           | Notes                                      |
//! |-----------|-------------------|--------------------------------------------|
//! | `goose`   | `goose`           | Needs `acp --with-builtin developer,...`   |
//! | `claude`  | `claude-code-acp` | Runs in ACP mode by default                |
//! | `codex`   | `codex-acp`       | Runs in ACP mode by default                |
//! | `pi`      | `pi-acp`          | Runs in ACP mode by default                |
//!
//! ## Process lifecycle
//!
//! The agent subprocess is spawned with `kill_on_drop(true)`. When the
//! future returned by [`AcpDriver::run`] completes — for any reason —
//! the child is dropped and the OS process is killed. This is the
//! primary guarantee that cancellation doesn't leave orphan processes.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use agent_client_protocol::{
    Agent, ClientSideConnection, ContentBlock as AcpContentBlock, Implementation,
    InitializeRequest, LoadSessionRequest, NewSessionRequest, PermissionOptionId, PromptRequest,
    ProtocolVersion, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, TextContent,
};
use async_trait::async_trait;
use serde::Serialize;
use tokio::process::Command;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;

use super::writer::MessageWriter;
use super::AgentDriver;
use crate::store::Store;

// =============================================================================
// Known agents — the registry of ACP-compatible providers
// =============================================================================

/// Static metadata for each known ACP agent.
struct KnownAgent {
    /// Unique identifier used in preferences and IPC.
    id: &'static str,
    /// Human-readable label for the UI.
    label: &'static str,
    /// CLI command name to search for.
    command: &'static str,
    /// Arguments to pass when spawning in ACP mode.
    acp_args: &'static [&'static str],
}

/// All agents we know how to talk to, in display order.
const KNOWN_AGENTS: &[KnownAgent] = &[
    KnownAgent {
        id: "goose",
        label: "Goose",
        command: "goose",
        acp_args: &[
            "acp",
            "--with-builtin",
            "developer",
            "--with-builtin",
            "extensionmanager",
        ],
    },
    KnownAgent {
        id: "claude",
        label: "Claude Code",
        command: "claude-code-acp",
        acp_args: &[],
    },
    KnownAgent {
        id: "codex",
        label: "Codex",
        command: "codex-acp",
        acp_args: &[],
    },
    KnownAgent {
        id: "pi",
        label: "Pi",
        command: "pi-acp",
        acp_args: &[],
    },
];

// =============================================================================
// Provider discovery — exposed to the frontend
// =============================================================================

/// Information about a discovered ACP provider, serialized to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct AcpProviderInfo {
    pub id: String,
    pub label: String,
}

/// Scan the system for all known ACP agents that are installed.
///
/// Returns only agents whose CLI binary can be found. The order matches
/// `KNOWN_AGENTS` (display order).
pub fn discover_providers() -> Vec<AcpProviderInfo> {
    KNOWN_AGENTS
        .iter()
        .filter(|agent| find_command(agent.command).is_some())
        .map(|agent| AcpProviderInfo {
            id: agent.id.to_string(),
            label: agent.label.to_string(),
        })
        .collect()
}

// =============================================================================
// AcpDriver — the public driver
// =============================================================================

pub struct AcpDriver {
    binary_path: PathBuf,
    acp_args: Vec<String>,
    agent_label: String,
    /// When true, this driver proxies through a remote Blox workspace.
    /// The local `working_dir` should NOT be sent in ACP session requests
    /// because it doesn't exist on the remote machine.
    is_remote: bool,
}

impl AcpDriver {
    /// Create a driver for the given provider ID (e.g. "goose", "claude").
    ///
    /// Looks up the agent in `KNOWN_AGENTS`, locates the binary on disk,
    /// and returns a ready-to-use driver. Fails immediately if the agent
    /// is unknown or its binary can't be found.
    pub fn new(provider_id: &str) -> Result<Self, String> {
        let agent = KNOWN_AGENTS
            .iter()
            .find(|a| a.id == provider_id)
            .ok_or_else(|| format!("Unknown agent provider: {provider_id}"))?;

        let binary_path = find_command(agent.command).ok_or_else(|| {
            format!(
                "Could not find `{}` binary. Install it and ensure it's on your PATH.",
                agent.command
            )
        })?;

        Ok(Self {
            binary_path,
            acp_args: agent.acp_args.iter().map(|s| s.to_string()).collect(),
            agent_label: agent.label.to_string(),
            is_remote: false,
        })
    }

    /// Create a driver for the first available provider.
    ///
    /// Tries each known agent in order and returns the first one found.
    /// This is the fallback when no provider preference is set.
    pub fn first_available() -> Result<Self, String> {
        for agent in KNOWN_AGENTS {
            if let Some(path) = find_command(agent.command) {
                return Ok(Self {
                    binary_path: path,
                    acp_args: agent.acp_args.iter().map(|s| s.to_string()).collect(),
                    agent_label: agent.label.to_string(),
                    is_remote: false,
                });
            }
        }
        Err(
            "No ACP agent found. Install Goose, Claude Code, Codex, or Pi and ensure it's on your PATH."
                .to_string(),
        )
    }

    /// Create a driver that proxies through `blox acp <workspace>`.
    ///
    /// This speaks the same ACP protocol over stdio, but the subprocess
    /// is `blox acp <workspace_name>` instead of a local agent binary.
    /// An optional `--command` flag is derived from the agent ID so the
    /// remote workspace spawns the right agent.
    pub fn for_workspace(workspace_name: &str, agent_id: Option<&str>) -> Result<Self, String> {
        let binary_path = find_command("blox").ok_or_else(|| {
            "Could not find `blox` binary. Install it and ensure it's on your PATH.".to_string()
        })?;

        let mut args = vec!["acp".to_string(), workspace_name.to_string()];

        // Map the agent ID to the command string the remote workspace needs.
        if let Some(id) = agent_id {
            if let Some(cmd) = blox_acp_command(id) {
                args.push(format!("--command={cmd}"));
            }
        }

        Ok(Self {
            binary_path,
            acp_args: args,
            agent_label: "Blox".to_string(),
            is_remote: true,
        })
    }
}

/// Map an agent ID to the `--command` value for `blox acp`.
///
/// Returns `None` if the agent uses the workspace default (no flag needed).
fn blox_acp_command(agent_id: &str) -> Option<String> {
    KNOWN_AGENTS.iter().find(|a| a.id == agent_id).map(|a| {
        // Build "command,arg1,arg2,..." from the command name and acp_args.
        let mut parts = vec![a.command];
        parts.extend(a.acp_args.iter().copied());
        parts.join(",")
    })
}

impl AgentDriver for AcpDriver {
    async fn run(
        &self,
        session_id: &str,
        prompt: &str,
        working_dir: &Path,
        store: &Arc<Store>,
        writer: &Arc<MessageWriter>,
        cancel_token: &CancellationToken,
        agent_session_id: Option<&str>,
    ) -> Result<(), String> {
        let mut child = Command::new(&self.binary_path)
            .args(&self.acp_args)
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr is intentionally discarded — we don't currently need
            // anything from it, and piping without draining would block
            // the agent if the OS pipe buffer fills up.
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Failed to spawn {}: {e}", self.agent_label))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to get stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to get stdout".to_string())?;

        let stdin_compat = stdin.compat_write();
        let stdout_compat = stdout.compat();

        // Start in replaying mode if we're loading an existing session —
        // notifications during load_session are historical and already
        // in the DB.
        let is_resuming = agent_session_id.is_some();
        let handler = Arc::new(AcpNotificationHandler::new(Arc::clone(writer), is_resuming));
        let handler_for_conn = Arc::clone(&handler);

        let (connection, io_future) =
            ClientSideConnection::new(handler_for_conn, stdin_compat, stdout_compat, |fut| {
                tokio::task::spawn_local(fut);
            });

        // Spawn IO task
        tokio::task::spawn_local(async move {
            if let Err(e) = io_future.await {
                log::error!("ACP IO error: {e:?}");
            }
        });

        // For remote workspaces, don't send the local path in ACP session
        // requests — the remote agent should use its own workspace directory.
        let acp_working_dir = if self.is_remote {
            PathBuf::from(".")
        } else {
            working_dir.to_path_buf()
        };

        // Race the protocol against cancellation.
        let protocol_result = tokio::select! {
            _ = cancel_token.cancelled() => {
                log::info!("Session {session_id} cancelled");
                writer.finalize().await;
                return Ok(());
            }
            result = run_acp_protocol(
                &connection, &acp_working_dir, prompt, store,
                session_id, agent_session_id, &handler,
            ) => result,
        };

        // Normal completion — finalize remaining text.
        writer.finalize().await;

        // Explicit kill (belt-and-suspenders with kill_on_drop).
        let _ = child.kill().await;

        protocol_result
    }
}

// =============================================================================
// ACP notification handler — translates ACP events → MessageWriter calls
// =============================================================================

/// Thin adapter between ACP's streaming notifications and our protocol-
/// agnostic [`MessageWriter`]. This is the only type that implements
/// `agent_client_protocol::Client`.
struct AcpNotificationHandler {
    writer: Arc<MessageWriter>,
    /// When true, notifications are from a `load_session` history replay
    /// and should be silently ignored (we already have them in the DB).
    replaying: AtomicBool,
}

impl AcpNotificationHandler {
    fn new(writer: Arc<MessageWriter>, replaying: bool) -> Self {
        Self {
            writer,
            replaying: AtomicBool::new(replaying),
        }
    }

    /// Stop ignoring notifications — called after `load_session` completes
    /// and before the new prompt is sent.
    fn set_live(&self) {
        self.replaying.store(false, Ordering::Release);
    }
}

#[async_trait(?Send)]
impl agent_client_protocol::Client for AcpNotificationHandler {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        // Auto-approve all permissions
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
            return Ok(());
        }

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
// ACP protocol helpers
// =============================================================================

/// The actual ACP protocol exchange: initialize → new/load session → prompt.
async fn run_acp_protocol(
    connection: &ClientSideConnection,
    working_dir: &Path,
    prompt: &str,
    store: &Arc<Store>,
    our_session_id: &str,
    acp_session_id: Option<&str>,
    handler: &Arc<AcpNotificationHandler>,
) -> Result<(), String> {
    let agent_session_id = setup_acp_session(
        connection,
        working_dir,
        store,
        our_session_id,
        acp_session_id,
    )
    .await?;

    // History replay (if any) is done — switch to live mode so new
    // notifications from the prompt get written to the DB.
    handler.set_live();

    let prompt_request = PromptRequest::new(
        agent_session_id,
        vec![AcpContentBlock::Text(TextContent::new(prompt))],
    );

    connection
        .prompt(prompt_request)
        .await
        .map_err(|e| format!("Prompt failed: {e:?}"))?;

    Ok(())
}

/// Initialize the ACP connection and either create a new session or load
/// an existing one. Returns the ACP session ID to use for the prompt.
///
/// The caller is responsible for passing the correct `working_dir`: the
/// local worktree path for local agents, or `"."` for remote Blox
/// workspaces (so the remote agent uses its own workspace directory).
async fn setup_acp_session(
    connection: &ClientSideConnection,
    working_dir: &Path,
    store: &Arc<Store>,
    our_session_id: &str,
    acp_session_id: Option<&str>,
) -> Result<String, String> {
    let client_info = Implementation::new("staged", env!("CARGO_PKG_VERSION"));
    let init_request = InitializeRequest::new(ProtocolVersion::LATEST).client_info(client_info);

    let init_response = connection
        .initialize(init_request)
        .await
        .map_err(|e| format!("ACP init failed: {e:?}"))?;

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
                .load_session(LoadSessionRequest::new(
                    existing_id.to_string(),
                    working_dir.to_path_buf(),
                ))
                .await
                .map_err(|e| format!("Failed to load ACP session: {e:?}"))?;

            Ok(existing_id.to_string())
        }
        None => {
            let session_response = connection
                .new_session(NewSessionRequest::new(working_dir.to_path_buf()))
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

// =============================================================================
// ACP content helpers
// =============================================================================

/// Extract a short text preview from ACP tool call content blocks.
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
// Binary discovery
// =============================================================================

/// Common paths where CLIs might be installed (GUI apps don't inherit shell PATH).
const COMMON_PATHS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/home/linuxbrew/.linuxbrew/bin",
];

/// Find a CLI binary by command name.
///
/// Searches in order:
/// 1. Login shell `which` (picks up user's PATH from `.zshrc` / `.bashrc`)
/// 2. Common install locations
fn find_command(cmd: &str) -> Option<PathBuf> {
    // Strategy 1: Login shell `which`
    if let Some(path) = find_via_login_shell(cmd) {
        if path.exists() {
            return Some(path);
        }
    }

    // Strategy 2: Common paths
    for dir in COMMON_PATHS {
        let path = PathBuf::from(dir).join(cmd);
        if path.exists() {
            return Some(path);
        }
    }

    None
}

fn find_via_login_shell(cmd: &str) -> Option<PathBuf> {
    let which_cmd = format!("which {cmd}");

    for shell in &["/bin/zsh", "/bin/bash"] {
        if let Ok(output) = std::process::Command::new(shell)
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
    }
    None
}
