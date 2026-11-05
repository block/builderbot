//! Sessions sharing one remote agent process through one `sq` proxy.

use super::*;
use crate::driver::{
    is_remote_agent_detached_error, is_remote_agent_lost_error, ReattachTurn, SessionConnection,
};
use nix::sys::signal::kill;
use nix::unistd::Pid;
use std::os::unix::fs::PermissionsExt;
use tokio::task::LocalSet;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(30);

/// Stands in for `sq`. As `sq blox acp` it is a fake ACP agent: each launch
/// records its pid, every frame it reads, and the SIGINT that stops it, and
/// logs its bloxlet command id (`cmd-<pid>`) the way `sq` does — or, when
/// `reuse` holds a command id, logs attaching to that earlier process. It
/// answers setup and prompts at once, except prompts saying
/// - "hold me": answered only once cancelled;
/// - "stall": starts a tool call and goes quiet;
/// - "disconnect": logs a stream disconnect and goes quiet;
/// - "orchestrator eof": answers, then logs that the orchestrator ended its
///   output stream, as `sq` does once the agent process exits;
/// - "exit": ends its output.
///
/// A session it loads while reusing a process is one an earlier connection
/// prompted: once `finish` exists, the agent streams the rest of that turn,
/// then answers the earlier connection's prompt with `owner_response`, if
/// that exists. A cancel with no held prompt answers `blocked_prompt`, if
/// that exists, as cancelled: the earlier connection's turn was blocked.
///
/// As `sq blox ws commands` it prints `active.json` or `history.json` and
/// records the call in `checks`.
const FAKE_SQ: &str = r#"#!/bin/sh
dir='@DIR@'
if [ "$2" = ws ]; then
  echo "$*" >> "$dir/checks"
  case "$*" in
    *--status=active*) cat "$dir/active.json" 2>/dev/null || echo '[]' ;;
    *) cat "$dir/history.json" 2>/dev/null || echo '[]' ;;
  esac
  exit 0
fi
echo $$ >> "$dir/launches"
if [ -f "$dir/reuse" ]; then
  printf '  0.42s \033[92mINF\033[0m Found existing ACP process, reusing \033[2mprocess_id=\033[0m%s\n' "$(cat "$dir/reuse")" >&2
else
  printf '  0.42s \033[92mINF\033[0m Started new ACP process \033[2mprocess_id=\033[0mcmd-%s\n' $$ >&2
fi
trap 'echo interrupted >> "$dir/signals"; exit 0' INT
n=0
held=
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$dir/received"
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":"\(staged-[^"]*\)".*/\1/p')
  result='{}'
  case "$line" in
    *'"method":"initialize"'*) result='{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}' ;;
    *'"method":"session/new"'*) n=$((n+1)); result="{\"sessionId\":\"sess-$$-$n\"}" ;;
    *'"method":"session/load"'*)
      if [ -f "$dir/reuse" ]; then
        sid=$(printf '%s\n' "$line" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')
        (
          while [ ! -f "$dir/finish" ] && kill -0 $$ 2>/dev/null; do sleep 0.02; done
          printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Committed the fix."}}}}\n' "$sid"
          cat "$dir/owner_response" 2>/dev/null
        ) &
      fi ;;
    *'"method":"session/prompt"'*'hold me'*) held=$id; continue ;;
    *'"method":"session/prompt"'*'stall'*)
      sid=$(printf '%s\n' "$line" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"tool_call","toolCallId":"tool-1","title":"Run the tests","status":"in_progress"}}}\n' "$sid"
      continue ;;
    *'"method":"session/prompt"'*'disconnect'*)
      echo '  9.10s WRN Stream disconnected, retrying error="stream error: unavailable"' >&2
      continue ;;
    *'"method":"session/prompt"'*'orchestrator eof'*)
      printf '{"jsonrpc":"2.0","id":"%s","result":{"stopReason":"end_turn"}}\n' "$id"
      echo '  9.10s INF Received EOF signal from orchestrator, connection closed' >&2
      continue ;;
    *'"method":"session/prompt"'*'exit'*) exit 0 ;;
    *'"method":"session/prompt"'*) result='{"stopReason":"end_turn"}' ;;
    *'"method":"session/cancel"'*)
      if [ -n "$held" ]; then
        printf '{"jsonrpc":"2.0","id":"%s","result":{"stopReason":"cancelled"}}\n' "$held"
        held=
      elif [ -f "$dir/blocked_prompt" ]; then
        printf '{"jsonrpc":"2.0","id":"%s","result":{"stopReason":"cancelled"}}\n' "$(cat "$dir/blocked_prompt")"
        rm -f "$dir/blocked_prompt"
      fi
      continue ;;
  esac
  if [ -n "$id" ]; then
    printf '{"jsonrpc":"2.0","id":"%s","result":%s}\n' "$id" "$result"
  fi
done
"#;

struct FakeSq {
    dir: PathBuf,
}

impl FakeSq {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "acp-remote-process-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("sq");
        std::fs::write(
            &script,
            FAKE_SQ.replace("@DIR@", &dir.display().to_string()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    /// A remote driver whose `sq` is the fake.
    fn driver(&self) -> AcpDriver {
        self.driver_with_args(vec![])
    }

    /// A remote driver for workspace `test-ws`, as `AcpDriver::for_workspace`
    /// builds it, so its proxy can ask bloxlet about its agent.
    fn workspace_driver(&self) -> AcpDriver {
        self.driver_with_args(blox_cli::acp_proxy_args("test-ws", None))
    }

    fn driver_with_args(&self, acp_args: Vec<String>) -> AcpDriver {
        AcpDriver {
            binary_path: self.dir.join("sq"),
            acp_args,
            agent_label: String::from("Blox"),
            is_remote: true,
            extra_env: vec![],
            env_snapshot: None,
            interpreter_env_snapshot: None,
            mcp_servers: vec![],
            remote_working_dir: Some(PathBuf::from("/home/bloxer/repo")),
            background_hold: None,
            background_hold_observer: None,
            steering_disabled: false,
        }
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.join(name)).unwrap_or_default()
    }

    /// The pid of each fake agent process launched.
    fn launches(&self) -> Vec<i32> {
        self.read("launches")
            .lines()
            .map(|pid| pid.parse().unwrap())
            .collect()
    }

    /// Have `sq` attach to the earlier agent process `command_id` instead of
    /// starting one.
    fn reuse(&self, command_id: &str) {
        std::fs::write(self.dir.join("reuse"), command_id).unwrap();
    }

    /// Let the agent finish the turn a reattached session's earlier
    /// connection prompted.
    fn finish_turn(&self) {
        std::fs::write(self.dir.join("finish"), "").unwrap();
    }

    /// Have the agent answer the earlier connection's prompt, request `id`,
    /// once it finishes the turn.
    fn answer_earlier_prompt(&self, id: &str) {
        std::fs::write(
            self.dir.join("owner_response"),
            format!(
                "{}\n",
                serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "end_turn"}})
            ),
        )
        .unwrap();
    }

    /// Leave the earlier connection's prompt, request `id`, blocked until a
    /// `session/cancel` arrives, as on a permission request nobody answers.
    fn block_earlier_prompt(&self, id: &str) {
        std::fs::write(self.dir.join("blocked_prompt"), id).unwrap();
    }

    /// Answer `sq blox ws commands` with these active commands and full
    /// history.
    fn set_commands(&self, active: &str, history: &str) {
        std::fs::write(self.dir.join("active.json"), active).unwrap();
        std::fs::write(self.dir.join("history.json"), history).unwrap();
    }

    /// The bloxlet command id the (only) launch announced.
    async fn command_id(&self) -> String {
        wait_until("the agent launches", || !self.launches().is_empty()).await;
        format!("cmd-{}", self.launches()[0])
    }

    /// How many times bloxlet was asked about the agent.
    fn checks(&self) -> usize {
        self.read("checks")
            .lines()
            .filter(|line| line.contains("--status=active"))
            .count()
    }

    fn interrupted(&self) -> bool {
        self.read("signals").contains("interrupted")
    }

    /// Every frame the agent read with this method.
    fn received(&self, method: &str) -> Vec<String> {
        let needle = format!("\"method\":\"{method}\"");
        self.read("received")
            .lines()
            .filter(|line| line.contains(&needle))
            .map(str::to_string)
            .collect()
    }
}

impl Drop for FakeSq {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn alive(pid: i32) -> bool {
    kill(Pid::from_raw(pid), None).is_ok()
}

struct Session {
    connection: SessionConnection,
    store: Arc<RecordingStore>,
    cancel: CancellationToken,
}

impl Session {
    async fn connect(driver: &AcpDriver, sq: &FakeSq, name: &str) -> Self {
        let store = Arc::new(RecordingStore::default());
        let dyn_store: Arc<dyn Store> = Arc::clone(&store) as Arc<dyn Store>;
        let writer: Arc<dyn MessageWriter> = Arc::new(BasicMessageWriter::new());
        let cancel = CancellationToken::new();
        let connection = timeout(
            WAIT,
            driver.connect(name, &sq.dir, &dyn_store, &writer, &cancel, None, &[], None),
        )
        .await
        .expect("connect should not hang")
        .expect("the fake sq should spawn");
        Self {
            connection,
            store,
            cancel,
        }
    }

    async fn prompt(&mut self, text: &str) -> Result<AgentRunOutcome, String> {
        timeout(WAIT, self.connection.prompt(text, &[]))
            .await
            .expect("the prompt should resolve")
    }

    fn agent_session_id(&self) -> String {
        self.store.agent_session_ids.lock().unwrap()[0].1.clone()
    }
}

async fn wait_until(what: &str, ready: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !ready() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_shared_remote_agent_stops_only_when_its_last_session_finishes() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("last-out");
            let driver = sq.driver();
            let mut first = Session::connect(&driver, &sq, "first").await;
            let mut second = Session::connect(&driver, &sq, "second").await;

            let outcome = first.prompt("one").await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            let launches = sq.launches();
            assert_eq!(launches.len(), 1, "both sessions should share one proxy");
            assert!(
                alive(launches[0]) && !sq.interrupted(),
                "the first session finishing must not stop the agent the second still uses"
            );

            let outcome = second.prompt("two").await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert!(
                sq.interrupted() && !alive(launches[0]),
                "the last session finishing should stop the proxy"
            );

            // The next session starts its own proxy instead of joining the
            // stopped one.
            let mut third = Session::connect(&driver, &sq, "third").await;
            let outcome = third.prompt("three").await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert_eq!(sq.launches().len(), 2);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_a_session_on_a_shared_remote_agent_cancels_only_that_session() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("cancel");
            let driver = sq.driver();
            let mut first = Session::connect(&driver, &sq, "first").await;
            let mut second = Session::connect(&driver, &sq, "second").await;

            let cancel_first = first.cancel.clone();
            let held = tokio::task::spawn_local(async move {
                let outcome = first.prompt("hold me").await;
                (first, outcome)
            });
            wait_until("the agent holds the first prompt", || {
                !sq.received("session/prompt").is_empty()
            })
            .await;
            cancel_first.cancel();
            let (first, outcome) = held.await.unwrap();
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Cancelled)),
                "{outcome:?}"
            );

            let cancels = sq.received("session/cancel");
            assert_eq!(cancels.len(), 1, "{cancels:?}");
            assert!(
                cancels[0].contains(&format!("\"sessionId\":\"{}\"", first.agent_session_id())),
                "the cancel should name only the cancelled session: {cancels:?}"
            );
            assert!(
                !sq.interrupted(),
                "a cancel must not stop the agent another session still uses"
            );

            let outcome = second.prompt("two").await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert_ne!(second.agent_session_id(), first.agent_session_id());
            assert_eq!(sq.launches().len(), 1);
            assert!(sq.interrupted());
        })
        .await;
}

/// The agent lost under two sessions mid-turn ends both with the same
/// remote-agent-lost error, naming why.
async fn assert_both_lost(
    first: tokio::task::JoinHandle<(Session, Result<AgentRunOutcome, String>)>,
    second: tokio::task::JoinHandle<(Session, Result<AgentRunOutcome, String>)>,
    cause: &[&str],
) {
    let (_, first) = first.await.unwrap();
    let (_, second) = second.await.unwrap();
    let error = first.expect_err("the first session's agent was lost");
    assert!(is_remote_agent_lost_error(&error), "{error}");
    assert!(
        cause.iter().any(|cause| error.contains(cause)),
        "the error should name the cause: {error}"
    );
    assert_eq!(second, Err(error), "both sessions lost the same agent");
}

fn prompt_in_background(
    mut session: Session,
    text: &'static str,
) -> tokio::task::JoinHandle<(Session, Result<AgentRunOutcome, String>)> {
    tokio::task::spawn_local(async move {
        let outcome = session.prompt(text).await;
        (session, outcome)
    })
}

#[tokio::test(flavor = "current_thread")]
async fn sessions_fail_as_remote_agent_lost_when_the_agent_output_ends() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("eof");
            let driver = sq.driver();
            let first = Session::connect(&driver, &sq, "first").await;
            let second = Session::connect(&driver, &sq, "second").await;

            let first = prompt_in_background(first, "hold me");
            wait_until("the agent holds the first prompt", || {
                !sq.received("session/prompt").is_empty()
            })
            .await;
            let second = prompt_in_background(second, "exit");

            assert_both_lost(
                first,
                second,
                &["output stream ended", "`sq blox acp` proxy exited"],
            )
            .await;
        })
        .await;
}

/// `sq` reattached to a process it did not start, so its exiting leaves that
/// process running: while bloxlet still runs it, the sessions are detached
/// (for their turns to be reattached to) rather than failed.
#[tokio::test(flavor = "current_thread")]
async fn sessions_detach_when_sq_exits_while_bloxlet_still_runs_the_reused_agent() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reused-exit");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_RUNNING"}]"#,
                "[]",
            );
            let driver = sq.workspace_driver();
            let first = Session::connect(&driver, &sq, "first").await;
            let second = Session::connect(&driver, &sq, "second").await;

            let first = prompt_in_background(first, "hold me");
            wait_until("the agent holds the first prompt", || {
                !sq.received("session/prompt").is_empty()
            })
            .await;
            let second = prompt_in_background(second, "exit");

            let (_, first) = first.await.unwrap();
            let (_, second) = second.await.unwrap();
            let error = first.expect_err("the proxy went");
            assert!(
                is_remote_agent_detached_error(&error) && !is_remote_agent_lost_error(&error),
                "{error}"
            );
            assert!(
                error.contains("bloxlet still runs the agent process (command cmd-earlier)"),
                "{error}"
            );
            assert_eq!(second, Err(error));
            assert!(
                sq.checks() > 0,
                "bloxlet is asked before anything is failed"
            );
            assert!(
                sq.received("session/cancel").is_empty(),
                "detached turns are left running for a reattachment"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn sessions_fail_as_remote_agent_lost_when_sq_exits_and_bloxlet_reports_the_reused_agent_gone(
) {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reused-exit-gone");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                "[]",
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_FAILED","message":"agent crashed"}]"#,
            );
            let driver = sq.workspace_driver();
            let first = Session::connect(&driver, &sq, "first").await;
            let second = Session::connect(&driver, &sq, "second").await;

            let first = prompt_in_background(first, "hold me");
            wait_until("the agent holds the first prompt", || {
                !sq.received("session/prompt").is_empty()
            })
            .await;
            let second = prompt_in_background(second, "exit");

            assert_both_lost(
                first,
                second,
                &["bloxlet reports the agent process (command cmd-earlier) as failed: agent crashed"],
            )
            .await;
        })
        .await;
}

/// `sq` only logs that the orchestrator ended the agent's output stream and
/// keeps its own stdout open, so that line alone prompts asking bloxlet —
/// here with no prompt in flight that could have gone quiet.
#[tokio::test(flavor = "current_thread")]
async fn the_orchestrator_ending_the_agent_stream_prompts_an_immediate_bloxlet_check() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("orchestrator-eof");
            let driver = sq.workspace_driver();
            let mut first = Session::connect(&driver, &sq, "first").await;
            // Keeps the proxy up once the first session's turn is over.
            let mut second = Session::connect(&driver, &sq, "second").await;
            let command = sq.command_id().await;
            sq.set_commands(
                "[]",
                &format!(
                    r#"[{{"commandId":"{command}","status":"COMMAND_STATUS_FAILED","message":"agent exited"}}]"#
                ),
            );

            let outcome = first.prompt("orchestrator eof").await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            wait_until("bloxlet is asked about the agent", || sq.checks() > 0).await;
            wait_until("the proxy is stopped", || sq.interrupted()).await;

            let error = second.prompt("next").await.expect_err("the agent is gone");
            assert!(is_remote_agent_lost_error(&error), "{error}");
            assert!(error.contains("agent exited"), "{error}");
            assert_eq!(
                sq.received("session/prompt").len(),
                1,
                "the second session's prompt never reached the lost agent"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn sessions_fail_as_remote_agent_lost_once_bloxlet_reports_a_quiet_prompts_agent_failed() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("stalled");
            let driver = sq.workspace_driver();
            let first = Session::connect(&driver, &sq, "first").await;
            let second = Session::connect(&driver, &sq, "second").await;
            let command = sq.command_id().await;
            sq.set_commands(
                &format!(r#"[{{"commandId":"{command}","status":"COMMAND_STATUS_RUNNING"}}]"#),
                "[]",
            );

            // One goes quiet mid tool call, the other without starting one.
            let first = prompt_in_background(first, "stall");
            let second = prompt_in_background(second, "hold me");
            wait_until("bloxlet is asked about the quiet prompts", || {
                sq.checks() > 0
            })
            .await;
            assert!(
                !first.is_finished() && !second.is_finished(),
                "an agent bloxlet still runs is left alone"
            );

            // bloxlet restarts under the agent, which the stream never shows.
            sq.set_commands(
                "[]",
                &format!(
                    r#"[{{"commandId":"{command}","status":"COMMAND_STATUS_FAILED","message":"bloxlet restarted while command was running"}}]"#
                ),
            );
            assert_both_lost(
                first,
                second,
                &[&format!(
                    "bloxlet reports the agent process (command {command}) as failed: \
                     bloxlet restarted while command was running"
                )],
            )
            .await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn sessions_fail_as_remote_agent_lost_when_a_reconnect_finds_the_agent_gone() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reconnect");
            let driver = sq.workspace_driver();
            let first = Session::connect(&driver, &sq, "first").await;
            let second = Session::connect(&driver, &sq, "second").await;
            let command = sq.command_id().await;

            let first = prompt_in_background(first, "hold me");
            wait_until("the agent holds the first prompt", || {
                !sq.received("session/prompt").is_empty()
            })
            .await;
            let second = prompt_in_background(second, "disconnect");

            assert_both_lost(
                first,
                second,
                &[&format!(
                    "bloxlet no longer lists the agent process (command {command}) as running"
                )],
            )
            .await;
        })
        .await;
}

fn reattach_in_background(
    driver: AcpDriver,
    sq: &FakeSq,
    writer: Arc<BasicMessageWriter>,
) -> tokio::task::JoinHandle<Result<AgentRunOutcome, String>> {
    reattach_turn_in_background(driver, sq, writer, now_ms(), None, false)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn reattach_turn_in_background(
    driver: AcpDriver,
    sq: &FakeSq,
    writer: Arc<BasicMessageWriter>,
    prompted_at: i64,
    owner_pid: Option<u32>,
    sole_turn_of_owner: bool,
) -> tokio::task::JoinHandle<Result<AgentRunOutcome, String>> {
    let dir = sq.dir.clone();
    tokio::task::spawn_local(async move {
        let store: Arc<dyn Store> = Arc::new(RecordingStore::default());
        let writer: Arc<dyn MessageWriter> = writer;
        timeout(
            WAIT,
            driver.reattach(
                "staged-session",
                &dir,
                &store,
                &writer,
                &CancellationToken::new(),
                &ReattachTurn {
                    agent_session_id: "sess-earlier".to_string(),
                    prompted_at,
                    owner_pid,
                    sole_turn_of_owner,
                },
            ),
        )
        .await
        .expect("the reattachment should end")
    })
}

#[tokio::test(flavor = "current_thread")]
async fn a_reattached_turn_whose_agent_survived_is_recorded_until_bloxlet_reports_it_idle() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_RUNNING"}]"#,
                "[]",
            );
            let writer = Arc::new(BasicMessageWriter::new());
            let reattached = reattach_in_background(sq.workspace_driver(), &sq, writer.clone());

            wait_until("bloxlet is asked whether the turn has ended", || {
                sq.checks() > 0
            })
            .await;
            assert!(
                !reattached.is_finished(),
                "a turn bloxlet still runs is waited on"
            );
            let loads = sq.received("session/load");
            assert_eq!(loads.len(), 1, "{loads:?}");
            assert!(
                loads[0].contains("\"sessionId\":\"sess-earlier\""),
                "{loads:?}"
            );

            // The agent finishes the turn the earlier connection prompted.
            sq.finish_turn();
            let deadline = tokio::time::Instant::now() + WAIT;
            while writer.get_text().await != "Committed the fix." {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the rest of the turn should be recorded: {:?}",
                    writer.get_text().await
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_IDLE"}]"#,
                "[]",
            );

            let outcome = reattached.await.unwrap();
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert!(
                sq.received("session/prompt").is_empty(),
                "reattaching prompts nothing"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reattaching_to_a_turn_whose_agent_is_gone_fails_as_remote_agent_lost() {
    LocalSet::new()
        .run_until(async {
            // No earlier process to attach to, so `sq` starts one.
            let sq = FakeSq::new("reattach-gone");
            let writer = Arc::new(BasicMessageWriter::new());

            let outcome = reattach_in_background(sq.workspace_driver(), &sq, writer)
                .await
                .unwrap();
            let error = outcome.expect_err("the process that ran the turn is gone");
            assert!(is_remote_agent_lost_error(&error), "{error}");
            assert!(error.contains("started a new one"), "{error}");
            assert_eq!(sq.checks(), 0, "bloxlet need not be asked");
            assert!(
                sq.interrupted(),
                "the process `sq` started for the reattachment is stopped again"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reattaching_to_a_turn_bloxlet_reports_failed_fails_as_remote_agent_lost() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-failed");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                "[]",
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_FAILED","message":"bloxlet restarted while command was running"}]"#,
            );
            let writer = Arc::new(BasicMessageWriter::new());

            let outcome = reattach_in_background(sq.workspace_driver(), &sq, writer)
                .await
                .unwrap();
            let error = outcome.expect_err("bloxlet reports the agent stopped");
            assert!(is_remote_agent_lost_error(&error), "{error}");
            assert!(
                error.contains(
                    "bloxlet reports the agent process (command cmd-earlier) as failed: \
                     bloxlet restarted while command was running"
                ),
                "{error}"
            );
        })
        .await;
}

/// A reattachment that joins a proxy another session already holds cannot
/// tell from `sq` whether the process it attached to ran the turn, so it asks
/// bloxlet when that process started.
#[tokio::test(flavor = "current_thread")]
async fn a_reattachment_joining_a_running_proxy_checks_the_process_predates_the_turn() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-joined");
            let driver = sq.workspace_driver();
            // Another session's proxy started a fresh process.
            let _holder = Session::connect(&driver, &sq, "holder").await;
            let command = sq.command_id().await;
            sq.set_commands(
                &format!(
                    r#"[{{"commandId":"{command}","status":"COMMAND_STATUS_IDLE","startedAt":"2026-11-05T01:00:00Z"}}]"#
                ),
                "[]",
            );
            let started_at_ms = 1_793_840_400_000;

            let error = reattach_turn_in_background(
                sq.workspace_driver(),
                &sq,
                Arc::new(BasicMessageWriter::new()),
                started_at_ms - 60_000,
                None,
                false,
            )
            .await
            .unwrap()
            .expect_err("the process serving the workspace started after the turn");
            assert!(is_remote_agent_lost_error(&error), "{error}");
            assert!(
                error.contains("started after the turn was prompted"),
                "{error}"
            );

            let outcome = reattach_turn_in_background(
                sq.workspace_driver(),
                &sq,
                Arc::new(BasicMessageWriter::new()),
                started_at_ms + 60_000,
                None,
                false,
            )
            .await
            .unwrap();
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "a process already running when the turn was prompted is the one that ran it: \
                 {outcome:?}"
            );
            assert_eq!(sq.launches().len(), 1, "both reattachments joined the proxy");
        })
        .await;
}

/// bloxlet reports idle per process, so with another session keeping the
/// process busy only the agent's answer to the dead instance's prompt tells
/// that the reattached turn has ended.
#[tokio::test(flavor = "current_thread")]
async fn a_reattached_turn_ends_when_the_agent_answers_its_owners_prompt() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-answered");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_RUNNING"}]"#,
                "[]",
            );
            // Another instance's prompt ends first, without ending this turn.
            sq.answer_earlier_prompt("staged-424242-3:9f1c");
            let writer = Arc::new(BasicMessageWriter::new());
            let reattached = reattach_turn_in_background(
                sq.workspace_driver(),
                &sq,
                writer.clone(),
                now_ms(),
                Some(424_242),
                true,
            );
            wait_until("bloxlet is asked about the turn", || sq.checks() > 0).await;

            sq.finish_turn();
            let outcome = reattached.await.unwrap();
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert_eq!(writer.get_text().await, "Committed the fix.");
            assert!(
                sq.received("session/cancel").is_empty(),
                "a turn that ended is not cancelled"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn another_instances_prompt_ending_does_not_end_a_reattached_turn() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-foreign-answer");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_RUNNING"}]"#,
                "[]",
            );
            sq.answer_earlier_prompt("staged-515151-3:9f1c");
            let reattached = reattach_turn_in_background(
                sq.workspace_driver(),
                &sq,
                Arc::new(BasicMessageWriter::new()),
                now_ms(),
                Some(424_242),
                true,
            );
            sq.finish_turn();
            wait_until("bloxlet is asked about the turn twice", || {
                sq.checks() > 1 || reattached.is_finished()
            })
            .await;
            // Under load the quiet cap can beat the second check; a turn it
            // ended was cancelled first, which a foreign answer never is.
            assert!(
                !reattached.is_finished() || !sq.received("session/cancel").is_empty(),
                "the other instance's answer ended the turn"
            );

            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_IDLE"}]"#,
                "[]",
            );
            let outcome = reattached.await.unwrap();
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
        })
        .await;
}

/// A turn that ended while other sessions keep the process busy looks like
/// this: bloxlet runs the process, and the turn's session hears nothing. The
/// wait is capped: the turn is cancelled, and since the agent answers no
/// prompt of its owner's as cancelled, it had ended on its own.
#[tokio::test(flavor = "current_thread")]
async fn a_reattached_turn_quiet_past_the_cap_that_had_ended_is_taken_as_ended() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-quiet");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_RUNNING"}]"#,
                "[]",
            );
            let started = tokio::time::Instant::now();

            let outcome = reattach_turn_in_background(
                sq.workspace_driver(),
                &sq,
                Arc::new(BasicMessageWriter::new()),
                now_ms(),
                Some(424_242),
                false,
            )
            .await
            .unwrap();
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert!(
                started.elapsed()
                    >= crate::driver::remote_liveness::REATTACHED_TURN_QUIET_CAP
                        + crate::driver::remote_liveness::CANCELLED_TURN_ANSWER_WAIT
            );
            let cancels = sq.received("session/cancel");
            assert_eq!(cancels.len(), 1, "{cancels:?}");
            assert!(
                cancels[0].contains(r#""sessionId":"sess-earlier""#),
                "{cancels:?}"
            );
        })
        .await;
}

/// A turn the dead instance left blocked on a permission request (which is
/// not re-sent to a new connection) looks the same until it is cancelled:
/// then the agent answers its prompt as cancelled, so the turn was cut
/// short and fails as lost, rather than reading as a completed one.
#[tokio::test(flavor = "current_thread")]
async fn a_reattached_turn_quiet_past_the_cap_that_was_blocked_fails_as_lost() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-blocked");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_RUNNING"}]"#,
                "[]",
            );
            sq.block_earlier_prompt("staged-424242-3:9f1c");

            // Its sole turn, so the answer also reaches the watch that would
            // otherwise take it as the turn ending.
            let outcome = reattach_turn_in_background(
                sq.workspace_driver(),
                &sq,
                Arc::new(BasicMessageWriter::new()),
                now_ms(),
                Some(424_242),
                true,
            )
            .await
            .unwrap();
            let error = outcome.expect_err("a turn cut short by the cap is not completed");
            assert!(is_remote_agent_lost_error(&error), "{error}");
            assert!(error.contains("permission request"), "{error}");
            assert_eq!(sq.received("session/cancel").len(), 1);
        })
        .await;
}
