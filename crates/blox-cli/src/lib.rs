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
    /// Numeric workstation ID used in proxy URLs.
    /// The CLI returns this as a string field called `workstation_id`.
    #[serde(default, deserialize_with = "deserialize_string_u64")]
    pub workstation_id: Option<u64>,
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
    #[serde(default, deserialize_with = "deserialize_string_u64")]
    pub workstation_id: Option<u64>,
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
        1 => "stopped".to_string(),
        2 => "starting".to_string(),
        3 => "running".to_string(),
        4 => "error".to_string(),
        5 => "shutting_down".to_string(),
        6 => "suspended".to_string(),
        7 => "deleted".to_string(),
        8 => "degraded".to_string(),
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

/// Deserialize a numeric ID that the CLI may return as a JSON string (e.g. `"21889"`).
fn deserialize_string_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let val = Option::<serde_json::Value>::deserialize(deserializer)?;
    match val {
        None => Ok(None),
        Some(serde_json::Value::Number(n)) => Ok(n.as_u64()),
        Some(serde_json::Value::String(s)) => Ok(s.parse::<u64>().ok()),
        _ => Ok(None),
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
pub fn acp_proxy_args(workspace_name: &str, command: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "blox".to_string(),
        "acp".to_string(),
        workspace_name.to_string(),
    ];

    if let Some(command) = command.map(str::trim).filter(|s| !s.is_empty()) {
        args.push(format!("--command={command}"));
    }

    args
}

/// Strip ANSI escape sequences (CSI and OSC) from a string so that
/// downstream string-matching (e.g. `is_auth_error`) is not confused by
/// terminal colour / style codes that `sq` may emit on stderr.
fn strip_ansi_escape_sequences(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            output.push(ch);
            continue;
        }

        match chars.peek().copied() {
            // CSI sequence: ESC [ … <final byte in @–~>
            Some('[') => {
                let _ = chars.next();
                for candidate in chars.by_ref() {
                    if ('@'..='~').contains(&candidate) {
                        break;
                    }
                }
            }
            // OSC sequence: ESC ] … (terminated by BEL or ST)
            Some(']') => {
                let _ = chars.next();
                let mut previous = '\0';
                for candidate in chars.by_ref() {
                    if candidate == '\u{0007}' {
                        break;
                    }
                    if previous == '\u{1b}' && candidate == '\\' {
                        break;
                    }
                    previous = candidate;
                }
            }
            _ => {}
        }
    }

    output
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

/// Run `sq blox <args…>` and return stdout as raw bytes.
fn run_bytes(args: &[&str], timeout: Duration) -> Result<Vec<u8>, BloxError> {
    let sq = sq_binary()?;

    let mut full_args = vec!["blox"];
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
        let stderr = strip_ansi_escape_sequences(&String::from_utf8_lossy(&stderr));
        if is_auth_error(&stderr) {
            return Err(BloxError::NotAuthenticated);
        }
        return Err(BloxError::CommandFailed(stderr));
    }

    Ok(stdout)
}

/// Run `sq blox <args…>` and return stdout as a string.
fn run(args: &[&str], timeout: Duration) -> Result<String, BloxError> {
    let bytes = run_bytes(args, timeout)?;
    String::from_utf8(bytes)
        .map_err(|e| BloxError::ParseError(format!("invalid UTF-8 in sq blox output: {e}")))
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
    let timeout_str = idle_timeout_minutes.map(|m| m.to_string());
    let mut args = vec!["ws", "start", name];
    if let Some(ref val) = timeout_str {
        args.push("--idle-timeout");
        args.push(val);
    }
    if let Some(src) = source {
        args.push(src);
    }
    let command_preview = {
        let mut parts = format!("sq blox ws start {name}");
        if let Some(ref val) = timeout_str {
            parts.push_str(&format!(" --idle-timeout {val}"));
        }
        if let Some(src) = source {
            parts.push_str(&format!(" {src}"));
        }
        parts
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

/// Resume a suspended Blox workspace.
///
/// Runs: `sq blox ws resume <name>`
pub fn ws_resume(name: &str) -> Result<(), BloxError> {
    run(&["ws", "resume", name], START_TIMEOUT)?;
    Ok(())
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

/// Execute a command inside a Blox workspace, returning raw bytes.
///
/// Like `ws_exec` but returns the raw stdout bytes without UTF-8 validation.
/// Use this when the command may produce binary output (e.g. `git show` on image files).
pub fn ws_exec_bytes(name: &str, args: &[&str]) -> Result<Vec<u8>, BloxError> {
    let mut full_args = vec!["ws", "exec", name, "--"];
    full_args.extend_from_slice(args);
    run_bytes(&full_args, EXEC_TIMEOUT)
}

/// A bootstrap command returned by `sq blox ws commands <name> --json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceCommand {
    pub command_id: String,
    pub command_type: u32,
    pub status: u32, // 1=pending, 2=running, 3=completed
    pub is_bootstrap: bool,
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Catch-all for any other fields the CLI returns.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// List bootstrap commands for a Blox workspace.
///
/// Runs: `sq blox ws commands <name> --json`
pub fn ws_commands(name: &str) -> Result<Vec<WorkspaceCommand>, BloxError> {
    let stdout = run(&["ws", "commands", name, "--json"], QUICK_TIMEOUT)?;
    serde_json::from_str(&stdout).map_err(|e| BloxError::ParseError(format!("{e}\nRaw: {stdout}")))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_plain_text_unchanged() {
        let input = "hello world";
        assert_eq!(strip_ansi_escape_sequences(input), "hello world");
    }

    #[test]
    fn strip_ansi_csi_sequences() {
        // Bold + red text with reset
        let input = "\x1b[1;31merror: not logged in\x1b[0m";
        assert_eq!(strip_ansi_escape_sequences(input), "error: not logged in");
    }

    #[test]
    fn strip_ansi_osc_sequence_bel_terminated() {
        // OSC to set window title, terminated by BEL (\x07)
        let input = "\x1b]0;my title\x07some text";
        assert_eq!(strip_ansi_escape_sequences(input), "some text");
    }

    #[test]
    fn strip_ansi_osc_sequence_st_terminated() {
        // OSC terminated by ST (ESC \)
        let input = "\x1b]0;my title\x1b\\some text";
        assert_eq!(strip_ansi_escape_sequences(input), "some text");
    }

    #[test]
    fn strip_ansi_mixed_sequences() {
        let input = "\x1b[31mError:\x1b[0m \x1b[1mnot authenticated\x1b[0m";
        assert_eq!(
            strip_ansi_escape_sequences(input),
            "Error: not authenticated"
        );
    }

    #[test]
    fn strip_ansi_empty_string() {
        assert_eq!(strip_ansi_escape_sequences(""), "");
    }

    #[test]
    fn strip_ansi_preserves_non_escape_special_chars() {
        let input = "line1\nline2\ttab";
        assert_eq!(strip_ansi_escape_sequences(input), "line1\nline2\ttab");
    }
}
