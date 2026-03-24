//! ACP AI provider implementation for action detection
//!
//! This module provides a concrete implementation of the `AiProvider` trait
//! using the acp-client crate to communicate with ACP agents.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use acp_client::{AcpDriver, AgentDriver, BasicMessageWriter, MessageWriter, Store};

use crate::detector::AiProvider;

/// Minimal store implementation for simple prompting (no persistence).
struct NoOpStore;

#[async_trait]
impl Store for NoOpStore {
    fn set_agent_session_id(
        &self,
        _session_id: &str,
        _agent_session_id: &str,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// AI provider that uses ACP agents (like Goose or Claude Code)
pub struct AcpAiProvider {
    working_dir: PathBuf,
}

impl AcpAiProvider {
    /// Create a new ACP AI provider
    ///
    /// # Arguments
    ///
    /// * `working_dir` - The working directory to use when running the agent
    ///
    /// # Returns
    ///
    /// An ACP provider, or an error if no ACP agent is available
    pub fn new(working_dir: PathBuf) -> Result<Self> {
        // Verify an agent is available
        acp_client::find_acp_agent()
            .ok_or_else(|| anyhow::anyhow!("No ACP agent found (tried goose, claude-agent-acp)"))?;
        Ok(Self { working_dir })
    }

    /// Create a provider with a specific agent ID
    ///
    /// # Arguments
    ///
    /// * `provider_id` - The provider ID ("goose", "claude", or "codex")
    /// * `working_dir` - The working directory to use when running the agent
    pub fn with_agent(provider_id: &str, working_dir: PathBuf) -> Result<Self> {
        // Verify the specific agent is available
        acp_client::find_acp_agent_by_id(provider_id).ok_or_else(|| {
            anyhow::anyhow!("ACP agent '{}' not found or not installed", provider_id)
        })?;
        Ok(Self { working_dir })
    }
}

#[async_trait]
impl AiProvider for AcpAiProvider {
    async fn prompt(&self, prompt: String) -> Result<String> {
        let driver =
            AcpDriver::first_available().map_err(|e| anyhow::anyhow!("No ACP agent found: {e}"))?;

        let working_dir = self.working_dir.clone();

        // Run the ACP session in a blocking task with its own runtime
        // because ACP uses !Send futures (LocalSet)
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("Failed to create runtime")?;

            let local = tokio::task::LocalSet::new();
            local.block_on(&rt, async move {
                let writer_impl = Arc::new(BasicMessageWriter::new());
                let writer = writer_impl.clone() as Arc<dyn MessageWriter>;
                let store = Arc::new(NoOpStore) as Arc<dyn Store>;
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
}
