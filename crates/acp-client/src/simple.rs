//! Simple one-shot ACP prompting without session management.

use std::path::Path;

use agent_client_protocol::{
    Agent, ClientSideConnection, ContentBlock as AcpContentBlock, Implementation,
    InitializeRequest, NewSessionRequest, PermissionOptionId, PromptRequest, ProtocolVersion,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    Result as AcpResult, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
    TextContent,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::types::AcpAgent;

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
    let agent_path = agent.binary_path.clone();
    let agent_label = agent.label.clone();
    let agent_args = agent.acp_args.clone();
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
            run_acp_session_inner(&agent_path, &agent_label, &agent_args, &working_dir, &prompt)
                .await
        })
    })
    .await
    .context("Task join error")?
}

/// Internal function to run the ACP session (runs on LocalSet)
async fn run_acp_session_inner(
    agent_path: &Path,
    agent_label: &str,
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
        .stderr(Stdio::null())
        .kill_on_drop(true); // Ensure child is killed if we exit early

    let mut child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn {agent_label}"))?;

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
            log::error!("ACP IO error: {e:?}");
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
