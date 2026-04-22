//! Blox CLI integration.
//!
//! Thin wrappers around shared `blox-cli` helpers so existing Staged code can
//! keep using `crate::blox::*`.

use std::sync::OnceLock;

pub use blox_cli::{BloxError, WorkspaceCommand, WorkspaceInfo, WorkspaceListEntry};

static SQ_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Check whether the `sq` CLI is available on this system.
///
/// The result is cached for the lifetime of the process since the PATH
/// won't change mid-session.
pub fn is_sq_available() -> bool {
    *SQ_AVAILABLE.get_or_init(|| blox_cli::is_sq_available())
}

/// Start a new Blox workspace.
///
/// Runs: `sq blox ws start <name> [--idle-timeout <minutes>] [<source>]`
///
/// Returns the workspace name on success.
pub fn ws_start(
    name: &str,
    source: Option<&str>,
    idle_timeout_minutes: Option<u32>,
) -> Result<String, BloxError> {
    blox_cli::ws_start(name, source, idle_timeout_minutes)
}

/// Delete a Blox workspace.
///
/// Runs: `sq blox ws delete <name>`
pub fn ws_delete(name: &str) -> Result<(), BloxError> {
    blox_cli::ws_delete(name)
}

/// Get info about a Blox workspace.
///
/// Runs: `sq blox ws info <name> --json`
pub fn ws_info(name: &str) -> Result<WorkspaceInfo, BloxError> {
    blox_cli::ws_info(name)
}

/// List all Blox workspaces.
///
/// Runs: `sq blox ws list --json`
pub fn ws_list() -> Result<Vec<WorkspaceListEntry>, BloxError> {
    blox_cli::ws_list()
}

/// Execute a command inside a Blox workspace.
///
/// Runs: `sq blox ws exec <name> -- <args…>`
///
/// Returns the command's stdout on success.
pub fn ws_exec(name: &str, args: &[&str]) -> Result<String, BloxError> {
    blox_cli::ws_exec(name, args)
}

/// Execute a command inside a Blox workspace, returning raw bytes.
///
/// Like `ws_exec` but returns the raw stdout bytes without UTF-8 validation.
/// Use this when the command may produce binary output (e.g. `git show` on image files).
pub fn ws_exec_bytes(name: &str, args: &[&str]) -> Result<Vec<u8>, BloxError> {
    blox_cli::ws_exec_bytes(name, args)
}

/// Quick authentication check — runs `sq blox ws list` and inspects the result.
///
/// Returns `Ok(())` if the user appears to be authenticated, or
/// `Err(BloxError::NotAuthenticated)` if the CLI reports an auth failure.
pub fn check_auth() -> Result<(), BloxError> {
    blox_cli::check_auth()
}

/// List bootstrap commands for a Blox workspace.
///
/// Runs: `sq blox ws commands <name> --json`
pub fn ws_commands(name: &str) -> Result<Vec<WorkspaceCommand>, BloxError> {
    blox_cli::ws_commands(name)
}

/// Resume a suspended Blox workspace.
///
/// Runs: `sq blox ws resume <name>`
pub fn ws_resume(name: &str) -> Result<(), BloxError> {
    blox_cli::ws_resume(name)
}
