//! Simple one-shot ACP prompting without session management.
//!
//! This module provides a convenience wrapper around the full-featured
//! AcpDriver for simple use cases that don't need session persistence.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::driver::{AgentDriver, BasicMessageWriter, MessageWriter};
use crate::types::AcpAgent;

/// Minimal store implementation for simple prompting (no persistence).
struct NoOpStore;

#[async_trait]
impl crate::driver::Store for NoOpStore {
    fn set_agent_session_id(
        &self,
        _session_id: &str,
        _agent_session_id: &str,
    ) -> Result<(), String> {
        // No-op: simple prompting doesn't persist sessions
        Ok(())
    }
}

/// Internal driver wrapper for simple prompting.
///
/// This wraps the binary path and args from AcpAgent into an AgentDriver
/// implementation compatible with the driver module's interface.
struct SimpleDriverWrapper {
    binary_path: std::path::PathBuf,
    acp_args: Vec<String>,
    agent_label: String,
}

impl SimpleDriverWrapper {
    fn from_agent(agent: &AcpAgent) -> Self {
        Self {
            binary_path: agent.binary_path.clone(),
            acp_args: agent.acp_args.clone(),
            agent_label: agent.label.clone(),
        }
    }
}

#[async_trait(?Send)]
impl AgentDriver for SimpleDriverWrapper {
    async fn run(
        &self,
        session_id: &str,
        prompt: &str,
        images: &[(String, String)],
        working_dir: &Path,
        store: &Arc<dyn crate::driver::Store>,
        writer: &Arc<dyn MessageWriter>,
        cancel_token: &CancellationToken,
        agent_session_id: Option<&str>,
    ) -> Result<(), String> {
        if !images.is_empty() {
            log::debug!(
                "SimpleDriverWrapper: discarding {} image(s) - not supported in simple mode",
                images.len()
            );
        }

        // Use the same implementation as AcpDriver, but with our own binary/args
        use agent_client_protocol::{
            Agent, ClientSideConnection, ContentBlock as AcpContentBlock, Implementation,
            InitializeRequest, LoadSessionRequest, NewSessionRequest, PermissionOptionId,
            PromptRequest, ProtocolVersion, RequestPermissionOutcome, RequestPermissionRequest,
            RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification,
            SessionUpdate, TextContent,
        };
        use std::process::Stdio;
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::process::Command;
        use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

        let mut child = Command::new(&self.binary_path)
            .args(&self.acp_args)
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
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

        // Simplified handler that just collects text (no tool call handling needed for simple use)
        struct SimpleHandler {
            writer: Arc<dyn MessageWriter>,
            replaying: AtomicBool,
        }

        #[async_trait(?Send)]
        impl agent_client_protocol::Client for SimpleHandler {
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
                        "[replay-guard] simple: dropping replayed notification: {:?}",
                        notification.update
                    );
                    return Ok(());
                }

                log::debug!(
                    "[replay-guard] simple: processing LIVE notification: {:?}",
                    notification.update
                );

                match &notification.update {
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let AcpContentBlock::Text(text) = &chunk.content {
                            self.writer.append_text(&text.text).await;
                        }
                    }
                    _ => {
                        // Ignore other updates for simple use
                    }
                }
                Ok(())
            }
        }

        let is_resuming = agent_session_id.is_some();
        let handler = Arc::new(SimpleHandler {
            writer: Arc::clone(writer),
            replaying: AtomicBool::new(is_resuming),
        });
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

        // Protocol execution
        let protocol_result = tokio::select! {
            _ = cancel_token.cancelled() => {
                log::info!("Session {session_id} cancelled");
                writer.finalize().await;
                return Ok(());
            }
            result = async {
                // Initialize
                let client_info = Implementation::new("acp-client", env!("CARGO_PKG_VERSION"));
                let init_request = InitializeRequest::new(ProtocolVersion::LATEST)
                    .client_info(client_info);

                let init_response = connection
                    .initialize(init_request)
                    .await
                    .map_err(|e| format!("ACP init failed: {e:?}"))?;

                // Create or resume session
                let agent_session_id = match agent_session_id {
                    Some(existing_id) => {
                        if !init_response.agent_capabilities.load_session {
                            return Err("Agent does not support load_session".to_string());
                        }
                        connection
                            .load_session(LoadSessionRequest::new(
                                existing_id.to_string(),
                                working_dir.to_path_buf(),
                            ))
                            .await
                            .map_err(|e| format!("Failed to load session: {e:?}"))?;
                        existing_id.to_string()
                    }
                    None => {
                        let session_response = connection
                            .new_session(NewSessionRequest::new(working_dir.to_path_buf()))
                            .await
                            .map_err(|e| format!("Failed to create session: {e:?}"))?;
                        let new_id = session_response.session_id.to_string();
                        store
                            .set_agent_session_id(session_id, &new_id)
                            .map_err(|e| format!("Failed to save session ID: {e}"))?;
                        new_id
                    }
                };

                // Now we're live (not replaying)
                log::debug!("[replay-guard] simple driver: transitioning from replaying to live");
                handler.replaying.store(false, Ordering::Release);

                // Send prompt
                let prompt_request = PromptRequest::new(
                    agent_session_id,
                    vec![AcpContentBlock::Text(TextContent::new(prompt))],
                );

                connection
                    .prompt(prompt_request)
                    .await
                    .map_err(|e| format!("Prompt failed: {e:?}"))?;

                Ok::<_, String>(())
            } => result,
        };

        writer.finalize().await;
        let _ = child.kill().await;

        protocol_result
    }
}

/// Run a one-shot prompt through ACP and return the response.
///
/// This is a convenience wrapper around the full-featured AcpDriver that
/// handles session setup/teardown automatically. Use this for simple
/// one-shot queries without session persistence.
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
pub async fn run_acp_prompt(agent: &AcpAgent, working_dir: &Path, prompt: &str) -> Result<String> {
    let working_dir = working_dir.to_path_buf();
    let prompt = prompt.to_string();
    let driver = SimpleDriverWrapper::from_agent(agent);

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
            let writer_impl = Arc::new(BasicMessageWriter::new());
            let writer = writer_impl.clone() as Arc<dyn MessageWriter>;
            let store = Arc::new(NoOpStore) as Arc<dyn crate::driver::Store>;
            let cancel_token = CancellationToken::new();

            driver
                .run(
                    "simple-session",
                    &prompt,
                    &[],
                    &working_dir,
                    &store,
                    &writer,
                    &cancel_token,
                    None,
                )
                .await
                .map_err(|e| anyhow::anyhow!("ACP driver error: {e}"))?;

            Ok(writer_impl.get_text().await)
        })
    })
    .await
    .context("Task join error")?
}
