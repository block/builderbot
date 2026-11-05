//! Shared Blox CLI integration.
//!
//! Thin wrappers around `sq blox` subcommands plus common command discovery,
//! and file transfer to and from a workstation (see [`ws_put_file`]).

use serde::{Deserialize, Deserializer, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use thiserror::Error;
use wait_timeout::ChildExt;

mod commands;
mod transfer;
mod workspace_commands;

pub use commands::{
    find_acp_process, find_ended_acp_process, ws_command_list, ws_output, CommandFilter,
    CommandStatus, WorkstationCommand,
};
pub use transfer::{
    download_in_parts, get_file_with, put_file_with, transfer_timeout, upload_in_parts,
    ws_download, ws_download_args, ws_get_file, ws_put_file, ws_ssh_args, ws_ssh_get, ws_ssh_put,
    ws_upload, ws_upload_args, FileRoute, SqWorkspace, WsTransport, MAX_PART_BYTES,
};
pub use workspace_commands::{ws_commands, CommandType, WorkspaceCommand, WorkspaceCommandStatus};

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

/// Slowest rate a `ws exec` whose work grows with its payload may go at
/// before it is treated as hung, as for file transfers.
const EXEC_MIN_BYTES_PER_SEC: u64 = 1_000_000;

/// How long a `ws exec` whose work grows with `bytes` (hashing a working tree,
/// writing or indexing a bundle, cloning) may run: the usual 300 s, plus one
/// second per megabyte.
pub fn exec_timeout(bytes: u64) -> Duration {
    EXEC_TIMEOUT + Duration::from_secs(bytes / EXEC_MIN_BYTES_PER_SEC)
}

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
/// 1. Login shell path lookup (picks up user's PATH from shell rc files)
/// 2. Common install locations
pub fn find_command(cmd: &str) -> Option<PathBuf> {
    if let Some(path) = find_via_login_shell(cmd) {
        return Some(path);
    }

    for dir in COMMON_PATHS {
        let path = PathBuf::from(dir).join(cmd);
        if is_executable_file(&path) {
            return Some(path);
        }
    }

    None
}

fn find_via_login_shell(cmd: &str) -> Option<PathBuf> {
    let quoted = shell_quote(cmd);
    let lookups = [
        ("/bin/zsh", format!("whence -p -- {quoted}")),
        ("/bin/bash", format!("type -P -- {quoted}")),
    ];

    for (shell, lookup_cmd) in lookups {
        let Ok(output) = Command::new(shell).args(["-l", "-c", &lookup_cmd]).output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(path) = candidate_paths_from_shell_output(stdout.as_ref())
            .filter(|path| is_executable_file(path))
            .last()
        {
            return Some(path);
        }
    }

    None
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn candidate_paths_from_shell_output(output: &str) -> impl Iterator<Item = PathBuf> + '_ {
    output
        .lines()
        .map(str::trim)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
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

/// Build args for `sq blox acp <workspace_name> [--command=...] [--env-vars=...]`.
///
/// `env_vars` are set in the *remote* ACP process's environment, not the
/// local proxy's: `sq blox acp` takes them as one `--env-vars=KEY=VALUE;...`
/// flag. Names and values must not contain `;` (the flag's separator); a pair
/// that does is skipped rather than silently splitting into two variables.
pub fn acp_proxy_args(
    workspace_name: &str,
    command: Option<&str>,
    env_vars: &[(String, String)],
) -> Vec<String> {
    let mut args = vec![
        "blox".to_string(),
        "acp".to_string(),
        workspace_name.to_string(),
    ];

    if let Some(command) = command.map(str::trim).filter(|s| !s.is_empty()) {
        args.push(format!("--command={command}"));
    }

    let env_pairs: Vec<String> = env_vars
        .iter()
        .filter(|(name, value)| {
            !name.is_empty() && !name.contains(';') && !name.contains('=') && !value.contains(';')
        })
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    if !env_pairs.is_empty() {
        args.push(format!("--env-vars={}", env_pairs.join(";")));
    }

    args
}

/// Strip ANSI escape sequences (CSI and OSC) from a string so that
/// downstream string-matching (e.g. `is_auth_error`) is not confused by
/// terminal colour / style codes that `sq` may emit on stderr.
pub fn strip_ansi_escape_sequences(input: &str) -> String {
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

/// Run `sq blox <args…>` and return stdout, stderr, and exit status.
///
/// Unlike `run_bytes`, a non-zero exit from the remote command does NOT
/// produce an `Err` — only infrastructure failures (CLI missing, timeout,
/// auth) do. This lets callers inspect both streams on command failure.
fn run_bytes_full(args: &[&str], timeout: Duration) -> Result<WsExecOutput, BloxError> {
    let sq = sq_binary()?;

    let mut full_args = vec!["blox"];
    full_args.extend_from_slice(args);

    let mut child = Command::new(&sq)
        .args(&full_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BloxError::CommandFailed(e.to_string()))?;

    let stdout_handle = child
        .stdout
        .take()
        .ok_or_else(|| BloxError::CommandFailed("Failed to capture stdout".to_string()))?;
    let stderr_handle = child
        .stderr
        .take()
        .ok_or_else(|| BloxError::CommandFailed("Failed to capture stderr".to_string()))?;

    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut reader = stdout_handle;
        let _ = reader.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut reader = stderr_handle;
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

    // Check for auth errors even in the full-output path.
    if !status.success() {
        let stderr_str = strip_ansi_escape_sequences(&String::from_utf8_lossy(&stderr));
        if is_auth_error(&stderr_str) {
            return Err(BloxError::NotAuthenticated);
        }
    }

    Ok(WsExecOutput {
        stdout,
        stderr,
        success: status.success(),
    })
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

/// Whether Blox records a `ws exec` call in the workstation's command history.
///
/// Every exec must pick one, so the choice is made deliberately at each call
/// site rather than inherited from a default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// `ws exec --transient`: never written to `ws commands` or the Blox UI.
    ///
    /// Transient execution has hard limits, measured against `sq blox` 1.16:
    /// - the server kills the command after 60 s;
    /// - stdout must be valid UTF-8 (`ws_exec_bytes` and `ws_exec_output`
    ///   base64-encode it on the workstation so binary output still works);
    /// - output is buffered until exit, never streamed;
    /// - it fails immediately when the workstation is disconnected.
    ///
    /// Use it for Staged's own plumbing: quick reads and writes the user did
    /// not write themselves.
    Transient,
    /// Plain `ws exec`: recorded in the workstation's command history.
    ///
    /// Required for anything that can run past 60 s or streams its output, and
    /// for commands the user wrote (repo actions).
    Tracked,
}

/// `ws exec [--transient] <name> -- <args…>`, without the leading `blox`.
fn ws_exec_argv<'a>(name: &'a str, mode: ExecMode, args: &[&'a str]) -> Vec<&'a str> {
    let mut argv = vec!["ws", "exec"];
    if mode == ExecMode::Transient {
        argv.push("--transient");
    }
    argv.push(name);
    argv.push("--");
    argv.extend_from_slice(args);
    argv
}

/// The full `sq` argv (starting with `blox`) for running `args` inside a
/// workspace, for callers that spawn `sq` themselves (e.g. to stream output).
pub fn ws_exec_command(name: &str, mode: ExecMode, args: &[&str]) -> Vec<String> {
    std::iter::once("blox")
        .chain(ws_exec_argv(name, mode, args))
        .map(str::to_string)
        .collect()
}

/// Runs `"$@"` with stdout captured to a temp file, then prints that file as
/// base64 and exits with the command's status. Transient execs reject
/// non-UTF-8 stdout, so binary-returning calls go through this. A temp file
/// rather than a pipe keeps the exit code without relying on `pipefail`.
const BASE64_STDOUT_SCRIPT: &str =
    r#"f=$(mktemp) || exit 125; "$@" >"$f"; rc=$?; base64 <"$f"; rm -f "$f"; exit $rc"#;

/// Wrap `args` in [`BASE64_STDOUT_SCRIPT`] when `mode` needs it.
fn binary_safe_args<'a>(mode: ExecMode, args: &[&'a str]) -> Vec<&'a str> {
    match mode {
        ExecMode::Tracked => args.to_vec(),
        ExecMode::Transient => {
            let mut wrapped = vec!["sh", "-c", BASE64_STDOUT_SCRIPT, "staged-b64"];
            wrapped.extend_from_slice(args);
            wrapped
        }
    }
}

/// Undo [`binary_safe_args`] on the command's stdout.
fn decode_binary_safe_stdout(mode: ExecMode, stdout: Vec<u8>) -> Result<Vec<u8>, BloxError> {
    match mode {
        ExecMode::Tracked => Ok(stdout),
        ExecMode::Transient => {
            use base64::Engine as _;
            // GNU `base64` wraps lines at 76 columns; strip all whitespace.
            let compact: Vec<u8> = stdout
                .into_iter()
                .filter(|b| !b.is_ascii_whitespace())
                .collect();
            base64::engine::general_purpose::STANDARD
                .decode(compact)
                .map_err(|e| BloxError::ParseError(format!("invalid base64 from ws exec: {e}")))
        }
    }
}

/// Execute a command inside a Blox workspace.
///
/// Runs: `sq blox ws exec [--transient] <name> -- <args…>`
///
/// Returns the command's stdout on success.
pub fn ws_exec(name: &str, mode: ExecMode, args: &[&str]) -> Result<String, BloxError> {
    run(&ws_exec_argv(name, mode, args), EXEC_TIMEOUT)
}

/// Execute a command inside a Blox workspace, returning raw bytes.
///
/// Like `ws_exec` but returns the raw stdout bytes without UTF-8 validation.
/// Use this when the command may produce binary output (e.g. `git show` on image files).
pub fn ws_exec_bytes(name: &str, mode: ExecMode, args: &[&str]) -> Result<Vec<u8>, BloxError> {
    let wrapped = binary_safe_args(mode, args);
    let stdout = run_bytes(&ws_exec_argv(name, mode, &wrapped), EXEC_TIMEOUT)?;
    decode_binary_safe_stdout(mode, stdout)
}

/// Output from a workspace command that preserves stdout, stderr, and exit
/// status separately — unlike `ws_exec` which discards stdout on failure.
pub struct WsExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub success: bool,
}

/// Execute a command inside a Blox workspace, returning full output regardless
/// of exit code. Infrastructure errors (CLI not found, timeout, auth failure)
/// still return `Err`, but a non-zero exit from the remote command returns
/// `Ok(WsExecOutput { success: false, .. })`.
pub fn ws_exec_output(
    name: &str,
    mode: ExecMode,
    args: &[&str],
) -> Result<WsExecOutput, BloxError> {
    ws_exec_output_with_timeout(name, mode, args, EXEC_TIMEOUT)
}

/// [`ws_exec_output`] with its own timeout, for a command whose running time
/// grows with what it works on (see [`exec_timeout`]). Past the timeout `sq`
/// is killed here; the remote command may still be running.
pub fn ws_exec_output_with_timeout(
    name: &str,
    mode: ExecMode,
    args: &[&str],
    timeout: Duration,
) -> Result<WsExecOutput, BloxError> {
    let wrapped = binary_safe_args(mode, args);
    let output = run_bytes_full(&ws_exec_argv(name, mode, &wrapped), timeout)?;
    Ok(WsExecOutput {
        stdout: decode_binary_safe_stdout(mode, output.stdout)?,
        ..output
    })
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
    fn exec_timeout_keeps_the_floor_and_grows_a_second_per_megabyte() {
        assert_eq!(exec_timeout(0), EXEC_TIMEOUT);
        assert_eq!(exec_timeout(999_999), EXEC_TIMEOUT);
        assert_eq!(
            exec_timeout(4_000_000_000),
            EXEC_TIMEOUT + Duration::from_secs(4_000)
        );
    }

    #[test]
    fn acp_proxy_args_without_command_or_env_is_bare() {
        assert_eq!(
            acp_proxy_args("ws-1", None, &[]),
            vec!["blox", "acp", "ws-1"]
        );
        assert_eq!(
            acp_proxy_args("ws-1", Some("  "), &[]),
            vec!["blox", "acp", "ws-1"]
        );
    }

    #[test]
    fn acp_proxy_args_passes_command_and_env_vars_as_single_flags() {
        let env = vec![
            ("GOOSE_MODE".to_string(), "auto".to_string()),
            ("STAGED_X".to_string(), "1".to_string()),
        ];
        assert_eq!(
            acp_proxy_args("ws-1", Some("goose,acp"), &env),
            vec![
                "blox",
                "acp",
                "ws-1",
                "--command=goose,acp",
                "--env-vars=GOOSE_MODE=auto;STAGED_X=1",
            ]
        );
    }

    #[test]
    fn acp_proxy_args_skips_env_vars_that_would_split_the_flag() {
        let env = vec![
            ("BAD;NAME".to_string(), "x".to_string()),
            ("GOOD".to_string(), "a;b".to_string()),
            ("".to_string(), "x".to_string()),
            ("OK".to_string(), "fine".to_string()),
        ];
        assert_eq!(
            acp_proxy_args("ws-1", None, &env),
            vec!["blox", "acp", "ws-1", "--env-vars=OK=fine"]
        );
    }

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

    #[test]
    fn candidate_paths_tolerate_startup_output_before_absolute_path() {
        let paths: Vec<_> =
            candidate_paths_from_shell_output("hello from shell init\n/opt/homebrew/bin/sq\n")
                .collect();

        assert_eq!(paths, vec![PathBuf::from("/opt/homebrew/bin/sq")]);
    }

    #[test]
    fn candidate_paths_ignore_relative_lines_and_function_bodies() {
        let paths: Vec<_> =
            candidate_paths_from_shell_output("sq () {\n\tcommand sq \"$@\"\n}\n").collect();

        assert!(paths.is_empty());
    }

    #[test]
    fn login_shell_picks_last_executable_absolute_path() {
        // Simulates a rc file printing an absolute path of an unrelated
        // executable before the shell builtin prints the real lookup answer.
        let dir = std::env::temp_dir().join(format!("blox-cli-last-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let decoy = dir.join("decoy");
        let real = dir.join("real");
        std::fs::File::create(&decoy).unwrap();
        std::fs::File::create(&real).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&decoy, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let stdout = format!("{}\n{}\n", decoy.display(), real.display());
        let picked = candidate_paths_from_shell_output(&stdout)
            .filter(|p| is_executable_file(p))
            .last();

        assert_eq!(picked, Some(real));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ws_exec_argv_marks_only_transient_calls() {
        assert_eq!(
            ws_exec_argv("ws1", ExecMode::Transient, &["git", "status"]),
            ["ws", "exec", "--transient", "ws1", "--", "git", "status"]
        );
        assert_eq!(
            ws_exec_argv("ws1", ExecMode::Tracked, &["git", "status"]),
            ["ws", "exec", "ws1", "--", "git", "status"]
        );
    }

    #[test]
    fn ws_exec_command_prefixes_blox() {
        assert_eq!(
            ws_exec_command("ws1", ExecMode::Tracked, &["sh", "-lc", "make test"]),
            ["blox", "ws", "exec", "ws1", "--", "sh", "-lc", "make test"]
        );
    }

    #[test]
    fn tracked_binary_reads_are_not_wrapped() {
        assert_eq!(
            binary_safe_args(ExecMode::Tracked, &["git", "show", "HEAD:a.png"]),
            ["git", "show", "HEAD:a.png"]
        );
        assert_eq!(
            decode_binary_safe_stdout(ExecMode::Tracked, vec![0xff, 0xfe]).unwrap(),
            [0xff, 0xfe]
        );
    }

    /// Run the transient wrapper locally under each POSIX shell a workstation
    /// might use, and check binary stdout and the exit code survive it.
    #[test]
    fn base64_wrapper_round_trips_binary_stdout_and_exit_code() {
        let payload: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let payload_path =
            std::env::temp_dir().join(format!("blox-cli-b64-{}", std::process::id()));
        std::fs::write(&payload_path, &payload).unwrap();
        let payload_arg = payload_path.to_str().unwrap().to_string();
        let script = "cat \"$0\"; echo oops >&2; exit 3";

        let mut shells_run = 0;
        for shell in ["/bin/sh", "/bin/dash"] {
            if !Path::new(shell).exists() {
                continue;
            }
            shells_run += 1;
            let args = binary_safe_args(ExecMode::Transient, &["sh", "-c", script, &payload_arg]);
            assert_eq!(args[0], "sh");
            let output = Command::new(shell).args(&args[1..]).output().unwrap();
            assert_eq!(output.status.code(), Some(3), "{shell}");
            assert_eq!(String::from_utf8_lossy(&output.stderr), "oops\n", "{shell}");
            assert!(std::str::from_utf8(&output.stdout).is_ok(), "{shell}");
            let decoded = decode_binary_safe_stdout(ExecMode::Transient, output.stdout).unwrap();
            assert_eq!(decoded, payload, "{shell}");
        }
        let _ = std::fs::remove_file(&payload_path);
        assert!(shells_run > 0);
    }

    #[test]
    fn base64_wrapper_decodes_empty_stdout() {
        assert!(decode_binary_safe_stdout(ExecMode::Transient, Vec::new())
            .unwrap()
            .is_empty());
    }
}
