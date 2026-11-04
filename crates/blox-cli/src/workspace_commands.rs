//! A workspace's commands, as `sq blox ws commands <name> --json` lists them,
//! for bootstrap progress. ACP process lookups read the same listing through
//! [`crate::WorkstationCommand`] instead (see `commands.rs`).
//!
//! Older CLIs printed snake_case keys and enum numbers (`"status": 3`).
//! Current ones print protojson: camelCase keys, enum value names
//! (`"status": "COMMAND_STATUS_SUCCEEDED"`), and no field that holds its
//! default value, so `isBootstrap` appears only when it is true. Both parse.

use serde::{Deserialize, Serialize};

use super::{run, BloxError, QUICK_TIMEOUT};

/// A command's state: the orchestrator's `CommandStatus`, as a
/// [`WorkspaceCommand`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkspaceCommandStatus {
    /// Unset, or a value this crate does not know.
    #[default]
    Unspecified = 0,
    Pending = 1,
    Running = 2,
    Succeeded = 3,
    Failed = 4,
    /// Still running, waiting for input (a long-lived agent process).
    Idle = 5,
}

impl WorkspaceCommandStatus {
    /// Whether the command has stopped, whether it worked or not.
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}

impl ProtoEnum for WorkspaceCommandStatus {
    const VALUES: &'static [(Self, &'static str)] = &[
        (Self::Unspecified, "COMMAND_STATUS_UNSPECIFIED"),
        (Self::Pending, "COMMAND_STATUS_PENDING"),
        (Self::Running, "COMMAND_STATUS_RUNNING"),
        (Self::Succeeded, "COMMAND_STATUS_SUCCEEDED"),
        (Self::Failed, "COMMAND_STATUS_FAILED"),
        (Self::Idle, "COMMAND_STATUS_IDLE"),
    ];
}

/// What a command does: the orchestrator's `CommandType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandType {
    /// Unset, or a value this crate does not know.
    #[default]
    Unspecified = 0,
    Checkout = 1,
    ExecuteProcess = 2,
    ProjectBootstrap = 3,
    ProvisionWorkspace = 4,
    RestoreSnapshot = 5,
    /// The bootstrap a new workspace runs before it reports itself running.
    ExecuteBootstrap = 6,
}

impl ProtoEnum for CommandType {
    const VALUES: &'static [(Self, &'static str)] = &[
        (Self::Unspecified, "COMMAND_TYPE_UNSPECIFIED"),
        (Self::Checkout, "COMMAND_TYPE_CHECKOUT"),
        (Self::ExecuteProcess, "COMMAND_TYPE_EXECUTE_PROCESS"),
        (Self::ProjectBootstrap, "COMMAND_TYPE_PROJECT_BOOTSTRAP"),
        (Self::ProvisionWorkspace, "COMMAND_TYPE_PROVISION_WORKSPACE"),
        (Self::RestoreSnapshot, "COMMAND_TYPE_RESTORE_SNAPSHOT"),
        (Self::ExecuteBootstrap, "COMMAND_TYPE_EXECUTE_BOOTSTRAP"),
    ];
}

/// An orchestrator proto enum.
trait ProtoEnum: Copy + Default + PartialEq + 'static {
    /// Every value with its protojson name, in number order from 0, so a
    /// value's number is its index.
    const VALUES: &'static [(Self, &'static str)];
}

/// `#[serde(with)]` for a [`ProtoEnum`] field. It reads either the value's
/// name or its number, taking `null` or a value this crate does not know as
/// the default (unspecified) one, and writes the name.
mod proto_enum {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::ProtoEnum;

    pub fn serialize<S: Serializer, T: ProtoEnum>(
        value: &T,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let name = T::VALUES
            .iter()
            .find(|(v, _)| v == value)
            .map_or(T::VALUES[0].1, |(_, name)| name);
        serializer.serialize_str(name)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: ProtoEnum>(
        deserializer: D,
    ) -> Result<T, D::Error> {
        let found = match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::Number(n)) => n
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .and_then(|i| T::VALUES.get(i)),
            Some(serde_json::Value::String(s)) => T::VALUES.iter().find(|(_, name)| *name == s),
            _ => None,
        };
        Ok(found.map_or_else(T::default, |(value, _)| *value))
    }
}

/// A command returned by `sq blox ws commands <name> --json`: the workspace's
/// bootstrap, and every command run in it since.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCommand {
    #[serde(default, alias = "command_id")]
    pub command_id: String,
    #[serde(default, alias = "command_type", with = "proto_enum")]
    pub command_type: CommandType,
    #[serde(default, with = "proto_enum")]
    pub status: WorkspaceCommandStatus,
    #[serde(default, alias = "is_bootstrap")]
    pub is_bootstrap: bool,
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Catch-all for any other fields the CLI returns.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// List a Blox workspace's commands, newest first. Without `--limit` the CLI
/// returns the 50 newest, so a workspace that has run more no longer lists
/// its bootstrap; a new one lists it until then.
///
/// Runs: `sq blox ws commands <name> --json`
pub fn ws_commands(name: &str) -> Result<Vec<WorkspaceCommand>, BloxError> {
    let stdout = run(&["ws", "commands", name, "--json"], QUICK_TIMEOUT)?;
    parse_ws_commands(&stdout)
}

fn parse_ws_commands(stdout: &str) -> Result<Vec<WorkspaceCommand>, BloxError> {
    serde_json::from_str(stdout).map_err(|e| BloxError::ParseError(format!("{e}\nRaw: {stdout}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `sq blox ws commands <name> --json` from sq-blox 1.16.0, on a workspace
    /// with a single `buzz-app` source, trimmed to four of its 50 commands
    /// (and their scripts shortened): two of Staged's own, an ACP agent with
    /// its tags, and the bootstrap, which is the oldest.
    const PROTOJSON: &str = r#"[
  {
    "commandId": "27121856",
    "commandType": "COMMAND_TYPE_EXECUTE_PROCESS",
    "status": "COMMAND_STATUS_SUCCEEDED",
    "createdAt": "2026-11-04T14:26:51Z",
    "updatedAt": "2026-11-04T14:26:51Z",
    "processCommand": "git",
    "processArgs": [
      "log",
      "--reverse",
      "--format=%x00%at%x01commit %H%nAuthor: %an%n%n%B",
      "0b2dfbf9889ea95873f175b216fbb0b5795f104e..HEAD"
    ],
    "startedAt": "2026-11-04T14:26:51Z"
  },
  {
    "commandId": "27121538",
    "commandType": "COMMAND_TYPE_EXECUTE_PROCESS",
    "status": "COMMAND_STATUS_FAILED",
    "message": "failed to start process: fork/exec /usr/bin/sh: argument list too long",
    "createdAt": "2026-11-04T14:24:22Z",
    "updatedAt": "2026-11-04T14:24:22Z",
    "processCommand": "sh",
    "processArgs": [
      "-c",
      "echo 'AAAAIGZ0eXBpc29tAAACAGlzb21pc28y' | base64 -d"
    ],
    "startedAt": "2026-11-04T14:24:22Z"
  },
  {
    "commandId": "27115122",
    "commandType": "COMMAND_TYPE_EXECUTE_PROCESS",
    "status": "COMMAND_STATUS_SUCCEEDED",
    "createdAt": "2026-11-04T13:48:12Z",
    "updatedAt": "2026-11-04T17:14:39Z",
    "processCommand": "sh",
    "processArgs": [
      "-c",
      "exec claude-agent-acp",
      "sh"
    ],
    "startedAt": "2026-11-04T13:48:12Z",
    "tags": {
      "blox:agent": "claude",
      "blox:role": "acp"
    }
  },
  {
    "commandId": "27105810",
    "commandType": "COMMAND_TYPE_EXECUTE_BOOTSTRAP",
    "status": "COMMAND_STATUS_SUCCEEDED",
    "isBootstrap": true,
    "createdAt": "2026-11-04T12:26:59Z",
    "updatedAt": "2026-11-04T12:27:18Z",
    "startedAt": "2026-11-04T12:27:12Z"
  }
]"#;

    /// The older output: snake_case keys, enum numbers, and every field
    /// present.
    const SNAKE_CASE: &str = r#"[
  {
    "command_id": "4101",
    "command_type": 4,
    "status": 3,
    "is_bootstrap": true,
    "dependencies": []
  },
  {
    "command_id": "4102",
    "command_type": 3,
    "status": 2,
    "is_bootstrap": true,
    "dependencies": ["4101"]
  },
  {
    "command_id": "4103",
    "command_type": 2,
    "status": 1,
    "is_bootstrap": false,
    "dependencies": []
  }
]"#;

    #[test]
    fn parses_current_protojson_output() {
        let commands = parse_ws_commands(PROTOJSON).unwrap();
        let summary: Vec<_> = commands
            .iter()
            .map(|c| {
                (
                    c.command_id.as_str(),
                    c.command_type,
                    c.status,
                    c.is_bootstrap,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "27121856",
                    CommandType::ExecuteProcess,
                    WorkspaceCommandStatus::Succeeded,
                    false
                ),
                (
                    "27121538",
                    CommandType::ExecuteProcess,
                    WorkspaceCommandStatus::Failed,
                    false
                ),
                (
                    "27115122",
                    CommandType::ExecuteProcess,
                    WorkspaceCommandStatus::Succeeded,
                    false
                ),
                (
                    "27105810",
                    CommandType::ExecuteBootstrap,
                    WorkspaceCommandStatus::Succeeded,
                    true
                ),
            ]
        );
        assert!(commands.iter().all(|c| c.dependencies.is_empty()));
        // Everything else lands in `extra`, under the CLI's own keys.
        assert_eq!(
            commands[1].extra["message"],
            "failed to start process: fork/exec /usr/bin/sh: argument list too long"
        );
        assert_eq!(commands[2].extra["tags"]["blox:role"], "acp");
        assert!(commands[3].extra.get("commandId").is_none());
        assert!(commands[3].extra.get("isBootstrap").is_none());
    }

    #[test]
    fn parses_a_bootstrap_still_in_progress() {
        let running = PROTOJSON.replace(
            r#""status": "COMMAND_STATUS_SUCCEEDED",
    "isBootstrap": true"#,
            r#""status": "COMMAND_STATUS_RUNNING",
    "isBootstrap": true"#,
        );
        assert_ne!(running, PROTOJSON);
        let commands = parse_ws_commands(&running).unwrap();
        let bootstrap: Vec<_> = commands.iter().filter(|c| c.is_bootstrap).collect();
        assert_eq!(bootstrap.len(), 1);
        assert_eq!(bootstrap[0].status, WorkspaceCommandStatus::Running);
        assert!(!bootstrap[0].status.is_finished());
    }

    #[test]
    fn parses_older_snake_case_output() {
        let commands = parse_ws_commands(SNAKE_CASE).unwrap();
        let summary: Vec<_> = commands
            .iter()
            .map(|c| (c.command_type, c.status, c.is_bootstrap))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    CommandType::ProvisionWorkspace,
                    WorkspaceCommandStatus::Succeeded,
                    true
                ),
                (
                    CommandType::ProjectBootstrap,
                    WorkspaceCommandStatus::Running,
                    true
                ),
                (
                    CommandType::ExecuteProcess,
                    WorkspaceCommandStatus::Pending,
                    false
                ),
            ]
        );
        assert_eq!(commands[0].command_id, "4101");
        assert_eq!(commands[1].dependencies, ["4101"]);
        assert_eq!(commands[0].extra, serde_json::json!({}));
    }

    #[test]
    fn reads_unknown_or_missing_enum_values_as_unspecified() {
        let commands = parse_ws_commands(
            r#"[
  {"commandId": "1", "commandType": "COMMAND_TYPE_FROM_THE_FUTURE", "status": 99},
  {"commandId": "2", "status": null},
  {"commandId": "3", "commandType": -1, "status": "COMMAND_STATUS_IDLE"}
]"#,
        )
        .unwrap();
        let summary: Vec<_> = commands
            .iter()
            .map(|c| (c.command_type, c.status))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    CommandType::Unspecified,
                    WorkspaceCommandStatus::Unspecified
                ),
                (
                    CommandType::Unspecified,
                    WorkspaceCommandStatus::Unspecified
                ),
                (CommandType::Unspecified, WorkspaceCommandStatus::Idle),
            ]
        );
    }

    #[test]
    fn writes_protojson_that_reads_back() {
        let commands = parse_ws_commands(PROTOJSON).unwrap();
        let written = serde_json::to_value(&commands[3]).unwrap();
        assert_eq!(written["commandType"], "COMMAND_TYPE_EXECUTE_BOOTSTRAP");
        assert_eq!(written["status"], "COMMAND_STATUS_SUCCEEDED");
        assert_eq!(written["isBootstrap"], true);
        let read: WorkspaceCommand = serde_json::from_value(written).unwrap();
        assert_eq!(read.command_type, CommandType::ExecuteBootstrap);
        assert_eq!(read.status, WorkspaceCommandStatus::Succeeded);
        assert_eq!(read.extra["startedAt"], "2026-11-04T12:27:12Z");
    }

    #[test]
    fn enum_tables_are_in_number_order() {
        for (i, (value, _)) in WorkspaceCommandStatus::VALUES.iter().enumerate() {
            assert_eq!(*value as usize, i);
        }
        for (i, (value, _)) in CommandType::VALUES.iter().enumerate() {
            assert_eq!(*value as usize, i);
        }
    }

    #[test]
    fn only_succeeded_and_failed_are_finished() {
        let finished: Vec<_> = WorkspaceCommandStatus::VALUES
            .iter()
            .filter(|(s, _)| s.is_finished())
            .map(|(s, _)| *s)
            .collect();
        assert_eq!(
            finished,
            [
                WorkspaceCommandStatus::Succeeded,
                WorkspaceCommandStatus::Failed
            ]
        );
    }
}
