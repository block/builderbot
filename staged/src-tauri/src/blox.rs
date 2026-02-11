//! Blox CLI integration.
//!
//! Thin wrappers around `blox ws` subcommands for managing remote
//! workspaces. Each function shells out to the `blox` CLI and parses
//! the result.

use serde::{Deserialize, Serialize};
use std::process::Command;

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

/// Start a new Blox workspace.
///
/// Runs: `blox ws start <name> [<source>]`
///
/// Returns the workspace name on success.
pub fn ws_start(name: &str, source: Option<&str>) -> Result<String, String> {
    let mut cmd = Command::new("blox");
    cmd.args(["ws", "start", name]);
    if let Some(src) = source {
        cmd.arg(src);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run `blox ws start`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("blox ws start failed: {stderr}"));
    }

    Ok(name.to_string())
}

/// Delete a Blox workspace.
///
/// Runs: `blox ws delete <name>`
pub fn ws_delete(name: &str) -> Result<(), String> {
    let output = Command::new("blox")
        .args(["ws", "delete", name])
        .output()
        .map_err(|e| format!("Failed to run `blox ws delete`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("blox ws delete failed: {stderr}"));
    }

    Ok(())
}

/// Get info about a Blox workspace.
///
/// Runs: `blox ws info <name> --json`
pub fn ws_info(name: &str) -> Result<WorkspaceInfo, String> {
    let output = Command::new("blox")
        .args(["ws", "info", name, "--json"])
        .output()
        .map_err(|e| format!("Failed to run `blox ws info`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("blox ws info failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout)
        .map_err(|e| format!("Failed to parse blox ws info output: {e}\nRaw: {stdout}"))
}

/// List all Blox workspaces.
///
/// Runs: `blox ws list --json`
pub fn ws_list() -> Result<Vec<WorkspaceListEntry>, String> {
    let output = Command::new("blox")
        .args(["ws", "list", "--json"])
        .output()
        .map_err(|e| format!("Failed to run `blox ws list`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("blox ws list failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout)
        .map_err(|e| format!("Failed to parse blox ws list output: {e}\nRaw: {stdout}"))
}

/// Send a prompt to a running Blox workspace.
///
/// Runs: `blox ws prompt <name> <prompt>`
pub fn ws_prompt(name: &str, prompt: &str) -> Result<String, String> {
    let output = Command::new("blox")
        .args(["ws", "prompt", name, prompt])
        .output()
        .map_err(|e| format!("Failed to run `blox ws prompt`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("blox ws prompt failed: {stderr}"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
