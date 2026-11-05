//! Reading bloxlet's view of a remote agent process.

use crate::driver::remote_liveness::{parse_commands, CommandRecord, Target};
use std::ffi::OsString;

fn record(id: &str, status: &str, message: &str) -> CommandRecord {
    CommandRecord {
        id: id.to_string(),
        status: status.to_string(),
        message: message.to_string(),
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
