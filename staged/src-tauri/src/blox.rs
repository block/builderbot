//! Blox CLI integration.
//!
//! Thin wrappers around `blox ws` subcommands for managing remote
//! workspaces. Each function shells out to the `blox` CLI and parses
//! the result.

use serde::{Deserialize, Deserializer, Serialize};
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

// Phase 3: Pause/resume lifecycle — workspaces auto-suspend after idle;
// use `blox ws resume <name>` to bring them back. There is no explicit
// `blox ws stop` command. Deletion is a single `blox ws delete` call.
