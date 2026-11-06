//! Attaching to a Blox session's process from a fresh connection, end to end
//! against a fake `sq blox acp` proxy: catch-up past the stored transcript,
//! settling on the session's own idle frame or the process status, the
//! proxy's stderr markers, teardown, and attach-cancel.

use super::*;
use crate::driver::attach::{AttachTiming, CatchUpThrough};
use crate::driver::{
    AcpToolCallMetadata, AttachOutcome, BloxProcess, CancelOutcome, LatestPrompt,
    RemoteProcessProbe, RemoteProcessStatus, TurnResult,
};
use crate::types::BloxAcpCommand;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A Blox proxy in `sh`. Logs its arguments, every line it reads and any
/// SIGINT to `$1`, and serves `$2`:
///
/// - `claude`: a Claude session cut off mid-turn. Before `initialize` is
///   answered it relays the in-flight turn's frames (which the replay
///   covers); `session/load` replays the whole session, ending on a pending
///   tool call; then the turn finishes live: the tool's result, a closing
///   message, the session's `result`, the dead connection's prompt
///   response, and `idle`.
/// - `streaming`: as `claude`, with a message still streaming when the
///   proxy attaches: it relays part of it before the load, the replay has
///   none of it, and its last chunk comes live.
/// - `holding`: as `claude`, except that the turn starts a background task
///   live, which is still running when the session goes idle.
/// - `plain`: an agent with no message ids and no raw SDK frames; the
///   replay adds one message and nothing follows.
/// - `replaced`: the pinned process is gone and the proxy started another.
/// - `eof`: the proxy loses its remote process after the load.
/// - `hang`: the turn goes on after the load, silently.
/// - `cancel`: `session/cancel` ends the turn with `result` and `idle`.
/// - `cancel-silent`: `session/cancel` is accepted with no frames.
/// - `interrupt`: as `cancel`, with the running tool call `t1` failed live
///   first (Claude Code's interrupt result, between progress and a late
///   completion that are not its end), which the replay of a later
///   `session/load` holds too, as the agent wrote it to its transcript.
/// - `close`: as `interrupt`, except that the replay leaves `t1` running
///   (the Claude bridge closing a call the cancel left open writes nothing
///   to the transcript).
#[cfg(unix)]
const FAKE_ATTACH_PROXY: &str = r#"
log=$1; mode=$2
echo "ARGS $*" >> "$log"
trap 'echo SIGINT >> "$log"; exit 0' INT
out() { printf '%s\n' "$1"; }
update() { out '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":'"$1"'}}'; }
sdk() { out '{"jsonrpc":"2.0","method":"_claude/sdkMessage","params":{"sessionId":"sess-1","message":'"$1"'}}'; }
reply() { out '{"jsonrpc":"2.0","id":"'"$1"'","result":'"$2"'}'; }
text() { update '{"sessionUpdate":"agent_message_chunk",'"$1"'"content":{"type":"text","text":"'"$2"'"}}'; }
tool() { update '{"sessionUpdate":"tool_call","toolCallId":"'"$1"'","title":"Run","status":"pending"}'; }
result() { update '{"sessionUpdate":"tool_call_update","toolCallId":"'"$1"'","status":"completed","content":[{"type":"content","content":{"type":"text","text":"'"$2"'"}}]}'; }
failed() { update '{"sessionUpdate":"tool_call_update","toolCallId":"'"$1"'","status":"failed","content":[{"type":"content","content":{"type":"text","text":"'"$2"'"}}]}'; }
idle() { sdk '{"type":"system","subtype":"session_state_changed","state":"idle","uuid":"idle-1"}'; }
case "$mode" in
  replaced)
    echo 'INF Requested ACP process has exited, matching its recorded command' >&2
    echo 'INF Started new ACP process process_id=2' >&2 ;;
  *) echo 'INF Found existing ACP process, reusing process_id=1' >&2 ;;
esac
sleep 0.1
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log"
  id=$(printf '%s' "$line" | sed -n 's/^.*"id":"\([^"]*\)".*$/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      case "$mode" in
        claude)
          tool t1
          text '"messageId":"m1",' ' wor' ;;
        streaming)
          text '"messageId":"m3",' 'Half ' ;;
      esac
      reply "$id" '{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}' ;;
    *'"method":"session/load"'*)
      case "$mode" in
        plain)
          update '{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"hi"}}'
          text '' 'Earlier.'
          text '' 'More.' ;;
        *)
          update '{"sessionUpdate":"user_message_chunk","messageId":"u1","content":{"type":"text","text":"hi"}}'
          text '"messageId":"m0",' 'Earlier.'
          tool t0
          result t0 'listed'
          text '"messageId":"m1",' 'Hello world'
          tool t1
          if [ "$mode" = interrupt ]; then
            failed t1 'Interrupted by user'
            update '{"sessionUpdate":"user_message_chunk","messageId":"u2","content":{"type":"text","text":"[Request interrupted by user for tool use]"}}'
          fi ;;
      esac
      reply "$id" '{}'
      case "$mode" in
        claude)
          result t1 'ok'
          text '"messageId":"m2",' 'Done'
          text '"messageId":"m2",' ' now'
          sdk '{"type":"result","subtype":"success","is_error":false,"num_turns":2,"stop_reason":"end_turn","terminal_reason":"completed","result":"Done now","uuid":"result-1","parent_tool_use_id":null}'
          reply old-prompt '{"stopReason":"end_turn"}'
          idle ;;
        holding)
          sdk '{"type":"system","subtype":"task_started","task_id":"bg-1","uuid":"task-1"}'
          result t1 'ok'
          sdk '{"type":"result","subtype":"success","is_error":false,"num_turns":2,"stop_reason":"end_turn","terminal_reason":"completed","result":"Done","uuid":"result-1","parent_tool_use_id":null}'
          reply old-prompt '{"stopReason":"end_turn"}'
          idle ;;
        streaming)
          text '"messageId":"m3",' 'rest.'
          sdk '{"type":"result","subtype":"success","is_error":false,"num_turns":2,"stop_reason":"end_turn","terminal_reason":"completed","result":"Half rest.","uuid":"result-1","parent_tool_use_id":null}'
          idle ;;
        eof) echo 'INF Received EOF signal from orchestrator, connection closed' >&2 ;;
      esac ;;
    *'"method":"session/cancel"'*)
      case "$mode" in
        interrupt|close)
          update '{"sessionUpdate":"tool_call_update","toolCallId":"t1","content":[{"type":"content","content":{"type":"text","text":"partial"}}]}'
          failed t1 'Interrupted'
          result t1 'late' ;;
      esac
      case "$mode" in
        cancel|interrupt|close)
          sdk '{"type":"result","subtype":"error_during_execution","is_error":true,"stop_reason":"tool_use","terminal_reason":"aborted_tools","uuid":"result-2","parent_tool_use_id":null}'
          idle ;;
      esac ;;
  esac
done
# A real proxy outlives its stdin until it is signalled: wait here, so the
# teardown's SIGINT (or its absence) is what the log records, not a race
# with the loop's EOF.
while :; do sleep 0.05; done
"#;

/// The transcript Staged stored before the disconnect: the prompt, a
/// finished message and tool call, the start of a message the disconnect
/// cut short, and the tool call that was running.
fn stored_transcript() -> Vec<ReplayBoundary> {
    let boundary =
        |role: &str, content: &str, message: Option<&str>, tool: Option<&str>| ReplayBoundary {
            role: role.to_string(),
            content: content.to_string(),
            acp_message_id: message.map(str::to_string),
            acp_tool_call_id: tool.map(str::to_string),
            acp_tool_status: None,
        };
    vec![
        boundary("user", "hi", None, None),
        boundary("assistant", "Earlier.", Some("m0"), None),
        boundary("assistant", "Earlier.", None, None),
        boundary("tool_call", "Run", None, Some("t0")),
        boundary("tool_result", "listed", None, Some("t0")),
        boundary("assistant", "Hello ", Some("m1"), None),
    ]
}

pub(super) struct TranscriptStore(pub(super) Vec<ReplayBoundary>);

impl Store for TranscriptStore {
    fn set_agent_session_id(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }

    fn get_session_replay_boundaries(&self, _: &str) -> Result<Vec<ReplayBoundary>, String> {
        Ok(self.0.clone())
    }
}

/// Records what the attach persisted, in order.
#[derive(Default)]
pub(super) struct TranscriptLog {
    log: Mutex<Vec<String>>,
}

impl TranscriptLog {
    fn push(&self, entry: String) {
        self.log.lock().unwrap().push(entry);
    }

    pub(super) fn rows(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl MessageWriter for TranscriptLog {
    async fn append_text(&self, text: &str) {
        self.push(format!("text:{text}"));
    }

    async fn finalize(&self) {}

    async fn record_tool_call(
        &self,
        tool_call_id: &str,
        _title: &str,
        _raw_input: Option<&serde_json::Value>,
    ) {
        self.push(format!("tool_call:{tool_call_id}"));
    }

    async fn update_tool_call_title(
        &self,
        _tool_call_id: &str,
        _title: Option<&str>,
        _raw_input: Option<&serde_json::Value>,
    ) {
    }

    async fn record_tool_result(&self, tool_call_id: &str, content: &str) {
        self.push(format!("tool_result:{tool_call_id}:{content}"));
    }

    /// Only the statuses that end a call: every call starts `pending`.
    async fn record_tool_call_metadata(&self, metadata: AcpToolCallMetadata) {
        if let (Some(id), Some(status @ ("completed" | "failed"))) =
            (&metadata.tool_call_id, metadata.tool_status.as_deref())
        {
            self.push(format!("status:{id}:{status}"));
        }
    }

    async fn record_acp_event_metadata(&self, metadata: AcpEventMetadata) {
        if metadata.event_kind.as_deref() == Some("response_result") {
            self.push("response_result".to_string());
        }
    }
}

/// Process statuses read in order, then `then` for good.
struct ScriptedProbe {
    statuses: Mutex<VecDeque<RemoteProcessStatus>>,
    then: RemoteProcessStatus,
    ended: Option<String>,
    reads: AtomicUsize,
}

impl ScriptedProbe {
    fn new(statuses: &[RemoteProcessStatus], then: RemoteProcessStatus) -> Arc<Self> {
        Arc::new(Self {
            statuses: Mutex::new(statuses.iter().cloned().collect()),
            then,
            ended: Some("bloxlet restarted while command was running".to_string()),
            reads: AtomicUsize::new(0),
        })
    }

    fn running() -> Arc<Self> {
        Self::new(&[], RemoteProcessStatus::Running)
    }
}

#[async_trait::async_trait]
impl RemoteProcessProbe for ScriptedProbe {
    async fn status(&self, _command_id: Option<&str>) -> Result<RemoteProcessStatus, String> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let next = self.statuses.lock().unwrap().pop_front();
        Ok(next.unwrap_or_else(|| self.then.clone()))
    }

    async fn ended_message(&self, _command_id: Option<&str>) -> Result<Option<String>, String> {
        Ok(self.ended.clone())
    }
}

const TIMING: AttachTiming = AttachTiming {
    poll: Duration::from_millis(50),
    idle_drain: Duration::from_millis(200),
    cancel_catch_up: Duration::from_secs(10),
};

fn driver(log: &Path, mode: &str, probe: Arc<ScriptedProbe>, dedicated: bool) -> AcpDriver {
    AcpDriver {
        binary_path: PathBuf::from("/bin/sh"),
        acp_args: vec![
            "-c".to_string(),
            FAKE_ATTACH_PROXY.to_string(),
            "fake-attach-proxy".to_string(),
            log.display().to_string(),
            mode.to_string(),
        ],
        agent_label: "FakeProxy".to_string(),
        is_remote: true,
        extra_env: vec![],
        env_snapshot: None,
        interpreter_env_snapshot: None,
        mcp_servers: vec![],
        remote_working_dir: Some(PathBuf::from("/home/bloxer/repo")),
        background_hold: None,
        background_hold_observer: None,
        steering_disabled: false,
        unattended_session_mode: None,
        blox_process: Some(BloxProcess {
            workspace_name: "ws".to_string(),
            command: BloxAcpCommand {
                argv: vec!["fake-agent".to_string()],
                explicit: true,
                dedicated,
            },
            probe,
        }),
    }
}

fn log_path(mode: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "acp-attach-{}-{mode}-{nanos}.log",
        std::process::id()
    ))
}

struct Attached {
    outcome: Result<AttachOutcome, String>,
    caught_up: bool,
    rows: Vec<String>,
    log: String,
}

/// Attach in `mode`, pinned to `command_id`; with `detach`, detach as soon
/// as the attach has caught up.
async fn attach(
    mode: &str,
    probe: Arc<ScriptedProbe>,
    dedicated: bool,
    command_id: Option<&str>,
    detach: bool,
) -> Attached {
    attach_to(
        mode,
        probe,
        dedicated,
        command_id,
        detach,
        stored_transcript(),
    )
    .await
}

/// [`attach`] over a transcript holding `stored`.
async fn attach_to(
    mode: &str,
    probe: Arc<ScriptedProbe>,
    dedicated: bool,
    command_id: Option<&str>,
    detach: bool,
    stored: Vec<ReplayBoundary>,
) -> Attached {
    let log = log_path(mode);
    let transcript = Arc::new(TranscriptLog::default());
    let local = tokio::task::LocalSet::new();
    let (outcome, caught_up) = local
        .run_until(async {
            let driver = driver(&log, mode, probe, dedicated);
            let store: Arc<dyn Store> = Arc::new(TranscriptStore(stored));
            let writer: Arc<dyn MessageWriter> = transcript.clone();
            let mut attached = driver
                .attach_with_timing(
                    "staged-session",
                    "sess-1",
                    command_id,
                    &std::env::temp_dir(),
                    &store,
                    &writer,
                    None,
                    TIMING,
                )
                .expect("the proxy spawns");
            let caught_up = attached.caught_up().await;
            if detach {
                assert!(
                    attached.detach_handle().detach(),
                    "the proxy group is armed"
                );
            }
            let outcome = tokio::time::timeout(Duration::from_secs(30), attached.settled())
                .await
                .expect("the attach settles");
            (outcome, caught_up)
        })
        .await;
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    Attached {
        outcome,
        caught_up,
        rows: transcript.rows(),
        log: text,
    }
}

async fn attach_cancel(
    mode: &str,
    probe: Arc<ScriptedProbe>,
    deadline: Duration,
) -> (Result<CancelOutcome, String>, String) {
    let caught_up = attach_cancel_with(mode, probe, deadline, None).await;
    (caught_up.outcome, caught_up.log)
}

/// What [`attach_cancel_with`] saw.
struct CaughtUp {
    outcome: Result<CancelOutcome, String>,
    /// Whether the catch-up's replay held the newest stored prompt.
    latest_prompt: Option<LatestPrompt>,
    /// The rows the catch-up recorded.
    rows: Vec<String>,
    /// The agent's log of what it was sent.
    log: String,
    /// What `on_cancelled` was told, with the rows recorded and the agent's
    /// log as it heard it.
    heard: Option<(CancelOutcome, Vec<String>, String)>,
}

/// [`attach_cancel`], catching up from a transcript holding `stored`.
async fn attach_cancel_with(
    mode: &str,
    probe: Arc<ScriptedProbe>,
    deadline: Duration,
    stored: Option<Vec<ReplayBoundary>>,
) -> CaughtUp {
    let log = log_path(mode);
    let transcript = Arc::new(TranscriptLog::default());
    let heard = std::cell::RefCell::new(None);
    let local = tokio::task::LocalSet::new();
    let outcome = local
        .run_until(async {
            let catch_up = stored.map(|stored| {
                let store: Arc<dyn Store> = Arc::new(TranscriptStore(stored));
                let writer: Arc<dyn MessageWriter> = transcript.clone();
                (store, writer)
            });
            driver(&log, mode, probe, true)
                .attach_cancel_with_timing(
                    "staged-session",
                    "sess-1",
                    Some("27294439"),
                    &std::env::temp_dir(),
                    catch_up.as_ref().map(|(store, writer)| CatchUpThrough {
                        store,
                        writer,
                        on_cancelled: Box::new(|outcome: &CancelOutcome| {
                            *heard.borrow_mut() = Some((
                                outcome.clone(),
                                transcript.rows(),
                                std::fs::read_to_string(&log).unwrap_or_default(),
                            ));
                        }),
                    }),
                    deadline,
                    TIMING,
                )
                .await
        })
        .await;
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    let latest_prompt = outcome.as_ref().ok().map(|(_, latest)| *latest);
    CaughtUp {
        outcome: outcome.map(|(outcome, _)| outcome),
        latest_prompt,
        rows: transcript.rows(),
        log: text,
        heard: heard.into_inner(),
    }
}

/// [`stored_transcript`] with the tool call that was running when the
/// connection went: `t1`, its first update (a shell command's description)
/// stored as its result.
pub(super) fn stored_with_running_call() -> Vec<ReplayBoundary> {
    let mut stored = stored_transcript();
    stored.push(ReplayBoundary {
        role: "tool_call".to_string(),
        content: "Run".to_string(),
        acp_message_id: None,
        acp_tool_call_id: Some("t1".to_string()),
        acp_tool_status: Some("pending".to_string()),
    });
    stored.push(ReplayBoundary {
        role: "tool_result".to_string(),
        content: "Runs the tests".to_string(),
        acp_message_id: None,
        acp_tool_call_id: Some("t1".to_string()),
        acp_tool_status: None,
    });
    stored
}

/// The full catch-up: only what the transcript lacks is recorded — the tail
/// of the message the disconnect cut, the tool call that was running and
/// its live result, the closing message — and the turn settles on the
/// session's own `result` + `idle`, not the dead connection's prompt
/// response. The frames relayed before the load are left to the replay.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn attach_records_only_what_the_transcript_lacks_and_settles_on_its_own_idle() {
    let run = attach(
        "claude",
        ScriptedProbe::running(),
        true,
        Some("27294439"),
        false,
    )
    .await;
    assert_eq!(
        run.outcome,
        Ok(AttachOutcome::Ended {
            result: Some(TurnResult::Stopped(StopReason::EndTurn)),
            latest_prompt: LatestPrompt::Replayed,
            background_work_live: false,
        })
    );
    assert!(run.caught_up);
    assert_eq!(
        run.rows,
        [
            "text:world",
            "tool_call:t1",
            "status:t1:completed",
            "tool_result:t1:ok",
            "text:Done",
            "text: now",
            "response_result",
        ]
    );
    assert!(run.log.contains("--command-id=27294439"), "{}", run.log);
    assert!(
        run.log.contains("\"method\":\"session/load\"") && run.log.contains("emitRawSDKMessages"),
        "the load re-registers the raw SDK filters: {}",
        run.log
    );
    assert!(
        run.log.contains("SIGINT"),
        "a dedicated process's proxy is stopped gracefully: {}",
        run.log
    );
}

/// The tool call was running when the connection went, so the transcript
/// holds it `pending`, with its description as its result. The replay
/// re-sends it unchanged, and its completion, live, is recorded as a live
/// turn records it: the status and the output both land.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_stored_running_call_ends_with_its_live_completion() {
    let run = attach_to(
        "claude",
        ScriptedProbe::running(),
        true,
        Some("27294439"),
        false,
        stored_with_running_call(),
    )
    .await;
    assert!(
        matches!(run.outcome, Ok(AttachOutcome::Ended { .. })),
        "{:?}",
        run.outcome
    );
    assert_eq!(
        run.rows,
        [
            "text:world",
            "status:t1:completed",
            "tool_result:t1:ok",
            "text:Done",
            "text: now",
            "response_result",
        ]
    );
}

/// The turn ends with a background task it started still running: the turn
/// is over, and the attach says the session's work is not.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_turn_that_ends_with_a_background_task_live_says_its_work_goes_on() {
    let run = attach("holding", ScriptedProbe::running(), true, None, false).await;
    assert_eq!(
        run.outcome,
        Ok(AttachOutcome::Ended {
            result: Some(TurnResult::Stopped(StopReason::EndTurn)),
            latest_prompt: LatestPrompt::Replayed,
            background_work_live: true,
        })
    );
}

/// Staged stored a resumed turn's prompt, then lost the connection before
/// the agent got it: the session's last turn, which the attach sees end, is
/// the one before, and the attach says so.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn attach_reports_a_newest_prompt_the_agent_never_received() {
    let mut stored = stored_transcript();
    stored.push(ReplayBoundary {
        role: "user".to_string(),
        content: "Now add a test".to_string(),
        acp_message_id: None,
        acp_tool_call_id: None,
        acp_tool_status: None,
    });
    let run = attach_to(
        "claude",
        ScriptedProbe::running(),
        true,
        Some("27294439"),
        false,
        stored,
    )
    .await;
    assert_eq!(
        run.outcome,
        Ok(AttachOutcome::Ended {
            result: Some(TurnResult::Stopped(StopReason::EndTurn)),
            latest_prompt: LatestPrompt::Missing,
            background_work_live: false,
        })
    );
}

/// The text of a message still streaming when the proxy attached, relayed
/// before the load and never replayed, is recorded ahead of its live tail.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn text_relayed_before_the_load_of_a_streaming_message_is_kept() {
    let run = attach(
        "streaming",
        ScriptedProbe::running(),
        true,
        Some("27294439"),
        false,
    )
    .await;
    assert!(
        matches!(run.outcome, Ok(AttachOutcome::Ended { .. })),
        "{:?}",
        run.outcome
    );
    let text: String = run
        .rows
        .iter()
        .filter_map(|row| row.strip_prefix("text:"))
        .collect();
    assert_eq!(text, "worldHalf rest.", "{:?}", run.rows);
    let position = |row: &str| run.rows.iter().position(|r| r == row);
    assert!(
        position("tool_call:t1") < run.rows.iter().position(|r| r.starts_with("text:Half")),
        "the held text follows the replay: {:?}",
        run.rows
    );
}

/// An agent with no message ids is matched by position, and with no raw SDK
/// frames the turn settles when the process reads `IDLE`.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn without_frames_the_attach_settles_when_the_process_reads_idle() {
    let probe = ScriptedProbe::new(
        &[RemoteProcessStatus::Running, RemoteProcessStatus::Running],
        RemoteProcessStatus::Idle,
    );
    let run = attach("plain", Arc::clone(&probe), true, None, false).await;
    assert_eq!(
        run.outcome,
        Ok(AttachOutcome::Ended {
            result: None,
            latest_prompt: LatestPrompt::Replayed,
            background_work_live: false,
        })
    );
    assert_eq!(run.rows, ["text:More."]);
    assert!(probe.reads.load(Ordering::SeqCst) >= 3);
    assert!(!run.log.contains("--command-id"), "{}", run.log);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_process_that_is_gone_settles_process_gone() {
    let probe = ScriptedProbe::new(
        &[],
        RemoteProcessStatus::Gone {
            message: Some("bloxlet restarted while command was running".to_string()),
        },
    );
    let run = attach("plain", probe, true, None, false).await;
    assert_eq!(
        run.outcome,
        Ok(AttachOutcome::ProcessGone {
            message: Some("bloxlet restarted while command was running".to_string()),
            latest_prompt: LatestPrompt::Replayed,
        })
    );
}

/// `Requested ACP process has exited`: the turn is over and a new process
/// started. The attach only backfills, from the new process's replay, and
/// never waits on a status.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_replaced_process_is_backfilled_and_settles_process_gone() {
    let probe = ScriptedProbe::running();
    let run = attach(
        "replaced",
        Arc::clone(&probe),
        true,
        Some("27294439"),
        false,
    )
    .await;
    assert_eq!(
        run.outcome,
        Ok(AttachOutcome::ProcessGone {
            message: Some("bloxlet restarted while command was running".to_string()),
            latest_prompt: LatestPrompt::Replayed,
        })
    );
    assert_eq!(run.rows, ["text:world", "tool_call:t1"]);
    assert_eq!(probe.reads.load(Ordering::SeqCst), 0);
}

/// `Received EOF signal from orchestrator` is a lost transport, and the
/// teardown after it sends no SIGINT.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn an_orchestrator_eof_settles_transport_lost_without_sigint() {
    let run = attach("eof", ScriptedProbe::running(), true, None, false).await;
    assert_eq!(run.outcome, Ok(AttachOutcome::TransportLost));
    assert!(run.caught_up);
    assert!(!run.log.contains("SIGINT"), "{}", run.log);
}

/// A process other sessions share is never SIGINTed, even at a normal end.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_shared_process_attach_tears_down_without_sigint() {
    let run = attach("claude", ScriptedProbe::running(), false, None, false).await;
    assert!(
        matches!(run.outcome, Ok(AttachOutcome::Ended { .. })),
        "{:?}",
        run.outcome
    );
    assert!(!run.log.contains("SIGINT"), "{}", run.log);
}

/// Detaching from a turn still running settles `Live`, and leaves the
/// process alone: no cancel, no SIGINT.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn detaching_from_a_running_turn_settles_live() {
    let run = attach("hang", ScriptedProbe::running(), true, None, true).await;
    assert_eq!(run.outcome, Ok(AttachOutcome::Live));
    assert!(!run.log.contains("session/cancel"), "{}", run.log);
    assert!(!run.log.contains("SIGINT"), "{}", run.log);
}

/// Attach-cancel: `initialize`, then `session/cancel` for the session, with
/// no load; the session's own `idle` confirms it.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn attach_cancel_cancels_without_loading_and_confirms_on_idle() {
    let (outcome, log) =
        attach_cancel("cancel", ScriptedProbe::running(), Duration::from_secs(10)).await;
    assert_eq!(outcome, Ok(CancelOutcome::Stopped));
    assert!(log.contains("--command-id=27294439"), "{log}");
    assert!(log.contains("\"method\":\"initialize\""), "{log}");
    assert!(
        log.contains("\"method\":\"session/cancel\"") && log.contains("\"sessionId\":\"sess-1\""),
        "{log}"
    );
    assert!(!log.contains("session/load"), "{log}");
}

/// With no frames, the process reading `IDLE` confirms the cancel.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn attach_cancel_falls_back_to_the_process_status() {
    let probe = ScriptedProbe::new(&[RemoteProcessStatus::Running], RemoteProcessStatus::Idle);
    let (outcome, log) = attach_cancel("cancel-silent", probe, Duration::from_secs(10)).await;
    assert_eq!(outcome, Ok(CancelOutcome::Stopped));
    assert!(log.contains("\"method\":\"session/cancel\""), "{log}");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn attach_cancel_is_unconfirmed_past_its_deadline() {
    let (outcome, log) = attach_cancel(
        "cancel-silent",
        ScriptedProbe::running(),
        Duration::from_millis(1500),
    )
    .await;
    assert_eq!(outcome, Ok(CancelOutcome::Unconfirmed));
    assert!(log.contains("\"method\":\"session/cancel\""), "{log}");
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn attach_cancel_on_a_replaced_process_sends_nothing() {
    let (outcome, log) = attach_cancel(
        "replaced",
        ScriptedProbe::running(),
        Duration::from_secs(10),
    )
    .await;
    assert_eq!(
        outcome,
        Ok(CancelOutcome::ProcessGone {
            message: Some("bloxlet restarted while command was running".to_string())
        })
    );
    assert!(!log.contains("session/cancel"), "{log}");
}

/// Stop on a disconnected session: the cancel goes out first, and once the
/// session is idle a `session/load` on the same connection backfills what
/// the transcript lacks. The running tool call ends as the agent's
/// transcript has it, failed with the interrupt's result, which replaces
/// the description stored as its result; the frames the cancel produced
/// live are left to the replay.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn attach_cancel_catches_up_after_the_cancel_and_ends_the_running_call() {
    let CaughtUp {
        outcome,
        latest_prompt,
        rows,
        log,
        heard,
    } = attach_cancel_with(
        "interrupt",
        ScriptedProbe::running(),
        Duration::from_secs(10),
        Some(stored_with_running_call()),
    )
    .await;
    assert_eq!(outcome, Ok(CancelOutcome::Stopped));
    // The replay re-sent the newest stored prompt, as an attach would say.
    assert_eq!(latest_prompt, Some(LatestPrompt::Replayed));
    assert_eq!(
        rows,
        [
            "text:world",
            "status:t1:failed",
            "tool_result:t1:Interrupted by user",
        ]
    );
    let cancel = log.find("\"method\":\"session/cancel\"").expect("{log}");
    let load = log.find("\"method\":\"session/load\"").expect("{log}");
    assert!(cancel < load, "the cancel goes out first: {log}");
    assert!(log.contains("emitRawSDKMessages"), "{log}");
    // The outcome is out before the load: a caller can end the session
    // without waiting on the catch-up.
    let (heard, rows_then, log_then) = heard.expect("on_cancelled is called");
    assert_eq!(heard, CancelOutcome::Stopped);
    assert!(rows_then.is_empty(), "{rows_then:?}");
    assert!(
        log_then.contains("\"method\":\"session/cancel\""),
        "{log_then}"
    );
    assert!(!log_then.contains("session/load"), "{log_then}");
}

/// A call the agent settled live only, and the replay leaves running, ends
/// with the settling the cancel produced.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn attach_cancel_records_a_live_only_settling_the_replay_lacks() {
    let CaughtUp { outcome, rows, .. } = attach_cancel_with(
        "close",
        ScriptedProbe::running(),
        Duration::from_secs(10),
        Some(stored_with_running_call()),
    )
    .await;
    assert_eq!(outcome, Ok(CancelOutcome::Stopped));
    assert_eq!(
        rows,
        [
            "text:world",
            "status:t1:failed",
            "tool_result:t1:Interrupted"
        ]
    );
}

/// A replay that ends on the prompt before the newest stored one says the
/// agent never received it, as an attach's does.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn attach_cancel_says_when_the_newest_prompt_never_arrived() {
    let mut stored = stored_with_running_call();
    stored.push(ReplayBoundary {
        role: "user".to_string(),
        content: "Now add a test".to_string(),
        acp_message_id: None,
        acp_tool_call_id: None,
        acp_tool_status: None,
    });
    let caught_up = attach_cancel_with(
        "close",
        ScriptedProbe::running(),
        Duration::from_secs(10),
        Some(stored),
    )
    .await;
    assert_eq!(caught_up.outcome, Ok(CancelOutcome::Stopped));
    assert_eq!(caught_up.latest_prompt, Some(LatestPrompt::Missing));
}

/// An unconfirmed cancel loads nothing: the turn may still be going.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn an_unconfirmed_attach_cancel_does_not_catch_up() {
    let CaughtUp {
        outcome,
        rows,
        log,
        heard,
        ..
    } = attach_cancel_with(
        "cancel-silent",
        ScriptedProbe::running(),
        Duration::from_millis(1500),
        Some(stored_with_running_call()),
    )
    .await;
    assert_eq!(outcome, Ok(CancelOutcome::Unconfirmed));
    assert!(rows.is_empty(), "{rows:?}");
    assert!(!log.contains("session/load"), "{log}");
    assert_eq!(
        heard.map(|(outcome, ..)| outcome),
        Some(CancelOutcome::Unconfirmed),
        "the outcome is heard whether or not a catch-up follows"
    );
}

/// A replaced process sends no cancel, and the catch-up backfills from the
/// new process's replay.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn attach_cancel_on_a_replaced_process_catches_up_from_its_replay() {
    let CaughtUp {
        outcome, rows, log, ..
    } = attach_cancel_with(
        "replaced",
        ScriptedProbe::running(),
        Duration::from_secs(10),
        Some(stored_with_running_call()),
    )
    .await;
    assert_eq!(
        outcome,
        Ok(CancelOutcome::ProcessGone {
            message: Some("bloxlet restarted while command was running".to_string())
        })
    );
    assert_eq!(rows, ["text:world"]);
    assert!(!log.contains("session/cancel"), "{log}");
    assert!(log.contains("\"method\":\"session/load\""), "{log}");
}

/// A transcript that can't be read costs the catch-up, never the cancel.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn an_unreadable_transcript_cancels_without_catching_up() {
    struct UnreadableStore;
    impl Store for UnreadableStore {
        fn set_agent_session_id(&self, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn get_session_replay_boundaries(&self, _: &str) -> Result<Vec<ReplayBoundary>, String> {
            Err("database is locked".to_string())
        }
    }
    let log = log_path("cancel");
    let transcript = Arc::new(TranscriptLog::default());
    let local = tokio::task::LocalSet::new();
    let outcome = local
        .run_until(async {
            let store: Arc<dyn Store> = Arc::new(UnreadableStore);
            let writer: Arc<dyn MessageWriter> = transcript.clone();
            driver(&log, "cancel", ScriptedProbe::running(), true)
                .attach_cancel_with_timing(
                    "staged-session",
                    "sess-1",
                    Some("27294439"),
                    &std::env::temp_dir(),
                    Some(CatchUpThrough {
                        store: &store,
                        writer: &writer,
                        on_cancelled: Box::new(|_: &CancelOutcome| {}),
                    }),
                    Duration::from_secs(10),
                    TIMING,
                )
                .await
        })
        .await;
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    assert_eq!(
        outcome,
        Ok((CancelOutcome::Stopped, LatestPrompt::Unknown)),
        "nothing loaded tells nothing about the prompt"
    );
    assert!(text.contains("\"method\":\"session/cancel\""), "{text}");
    assert!(!text.contains("session/load"), "{text}");
    assert!(transcript.rows().is_empty());
}
