//! Simple one-shot ACP prompting without session management.
//!
//! This module provides a convenience wrapper around the full-featured
//! AcpDriver for simple use cases that don't need session persistence.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::driver::{AcpDriver, AgentDriver, AgentRunOutcome, BasicMessageWriter, MessageWriter};
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

struct SimpleDriverWrapper {
    driver: AcpDriver,
}

impl SimpleDriverWrapper {
    fn from_agent(agent: &AcpAgent) -> Self {
        Self {
            driver: AcpDriver::from_agent(agent),
        }
    }

    fn with_interpreter_env_snapshot(mut self, vars: Vec<(String, String)>) -> Self {
        self.driver = self.driver.with_interpreter_env_snapshot(vars);
        self
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
    ) -> Result<AgentRunOutcome, String> {
        if !images.is_empty() {
            log::debug!(
                "SimpleDriverWrapper: discarding {} image(s) - not supported in simple mode",
                images.len()
            );
        }

        self.driver
            .run(
                session_id,
                prompt,
                &[],
                working_dir,
                store,
                writer,
                cancel_token,
                agent_session_id,
            )
            .await
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
