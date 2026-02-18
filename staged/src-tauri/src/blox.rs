//! Blox CLI integration.
//!
//! Thin wrappers around shared `blox-cli` helpers so existing staged code can
//! keep using `crate::blox::*`.

pub use blox_cli::{BloxError, WorkspaceInfo, WorkspaceListEntry};

/// Check whether the `sq` CLI is available on this system.
pub fn is_sq_available() -> bool {
    blox_cli::is_sq_available()
}

/// Start a new Blox workspace.
///
/// Runs: `sq blox ws start <name> [<source>]`
///
/// Returns the workspace name on success.
pub fn ws_start(name: &str, source: Option<&str>) -> Result<String, BloxError> {
    blox_cli::ws_start(name, source)
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

/// Quick authentication check — runs `sq blox ws list` and inspects the result.
///
/// Returns `Ok(())` if the user appears to be authenticated, or
/// `Err(BloxError::NotAuthenticated)` if the CLI reports an auth failure.
pub fn check_auth() -> Result<(), BloxError> {
    blox_cli::check_auth()
}

// Phase 3: Pause/resume lifecycle — workspaces auto-suspend after idle;
// use `sq blox ws resume <name>` to bring them back. There is no explicit
// `sq blox ws stop` command. Deletion is a single `sq blox ws delete` call.
