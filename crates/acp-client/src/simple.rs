//! Simple one-shot ACP prompting without session management.
//!
//! This module provides a convenience wrapper around the full-featured
//! AcpDriver for simple use cases that don't need session persistence.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::driver::{acp_spawn_command, AgentDriver, BasicMessageWriter, MessageWriter};
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
    interpreter_env_snapshot: Option<Vec<(String, String)>>,
}

impl SimpleDriverWrapper {
    fn from_agent(agent: &AcpAgent) -> Self {
        Self {
            binary_path: agent.binary_path.clone(),
            acp_args: agent.acp_args.clone(),
            agent_label: agent.label.clone(),
            interpreter_env_snapshot: None,
        }
    }

    fn with_interpreter_env_snapshot(mut self, vars: Vec<(String, String)>) -> Self {
        self.interpreter_env_snapshot = Some(vars);
        self
    }

    fn spawn_command(&self) -> crate::driver::AcpSpawnCommand {
        acp_spawn_command(
            &self.binary_path,
            &self.acp_args,
            self.interpreter_env_snapshot.as_deref(),
        )
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
        use tokio::process::Command;
        use tokio::sync::Mutex;
        use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

        let spawn_command = self.spawn_command();
        let mut child = Command::new(&spawn_command.program)
            .args(&spawn_command.args)
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

        // Phase-based handler for simple prompting.
        // With empty db_messages, replay completes immediately.
        enum SimpleHandlerPhase {
            Replaying,
            WaitingForPrompt,
            Live,
        }

        struct SimpleHandler {
            writer: Arc<dyn MessageWriter>,
            phase: Mutex<SimpleHandlerPhase>,
        }

        impl SimpleHandler {
            async fn transition_to_waiting(&self) {
                let mut phase = self.phase.lock().await;
                *phase = SimpleHandlerPhase::WaitingForPrompt;
            }

            async fn transition_to_live(&self) {
                let mut phase = self.phase.lock().await;
                *phase = SimpleHandlerPhase::Live;
            }
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
                let phase = self.phase.lock().await;

                match &*phase {
                    SimpleHandlerPhase::Replaying | SimpleHandlerPhase::WaitingForPrompt => {
                        return Ok(());
                    }
                    SimpleHandlerPhase::Live => {
                        // Drop the lock before calling writer
                        drop(phase);
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
                    }
                }
                Ok(())
            }
        }

        let is_resuming = agent_session_id.is_some();
        let handler = Arc::new(SimpleHandler {
            writer: Arc::clone(writer),
            phase: Mutex::new(if is_resuming {
                SimpleHandlerPhase::Replaying
            } else {
                SimpleHandlerPhase::Live
            }),
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

                // Transition: Replaying -> WaitingForPrompt -> Live
                // For the simple driver, no DB messages means replay is
                // effectively instant. The transitions still happen so that
                // any straggler notifications arriving between load_session
                // and the prompt send are correctly dropped.
                if is_resuming {
                    handler.transition_to_waiting().await;
                }

                // Build and send prompt
                let prompt_request = PromptRequest::new(
                    agent_session_id,
                    vec![AcpContentBlock::Text(TextContent::new(prompt))],
                );

                if is_resuming {
                    handler.transition_to_live().await;
                }

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
    run_acp_prompt_with_options(agent, working_dir, prompt, None).await
}

/// Run a one-shot prompt through ACP using a separate environment snapshot
/// only for env-shebang interpreter resolution.
///
/// The spawned agent process still inherits the caller's environment. The
/// snapshot is consulted only to turn launchers such as `#!/usr/bin/env node`
/// into `<resolved-node> <launcher>` so repo-local PATH entries do not choose
/// the ACP bridge interpreter.
pub async fn run_acp_prompt_with_interpreter_env_snapshot(
    agent: &AcpAgent,
    working_dir: &Path,
    prompt: &str,
    interpreter_env_snapshot: Vec<(String, String)>,
) -> Result<String> {
    run_acp_prompt_with_options(agent, working_dir, prompt, Some(interpreter_env_snapshot)).await
}

async fn run_acp_prompt_with_options(
    agent: &AcpAgent,
    working_dir: &Path,
    prompt: &str,
    interpreter_env_snapshot: Option<Vec<(String, String)>>,
) -> Result<String> {
    let working_dir = working_dir.to_path_buf();
    let prompt = prompt.to_string();
    let mut driver = SimpleDriverWrapper::from_agent(agent);
    if let Some(snapshot) = interpreter_env_snapshot {
        driver = driver.with_interpreter_env_snapshot(snapshot);
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
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

    #[test]
    fn simple_driver_uses_interpreter_snapshot_for_env_shebang_bridge() {
        let dir = unique_test_dir("acp-simple-home-interpreter");
        let home_bin = dir.join("home-bin");
        let project_bin = dir.join("project-bin");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("claude-agent-acp");
        let home_node = home_bin.join("node");
        write_executable(&home_node, "#!/bin/sh\n");
        write_executable(&project_bin.join("node"), "#!/bin/sh\n");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let agent = AcpAgent {
            binary_path: launcher.clone(),
            acp_args: vec![String::from("--stdio")],
            label: String::from("Claude Code"),
        };
        let home_snapshot = vec![(
            String::from("PATH"),
            join_path_entries(std::slice::from_ref(&home_bin)),
        )];

        let command = SimpleDriverWrapper::from_agent(&agent)
            .with_interpreter_env_snapshot(home_snapshot)
            .spawn_command();

        assert_eq!(command.program, home_node);
        assert_eq!(
            command.args,
            vec![
                launcher.as_os_str().to_os_string(),
                std::ffi::OsString::from("--stdio"),
            ]
        );

        std::fs::remove_dir_all(dir).expect("cleanup test dir");
    }
}
