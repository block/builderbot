//! Blox CLI integration.
//!
//! Thin wrappers around shared `blox-cli` helpers so existing Staged code can
//! keep using `crate::blox::*`.

use std::sync::OnceLock;

pub use blox_cli::{BloxError, WorkspaceCommand, WorkspaceInfo, WorkspaceListEntry, WsExecOutput};

static SQ_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Every workspace the `ws_exec*` wrappers were called for in this test
/// process, so a test can prove a code path never reached its workspace
/// without faking `sq`. Tests name a workspace no other test uses.
#[cfg(test)]
static WS_EXEC_TARGETS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn record_ws_exec(name: &str) {
    WS_EXEC_TARGETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(name.to_string());
}

/// How many `ws_exec*` calls have targeted `name` in this test process.
#[cfg(test)]
pub(crate) fn ws_exec_calls_for(name: &str) -> usize {
    WS_EXEC_TARGETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .filter(|target| *target == name)
        .count()
}

/// Check whether the `sq` CLI is available on this system.
///
/// The result is cached for the lifetime of the process since the PATH
/// won't change mid-session.
pub fn is_sq_available() -> bool {
    *SQ_AVAILABLE.get_or_init(blox_cli::is_sq_available)
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
    let info = blox_cli::ws_info(name);
    if matches!(&info, Ok(info) if info.status.as_deref() == Some("running")) {
        crate::session_runner::reconcile::note_workspace_reachable(name);
    }
    info
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
    #[cfg(test)]
    record_ws_exec(name);
    reachable_on_success(name, blox_cli::ws_exec(name, args))
}

/// Execute a command inside a Blox workspace, returning raw bytes.
///
/// Like `ws_exec` but returns the raw stdout bytes without UTF-8 validation.
/// Use this when the command may produce binary output (e.g. `git show` on image files).
pub fn ws_exec_bytes(name: &str, args: &[&str]) -> Result<Vec<u8>, BloxError> {
    #[cfg(test)]
    record_ws_exec(name);
    reachable_on_success(name, blox_cli::ws_exec_bytes(name, args))
}

/// Execute a command inside a Blox workspace, returning full output regardless
/// of exit code. Infrastructure errors still return `Err`.
pub fn ws_exec_output(name: &str, args: &[&str]) -> Result<WsExecOutput, BloxError> {
    #[cfg(test)]
    record_ws_exec(name);
    reachable_on_success(name, blox_cli::ws_exec_output(name, args))
}

/// A call that reached the workstation proves it reachable: `disconnected`
/// sessions on it that failed to reconcile are worth another try (see
/// [`crate::session_runner::reconcile::note_workspace_reachable`]).
fn reachable_on_success<T>(name: &str, result: Result<T, BloxError>) -> Result<T, BloxError> {
    if result.is_ok() {
        crate::session_runner::reconcile::note_workspace_reachable(name);
    }
    result
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
