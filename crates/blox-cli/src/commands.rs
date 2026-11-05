//! Typed `sq blox ws commands --json` listings, and finding one ACP agent
//! process in them.
//!
//! A remote ACP session's state lives in its workstation process: `RUNNING`
//! while a `session/prompt` is unanswered on it, `IDLE` once every prompt is
//! answered, and gone (with a `FAILED` record and its `message`, typically)
//! when the process died. Polling reads only the listing: it never replays
//! output, and it never passes `--acp-session-ids`, which annotates each
//! process by reading its recorded output.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

use crate::{run, BloxError, QUICK_TIMEOUT};

/// Most records a full listing asks for. `ws commands` returns only the 50
/// newest without a limit, and a busy workstation's finished processes would
/// push an older session's record out of that window.
const FULL_LISTING_LIMIT: &str = "500";

/// Tag Blox puts on every process `blox acp` starts.
const ACP_ROLE_TAG: &str = "blox:role";
const ACP_ROLE: &str = "acp";

/// A workstation command's status, from the orchestrator's
/// `COMMAND_STATUS_*` enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandStatus {
    Pending,
    /// For an ACP process: a `session/prompt` is unanswered on it (a turn in
    /// flight, or one blocked on a permission request).
    Running,
    /// For an ACP process: running, with every prompt answered.
    Idle,
    Succeeded,
    Failed,
    /// A status this crate does not know, verbatim.
    Other(String),
}

impl CommandStatus {
    fn from_wire(status: &str) -> Self {
        match status.strip_prefix("COMMAND_STATUS_").unwrap_or(status) {
            "PENDING" => Self::Pending,
            "RUNNING" => Self::Running,
            "IDLE" => Self::Idle,
            "SUCCEEDED" => Self::Succeeded,
            "FAILED" => Self::Failed,
            _ => Self::Other(status.to_string()),
        }
    }

    /// Whether the process is still running: what `--status=active` lists,
    /// and what `blox acp` will reuse.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Running | Self::Idle)
    }
}

impl<'de> Deserialize<'de> for CommandStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_wire(&String::deserialize(deserializer)?))
    }
}

/// One record from `sq blox ws commands <ws> --json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkstationCommand {
    pub command_id: String,
    #[serde(default)]
    pub command_type: Option<String>,
    pub status: CommandStatus,
    /// Why the command ended, when it failed (e.g. "bloxlet restarted while
    /// command was running").
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub process_command: Option<String>,
    #[serde(default)]
    pub process_args: Vec<String>,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

impl WorkstationCommand {
    /// Whether `blox acp` started this process.
    pub fn is_acp_process(&self) -> bool {
        self.tags.get(ACP_ROLE_TAG).map(String::as_str) == Some(ACP_ROLE)
    }

    /// Whether the process runs exactly `argv`: the comparison `blox acp`
    /// itself reuses a process by.
    pub fn runs(&self, argv: &[String]) -> bool {
        match argv.split_first() {
            Some((command, args)) => {
                self.process_command.as_deref() == Some(command.as_str())
                    && self.process_args == args
            }
            None => false,
        }
    }
}

/// Which records [`ws_command_list`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandFilter {
    /// `--status=active`: running and idle processes.
    Active,
    /// Every record, newest first, up to a generous limit: where a process
    /// that ended is found, with its `message`.
    All,
}

/// List a workstation's commands, newest first.
///
/// Runs: `sq blox ws commands <name> --json --status=active` or
/// `sq blox ws commands <name> --json -n 500`.
pub fn ws_command_list(
    name: &str,
    filter: CommandFilter,
) -> Result<Vec<WorkstationCommand>, BloxError> {
    let mut args = vec!["ws", "commands", name, "--json"];
    match filter {
        CommandFilter::Active => args.push("--status=active"),
        CommandFilter::All => args.extend(["-n", FULL_LISTING_LIMIT]),
    }
    parse_command_list(&run(&args, QUICK_TIMEOUT)?)
}

fn parse_command_list(stdout: &str) -> Result<Vec<WorkstationCommand>, BloxError> {
    // An empty listing prints `[]`; tolerate nothing at all, and `null`.
    if matches!(stdout.trim(), "" | "null") {
        return Ok(Vec::new());
    }
    serde_json::from_str(stdout).map_err(|e| BloxError::ParseError(format!("{e}\nRaw: {stdout}")))
}

/// Find a session's ACP process in a listing: by `command_id` when one is
/// known, otherwise the newest ACP process running exactly `argv`.
///
/// A command id that is not in the listing finds nothing, even if another
/// process runs the same argv: a pinned process that is gone is gone.
pub fn find_acp_process<'a>(
    commands: &'a [WorkstationCommand],
    command_id: Option<&str>,
    argv: &[String],
) -> Option<&'a WorkstationCommand> {
    match command_id {
        Some(id) => commands.iter().find(|command| command.command_id == id),
        None => commands
            .iter()
            .find(|command| command.is_acp_process() && command.runs(argv)),
    }
}

/// The newest *ended* ACP process for a session: the record that says why
/// the process is gone after `blox acp` replaced it (`FAILED` and its
/// `message`, typically). Matched like [`find_acp_process`], skipping
/// processes that are still active.
pub fn find_ended_acp_process<'a>(
    commands: &'a [WorkstationCommand],
    command_id: Option<&str>,
    argv: &[String],
) -> Option<&'a WorkstationCommand> {
    commands
        .iter()
        .filter(|command| !command.status.is_active())
        .find(|command| match command_id {
            Some(id) => command.command_id == id,
            None => command.is_acp_process() && command.runs(argv),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a live `ws commands --json` listing.
    const LISTING: &str = r#"[
      {
        "commandId": "27294439",
        "commandType": "COMMAND_TYPE_EXECUTE_PROCESS",
        "status": "COMMAND_STATUS_IDLE",
        "createdAt": "2026-11-05T02:49:19Z",
        "updatedAt": "2026-11-05T02:51:51Z",
        "processCommand": "claude-agent-acp",
        "processArgs": ["--staged-session=sessA"],
        "startedAt": "2026-11-05T02:49:19Z",
        "tags": {"blox:agent": "claude", "blox:role": "acp"}
      },
      {
        "commandId": "27293552",
        "commandType": "COMMAND_TYPE_EXECUTE_PROCESS",
        "status": "COMMAND_STATUS_RUNNING",
        "processCommand": "sh",
        "processArgs": ["-c", "exec \"$@\"", "staged-session-s1", "goose", "acp"],
        "tags": {"blox:role": "acp"}
      },
      {
        "commandId": "27270001",
        "commandType": "COMMAND_TYPE_EXECUTE_PROCESS",
        "status": "COMMAND_STATUS_FAILED",
        "message": "bloxlet restarted while command was running",
        "processCommand": "claude-agent-acp",
        "processArgs": ["--staged-session=sessA"],
        "tags": {"blox:agent": "claude", "blox:role": "acp"}
      },
      {
        "commandId": "27260000",
        "commandType": "COMMAND_TYPE_EXECUTE_PROCESS",
        "status": "COMMAND_STATUS_SUCCEEDED",
        "processCommand": "claude-agent-acp",
        "processArgs": ["--staged-session=sessA"],
        "tags": {}
      },
      {
        "commandId": "27250000",
        "status": "COMMAND_STATUS_SOMETHING_NEW"
      }
    ]"#;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn parses_the_live_listing_shape() {
        let commands = parse_command_list(LISTING).expect("parses");
        assert_eq!(commands.len(), 5);
        assert_eq!(commands[0].command_id, "27294439");
        assert_eq!(commands[0].status, CommandStatus::Idle);
        assert_eq!(
            commands[0].command_type.as_deref(),
            Some("COMMAND_TYPE_EXECUTE_PROCESS")
        );
        assert_eq!(commands[0].process_args, argv(&["--staged-session=sessA"]));
        assert!(commands[0].is_acp_process());
        assert_eq!(commands[1].status, CommandStatus::Running);
        assert_eq!(commands[2].status, CommandStatus::Failed);
        assert_eq!(
            commands[2].message.as_deref(),
            Some("bloxlet restarted while command was running")
        );
        assert_eq!(
            commands[4].status,
            CommandStatus::Other("COMMAND_STATUS_SOMETHING_NEW".to_string())
        );
        assert!(!commands[4].is_acp_process());
        for empty in ["[]\n", "  \n", "null"] {
            assert!(parse_command_list(empty).expect("empty").is_empty());
        }
    }

    #[test]
    fn only_running_and_idle_are_active() {
        assert!(CommandStatus::Running.is_active());
        assert!(CommandStatus::Idle.is_active());
        for status in [
            CommandStatus::Pending,
            CommandStatus::Succeeded,
            CommandStatus::Failed,
            CommandStatus::Other("X".to_string()),
        ] {
            assert!(!status.is_active(), "{status:?}");
        }
    }

    #[test]
    fn finds_a_process_by_exact_argv_newest_first() {
        let commands = parse_command_list(LISTING).unwrap();
        let claude = argv(&["claude-agent-acp", "--staged-session=sessA"]);
        assert_eq!(
            find_acp_process(&commands, None, &claude).map(|c| c.command_id.as_str()),
            Some("27294439")
        );
        let goose = argv(&[
            "sh",
            "-c",
            "exec \"$@\"",
            "staged-session-s1",
            "goose",
            "acp",
        ]);
        assert_eq!(
            find_acp_process(&commands, None, &goose).map(|c| c.command_id.as_str()),
            Some("27293552")
        );
        // A prefix, or an extra argument, is a different process.
        assert_eq!(
            find_acp_process(&commands, None, &argv(&["claude-agent-acp"])),
            None
        );
        assert_eq!(
            find_acp_process(&commands, None, &argv(&["sh", "-c", "exec \"$@\""])),
            None
        );
        assert_eq!(find_acp_process(&commands, None, &[]), None);
    }

    #[test]
    fn a_known_command_id_wins_and_never_falls_back_to_argv() {
        let commands = parse_command_list(LISTING).unwrap();
        let claude = argv(&["claude-agent-acp", "--staged-session=sessA"]);
        assert_eq!(
            find_acp_process(&commands, Some("27270001"), &claude).map(|c| &c.status),
            Some(&CommandStatus::Failed)
        );
        assert_eq!(find_acp_process(&commands, Some("404"), &claude), None);
    }

    #[test]
    fn the_ended_record_skips_active_processes_and_untagged_ones() {
        let commands = parse_command_list(LISTING).unwrap();
        let claude = argv(&["claude-agent-acp", "--staged-session=sessA"]);
        let ended = find_ended_acp_process(&commands, None, &claude).expect("found");
        assert_eq!(ended.command_id, "27270001");
        assert_eq!(
            ended.message.as_deref(),
            Some("bloxlet restarted while command was running")
        );
        // The pinned id is the active one: nothing has ended.
        assert_eq!(
            find_ended_acp_process(&commands, Some("27294439"), &claude),
            None
        );
        assert_eq!(
            find_ended_acp_process(&commands, Some("27260000"), &claude).map(|c| &c.status),
            Some(&CommandStatus::Succeeded)
        );
    }
}
