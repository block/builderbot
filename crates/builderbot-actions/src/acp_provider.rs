//! ACP AI provider implementation for action detection
//!
//! This module provides a concrete implementation of the `AiProvider` trait
//! using the acp-client crate to communicate with ACP agents.

use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;

use crate::detector::AiProvider;

/// AI provider that uses ACP agents (like Goose or Claude Code)
pub struct AcpAiProvider {
    working_dir: PathBuf,
    provider_id: Option<String>,
    interpreter_env_snapshot: Option<Vec<(String, String)>>,
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
        acp_client::find_acp_agent().ok_or_else(|| {
            anyhow::anyhow!(
                "No ACP agent found (tried {})",
                acp_client::known_agent_commands().join(", ")
            )
        })?;
        Ok(Self {
            working_dir,
            provider_id: None,
            interpreter_env_snapshot: None,
        })
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
        Ok(Self {
            working_dir,
            provider_id: Some(provider_id.to_string()),
            interpreter_env_snapshot: None,
        })
    }

    /// Set a separate environment snapshot used only to resolve env-shebang
    /// interpreters for ACP bridge startup.
    pub fn with_interpreter_env_snapshot(mut self, vars: Vec<(String, String)>) -> Self {
        self.interpreter_env_snapshot = Some(vars);
        self
    }
}

#[async_trait]
impl AiProvider for AcpAiProvider {
    async fn prompt(&self, prompt: String) -> Result<String> {
        let agent = match &self.provider_id {
            Some(id) => acp_client::find_acp_agent_by_id(id),
            None => acp_client::find_acp_agent(),
        }
        .ok_or_else(|| anyhow::anyhow!("No ACP agent found"))?;

        match &self.interpreter_env_snapshot {
            Some(snapshot) => {
                acp_client::run_acp_prompt_with_interpreter_env_snapshot(
                    &agent,
                    &self.working_dir,
                    &prompt,
                    snapshot.clone(),
                )
                .await
            }
            None => acp_client::run_acp_prompt(&agent, &self.working_dir, &prompt).await,
        }
    }
}
