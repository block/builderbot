//! Blox CLI integration.
//!
//! Thin wrappers around `sq blox ws` subcommands for managing remote
//! workspaces. Each function shells out to the `sq` CLI and parses
//! the result.

use acp_client::find_command;
use serde::{Deserialize, Deserializer, Serialize};
use std::process::Command;
use thiserror::Error;

/// Structured errors from Blox CLI operations, mirroring `GitError`.
#[derive(Error, Debug)]
pub enum BloxError {
    #[error("sq CLI not found — is sq installed and on your PATH?")]
    NotFound,

    #[error("sq blox command failed: {0}")]
    CommandFailed(String),

    #[error("failed to parse sq blox output: {0}")]
    ParseError(String),
}

/// Information about a Blox workspace, as returned by `blox ws info --json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_status")]
    pub status: Option<String>,
    /// Catch-all for any other fields the CLI returns.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Summary entry from `blox ws list --json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceListEntry {
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_status")]
    pub status: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// Blox returns status as an integer enum. Map known codes to strings
/// that match our `WorkspaceStatus` values; fall back to the raw number.
fn status_code_to_string(code: u64) -> String {
    match code {
        0 => "unknown".to_string(),
        1 => "starting".to_string(),
        2 => "stopped".to_string(),
        3 => "running".to_string(),
        4 => "error".to_string(),
        other => format!("unknown({other})"),
    }
}

/// Deserialize status from either a string or an integer.
fn deserialize_status<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let val = Option::<serde_json::Value>::deserialize(deserializer)?;
    match val {
        None => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s)),
        Some(serde_json::Value::Number(n)) => {
            Ok(Some(status_code_to_string(n.as_u64().unwrap_or(0))))
        }
        Some(other) => Ok(Some(other.to_string())),
    }
}

/// Locate the `sq` binary, returning its path or `BloxError::NotFound`.
fn sq_binary() -> Result<std::path::PathBuf, BloxError> {
    find_command("sq").ok_or(BloxError::NotFound)
}

/// Run `sq blox <args…>` and return stdout as a string.
fn run(args: &[&str]) -> Result<String, BloxError> {
    let sq = sq_binary()?;

    let mut full_args = vec!["blox"];
    full_args.extend_from_slice(args);

    let output = Command::new(&sq)
        .args(&full_args)
        .output()
        .map_err(|e| BloxError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BloxError::CommandFailed(stderr.into_owned()));
    }

    String::from_utf8(output.stdout)
        .map_err(|e| BloxError::ParseError(format!("invalid UTF-8 in sq blox output: {e}")))
}

/// Check whether the `sq` CLI is available on this system.
pub fn is_sq_available() -> bool {
    find_command("sq").is_some()
}

/// Start a new Blox workspace.
///
/// Runs: `sq blox ws start <name> [<source>]`
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
/// Runs: `sq blox ws delete <name>`
pub fn ws_delete(name: &str) -> Result<(), BloxError> {
    run(&["ws", "delete", name])?;
    Ok(())
}

/// Get info about a Blox workspace.
///
/// Runs: `sq blox ws info <name> --json`
pub fn ws_info(name: &str) -> Result<WorkspaceInfo, BloxError> {
    let stdout = run(&["ws", "info", name, "--json"])?;
    serde_json::from_str(&stdout).map_err(|e| BloxError::ParseError(format!("{e}\nRaw: {stdout}")))
}

/// List all Blox workspaces.
///
/// Runs: `sq blox ws list --json`
pub fn ws_list() -> Result<Vec<WorkspaceListEntry>, BloxError> {
    let stdout = run(&["ws", "list", "--json"])?;
    serde_json::from_str(&stdout).map_err(|e| BloxError::ParseError(format!("{e}\nRaw: {stdout}")))
}

/// Execute a command inside a Blox workspace.
///
/// Runs: `sq blox ws exec <name> -- <args…>`
///
/// Returns the command's stdout on success. Useful for running git commands
/// against a remote workspace (e.g. `ws_exec(name, &["git", "rev-parse", "HEAD"])`).
pub fn ws_exec(name: &str, args: &[&str]) -> Result<String, BloxError> {
    let mut full_args = vec!["ws", "exec", name, "--"];
    full_args.extend_from_slice(args);
    run(&full_args)
}

// Phase 3: Pause/resume lifecycle — workspaces auto-suspend after idle;
// use `sq blox ws resume <name>` to bring them back. There is no explicit
// `sq blox ws stop` command. Deletion is a single `sq blox ws delete` call.
