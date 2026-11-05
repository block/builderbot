//! How a Blox session's last turn ended, read from its process's recorded
//! output after the process is gone.
//!
//! A gone process can no longer be asked (`RUNNING`/`IDLE` describe live
//! processes only), and the record Blox keeps for it says little about the
//! turn: after a workstation suspends, every process reads `FAILED`
//! "bloxlet restarted while command was running", including one whose turn
//! had long finished. The recording still holds the agent's own account,
//! where the agent gives one: the Claude bridge stamps a
//! `session_state_changed` frame (`running` / `requires_action` / `idle`) and
//! a `result` frame with the session id, because Staged asks for them on
//! `session/new` and `session/load`. Other agents send no per-session turn
//! state, and their recording says nothing reliable about the turn.
use super::attach::{result_turn, TurnResult};
use super::*;

/// How a session's last turn stands in a process recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedTurn {
    /// The session's last state frame was `idle`: its turn ended before the
    /// process did. `result` comes from the `result` frame before it, when
    /// there was one.
    Ended { result: Option<TurnResult> },
    /// The recording stops with the turn running, or waiting on a
    /// permission: the process died under it.
    InFlight,
    /// The recording carries no turn state for the session.
    Unknown,
}

/// The wire method of the Claude bridge's raw SDK frames (the handler sees it
/// without the leading `_`; see [`CLAUDE_SDK_MESSAGE_METHOD`]).
const RAW_SDK_MESSAGE_WIRE_METHOD: &str = "_claude/sdkMessage";

/// Read `agent_session_id`'s last turn out of a process recording: one
/// agent-to-client JSON-RPC frame per line, as `sq blox ws output` prints it.
/// Lines that are not JSON, and other sessions' frames, are skipped.
pub(crate) fn recorded_turn(output: &str, agent_session_id: &str) -> RecordedTurn {
    let mut turn = RecordedTurn::Unknown;
    let mut last_result: Option<TurnResult> = None;
    for line in output.lines() {
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if frame.get("method").and_then(serde_json::Value::as_str)
            != Some(RAW_SDK_MESSAGE_WIRE_METHOD)
        {
            continue;
        }
        let params = &frame["params"];
        if params.get("sessionId").and_then(serde_json::Value::as_str) != Some(agent_session_id) {
            continue;
        }
        let message = &params["message"];
        if message.get("type").and_then(serde_json::Value::as_str) == Some("result") {
            last_result = result_turn(message);
            continue;
        }
        match sdk_message_session_state(message) {
            Some(SdkSessionState::Idle) => {
                turn = RecordedTurn::Ended {
                    result: last_result.clone(),
                };
            }
            Some(SdkSessionState::Busy) => {
                // A new turn (or a permission wait inside one): the result of
                // the turn before it no longer describes anything.
                last_result = None;
                turn = RecordedTurn::InFlight;
            }
            None => {}
        }
    }
    turn
}

impl AcpDriver {
    /// How this Blox session's last turn ended on the process `command_id`,
    /// read from that process's recorded output (see [`RecordedTurn`]).
    /// Meant for a process that is gone; blocking (one full `ws output` read,
    /// about 2 s). `None` for a local driver.
    pub fn recorded_turn(
        &self,
        command_id: &str,
        agent_session_id: &str,
    ) -> Option<Result<RecordedTurn, String>> {
        let blox = self.blox_process.as_ref()?;
        Some(
            blox_cli::ws_output(&blox.workspace_name, command_id)
                .map(|output| recorded_turn(&output, agent_session_id))
                .map_err(|e| e.to_string()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "1632bee2-e827-44ce-abc3-dcc37e270f6f";

    fn state(session: &str, state: &str) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "_claude/sdkMessage",
            "params": {
                "sessionId": session,
                "message": {
                    "type": "system",
                    "subtype": "session_state_changed",
                    "state": state,
                    "session_id": session,
                },
            },
        })
        .to_string()
    }

    fn result(session: &str, subtype: &str, stop_reason: &str, terminal_reason: &str) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "_claude/sdkMessage",
            "params": {
                "sessionId": session,
                "message": {
                    "type": "result",
                    "subtype": subtype,
                    "stop_reason": stop_reason,
                    "terminal_reason": terminal_reason,
                    "session_id": session,
                },
            },
        })
        .to_string()
    }

    /// The shape of a live recording: the prompt's own response (no session
    /// id) between the `result` and the `idle`, as Blox records it.
    fn ended_turn(session: &str) -> Vec<String> {
        vec![
            state(session, "running"),
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"x","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}}}"#.to_string(),
            result(session, "success", "end_turn", "completed"),
            r#"{"jsonrpc":"2.0","id":"224342af","result":{"stopReason":"end_turn"}}"#.to_string(),
            state(session, "idle"),
        ]
    }

    #[test]
    fn a_turn_that_went_idle_ended_with_its_result() {
        let output = ended_turn(SESSION).join("\n");
        assert_eq!(
            recorded_turn(&output, SESSION),
            RecordedTurn::Ended {
                result: Some(TurnResult::Stopped(StopReason::EndTurn))
            }
        );
    }

    #[test]
    fn the_last_turn_decides_and_a_cancelled_one_says_so() {
        let mut lines = ended_turn(SESSION);
        lines.push(state(SESSION, "running"));
        lines.push(result(
            SESSION,
            "error_during_execution",
            "tool_use",
            "aborted_tools",
        ));
        lines.push(state(SESSION, "idle"));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION),
            RecordedTurn::Ended {
                result: Some(TurnResult::Stopped(StopReason::Cancelled))
            }
        );
    }

    #[test]
    fn a_turn_that_errored_ended_failed() {
        let mut lines = ended_turn(SESSION);
        lines.push(state(SESSION, "running"));
        lines.push(
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "_claude/sdkMessage",
                "params": {
                    "sessionId": SESSION,
                    "message": {
                        "type": "result",
                        "subtype": "error_during_execution",
                        "is_error": true,
                        "errors": ["API Error: 500"],
                        "terminal_reason": "completed",
                        "session_id": SESSION,
                    },
                },
            })
            .to_string(),
        );
        lines.push(state(SESSION, "idle"));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION),
            RecordedTurn::Ended {
                result: Some(TurnResult::Failed {
                    message: Some("API Error: 500".to_string())
                })
            }
        );
    }

    #[test]
    fn a_recording_that_stops_mid_turn_is_in_flight() {
        let mut lines = ended_turn(SESSION);
        lines.push(state(SESSION, "running"));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION),
            RecordedTurn::InFlight
        );
        // A permission wait is a turn in flight too.
        lines.push(state(SESSION, "requires_action"));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION),
            RecordedTurn::InFlight
        );
    }

    #[test]
    fn an_idle_with_no_result_since_the_turn_began_has_no_stop_reason() {
        let mut lines = ended_turn(SESSION);
        lines.push(state(SESSION, "running"));
        lines.push(state(SESSION, "idle"));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION),
            RecordedTurn::Ended { result: None }
        );
    }

    #[test]
    fn other_sessions_and_stray_lines_are_ignored() {
        let mut lines = vec!["not json at all".to_string(), String::new()];
        lines.extend(ended_turn(SESSION));
        lines.push(state("another-session", "running"));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION),
            RecordedTurn::Ended {
                result: Some(TurnResult::Stopped(StopReason::EndTurn))
            }
        );
    }

    #[test]
    fn a_recording_without_turn_state_is_unknown() {
        let output = [
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}"#,
        ]
        .join("\n");
        assert_eq!(recorded_turn(&output, "s"), RecordedTurn::Unknown);
        assert_eq!(recorded_turn("", "s"), RecordedTurn::Unknown);
    }
}
