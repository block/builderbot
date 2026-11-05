//! How a Blox session's last turn ended, read from its process's recorded
//! output.
//!
//! A gone process can no longer be asked (`RUNNING`/`IDLE` describe live
//! processes only), and the record Blox keeps for it says little about the
//! turn: after a workstation suspends, every process reads `FAILED`
//! "bloxlet restarted while command was running", including one whose turn
//! had long finished. And a live one reading `IDLE` only says every
//! `session/prompt` was answered, not that the session stopped working. The
//! recording still holds the agent's own account, where the agent gives one:
//! the Claude bridge stamps a `session_state_changed` frame (`running` /
//! `requires_action` / `idle`), a `result` frame and its background tasks'
//! `task_*` frames with the session id, because Staged asks for them on
//! `session/new` and `session/load`. Other agents send no per-session turn
//! state; for them the recording's `session/prompt` answers are all there is
//! (see [`RecordedTurn::Answered`]).
use super::attach::{result_turn, TurnResult};
use super::*;

/// How a session's last turn stands in a process recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedTurn {
    /// The session's last state frame was `idle`, and no background task it
    /// started is still live: its turn ended before the recording does, and
    /// so did the work the turn set going. `result` comes from the `result`
    /// frame before it, when there was one.
    Ended { result: Option<TurnResult> },
    /// As [`RecordedTurn::Ended`], except that background tasks the session
    /// started never settle in the recording: the turn is over, but its work
    /// goes on, and a task settling may wake the model for a continuation
    /// cycle of its own.
    Holding { result: Option<TurnResult> },
    /// The recording stops with the session busy: a turn running, a
    /// permission wait inside one, or a continuation cycle a settled task
    /// woke. Of a process that is gone, it died under it; of one that reads
    /// `IDLE`, the work is out of turn and still going.
    InFlight,
    /// No turn state for the session (an agent other than the Claude bridge),
    /// but the process's last `session/prompt` answer: its `stopReason`, or an
    /// error answered after it. That says how the last answered prompt ended,
    /// not that none was in flight after it, so it describes the session's
    /// turn only once the process reads `IDLE` (every prompt answered).
    Answered { result: TurnResult },
    /// The recording carries nothing about the session's turns.
    Unknown,
}

/// The wire method of the Claude bridge's raw SDK frames (the handler sees it
/// without the leading `_`; see [`CLAUDE_SDK_MESSAGE_METHOD`]).
const RAW_SDK_MESSAGE_WIRE_METHOD: &str = "_claude/sdkMessage";

/// Read `agent_session_id`'s last turn out of a process recording: one
/// agent-to-client JSON-RPC frame per line, as `sq blox ws output` prints it.
/// Lines that are not JSON, and other sessions' frames, are skipped.
/// `dedicated` says the process serves this session alone, so its prompt
/// answers, which carry no session id, are this session's (see
/// [`PromptAnswers`]).
pub(crate) fn recorded_turn(output: &str, agent_session_id: &str, dedicated: bool) -> RecordedTurn {
    let mut turn = RecordedTurn::Unknown;
    let mut last_result: Option<TurnResult> = None;
    let mut tasks = BackgroundTaskSet::default();
    let mut answers = PromptAnswers::default();
    for line in output.lines() {
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        let Some(method) = frame.get("method").and_then(serde_json::Value::as_str) else {
            answers.observe(&frame);
            continue;
        };
        if method != RAW_SDK_MESSAGE_WIRE_METHOD {
            continue;
        }
        let params = &frame["params"];
        if params.get("sessionId").and_then(serde_json::Value::as_str) != Some(agent_session_id) {
            continue;
        }
        let message = &params["message"];
        tasks.apply_sdk_message(message);
        if message.get("type").and_then(serde_json::Value::as_str) == Some("result") {
            // A subagent's own result closes its task, not the session's
            // turn; the live handler skips it the same way.
            if message
                .get("parent_tool_use_id")
                .is_none_or(serde_json::Value::is_null)
            {
                last_result = result_turn(message);
            }
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
    match turn {
        RecordedTurn::Ended { result } if !tasks.task_ids.is_empty() => {
            RecordedTurn::Holding { result }
        }
        RecordedTurn::Unknown if dedicated => answers
            .last
            .map_or(RecordedTurn::Unknown, |result| RecordedTurn::Answered {
                result,
            }),
        turn => turn,
    }
}

/// The `session/prompt` answers in a recording. It holds only what the agent
/// wrote, not the requests, so a response is told apart by its shape and its
/// place in its connection: an `initialize` answer (`protocolVersion`) opens
/// a connection's setup, the first other success closes it (the session
/// created or loaded, or an `authenticate`), and from then on a `stopReason`
/// answers a prompt (a connection can carry several). An error answers
/// whatever was asked last: in setup that is the setup, which failed, and the
/// connection never prompted; past it, normally a prompt. Staged's other
/// requests past setup leave that standing: a config option it fails to set
/// fails the session before any prompt goes out, the `session/set_mode` it
/// sends best effort goes to the Claude bridge alone (whose turns are read
/// from its own frames), and the steering and async-task extensions answer a
/// refusal with a result, not an error. Only one of those failing outright
/// as a turn ends would read as that turn's error.
#[derive(Debug, Default)]
struct PromptAnswers {
    in_setup: bool,
    last: Option<TurnResult>,
}

impl PromptAnswers {
    fn observe(&mut self, frame: &serde_json::Value) {
        if frame.get("id").is_none() {
            return;
        }
        if let Some(error) = frame.get("error") {
            if !self.in_setup {
                self.last = Some(TurnResult::Failed {
                    message: Some(prompt_error_message(error)),
                });
            }
            return;
        }
        let Some(result) = frame.get("result") else {
            return;
        };
        if result.get("protocolVersion").is_some() {
            self.in_setup = true;
            return;
        }
        self.in_setup = false;
        if let Some(reason) = result.get("stopReason") {
            // A stop reason this build doesn't know still answered the
            // prompt, without an error.
            let reason = serde_json::from_value(reason.clone()).unwrap_or(StopReason::EndTurn);
            self.last = Some(TurnResult::Stopped(reason));
        }
    }
}

/// The text the live path shows for a prompt answered with `error` (see
/// [`describe_prompt_error`]).
fn prompt_error_message(error: &serde_json::Value) -> String {
    match serde_json::from_value::<agent_client_protocol::Error>(error.clone()) {
        Ok(error) => describe_prompt_error(error, &[]),
        Err(_) => format!("Prompt failed: {error}"),
    }
}

impl AcpDriver {
    /// How this Blox session's last turn stands on the process `command_id`,
    /// read from that process's recorded output (see [`RecordedTurn`]): for a
    /// process that is gone, or one that reads `IDLE` but may still be
    /// working. Blocking (one full `ws output` read, about 2 s). `None` for a
    /// local driver.
    pub fn recorded_turn(
        &self,
        command_id: &str,
        agent_session_id: &str,
    ) -> Option<Result<RecordedTurn, String>> {
        let blox = self.blox_process.as_ref()?;
        Some(
            blox_cli::ws_output(&blox.workspace_name, command_id)
                .map(|output| recorded_turn(&output, agent_session_id, blox.command.dedicated))
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
            recorded_turn(&output, SESSION, true),
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
            recorded_turn(&lines.join("\n"), SESSION, true),
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
            recorded_turn(&lines.join("\n"), SESSION, true),
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
            recorded_turn(&lines.join("\n"), SESSION, true),
            RecordedTurn::InFlight
        );
        // A permission wait is a turn in flight too.
        lines.push(state(SESSION, "requires_action"));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION, true),
            RecordedTurn::InFlight
        );
    }

    #[test]
    fn an_idle_with_no_result_since_the_turn_began_has_no_stop_reason() {
        let mut lines = ended_turn(SESSION);
        lines.push(state(SESSION, "running"));
        lines.push(state(SESSION, "idle"));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION, true),
            RecordedTurn::Ended { result: None }
        );
    }

    #[test]
    fn other_sessions_and_stray_lines_are_ignored() {
        let mut lines = vec!["not json at all".to_string(), String::new()];
        lines.extend(ended_turn(SESSION));
        lines.push(state("another-session", "running"));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION, true),
            RecordedTurn::Ended {
                result: Some(TurnResult::Stopped(StopReason::EndTurn))
            }
        );
    }

    #[test]
    fn a_recording_without_turn_state_or_answers_is_unknown() {
        let output = [
            r#"{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":1}}"#,
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}}}"#,
        ]
        .join("\n");
        assert_eq!(recorded_turn(&output, "s", true), RecordedTurn::Unknown);
        assert_eq!(recorded_turn("", "s", true), RecordedTurn::Unknown);
    }

    fn task(session: &str, subtype: &str, task_id: &str) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "_claude/sdkMessage",
            "params": {
                "sessionId": session,
                "message": {
                    "type": "system",
                    "subtype": subtype,
                    "task_id": task_id,
                    "session_id": session,
                },
            },
        })
        .to_string()
    }

    /// The turn went idle with a background task still running: the turn is
    /// over, its work is not.
    #[test]
    fn a_background_task_that_never_settles_leaves_the_turn_holding() {
        let mut lines = vec![
            state(SESSION, "running"),
            task(SESSION, "task_started", "t1"),
        ];
        lines.extend(ended_turn(SESSION).into_iter().skip(1));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION, true),
            RecordedTurn::Holding {
                result: Some(TurnResult::Stopped(StopReason::EndTurn))
            }
        );

        // Another session's task settling changes nothing.
        lines.push(task("another-session", "task_notification", "t1"));
        assert!(matches!(
            recorded_turn(&lines.join("\n"), SESSION, true),
            RecordedTurn::Holding { .. }
        ));

        // Once it settles, the turn and its work have ended.
        lines.push(task(SESSION, "task_notification", "t1"));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION, true),
            RecordedTurn::Ended {
                result: Some(TurnResult::Stopped(StopReason::EndTurn))
            }
        );
    }

    /// A settled task wakes the model with no prompt in flight: the process
    /// reads `IDLE`, but the recording shows the session busy.
    #[test]
    fn a_continuation_cycle_after_the_turn_is_in_flight() {
        let mut lines = vec![
            state(SESSION, "running"),
            task(SESSION, "task_started", "t1"),
        ];
        lines.extend(ended_turn(SESSION).into_iter().skip(1));
        lines.push(task(SESSION, "task_notification", "t1"));
        lines.push(state(SESSION, "running"));
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION, true),
            RecordedTurn::InFlight
        );
    }

    #[test]
    fn a_subagent_result_does_not_close_the_turn() {
        let mut subagent: serde_json::Value = serde_json::from_str(&result(
            SESSION,
            "error_during_execution",
            "end_turn",
            "completed",
        ))
        .unwrap();
        subagent["params"]["message"]["is_error"] = true.into();
        subagent["params"]["message"]["parent_tool_use_id"] = "toolu_1".into();
        let mut lines = ended_turn(SESSION);
        lines.insert(2, subagent.to_string());
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION, true),
            RecordedTurn::Ended {
                result: Some(TurnResult::Stopped(StopReason::EndTurn))
            }
        );
    }

    const INITIALIZED: &str = r#"{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}"#;
    const LOADED: &str = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
    const CHUNK: &str = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}}}"#;
    const INTERNAL_ERROR: &str =
        r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32603,"message":"Internal error"}}"#;

    fn answered(stop_reason: &str) -> String {
        format!(r#"{{"jsonrpc":"2.0","id":2,"result":{{"stopReason":"{stop_reason}"}}}}"#)
    }

    /// An agent with no turn state of its own (Codex, Goose, Pi, Amp): the
    /// prompt's answer is the turn's end.
    #[test]
    fn a_prompt_answer_is_the_turn_of_an_agent_without_turn_state() {
        for (stop_reason, expected) in [
            ("end_turn", StopReason::EndTurn),
            ("cancelled", StopReason::Cancelled),
            ("max_tokens", StopReason::MaxTokens),
            // One this build doesn't know still answered without an error.
            ("something_new", StopReason::EndTurn),
        ] {
            let output = [INITIALIZED, LOADED, CHUNK, &answered(stop_reason)].join("\n");
            assert_eq!(
                recorded_turn(&output, "s", true),
                RecordedTurn::Answered {
                    result: TurnResult::Stopped(expected)
                },
                "{stop_reason}"
            );
        }
    }

    /// An error answered after the last stop reason is the newest prompt's,
    /// worded as the live path words it; a later stop reason answers a
    /// later prompt.
    #[test]
    fn an_error_after_the_last_stop_reason_fails_the_turn() {
        let mut lines = vec![INITIALIZED, LOADED, CHUNK];
        let first = answered("end_turn");
        lines.push(&first);
        lines.push(INTERNAL_ERROR);
        assert_eq!(
            recorded_turn(&lines.join("\n"), "s", true),
            RecordedTurn::Answered {
                result: TurnResult::Failed {
                    message: Some(
                        r#"Prompt failed: Error { code: -32603: Internal error, message: "Internal error", data: None }"#
                            .to_string()
                    )
                }
            }
        );

        let later = answered("end_turn");
        lines.push(&later);
        assert_eq!(
            recorded_turn(&lines.join("\n"), "s", true),
            RecordedTurn::Answered {
                result: TurnResult::Stopped(StopReason::EndTurn)
            }
        );
    }

    /// A later connection whose `session/load` failed (an earlier reconcile
    /// attempt's) never prompted: its error is not the turn's.
    #[test]
    fn a_failed_setup_is_not_a_failed_prompt() {
        let first = answered("end_turn");
        let output = [
            INITIALIZED,
            LOADED,
            &first,
            INITIALIZED,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32002,"message":"Resource not found"}}"#,
            INITIALIZED,
            LOADED,
        ]
        .join("\n");
        assert_eq!(
            recorded_turn(&output, "s", true),
            RecordedTurn::Answered {
                result: TurnResult::Stopped(StopReason::EndTurn)
            }
        );
    }

    /// A process other sessions share answers their prompts too, and answers
    /// carry no session id.
    #[test]
    fn a_shared_process_s_answers_are_nobody_s_in_particular() {
        let output = [INITIALIZED, LOADED, INTERNAL_ERROR].join("\n");
        assert_eq!(recorded_turn(&output, "s", false), RecordedTurn::Unknown);
    }

    /// The Claude bridge's own frames decide its turns; a prompt answer
    /// beside them changes nothing.
    #[test]
    fn turn_state_wins_over_prompt_answers() {
        let mut lines = ended_turn(SESSION);
        lines.push(INTERNAL_ERROR.to_string());
        assert_eq!(
            recorded_turn(&lines.join("\n"), SESSION, true),
            RecordedTurn::Ended {
                result: Some(TurnResult::Stopped(StopReason::EndTurn))
            }
        );
    }
}
