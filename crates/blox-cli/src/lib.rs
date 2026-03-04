//! Shared Blox CLI integration.
//!
//! Thin wrappers around `sq blox` subcommands plus common command discovery.

use serde::{Deserialize, Deserializer, Serialize};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use thiserror::Error;
use wait_timeout::ChildExt;

const COMMON_PATHS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/home/linuxbrew/.linuxbrew/bin",
];

const QUICK_TIMEOUT: Duration = Duration::from_secs(20);
const START_TIMEOUT: Duration = Duration::from_secs(120);
const DELETE_TIMEOUT: Duration = Duration::from_secs(60);
const EXEC_TIMEOUT: Duration = Duration::from_secs(300);

/// Structured errors from Blox CLI operations.
#[derive(Error, Debug)]
pub enum BloxError {
    #[error("sq CLI not found — is sq installed and on your PATH?")]
    NotFound,

    #[error("Not authenticated with Blox. Run: sq login")]
    NotAuthenticated,

    #[error("sq blox command timed out after {0}s")]
    Timeout(u64),

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

/// Find a CLI binary by command name.
///
/// Searches in order:
/// 1. Login shell `which` (picks up user's PATH from shell rc files)
/// 2. Common install locations
pub fn find_command(cmd: &str) -> Option<PathBuf> {
    if let Some(path) = find_via_login_shell(cmd) {
        if path.exists() {
            return Some(path);
        }
    }

    for dir in COMMON_PATHS {
        let path = PathBuf::from(dir).join(cmd);
        if path.exists() {
            return Some(path);
        }
    }

    None
}

fn find_via_login_shell(cmd: &str) -> Option<PathBuf> {
    let which_cmd = format!("which {cmd}");

    for shell in ["/bin/zsh", "/bin/bash"] {
        if let Ok(output) = Command::new(shell).args(["-l", "-c", &which_cmd]).output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(path_str) = stdout.lines().rfind(|line| !line.is_empty()) {
                    let path_str = path_str.trim();
                    if !path_str.is_empty() && path_str.starts_with('/') {
                        return Some(PathBuf::from(path_str));
                    }
                }
            }
        }
    }

    None
}

/// Locate the `sq` binary.
pub fn find_sq_binary() -> Option<PathBuf> {
    find_command("sq")
}

/// Locate the `sq` binary, returning `BloxError::NotFound` if unavailable.
pub fn sq_binary() -> Result<PathBuf, BloxError> {
    find_sq_binary().ok_or(BloxError::NotFound)
}

/// Check whether the `sq` CLI is available on this system.
pub fn is_sq_available() -> bool {
    find_sq_binary().is_some()
}

/// Build args for `sq blox acp <workspace_name> [--command=...]`.
///
/// When the `BLOX_ENV` environment variable is set, `--env <value>` is
/// inserted after `blox` so that ACP proxy connections target the specified
/// environment.
pub fn acp_proxy_args(workspace_name: &str, command: Option<&str>) -> Vec<String> {
    let mut args = vec!["blox".to_string()];
    if let Ok(env) = std::env::var("BLOX_ENV") {
        args.push("--env".to_string());
        args.push(env);
    }
    args.push("acp".to_string());
    args.push(workspace_name.to_string());

    if let Some(command) = command.map(str::trim).filter(|s| !s.is_empty()) {
        args.push(format!("--command={command}"));
    }

    args
}

/// Heuristic: does the CLI stderr look like an authentication / login error?
fn is_auth_error(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("not logged in")
        || lower.contains("not authenticated")
        || lower.contains("unauthenticated")
        || lower.contains("login required")
        || lower.contains("session expired")
        || lower.contains("token expired")
        || lower.contains("unauthorized")
        || lower.contains("401")
}

/// Run `sq blox <args…>` and return stdout as a string.
///
/// When the `BLOX_ENV` environment variable is set, `--env <value>` is
/// automatically inserted after `blox` so that all commands target the
/// specified environment.
fn run(args: &[&str], timeout: Duration) -> Result<String, BloxError> {
    let sq = sq_binary()?;

    let mut full_args = vec!["blox"];
    let env_value = std::env::var("BLOX_ENV").ok();
    if let Some(ref env) = env_value {
        full_args.push("--env");
        full_args.push(env);
    }
    full_args.extend_from_slice(args);

    let mut child = Command::new(&sq)
        .args(&full_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BloxError::CommandFailed(e.to_string()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| BloxError::CommandFailed("Failed to capture stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| BloxError::CommandFailed("Failed to capture stderr".to_string()))?;

    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut reader = stdout;
        let _ = reader.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut reader = stderr;
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    let status = match child
        .wait_timeout(timeout)
        .map_err(|e| BloxError::CommandFailed(e.to_string()))?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(BloxError::Timeout(timeout.as_secs()));
        }
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        if is_auth_error(&stderr) {
            return Err(BloxError::NotAuthenticated);
        }
        return Err(BloxError::CommandFailed(stderr.into_owned()));
    }

    String::from_utf8(stdout)
        .map_err(|e| BloxError::ParseError(format!("invalid UTF-8 in sq blox output: {e}")))
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
    let command_preview = match source {
        Some(src) => format!("sq blox ws start {name} {src}"),
        None => format!("sq blox ws start {name}"),
    };
    log::info!(
        "[blox-cli] workspace start begin: workspace={} command=\"{}\"",
        name,
        command_preview
    );
    let started_at = Instant::now();
    let result = run(&args, START_TIMEOUT);
    match &result {
        Ok(_) => {
            log::info!(
                "[blox-cli] workspace start complete: workspace={} elapsed_ms={} command=\"{}\"",
                name,
                started_at.elapsed().as_millis(),
                command_preview
            );
        }
        Err(e) => {
            log::warn!(
                "[blox-cli] workspace start failed: workspace={} elapsed_ms={} command=\"{}\" error={}",
                name,
                started_at.elapsed().as_millis(),
                command_preview,
                e
            );
        }
    }
    result?;
    Ok(name.to_string())
}

/// Delete a Blox workspace.
///
/// Runs: `sq blox ws delete <name>`
pub fn ws_delete(name: &str) -> Result<(), BloxError> {
    run(&["ws", "delete", name], DELETE_TIMEOUT)?;
    Ok(())
}

/// Get info about a Blox workspace.
///
/// Runs: `sq blox ws info <name> --json`
pub fn ws_info(name: &str) -> Result<WorkspaceInfo, BloxError> {
    let stdout = run(&["ws", "info", name, "--json"], QUICK_TIMEOUT)?;
    serde_json::from_str(&stdout).map_err(|e| BloxError::ParseError(format!("{e}\nRaw: {stdout}")))
}

/// List all Blox workspaces.
///
/// Runs: `sq blox ws list --json`
pub fn ws_list() -> Result<Vec<WorkspaceListEntry>, BloxError> {
    let stdout = run(&["ws", "list", "--json"], QUICK_TIMEOUT)?;
    serde_json::from_str(&stdout).map_err(|e| BloxError::ParseError(format!("{e}\nRaw: {stdout}")))
}

/// Execute a command inside a Blox workspace.
///
/// Runs: `sq blox ws exec <name> -- <args…>`
///
/// Returns the command's stdout on success.
pub fn ws_exec(name: &str, args: &[&str]) -> Result<String, BloxError> {
    let mut full_args = vec!["ws", "exec", name, "--"];
    full_args.extend_from_slice(args);
    run(&full_args, EXEC_TIMEOUT)
}

/// Quick authentication check — runs `sq blox ws list` and inspects the result.
///
/// Returns `Ok(())` if the user appears to be authenticated, or
/// `Err(BloxError::NotAuthenticated)` if the CLI reports an auth failure.
pub fn check_auth() -> Result<(), BloxError> {
    match run(&["ws", "list"], QUICK_TIMEOUT) {
        Ok(_) => Ok(()),
        Err(BloxError::NotAuthenticated) => Err(BloxError::NotAuthenticated),
        // Any other error (e.g. network timeout) — not necessarily an auth issue,
        // so let it through and let the caller decide.
        Err(_) => Ok(()),
    }
}
