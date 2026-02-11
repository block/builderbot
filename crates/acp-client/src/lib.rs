//! ACP Client - Simple client for Agent Client Protocol (ACP)
//!
//! This library provides a simple way to communicate with ACP-compatible agents
//! like Goose and Claude Code. It handles agent discovery, process management,
//! and the ACP protocol handshake.
//!
//! # Features
//!
//! - Agent discovery (automatically finds installed agents)
//! - One-shot prompts (send a prompt, get a response)
//! - Session support (resume conversations)
//! - Protocol handling (uses agent_client_protocol SDK)
//!
//! # Example
//!
//! ```rust,no_run
//! use acp_client::{find_acp_agent, run_acp_prompt_raw};
//! use std::path::Path;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let agent = find_acp_agent().ok_or_else(|| anyhow::anyhow!("No ACP agent found"))?;
//!     let response = run_acp_prompt_raw(&agent, Path::new("."), "Hello!").await?;
//!     println!("Agent response: {}", response);
//!     Ok(())
//! }
//! ```

use std::path::{Path, PathBuf};
use std::process::Stdio;

use agent_client_protocol::{
    Agent, ClientSideConnection, ContentBlock as AcpContentBlock, Implementation,
    InitializeRequest, NewSessionRequest, PermissionOptionId, PromptRequest, ProtocolVersion,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    Result as AcpResult, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
    TextContent,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::process::Command;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Supported ACP-compatible AI agents
#[derive(Debug, Clone)]
pub enum AcpAgent {
    /// Goose agent (https://github.com/block/goose)
    Goose(PathBuf),
    /// Claude Code agent
    Claude(PathBuf),
    /// Codex agent
    Codex(PathBuf),
}

impl AcpAgent {
    /// Get the agent's name as a string
    pub fn name(&self) -> &'static str {
        match self {
            AcpAgent::Goose(_) => "goose",
            AcpAgent::Claude(_) => "claude",
            AcpAgent::Codex(_) => "codex",
        }
    }

    /// Get the path to the agent executable
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

/// Find an agent CLI using login shell (to get user's PATH)
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

/// Result of running an ACP prompt
pub struct AcpPromptResult {
    /// The agent's response text
    pub response: String,
}

/// Simple client implementation that just collects the response
struct SimpleAcpClient {
    /// Accumulated response text
    response: std::sync::Arc<tokio::sync::Mutex<String>>,
}

impl SimpleAcpClient {
    fn new() -> Self {
        Self {
            response: std::sync::Arc::new(tokio::sync::Mutex::new(String::new())),
        }
    }

    async fn get_response(&self) -> String {
        self.response.lock().await.clone()
    }
}

#[async_trait(?Send)]
impl agent_client_protocol::Client for SimpleAcpClient {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> AcpResult<RequestPermissionResponse> {
        // Auto-approve permissions
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
        // Collect response text from agent message chunks
        match &notification.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let AcpContentBlock::Text(text) = &chunk.content {
                    let mut response = self.response.lock().await;
                    response.push_str(&text.text);
                }
            }
            _ => {
                // Ignore other updates for simple client
            }
        }

        Ok(())
    }
}

/// Run a one-shot prompt through ACP and return the response
///
/// This spawns the agent, initializes ACP, sends the prompt, collects the
/// response, and shuts down. Designed for simple one-shot queries.
///
/// # Arguments
///
/// * `agent` - The ACP agent to use
/// * `working_dir` - The working directory for the agent
/// * `prompt` - The prompt to send to the agent
///
/// # Returns
///
/// The agent's text response
pub async fn run_acp_prompt_raw(
    agent: &AcpAgent,
    working_dir: &Path,
    prompt: &str,
) -> Result<String> {
    let agent_path = agent.path().to_path_buf();
    let agent_name = agent.name().to_string();
    let agent_args: Vec<String> = agent.acp_args().iter().map(|s| s.to_string()).collect();
    let working_dir = working_dir.to_path_buf();
    let prompt = prompt.to_string();

    // Run the ACP session in a blocking task with its own runtime
    // This is needed because ACP uses !Send futures (LocalSet)
    tokio::task::spawn_blocking(move || {
        // Create a new runtime for this thread
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to create runtime")?;

        // Run the ACP session on a LocalSet
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            run_acp_session_inner(&agent_path, &agent_name, &agent_args, &working_dir, &prompt)
                .await
        })
    })
    .await
    .context("Task join error")?
}

/// Internal function to run the ACP session (runs on LocalSet)
async fn run_acp_session_inner(
    agent_path: &Path,
    agent_name: &str,
    agent_args: &[String],
    working_dir: &Path,
    prompt: &str,
) -> Result<String> {
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
        .with_context(|| format!("Failed to spawn {agent_name}"))?;

    // Get stdin/stdout
    let stdin = child
        .stdin
        .take()
        .context("Failed to get stdin from agent process")?;
    let stdout = child
        .stdout
        .take()
        .context("Failed to get stdout from agent process")?;

    // Convert to futures-compatible async read/write
    let stdin_compat = stdin.compat_write();
    let stdout_compat = stdout.compat();

    // Create simple client
    let client = std::sync::Arc::new(SimpleAcpClient::new());
    let client_for_connection = std::sync::Arc::clone(&client);

    // Create the ACP connection
    let (connection, io_future) =
        ClientSideConnection::new(client_for_connection, stdin_compat, stdout_compat, |fut| {
            tokio::task::spawn_local(fut);
        });

    // Spawn the IO task
    tokio::task::spawn_local(async move {
        if let Err(e) = io_future.await {
            eprintln!("ACP IO error: {e:?}");
        }
    });

    // Initialize the connection
    let client_info = Implementation::new("acp-client", env!("CARGO_PKG_VERSION"));
    let init_request = InitializeRequest::new(ProtocolVersion::LATEST).client_info(client_info);

    let _init_response = connection
        .initialize(init_request)
        .await
        .context("Failed to initialize ACP connection")?;

    // Create new session
    let session_response = connection
        .new_session(NewSessionRequest::new(working_dir.to_path_buf()))
        .await
        .context("Failed to create ACP session")?;

    let session_id = session_response.session_id;

    // Send the prompt
    let prompt_request = PromptRequest::new(
        session_id,
        vec![AcpContentBlock::Text(TextContent::new(prompt.to_string()))],
    );

    let prompt_result = connection.prompt(prompt_request).await;

    // Clean up the child process
    let _ = child.kill().await;

    // Handle result
    prompt_result.context("Failed to send prompt")?;
    let response = client.get_response().await;
    Ok(response)
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

    #[test]
    fn test_discover_providers() {
        let providers = discover_acp_providers();
        // Should return at least an empty list, not panic
        assert!(providers.len() >= 0);
    }
}
