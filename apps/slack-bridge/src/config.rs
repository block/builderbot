//! Runtime configuration, sourced entirely from environment variables.
//!
//! No secrets are ever hard-coded or written to disk. See `.env.example` and
//! the README for the full list and Slack app setup.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Environment variable names, in one place.
pub mod env {
    pub const APP_TOKEN: &str = "SLACK_APP_TOKEN";
    pub const BOT_TOKEN: &str = "SLACK_BOT_TOKEN";
    pub const SIGNING_SECRET: &str = "SLACK_SIGNING_SECRET";
    pub const AGENT: &str = "BUILDERBOT_AGENT";
    pub const WORKDIR: &str = "BUILDERBOT_WORKDIR";
}

/// Validated configuration for a live Socket Mode session.
#[derive(Debug, Clone)]
pub struct Config {
    /// App-level token (`xapp-…`) used to open the Socket Mode connection.
    pub app_token: String,
    /// Bot token (`xoxb-…`) used for Web API calls.
    pub bot_token: String,
    /// ACP provider id (e.g. `codex`, `claude`).
    pub agent: String,
    /// Working directory the agent runs in (defaults to the current dir).
    pub workdir: PathBuf,
}

/// Default agent when `BUILDERBOT_AGENT` is unset.
pub const DEFAULT_AGENT: &str = "codex";

impl Config {
    /// Load and validate configuration from the process environment.
    pub fn from_env() -> Result<Self> {
        let app_token = require(env::APP_TOKEN)?;
        let bot_token = require(env::BOT_TOKEN)?;

        if !app_token.starts_with("xapp-") {
            anyhow::bail!(
                "{} must be an app-level token starting with 'xapp-'",
                env::APP_TOKEN
            );
        }
        if !bot_token.starts_with("xoxb-") {
            anyhow::bail!(
                "{} must be a bot token starting with 'xoxb-'",
                env::BOT_TOKEN
            );
        }

        Ok(Self {
            app_token,
            bot_token,
            agent: agent_from_env(),
            workdir: workdir_from_env()?,
        })
    }
}

/// Resolve the agent id from the environment, falling back to [`DEFAULT_AGENT`].
pub fn agent_from_env() -> String {
    std::env::var(env::AGENT)
        .ok()
        .map(|v| normalize_agent(&v))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_AGENT.to_string())
}

/// Resolve the working directory, defaulting to the current directory.
pub fn workdir_from_env() -> Result<PathBuf> {
    match std::env::var(env::WORKDIR) {
        Ok(v) if !v.trim().is_empty() => Ok(PathBuf::from(v)),
        _ => std::env::current_dir().context("failed to resolve current directory"),
    }
}

/// Normalize an agent value so the `-acp` binary suffix and surrounding
/// whitespace are accepted (e.g. `codex-acp` → `codex`), matching the provider
/// ids in `acp-client`.
pub fn normalize_agent(value: &str) -> String {
    let v = value.trim();
    v.strip_suffix("-acp").unwrap_or(v).to_string()
}

fn require(key: &str) -> Result<String> {
    let value = std::env::var(key)
        .with_context(|| format!("missing required environment variable {key}"))?;
    if value.trim().is_empty() {
        anyhow::bail!("environment variable {key} is set but empty");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_acp_suffix() {
        assert_eq!(normalize_agent("codex-acp"), "codex");
        assert_eq!(normalize_agent("  claude  "), "claude");
        assert_eq!(normalize_agent("goose"), "goose");
    }
}
