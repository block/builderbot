//! Blox CLI integration.
//!
//! Thin wrappers around `blox ws` subcommands for managing remote
//! workspaces. Each function shells out to the `blox` CLI and parses
//! the result.

use serde::{Deserialize, Serialize};
use std::process::Command;
use thiserror::Error;

/// Structured errors from Blox CLI operations, mirroring `GitError`.
#[derive(Error, Debug)]
pub enum BloxError {
    #[error("blox CLI not found — is blox installed?")]
    NotFound,

    #[error("blox command failed: {0}")]
    CommandFailed(String),

    #[error("failed to parse blox output: {0}")]
    ParseError(String),
}

/// Information about a Blox workspace, as returned by `blox ws info --json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
    /// Catch-all for any other fields the CLI returns.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Summary entry from `blox ws list --json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceListEntry {
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Run a blox command and return stdout as a string.
fn run(args: &[&str]) -> Result<String, BloxError> {
    let output = Command::new("blox").args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            BloxError::NotFound
        } else {
            BloxError::CommandFailed(e.to_string())
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BloxError::CommandFailed(stderr.into_owned()));
    }

    String::from_utf8(output.stdout)
        .map_err(|e| BloxError::ParseError(format!("invalid UTF-8 in blox output: {e}")))
}

/// Start a new Blox workspace.
///
/// Runs: `blox ws start <name> [<source>]`
///
/// Returns the workspace name on success.
pub fn ws_start(name: &str, source: Option<&str>) -> Result<String, BloxError> {
    let mut args = vec!["ws", "start", name];
    if let Some(src) = source {
        args.push(src);
    }
    run(&args)?;
    Ok(name.to_string())
}

/// Delete a Blox workspace.
///
/// Runs: `blox ws delete <name>`
pub fn ws_delete(name: &str) -> Result<(), BloxError> {
    run(&["ws", "delete", name])?;
    Ok(())
}

/// Get info about a Blox workspace.
///
/// Runs: `blox ws info <name> --json`
pub fn ws_info(name: &str) -> Result<WorkspaceInfo, BloxError> {
    let stdout = run(&["ws", "info", name, "--json"])?;
    serde_json::from_str(&stdout).map_err(|e| BloxError::ParseError(format!("{e}\nRaw: {stdout}")))
}

/// List all Blox workspaces.
///
/// Runs: `blox ws list --json`
pub fn ws_list() -> Result<Vec<WorkspaceListEntry>, BloxError> {
    let stdout = run(&["ws", "list", "--json"])?;
    serde_json::from_str(&stdout).map_err(|e| BloxError::ParseError(format!("{e}\nRaw: {stdout}")))
}

// Phase 2: Agent interaction — will be wired to a Tauri command once
// the frontend has a prompt UI for remote branches.
/// Send a prompt to a running Blox workspace.
///
/// Runs: `blox ws prompt <name> <prompt>`
pub fn ws_prompt(name: &str, prompt: &str) -> Result<String, BloxError> {
    run(&["ws", "prompt", name, prompt])
}

// Phase 3: Pause/resume lifecycle — `blox ws stop` will be needed here
// to support stopping workspaces without destroying them. The original
// design note also describes deletion as a two-step stop+rm flow.
