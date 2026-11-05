//! Reading bloxlet's view of a remote agent process.

use crate::driver::remote_liveness::{
    parse_commands, parse_utc_timestamp, CommandRecord, Target, REATTACHED_TURN_QUIET_CAP,
};
use crate::driver::AcpDriver;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

fn record(id: &str, status: &str, message: &str) -> CommandRecord {
    CommandRecord {
        id: id.to_string(),
        status: status.to_string(),
        message: message.to_string(),
        started_at: None,
    }
}

#[test]
fn reads_protojson_command_summaries() {
    let commands = parse_commands(
        br#"[
            {"commandId":"cmd-1","status":"COMMAND_STATUS_IDLE"},
            {"commandId":"cmd-2","status":"COMMAND_STATUS_FAILED","message":" bloxlet restarted while command was running\n"}
        ]"#,
    )
    .unwrap();

    assert_eq!(
        commands,
        vec![
            record("cmd-1", "idle", ""),
            record(
                "cmd-2",
                "failed",
                "bloxlet restarted while command was running"
            ),
        ]
    );
}

#[test]
fn reads_when_a_command_started() {
    let commands = parse_commands(
        br#"[
            {"commandId":"cmd-1","status":"COMMAND_STATUS_RUNNING",
             "createdAt":"2026-11-05T01:02:03Z","startedAt":"2026-11-05T01:02:04.250Z"},
            {"commandId":"cmd-2","status":"COMMAND_STATUS_RUNNING",
             "createdAt":"2026-11-05T01:02:03.999999Z"},
            {"commandId":"cmd-3","status":"COMMAND_STATUS_RUNNING","startedAt":"soon"}
        ]"#,
    )
    .unwrap();

    let started: Vec<_> = commands.iter().map(|c| c.started_at).collect();
    assert_eq!(started, [Some(1_793_840_524), Some(1_793_840_523), None]);
}

#[test]
fn parses_utc_timestamps_to_epoch_seconds() {
    assert_eq!(parse_utc_timestamp("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        parse_utc_timestamp("2000-03-01T00:00:00Z"),
        Some(951_868_800)
    );
    assert_eq!(
        parse_utc_timestamp("2026-11-05T01:02:03.456789Z"),
        Some(1_793_840_523)
    );
    assert_eq!(parse_utc_timestamp("2026-11-05T01:02:03+11:00"), None);
    assert_eq!(parse_utc_timestamp("2026-11-05"), None);
}

#[test]
fn reads_older_snake_case_command_summaries() {
    let commands =
        parse_commands(br#"[{"command_id":"cmd-1","status":2},{"command_id":"cmd-2","status":4}]"#)
            .unwrap();

    assert_eq!(
        commands,
        vec![
            record("cmd-1", "running", ""),
            record("cmd-2", "failed", "")
        ]
    );
}

#[test]
fn skips_summaries_without_a_command_id_and_rejects_unreadable_output() {
    assert_eq!(
        parse_commands(br#"[{"status":"COMMAND_STATUS_RUNNING"},{"commandId":"cmd-1"}]"#).unwrap(),
        vec![record("cmd-1", "unknown", "")]
    );
    assert!(parse_commands(b"Error: not logged in").is_err());
}

#[test]
fn only_a_blox_acp_proxy_has_a_workspace_to_ask() {
    let args = |args: &[&str]| args.iter().map(OsString::from).collect::<Vec<_>>();

    assert!(Target::of(
        "sq".as_ref(),
        &args(&["blox", "acp", "ws-1", "--command", "x"])
    )
    .is_some());
    assert!(Target::of("sq".as_ref(), &args(&["blox", "ws", "ws-1"])).is_none());
    assert!(Target::of("claude-agent-acp".as_ref(), &args(&[])).is_none());
}

/// The quiet cap's target for the proxy `AcpDriver::for_workspace` builds for
/// `agent_id`.
fn workspace_target(agent_id: Option<&str>) -> Target {
    let driver =
        AcpDriver::for_workspace_with_binary(PathBuf::from("sq"), "ws-1", agent_id, "session-1");
    let args: Vec<OsString> = driver.acp_args.iter().map(OsString::from).collect();
    Target::of(OsStr::new("sq"), &args).expect("a blox acp proxy")
}

/// A reattached turn goes uncapped only on a process dedicated to its
/// session whose agent runs unattended (Goose's `GOOSE_MODE=auto`, Claude's
/// `bypassPermissions`). A dedicated Codex, Pi or Amp process keeps the cap,
/// as do Blox's shared default and a process pinned by id: those agents ask
/// permission, and a request the lost connection never answered would
/// otherwise hold the turn forever.
#[test]
fn only_a_dedicated_unattended_agent_is_waited_on_past_the_quiet_cap() {
    for agent in ["claude", "goose"] {
        assert_eq!(
            workspace_target(Some(agent)).reattached_turn_quiet_cap(),
            None,
            "{agent}"
        );
    }
    for agent in ["codex", "pi", "amp"] {
        assert_eq!(
            workspace_target(Some(agent)).reattached_turn_quiet_cap(),
            Some(REATTACHED_TURN_QUIET_CAP),
            "{agent}"
        );
    }
    assert_eq!(
        workspace_target(None).reattached_turn_quiet_cap(),
        Some(REATTACHED_TURN_QUIET_CAP),
        "Blox's shared default"
    );
    let pinned: Vec<OsString> = ["blox", "acp", "ws-1", "--command-id=cmd-1"]
        .iter()
        .map(OsString::from)
        .collect();
    assert_eq!(
        Target::of(OsStr::new("sq"), &pinned)
            .unwrap()
            .reattached_turn_quiet_cap(),
        Some(REATTACHED_TURN_QUIET_CAP)
    );
}
