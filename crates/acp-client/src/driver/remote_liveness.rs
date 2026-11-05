//! Asking bloxlet whether a remote agent process is still running.
//!
//! An agent can die on the workstation without its output stream ending:
//! when bloxlet restarts under it (as it does after the workstation runs out
//! of memory) it marks the agent's command failed with "bloxlet restarted
//! while command was running", while `sq blox acp` keeps the stream open and
//! retries disconnects indefinitely. Nothing then reaches the sessions on
//! it, so they would wait forever. The proxy uses [`check`] to find out, when
//! the stream has just disconnected or a session's tool call has gone quiet
//! for [`STALLED_TOOL_CALL_AFTER`].

use std::ffi::{OsStr, OsString};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

/// How often the proxy considers whether a check is due.
#[cfg(not(test))]
pub(super) const WATCH_TICK: Duration = Duration::from_secs(30);
#[cfg(test)]
pub(super) const WATCH_TICK: Duration = Duration::from_millis(20);

/// How long a session may have a tool call running with no frame from the
/// agent before the proxy checks on the agent. Tool calls can legitimately
/// run far longer; the check is what tells the two apart.
#[cfg(not(test))]
pub(super) const STALLED_TOOL_CALL_AFTER: Duration = Duration::from_secs(5 * 60);
#[cfg(test)]
pub(super) const STALLED_TOOL_CALL_AFTER: Duration = Duration::from_millis(200);

/// How long after the stream last disconnected the proxy keeps checking, and
/// how often. bloxlet takes a minute or two to restart and mark the agent's
/// command failed, so one check right away would miss it.
pub(super) const DISCONNECT_WATCH: Duration = Duration::from_secs(5 * 60);
#[cfg(not(test))]
pub(super) const CHECK_AFTER_DISCONNECT_EVERY: Duration = Duration::from_secs(30);
#[cfg(test)]
pub(super) const CHECK_AFTER_DISCONNECT_EVERY: Duration = Duration::from_millis(100);

const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// Larger than any workstation's command history the full listing needs to
/// reach back through to find a stopped agent's record.
const COMMAND_HISTORY_LIMIT: &str = "500";

/// Where to ask: the `sq` the proxy runs, and the workspace it attaches to.
pub(super) struct Target {
    program: OsString,
    workspace: String,
}

impl Target {
    /// The target for a proxy running `program args…`, if those are `blox acp
    /// <workspace> …` arguments.
    pub(super) fn of(program: &OsStr, args: &[OsString]) -> Option<Self> {
        let [blox, acp, workspace, ..] = args else {
            return None;
        };
        if blox != "blox" || acp != "acp" {
            return None;
        }
        Some(Self {
            program: program.to_owned(),
            workspace: workspace.to_str()?.to_string(),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Liveness {
    Running,
    /// The process has stopped, for the given reason.
    Gone(String),
}

/// Whether bloxlet still runs the agent process `command_id`. An error means
/// the check itself failed and says nothing about the process.
pub(super) async fn check(target: &Target, command_id: &str) -> Result<Liveness, String> {
    let active = list_commands(target, &["--status=active"]).await?;
    if active.iter().any(|command| command.id == command_id) {
        return Ok(Liveness::Running);
    }
    // No longer active. Its record says why, when it is still listed.
    let record = list_commands(target, &[&format!("--limit={COMMAND_HISTORY_LIMIT}")])
        .await
        .ok()
        .and_then(|commands| {
            commands
                .into_iter()
                .find(|command| command.id == command_id)
        });
    Ok(Liveness::Gone(match record {
        Some(CommandRecord {
            status, message, ..
        }) if message.is_empty() => {
            format!("bloxlet reports the agent process (command {command_id}) as {status}")
        }
        Some(CommandRecord {
            status, message, ..
        }) => format!(
            "bloxlet reports the agent process (command {command_id}) as {status}: {message}"
        ),
        None => {
            format!("bloxlet no longer lists the agent process (command {command_id}) as running")
        }
    }))
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct CommandRecord {
    pub(super) id: String,
    pub(super) status: String,
    pub(super) message: String,
}

async fn list_commands(target: &Target, filter: &[&str]) -> Result<Vec<CommandRecord>, String> {
    let mut cmd = Command::new(&target.program);
    cmd.args(["blox", "ws", "commands", &target.workspace, "--json"])
        .args(filter)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(CHECK_TIMEOUT, cmd.output())
        .await
        .map_err(|_| "`sq blox ws commands` timed out".to_string())?
        .map_err(|error| format!("`sq blox ws commands` failed to run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`sq blox ws commands` exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_commands(&output.stdout)
}

/// Parse `sq blox ws commands --json`: protojson command summaries, so
/// camelCase keys and enum names, though older CLIs printed snake_case keys
/// and enum numbers.
pub(super) fn parse_commands(stdout: &[u8]) -> Result<Vec<CommandRecord>, String> {
    let commands: Vec<Value> = serde_json::from_slice(stdout)
        .map_err(|error| format!("unreadable `sq blox ws commands` output: {error}"))?;
    Ok(commands
        .iter()
        .filter_map(|command| {
            let id = command
                .get("commandId")
                .or_else(|| command.get("command_id"))?
                .as_str()?;
            Some(CommandRecord {
                id: id.to_string(),
                status: command.get("status").map_or_else(
                    || "unknown".to_string(),
                    |status| match status {
                        Value::String(name) => name
                            .trim_start_matches("COMMAND_STATUS_")
                            .to_ascii_lowercase(),
                        Value::Number(number) => match number.as_u64() {
                            Some(1) => "pending",
                            Some(2) => "running",
                            Some(3) => "succeeded",
                            Some(4) => "failed",
                            Some(5) => "idle",
                            _ => "unknown",
                        }
                        .to_string(),
                        _ => "unknown".to_string(),
                    },
                ),
                message: command
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            })
        })
        .collect())
}
