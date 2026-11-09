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
/// records its pid, every frame it reads (and the SIGINT that stops it, also
/// in `signals`), and logs its bloxlet command id (`cmd-<pid>`) the way `sq`
/// does — or, when `reuse` holds a command id, logs attaching to that
/// earlier process. It answers setup and prompts at once, except prompts
/// saying
/// - "hold me": answered only once cancelled;
/// - "stall": starts a tool call and goes quiet;
/// - "disconnect": logs a stream disconnect and goes quiet;
/// - "orchestrator eof": answers, then logs that the orchestrator ended its
///   output stream, as `sq` does once the agent process exits, and like
///   `sq` relays nothing more;
/// - "exit": ends its output;
/// - "crash": dies of SIGKILL, as an `sq` killed or cut off by the network
///   does, leaving the agent process it started running on the workstation;
/// - "flood": streams 200 chunks of 4000 bytes, then holds the prompt like
///   "hold me".
///
/// Once its stdin closes it notes that in `stdin_closed` and exits — unless
/// `linger` exists, when it keeps running until `release` does. With `silent`
/// it logs neither starting nor reusing a process.
///
/// It writes the frames in `preload`, if that exists, ahead of its answer to
/// `initialize`: the running turn's, relayed as soon as `sq` attaches.
///
/// While `hold_load` exists it never answers `session/load`. A session it
/// loads replays the frames in `replay` first, if that exists,
/// with `@SID@` standing for the session id. A session it loads while
/// reusing a process is one an earlier connection
/// prompted: once `finish` exists, the agent streams the rest of that turn,
/// then answers the earlier connection's prompt with `owner_response`, if
/// that exists. A cancel with no held prompt, naming the session in
/// `blocked_prompt`, answers the request id there as cancelled: the earlier
/// connection's turn on that session was blocked.
///
/// As `sq blox ws commands` it records the call in `checks`, then fails
/// while `failing_checks` counts down, or prints `active.json` or
/// `history.json`.
const FAKE_SQ: &str = r#"#!/bin/sh
dir='@DIR@'
if [ "$2" = ws ]; then
  echo "$*" >> "$dir/checks"
  left=$(cat "$dir/failing_checks" 2>/dev/null)
  if [ -n "$left" ] && [ "$left" -gt 0 ]; then
    echo $((left - 1)) > "$dir/failing_checks"
    echo 'bloxlet unavailable' >&2
    exit 1
  fi
  case "$*" in
    *--status=active*) cat "$dir/active.json" 2>/dev/null || echo '[]' ;;
    *) cat "$dir/history.json" 2>/dev/null || echo '[]' ;;
  esac
  exit 0
fi
echo $$ >> "$dir/launches"
if [ -f "$dir/silent" ]; then
  :
elif [ -f "$dir/reuse" ]; then
  printf '  0.42s \033[92mINF\033[0m Found existing ACP process, reusing \033[2mprocess_id=\033[0m%s\n' "$(cat "$dir/reuse")" >&2
else
  printf '  0.42s \033[92mINF\033[0m Started new ACP process \033[2mprocess_id=\033[0mcmd-%s\n' $$ >&2
fi
trap 'echo interrupted >> "$dir/signals"; echo SIGINT >> "$dir/received"; exit 0' INT
n=0
held=
eof=
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$dir/received"
  [ -n "$eof" ] && continue
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":"\(staged-[^"]*\)".*/\1/p')
  result='{}'
  case "$line" in
    *'"method":"initialize"'*)
      [ -f "$dir/preload" ] && cat "$dir/preload"
      result='{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}' ;;
    *'"method":"session/new"'*) n=$((n+1)); result="{\"sessionId\":\"sess-$$-$n\"}" ;;
    *'"method":"session/load"'*)
      [ -f "$dir/hold_load" ] && continue
      sid=$(printf '%s\n' "$line" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')
      [ -f "$dir/replay" ] && sed "s/@SID@/$sid/g" "$dir/replay"
      if [ -f "$dir/reuse" ]; then
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
      eof=1
      continue ;;
    *'"method":"session/prompt"'*'exit'*) exit 0 ;;
    *'"method":"session/prompt"'*'crash'*) kill -9 $$ ;;
    *'"method":"session/prompt"'*'flood'*)
      sid=$(printf '%s\n' "$line" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')
      pad=$(printf '%4000s' '' | tr ' ' x)
      i=0
      while [ $i -lt 200 ]; do
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"%s"}}}}\n' "$sid" "$pad"
        i=$((i+1))
      done
      held=$id; continue ;;
    *'"method":"session/prompt"'*) result='{"stopReason":"end_turn"}' ;;
    *'"method":"session/cancel"'*)
      sid=$(printf '%s\n' "$line" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p')
      if [ -n "$held" ]; then
        printf '{"jsonrpc":"2.0","id":"%s","result":{"stopReason":"cancelled"}}\n' "$held"
        held=
      elif [ -f "$dir/blocked_prompt" ] && [ "$(cut -d' ' -f1 "$dir/blocked_prompt")" = "$sid" ]; then
        printf '{"jsonrpc":"2.0","id":"%s","result":{"stopReason":"cancelled"}}\n' "$(cut -d' ' -f2 "$dir/blocked_prompt")"
        rm -f "$dir/blocked_prompt"
      fi
      continue ;;
  esac
  if [ -n "$id" ]; then
    printf '{"jsonrpc":"2.0","id":"%s","result":%s}\n' "$id" "$result"
  fi
done
echo closed >> "$dir/stdin_closed"
if [ -f "$dir/linger" ]; then
  while [ ! -f "$dir/release" ]; do sleep 0.02; done
fi
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

    /// A remote driver proxying `sq blox acp test-ws`, so its proxy can ask
    /// bloxlet about its agent. Unlike the driver `AcpDriver::for_workspace`
    /// builds ([`FakeSq::blox_driver`]) it carries no Blox process, so its
    /// sessions' last release interrupts the proxy whatever argv it runs.
    fn workspace_driver(&self) -> AcpDriver {
        self.driver_with_args(blox_cli::acp_proxy_args("test-ws", None, &[]))
    }

    /// The driver `AcpDriver::for_workspace` builds for Staged session
    /// `staged_session_id` on workspace `test-ws`, with the fake as `sq`: a
    /// known `agent_id` gets the session a dedicated process; `None` runs
    /// Blox's default agent on the process every such session shares.
    fn blox_driver(&self, agent_id: Option<&str>, staged_session_id: &str) -> AcpDriver {
        AcpDriver::for_workspace_with_binary(
            self.dir.join("sq"),
            "test-ws",
            agent_id,
            staged_session_id,
        )
        .with_remote_working_dir(PathBuf::from("/home/bloxer/repo"))
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
            unattended_session_mode: None,
            blox_process: None,
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

    /// Leave the earlier connection's prompt on `session`, request `id`,
    /// blocked until a `session/cancel` for that session arrives, as on a
    /// permission request nobody answers.
    fn block_earlier_prompt(&self, session: &str, id: &str) {
        std::fs::write(self.dir.join("blocked_prompt"), format!("{session} {id}")).unwrap();
    }

    /// Have `session/load` replay these `session/update`s, each for the
    /// loaded session, ahead of its answer.
    fn replay(&self, updates: &[serde_json::Value]) {
        let frames: String = updates
            .iter()
            .map(|update| {
                let frame = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {"sessionId": "@SID@", "update": update},
                });
                format!("{frame}\n")
            })
            .collect();
        std::fs::write(self.dir.join("replay"), frames).unwrap();
    }

    /// Have the agent relay these `session/update`s for `session` as soon as
    /// it is attached to, ahead of its answer to `initialize`.
    fn preload(&self, session: &str, updates: &[serde_json::Value]) {
        let frames: String = updates
            .iter()
            .map(|update| {
                let frame = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {"sessionId": session, "update": update},
                });
                format!("{frame}\n")
            })
            .collect();
        std::fs::write(self.dir.join("preload"), frames).unwrap();
    }

    /// Leave every `session/load` unanswered.
    fn hold_load(&self) {
        std::fs::write(self.dir.join("hold_load"), "").unwrap();
    }

    /// Fail the next `count` `sq blox ws commands` calls.
    fn fail_checks(&self, count: usize) {
        std::fs::write(self.dir.join("failing_checks"), count.to_string()).unwrap();
    }

    /// Log nothing about which agent process `sq` reached.
    fn silent(&self) {
        std::fs::write(self.dir.join("silent"), "").unwrap();
    }

    /// Keep running once stdin closes.
    fn linger(&self) {
        std::fs::write(self.dir.join("linger"), "").unwrap();
    }

    /// Whether the agent's stdin has closed.
    fn stdin_closed(&self) -> bool {
        self.dir.join("stdin_closed").exists()
    }

    /// Answer `sq blox ws commands` with these active commands and full
    /// history.
    ///
    /// History is written first. The liveness check reads the active list
    /// and consults history only when the command is missing from it, so a
    /// poll that lands between the two writes must find the new history
    /// record already complete; the other order let it read the old or a
    /// half-written history file and report the agent as "no longer listed"
    /// instead of failed.
    fn set_commands(&self, active: &str, history: &str) {
        std::fs::write(self.dir.join("history.json"), history).unwrap();
        std::fs::write(self.dir.join("active.json"), active).unwrap();
    }

    /// Once `count` agents have launched, have bloxlet list each as running,
    /// so a prompt quiet long enough for the proxy to check on its agent
    /// (as one can be under load) finds it there, not gone.
    async fn list_launched_as_running(&self, count: usize) {
        wait_until("the agents launch", || self.launches().len() == count).await;
        let active: Vec<_> = self
            .launches()
            .iter()
            .map(|pid| {
                serde_json::json!({
                    "commandId": format!("cmd-{pid}"),
                    "status": "COMMAND_STATUS_RUNNING",
                })
            })
            .collect();
        self.set_commands(&serde_json::Value::from(active).to_string(), "[]");
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
        self.interrupts() > 0
    }

    /// How many proxies were sent SIGINT.
    fn interrupts(&self) -> usize {
        self.read("signals")
            .lines()
            .filter(|line| *line == "interrupted")
            .count()
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

/// Sessions on Blox's default agent share its process, so none of them may
/// SIGINT the proxy — not even the last to let go, which `sq` would take as
/// stopping the process and the other apps' turns on it.
#[tokio::test(flavor = "current_thread")]
async fn a_shared_remote_agent_stops_only_when_its_last_session_finishes() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("last-out");
            let mut first = Session::connect(&sq.blox_driver(None, "first"), &sq, "first").await;
            let mut second = Session::connect(&sq.blox_driver(None, "second"), &sq, "second").await;
            sq.list_launched_as_running(1).await;

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
                !alive(launches[0]),
                "the last session finishing should stop the proxy"
            );
            assert!(
                !sq.interrupted(),
                "a shared proxy is stopped without SIGINT, which would stop the agent"
            );

            // The next session starts its own proxy instead of joining the
            // stopped one.
            let mut third = Session::connect(&sq.blox_driver(None, "third"), &sq, "third").await;
            sq.list_launched_as_running(2).await;
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
            let mut first = Session::connect(&sq.blox_driver(None, "first"), &sq, "first").await;
            let mut second = Session::connect(&sq.blox_driver(None, "second"), &sq, "second").await;
            sq.list_launched_as_running(1).await;

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
            let launches = sq.launches();
            assert_eq!(launches.len(), 1);
            assert!(!alive(launches[0]), "the last session out stops the proxy");
            assert!(!sq.interrupted(), "a shared proxy is never sent SIGINT");
        })
        .await;
}

/// A known provider runs on a process of the session's own: each session
/// leases a proxy of its own, and its last connection letting go stops it
/// the way stopping a local agent does, with SIGINT.
#[tokio::test(flavor = "current_thread")]
async fn a_dedicated_remote_agent_is_leased_per_session_and_interrupted_by_its_last_release() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("dedicated");
            let first_driver = sq.blox_driver(Some("claude"), "first");
            let mut first = Session::connect(&first_driver, &sq, "first").await;
            let mut first_again = Session::connect(&first_driver, &sq, "first again").await;
            wait_until("the first session's proxy launches", || {
                sq.received("session/new").len() == 2
            })
            .await;
            assert_eq!(
                sq.launches().len(),
                1,
                "a session's own connections share its proxy"
            );
            let mut second =
                Session::connect(&sq.blox_driver(Some("claude"), "second"), &sq, "second").await;
            sq.list_launched_as_running(2).await;
            let launches = sq.launches();

            let outcome = first.prompt("one").await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert!(
                alive(launches[0]) && !sq.interrupted(),
                "the session's other connection still uses its proxy"
            );

            let outcome = first_again.prompt("one again").await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert!(
                !alive(launches[0]) && sq.interrupts() == 1,
                "the session's last connection out interrupts its proxy"
            );
            assert!(
                alive(launches[1]),
                "another session's proxy is left running"
            );

            let outcome = second.prompt("two").await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert!(!alive(launches[1]) && sq.interrupts() == 2);
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

/// Two sessions on one proxy, one holding a prompt, when `sq` dies of
/// SIGKILL under the other's: what each session's prompt ended with.
/// `listed` gets the agent's command id once both are connected.
async fn prompts_when_sq_dies(
    sq: &FakeSq,
    listed: impl FnOnce(&str),
) -> (
    Result<AgentRunOutcome, String>,
    Result<AgentRunOutcome, String>,
) {
    let driver = sq.workspace_driver();
    let first = Session::connect(&driver, sq, "first").await;
    let second = Session::connect(&driver, sq, "second").await;
    listed(&sq.command_id().await);
    let first = prompt_in_background(first, "hold me");
    wait_until("the agent holds the first prompt", || {
        !sq.received("session/prompt").is_empty()
    })
    .await;
    let second = prompt_in_background(second, "crash");
    let (_, first) = first.await.unwrap();
    let (_, second) = second.await.unwrap();
    (first, second)
}

/// Only an interrupted `sq` stops the agent process it started: one
/// SIGKILLed, or cut off by the network, leaves it running. So while bloxlet
/// still lists it the sessions are detached, as for a process `sq` reused,
/// for their turns to be reattached to rather than failed.
#[tokio::test(flavor = "current_thread")]
async fn sessions_detach_when_sq_dies_while_bloxlet_still_runs_the_agent_it_started() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("started-crash");
            let mut started = String::new();
            let (first, second) = prompts_when_sq_dies(&sq, |command| {
                started = command.to_string();
                sq.set_commands(
                    &format!(r#"[{{"commandId":"{command}","status":"COMMAND_STATUS_RUNNING"}}]"#),
                    "[]",
                );
            })
            .await;

            let error = first.expect_err("the proxy went");
            assert!(
                is_remote_agent_detached_error(&error) && !is_remote_agent_lost_error(&error),
                "{error}"
            );
            assert!(
                error.contains(&format!(
                    "bloxlet still runs the agent process (command {started})"
                )),
                "{error}"
            );
            assert_eq!(second, Err(error));
            assert!(
                sq.checks() > 0,
                "bloxlet is asked before anything is failed"
            );
            assert!(
                sq.received("session/cancel").is_empty() && !sq.interrupted(),
                "detached turns are left running for a reattachment"
            );
        })
        .await;
}

/// The same death with bloxlet reporting the process gone is a loss, and
/// when bloxlet can't be asked the sessions are detached: nothing says the
/// turn ended, so it is left for a reattachment (and then reconcile) to
/// settle.
#[tokio::test(flavor = "current_thread")]
async fn sessions_dying_with_a_started_agent_follow_what_bloxlet_says() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("started-crash-gone");
            let mut command = String::new();
            let (first, second) =
                prompts_when_sq_dies(&sq, |listed| command = listed.to_string()).await;
            let error = first.expect_err("the agent went");
            assert!(is_remote_agent_lost_error(&error), "{error}");
            assert!(
                error.contains(&format!(
                    "bloxlet no longer lists the agent process (command {command}) as running"
                )),
                "{error}"
            );
            assert_eq!(second, Err(error));

            let sq = FakeSq::new("started-crash-unknown");
            sq.fail_checks(1000);
            let mut command = String::new();
            let (first, second) =
                prompts_when_sq_dies(&sq, |listed| command = listed.to_string()).await;
            let error = first.expect_err("the proxy went");
            assert!(
                is_remote_agent_detached_error(&error) && !is_remote_agent_lost_error(&error),
                "{error}"
            );
            assert!(
                error.contains(&format!(
                    "whether the agent process (command {command}) still runs is unknown"
                )),
                "{error}"
            );
            assert_eq!(second, Err(error));
        })
        .await;
}

/// `sq` never said which agent process it reached: bloxlet is asked about
/// the one the proxy's arguments name, matched by argv as `sq` matches it.
#[tokio::test(flavor = "current_thread")]
async fn sessions_dying_on_an_unnamed_agent_ask_bloxlet_by_its_argv() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("unnamed-crash");
            sq.silent();
            sq.set_commands(
                r#"[{"commandId":"cmd-other","status":"COMMAND_STATUS_RUNNING","processCommand":"claude-agent-acp","processArgs":[],"tags":{"blox:role":"acp"}},
                    {"commandId":"cmd-goose","status":"COMMAND_STATUS_IDLE","processCommand":"goose","processArgs":["acp"],"tags":{"blox:role":"acp"}}]"#,
                "[]",
            );
            let (first, second) = prompts_when_sq_dies(&sq, |_| {}).await;
            let error = first.expect_err("the proxy went");
            assert!(
                is_remote_agent_detached_error(&error) && !is_remote_agent_lost_error(&error),
                "{error}"
            );
            assert!(
                error.contains("bloxlet still runs the agent process (running `goose acp`)"),
                "{error}"
            );
            assert_eq!(second, Err(error));

            let sq = FakeSq::new("unnamed-crash-gone");
            sq.silent();
            sq.set_commands(
                r#"[{"commandId":"cmd-other","status":"COMMAND_STATUS_RUNNING","processCommand":"claude-agent-acp","processArgs":[],"tags":{"blox:role":"acp"}}]"#,
                "[]",
            );
            let (first, _) = prompts_when_sq_dies(&sq, |_| {}).await;
            let error = first.expect_err("the agent went");
            assert!(is_remote_agent_lost_error(&error), "{error}");
            assert!(
                error.contains("bloxlet lists no running agent process running `goose acp`"),
                "{error}"
            );
        })
        .await;
}

/// `sq` only logs that the orchestrator ended the agent's output stream: it
/// relays nothing after that, yet keeps its stdout open until its stdin
/// closes, while bloxlet can go on listing the process as running. So the
/// proxy closes `sq`'s stdin, the session still attached learns it lost its
/// agent once `sq` exits — detached, while bloxlet still lists the process,
/// for the runner's reattach to settle — and the next session gets a proxy
/// of its own rather than joining a stream that would never answer its
/// `initialize`.
#[tokio::test(flavor = "current_thread")]
async fn the_orchestrator_ending_the_agent_stream_closes_the_proxy_to_new_sessions() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("orchestrator-eof");
            let driver = sq.workspace_driver();
            let mut first = Session::connect(&driver, &sq, "first").await;
            // Keeps the proxy up once the first session's turn is over.
            let mut second = Session::connect(&driver, &sq, "second").await;
            let command = sq.command_id().await;
            // bloxlet has yet to notice.
            sq.set_commands(
                &format!(r#"[{{"commandId":"{command}","status":"COMMAND_STATUS_RUNNING"}}]"#),
                "[]",
            );

            let outcome = first.prompt("orchestrator eof").await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            wait_until("the proxy closes `sq`'s stdin", || sq.stdin_closed()).await;

            let error = second.prompt("next").await.expect_err("the agent is gone");
            assert!(
                is_remote_agent_detached_error(&error) && !is_remote_agent_lost_error(&error),
                "{error}"
            );
            assert!(
                error.contains(&format!(
                    "the orchestrator ended the agent's output stream, but bloxlet still runs \
                     the agent process (command {command})"
                )),
                "{error}"
            );
            assert!(!sq.interrupted(), "`sq` exited once its stdin closed");
            assert_eq!(
                sq.received("session/prompt").len(),
                1,
                "the second session's prompt never reached the lost agent"
            );

            let mut third = Session::connect(&driver, &sq, "third").await;
            let outcome = third.prompt("three").await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert_eq!(
                sq.launches().len(),
                2,
                "the third session got a proxy of its own"
            );
        })
        .await;
}

/// Should `sq` keep running once its stdin is closed, the proxy is stopped
/// after a grace period; a session connecting meanwhile waits for that
/// rather than joining it. Returns whether the lingering proxy was sent
/// SIGINT. `bloxlet` sets up what bloxlet says of the agent process (its
/// command id passed in) once the agent has launched.
async fn lingering_proxy_interrupted(label: &str, bloxlet: impl FnOnce(&FakeSq, &str)) -> bool {
    let sq = FakeSq::new(label);
    sq.linger();
    let driver = sq.workspace_driver();
    let mut first = Session::connect(&driver, &sq, "first").await;
    let _second = Session::connect(&driver, &sq, "second").await;
    let command = sq.command_id().await;
    bloxlet(&sq, &command);

    let outcome = first.prompt("orchestrator eof").await;
    assert!(
        matches!(outcome, Ok(AgentRunOutcome::Completed)),
        "{label}: {outcome:?}"
    );
    wait_until("the proxy closes `sq`'s stdin", || sq.stdin_closed()).await;
    let lingering = sq.launches()[0];

    let mut third = Session::connect(&driver, &sq, "third").await;
    assert!(
        !alive(lingering),
        "{label}: the lingering proxy is stopped before another starts"
    );
    // Read before the third session's prompt: its proxy is interrupted once
    // that connection lets go.
    let interrupted = sq.interrupted();
    let outcome = third.prompt("three").await;
    assert!(
        matches!(outcome, Ok(AgentRunOutcome::Completed)),
        "{label}: {outcome:?}"
    );
    assert_eq!(sq.launches().len(), 2, "{label}");
    interrupted
}

/// The `sq` that started the agent lingers after its output stream ended
/// while bloxlet still lists the agent — or can't be asked: the session still
/// attached is detached, for its turn to be reattached to, so the proxy is
/// killed without SIGINT, which `sq` would take as stopping the agent it
/// started. Only an agent bloxlet reports gone leaves nothing for a SIGINT to
/// stop.
#[tokio::test(flavor = "current_thread")]
async fn a_proxy_lingering_after_the_agent_stream_ended_is_stopped_and_not_joined() {
    LocalSet::new()
        .run_until(async {
            let running = lingering_proxy_interrupted("linger-running", |sq, command| {
                sq.set_commands(
                    &format!(r#"[{{"commandId":"{command}","status":"COMMAND_STATUS_RUNNING"}}]"#),
                    "[]",
                );
            })
            .await;
            assert!(
                !running,
                "an agent bloxlet still runs is not interrupted under its detached session"
            );

            let unknown =
                lingering_proxy_interrupted("linger-unknown", |sq, _| sq.fail_checks(1000)).await;
            assert!(
                !unknown,
                "an agent bloxlet can't say is gone is not interrupted either"
            );

            let gone = lingering_proxy_interrupted("linger-gone", |_, _| {}).await;
            assert!(gone, "a proxy whose agent is gone is stopped as usual");
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

/// A reattachment to the turn an earlier connection of instance `owner_pid`
/// prompted on `agent_session_id`.
struct Reattach {
    agent_session_id: &'static str,
    prompted_at: i64,
    connected_at: Option<i64>,
    owner_pid: Option<u32>,
    sole_turn_of_owner: bool,
    cancel: CancellationToken,
}

impl Default for Reattach {
    fn default() -> Self {
        let now = now_ms();
        Self {
            agent_session_id: "sess-earlier",
            prompted_at: now,
            connected_at: Some(now),
            owner_pid: None,
            sole_turn_of_owner: false,
            cancel: CancellationToken::new(),
        }
    }
}

impl Reattach {
    fn start(
        self,
        driver: AcpDriver,
        sq: &FakeSq,
        writer: Arc<BasicMessageWriter>,
    ) -> tokio::task::JoinHandle<Result<AgentRunOutcome, String>> {
        self.start_on(driver, sq, Arc::new(RecordingStore::default()), writer)
    }

    /// [`Self::start`] on a session whose transcript is `store`'s.
    fn start_on(
        self,
        driver: AcpDriver,
        sq: &FakeSq,
        store: Arc<dyn Store>,
        writer: Arc<dyn MessageWriter>,
    ) -> tokio::task::JoinHandle<Result<AgentRunOutcome, String>> {
        let dir = sq.dir.clone();
        tokio::task::spawn_local(async move {
            timeout(
                WAIT,
                driver.reattach(
                    "staged-session",
                    &dir,
                    &store,
                    &writer,
                    &self.cancel,
                    &ReattachTurn {
                        agent_session_id: self.agent_session_id.to_string(),
                        prompted_at: self.prompted_at,
                        connected_at: self.connected_at,
                        owner_pid: self.owner_pid,
                        sole_turn_of_owner: self.sole_turn_of_owner,
                    },
                ),
            )
            .await
            .expect("the reattachment should end")
        })
    }
}

impl Reattach {
    /// [`Self::start`] through [`AcpDriver::reattach_detachable`], for how
    /// the connection settled.
    fn settle(
        self,
        driver: AcpDriver,
        sq: &FakeSq,
    ) -> tokio::task::JoinHandle<Result<crate::driver::SessionSettled, String>> {
        let dir = sq.dir.clone();
        tokio::task::spawn_local(async move {
            let store: Arc<dyn Store> = Arc::new(RecordingStore::default());
            let writer: Arc<dyn MessageWriter> = Arc::new(BasicMessageWriter::new());
            timeout(
                WAIT,
                driver.reattach_detachable(
                    "staged-session",
                    &dir,
                    &store,
                    &writer,
                    &self.cancel,
                    &ReattachTurn {
                        agent_session_id: self.agent_session_id.to_string(),
                        prompted_at: self.prompted_at,
                        connected_at: self.connected_at,
                        owner_pid: self.owner_pid,
                        sole_turn_of_owner: self.sole_turn_of_owner,
                    },
                    &|_| {},
                ),
            )
            .await
            .expect("the reattachment should end")
        })
    }
}

fn reattach_in_background(
    driver: AcpDriver,
    sq: &FakeSq,
    writer: Arc<BasicMessageWriter>,
) -> tokio::task::JoinHandle<Result<AgentRunOutcome, String>> {
    Reattach::default().start(driver, sq, writer)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
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

/// When bloxlet's listing of the holder's process (`cmd-<pid>`) says it
/// started: 2026-11-05T01:00:00Z.
const HOLDER_STARTED_AT_MS: i64 = 1_793_840_400_000;

/// Starts a proxy, as another session's, whose process bloxlet lists as idle
/// and started at [`HOLDER_STARTED_AT_MS`] (unless `started_at` is false).
async fn hold_proxy(sq: &FakeSq, started_at: bool) -> Session {
    let holder = Session::connect(&sq.workspace_driver(), sq, "holder").await;
    let command = sq.command_id().await;
    let started_at = if started_at {
        r#","startedAt":"2026-11-05T01:00:00Z""#
    } else {
        ""
    };
    sq.set_commands(
        &format!(r#"[{{"commandId":"{command}","status":"COMMAND_STATUS_IDLE"{started_at}}}]"#),
        "[]",
    );
    holder
}

/// Reattach, joining the holder's proxy, to a turn prompted and connected at
/// these offsets from when bloxlet says the holder's process started.
async fn join_reattach(
    sq: &FakeSq,
    prompted_at: i64,
    connected_at: Option<i64>,
) -> Result<AgentRunOutcome, String> {
    Reattach {
        prompted_at: HOLDER_STARTED_AT_MS + prompted_at,
        connected_at: connected_at.map(|offset| HOLDER_STARTED_AT_MS + offset),
        ..Reattach::default()
    }
    .start(
        sq.workspace_driver(),
        sq,
        Arc::new(BasicMessageWriter::new()),
    )
    .await
    .unwrap()
}

/// A reattachment that joins a proxy another session already holds cannot
/// tell from `sq` whether the process it attached to ran the turn, so it asks
/// bloxlet when that process started.
#[tokio::test(flavor = "current_thread")]
async fn a_reattachment_joining_a_running_proxy_checks_the_process_predates_the_turn() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-joined");
            let _holder = hold_proxy(&sq, true).await;

            let error = join_reattach(&sq, -125_000, Some(-120_000))
                .await
                .expect_err("the process serving the workspace started after the turn's");
            assert!(is_remote_agent_lost_error(&error), "{error}");
            assert!(
                error.contains("started after the turn's connection to the agent was up"),
                "{error}"
            );

            let outcome = join_reattach(&sq, 60_000, Some(62_000)).await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "a process already running when the turn was prompted is the one that ran it: \
                 {outcome:?}"
            );
            assert_eq!(
                sq.launches().len(),
                1,
                "both reattachments joined the proxy"
            );
        })
        .await;
}

/// The session whose own connection launched the process: its prompt was
/// recorded seconds before `sq` started that process, and its connection's
/// `initialize` seconds after. So when it rejoins a proxy another session's
/// reattachment started — the two sweep threads race — the process is its.
#[tokio::test(flavor = "current_thread")]
async fn a_reattachment_joining_the_proxy_of_the_process_its_own_connection_started_survives() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-joined-own");
            let _holder = hold_proxy(&sq, true).await;

            let outcome = join_reattach(&sq, -4_000, Some(2_000)).await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "prompted before its process started, connected after: {outcome:?}"
            );
            // This app's clock running behind bloxlet's.
            let outcome = join_reattach(&sq, -24_000, Some(-18_000)).await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "within the clock-skew slack: {outcome:?}"
            );
            // Not known when its connection was up: the latest its setup
            // could have reached the agent after the prompt stands in.
            let outcome = join_reattach(&sq, -60_000, None).await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "within the setup timeout of the prompt: {outcome:?}"
            );
        })
        .await;
}

/// bloxlet failing to say when the process started says nothing about it,
/// so it is asked again rather than the turn being failed; a listing with no
/// start time cannot tell either, and the process is watched as the turn's.
#[tokio::test(flavor = "current_thread")]
async fn a_reattachment_joining_a_running_proxy_asks_bloxlet_again_when_it_cannot_answer() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-joined-inconclusive");
            let _holder = hold_proxy(&sq, true).await;

            sq.fail_checks(2);
            let outcome = join_reattach(&sq, -4_000, Some(2_000)).await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert!(
                sq.checks() >= 4,
                "asked again after failing: {}",
                sq.checks()
            );

            list_holder_without_start_time(&sq).await;
            let outcome = join_reattach(&sq, -4_000, Some(2_000)).await;
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
        })
        .await;
}

async fn list_holder_without_start_time(sq: &FakeSq) {
    let command = sq.command_id().await;
    sq.set_commands(
        &format!(r#"[{{"commandId":"{command}","status":"COMMAND_STATUS_IDLE"}}]"#),
        "[]",
    );
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
            let reattached = Reattach {
                owner_pid: Some(424_242),
                sole_turn_of_owner: true,
                ..Reattach::default()
            }
            .start(sq.workspace_driver(), &sq, writer.clone());
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
            let reattached = Reattach {
                owner_pid: Some(424_242),
                sole_turn_of_owner: true,
                ..Reattach::default()
            }
            .start(
                sq.workspace_driver(),
                &sq,
                Arc::new(BasicMessageWriter::new()),
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

            let outcome = Reattach {
                owner_pid: Some(424_242),
                ..Reattach::default()
            }
            .start(
                sq.workspace_driver(),
                &sq,
                Arc::new(BasicMessageWriter::new()),
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
/// short and fails as lost, rather than reading as a completed one. As its
/// instance's sole turn, it already watches every answer to that instance's
/// prompts, and that watch is the one to hear the cancelled answer.
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
            sq.block_earlier_prompt("sess-earlier", "staged-424242-3:9f1c");

            let outcome = Reattach {
                owner_pid: Some(424_242),
                sole_turn_of_owner: true,
                ..Reattach::default()
            }
            .start(
                sq.workspace_driver(),
                &sq,
                Arc::new(BasicMessageWriter::new()),
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

/// The agent's cancelled answer names the instance that prompted a turn, not
/// its session. Two of one instance's turns that go quiet together are
/// cancelled one at a time, so the blocked one's answer cannot be taken for
/// the answer to the other's cancel.
#[tokio::test(flavor = "current_thread")]
async fn two_quiet_reattached_turns_of_one_instance_are_told_apart() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-two-quiet");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_RUNNING","startedAt":"2026-01-01T00:00:00Z"}]"#,
                "[]",
            );
            // `sess-done`'s turn had ended; `sess-blocked`'s is blocked.
            sq.block_earlier_prompt("sess-blocked", "staged-424242-5:7c2e");
            let turn = |agent_session_id| Reattach {
                agent_session_id,
                owner_pid: Some(424_242),
                ..Reattach::default()
            };
            let done = turn("sess-done").start(
                sq.workspace_driver(),
                &sq,
                Arc::new(BasicMessageWriter::new()),
            );
            let blocked = turn("sess-blocked").start(
                sq.workspace_driver(),
                &sq,
                Arc::new(BasicMessageWriter::new()),
            );

            let done = done.await.unwrap();
            let blocked = blocked.await.unwrap();
            assert!(
                matches!(done, Ok(AgentRunOutcome::Completed)),
                "the turn that had ended is not failed for the other's answer: {done:?}"
            );
            let error = blocked.expect_err("the blocked turn was cut short");
            assert!(is_remote_agent_lost_error(&error), "{error}");
            assert!(error.contains("permission request"), "{error}");
            assert_eq!(sq.received("session/cancel").len(), 2);
            assert_eq!(sq.launches().len(), 1, "both turns share the proxy");
        })
        .await;
}

/// A process dedicated to the turn's session has no co-tenants to hide the
/// turn's end behind, and Claude there runs unattended (`bypassPermissions`),
/// so should ask no permission: a turn quiet there is most likely busy in a
/// long, silent tool. It is waited on past the quiet cap, under the much
/// longer one such a process gets, until bloxlet reports the process idle,
/// and not cancelled. (A dedicated Codex process, which does ask, keeps the
/// quiet cap: see `only_a_dedicated_unattended_agent_is_waited_on_past_the_quiet_cap`.)
#[tokio::test(flavor = "current_thread")]
async fn a_quiet_reattached_turn_on_a_dedicated_process_outlasts_the_quiet_cap() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-dedicated-quiet");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_RUNNING"}]"#,
                "[]",
            );
            let reattached = Reattach {
                owner_pid: Some(424_242),
                ..Reattach::default()
            }
            .start(
                sq.blox_driver(Some("claude"), "staged-session"),
                &sq,
                Arc::new(BasicMessageWriter::new()),
            );

            // A cap's cancel would go out the moment it fired.
            tokio::time::sleep(
                crate::driver::remote_liveness::REATTACHED_TURN_QUIET_CAP + Duration::from_secs(1),
            )
            .await;
            assert!(!reattached.is_finished(), "the quiet turn was given up on");
            assert!(sq.received("session/cancel").is_empty());

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
                sq.received("session/cancel").is_empty(),
                "a turn that ended is not cancelled"
            );
        })
        .await;
}

/// The tool call running when the earlier connection went is stored
/// `pending`, with its description as its result; the replay of the
/// reattachment's `session/load` holds its completion. That completion
/// lands on the stored call, as a live one would, and nothing the
/// transcript holds is recorded again.
#[tokio::test(flavor = "current_thread")]
async fn a_stored_running_call_the_reattached_replay_finished_is_completed() {
    use super::attach::{stored_with_running_call, TranscriptLog, TranscriptStore};
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-replay-settles");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_RUNNING"}]"#,
                "[]",
            );
            let text = |message_id: &str, text: &str| {
                serde_json::json!({"sessionUpdate": "agent_message_chunk", "messageId": message_id,
                    "content": {"type": "text", "text": text}})
            };
            let tool = |id: &str| {
                serde_json::json!({"sessionUpdate": "tool_call", "toolCallId": id, "title": "Run",
                    "status": "pending"})
            };
            let completed = |id: &str, output: &str| {
                serde_json::json!({"sessionUpdate": "tool_call_update", "toolCallId": id,
                    "status": "completed",
                    "content": [{"type": "content", "content": {"type": "text", "text": output}}]})
            };
            sq.replay(&[
                serde_json::json!({"sessionUpdate": "user_message_chunk", "messageId": "u1",
                    "content": {"type": "text", "text": "hi"}}),
                text("m0", "Earlier."),
                tool("t0"),
                completed("t0", "listed"),
                text("m1", "Hello "),
                tool("t1"),
                completed("t1", "3 passed"),
            ]);
            let log = Arc::new(TranscriptLog::default());
            let reattached = Reattach::default().start_on(
                sq.workspace_driver(),
                &sq,
                Arc::new(TranscriptStore(stored_with_running_call())),
                log.clone(),
            );
            wait_until("the turn is being watched", || sq.checks() > 0).await;

            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_IDLE"}]"#,
                "[]",
            );
            let outcome = reattached.await.unwrap();
            assert!(
                matches!(outcome, Ok(AgentRunOutcome::Completed)),
                "{outcome:?}"
            );
            assert_eq!(
                log.rows(),
                ["status:t1:completed", "tool_result:t1:3 passed"]
            );
        })
        .await;
}

/// The rows a reattach to the session's own process records when the agent
/// relays a tool call's `tool_call` and completion as soon as the proxy
/// attaches, ahead of the `session/load` (`replayed`: the load's replay
/// holds the call too).
async fn rows_of_a_call_relayed_before_the_load(label: &str, replayed: bool) -> Vec<String> {
    use super::attach::{stored_with_running_call, TranscriptLog, TranscriptStore};
    let sq = FakeSq::new(label);
    sq.reuse("cmd-earlier");
    sq.set_commands(
        r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_RUNNING"}]"#,
        "[]",
    );
    let started = serde_json::json!({"sessionUpdate": "tool_call", "toolCallId": "t2",
        "title": "Run", "rawInput": {"command": "cargo test"}, "status": "pending"});
    let completed = serde_json::json!({"sessionUpdate": "tool_call_update", "toolCallId": "t2",
        "status": "completed",
        "content": [{"type": "content", "content": {"type": "text", "text": "ok"}}]});
    sq.preload("sess-earlier", &[started.clone(), completed.clone()]);
    let mut replay = vec![
        serde_json::json!({"sessionUpdate": "user_message_chunk", "messageId": "u1",
            "content": {"type": "text", "text": "hi"}}),
        serde_json::json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Run",
            "status": "pending"}),
        serde_json::json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1",
            "status": "completed",
            "content": [{"type": "content", "content": {"type": "text", "text": "3 passed"}}]}),
    ];
    if replayed {
        replay.extend([started, completed]);
    }
    sq.replay(&replay);
    let log = Arc::new(TranscriptLog::default());
    let reattached = Reattach::default().start_on(
        sq.blox_driver(Some("claude"), "staged-session"),
        &sq,
        Arc::new(TranscriptStore(stored_with_running_call())),
        log.clone(),
    );
    wait_until("the turn is being watched", || sq.checks() > 0).await;

    sq.set_commands(
        r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_IDLE"}]"#,
        "[]",
    );
    let outcome = reattached.await.unwrap();
    assert!(
        matches!(outcome, Ok(AgentRunOutcome::Completed)),
        "{outcome:?}"
    );
    log.rows()
}

/// The proxy routes a reattached session's frames to its connection from
/// the start, so a tool call the agent announced before the load — once:
/// the Claude bridge sends only updates after its `tool_call` — is not lost
/// with its title, leaving its updates nothing to land on. Absent from the
/// replay (the agent had not written it to its transcript yet), it is
/// recorded once the replay is over; in the replay too, it is recorded once.
#[tokio::test(flavor = "current_thread")]
async fn a_tool_call_relayed_before_the_reattached_load_is_recorded_once() {
    LocalSet::new()
        .run_until(async {
            let expected = [
                "status:t1:completed",
                "tool_result:t1:3 passed",
                "tool_call:t2",
                "status:t2:completed",
                "tool_result:t2:ok",
            ];
            assert_eq!(
                rows_of_a_call_relayed_before_the_load("reattach-preload", false).await,
                expected,
                "the call the replay lacks is recorded after it"
            );
            assert_eq!(
                rows_of_a_call_relayed_before_the_load("reattach-preload-replayed", true).await,
                expected,
                "the call the replay holds is recorded from it alone"
            );
        })
        .await;
}

/// Only a reattach to the session's own process claims its frames when the
/// proxy attaches. On Blox's shared default argv (no `--staged-session`)
/// another session's connection may own the agent session, so nothing is
/// claimed until the `session/load`.
#[test]
fn only_a_dedicated_reattach_claims_its_session_at_attach_time() {
    let turn = ReattachTurn {
        agent_session_id: "sess-earlier".to_string(),
        prompted_at: 0,
        connected_at: None,
        owner_pid: None,
        sole_turn_of_owner: false,
    };
    let driver = |agent_id| {
        AcpDriver::for_workspace_with_binary(PathBuf::from("sq"), "test-ws", agent_id, "session-1")
    };

    let shared = driver(None);
    assert!(
        !shared
            .acp_args
            .iter()
            .any(|arg| arg.contains("--staged-session")),
        "{:?}",
        shared.acp_args
    );
    assert_eq!(shared.reattach_session_claim(Some(&turn)), None);

    let dedicated = driver(Some("claude"));
    assert_eq!(
        dedicated.reattach_session_claim(Some(&turn)),
        Some("sess-earlier")
    );
    assert_eq!(
        dedicated.reattach_session_claim(None),
        None,
        "not a reattach"
    );
}

/// Stopping a reattached turn cancels it on the agent, which `sq` leaves
/// running, even though nothing waits for the answer and the session was the
/// last one on the proxy, which is stopped right after.
#[tokio::test(flavor = "current_thread")]
async fn stopping_a_reattached_turn_cancels_it_before_the_proxy_stops() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-stop");
            sq.reuse("cmd-earlier");
            sq.set_commands(
                r#"[{"commandId":"cmd-earlier","status":"COMMAND_STATUS_RUNNING"}]"#,
                "[]",
            );
            let cancel = CancellationToken::new();
            let reattached = Reattach {
                cancel: cancel.clone(),
                ..Reattach::default()
            }
            .settle(sq.workspace_driver(), &sq);
            wait_until("the turn is being watched", || sq.checks() > 0).await;

            cancel.cancel();
            let settled = reattached.await.unwrap().expect("the connection settles");
            assert!(
                matches!(settled.outcome, Ok(AgentRunOutcome::Cancelled)),
                "{:?}",
                settled.outcome
            );
            assert!(
                !settled.cancel_unacknowledged,
                "the cancel went out on a proxy still connected"
            );
            let received = sq.read("received");
            let cancelled_at = received
                .lines()
                .position(|line| {
                    line.contains(r#""method":"session/cancel""#)
                        && line.contains(r#""sessionId":"sess-earlier""#)
                })
                .unwrap_or_else(|| panic!("the turn was not cancelled: {received}"));
            let interrupted_at = received
                .lines()
                .position(|line| line == "SIGINT")
                .unwrap_or_else(|| panic!("the proxy was not stopped: {received}"));
            assert!(
                cancelled_at < interrupted_at,
                "the cancel reached the agent before the proxy stopped: {received}"
            );
        })
        .await;
}

/// Stopped before its `session/load` answers, a reattachment sends the agent
/// no `session/cancel`: the turn runs on, and the connection says so, for
/// the caller to cancel it over a fresh one.
#[tokio::test(flavor = "current_thread")]
async fn stopping_a_reattachment_still_setting_up_leaves_its_cancel_unacknowledged() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("reattach-stop-setup");
            sq.reuse("cmd-earlier");
            sq.hold_load();
            let cancel = CancellationToken::new();
            let reattached = Reattach {
                cancel: cancel.clone(),
                ..Reattach::default()
            }
            .settle(sq.workspace_driver(), &sq);
            wait_until("the session is being loaded", || {
                !sq.received("session/load").is_empty()
            })
            .await;

            cancel.cancel();
            let settled = reattached.await.unwrap().expect("the connection settles");
            assert!(
                matches!(settled.outcome, Ok(AgentRunOutcome::Cancelled)),
                "{:?}",
                settled.outcome
            );
            assert!(settled.cancel_unacknowledged);
            assert!(
                sq.received("session/cancel").is_empty(),
                "a connection still setting up sends no cancel"
            );
        })
        .await;
}

/// Blocks the thread on the first text it is given until `release` is set,
/// the way a storage write stalled on the database lock blocks a session's
/// message writer.
struct StalledWriter {
    inner: BasicMessageWriter,
    stalled: std::sync::atomic::AtomicBool,
    release: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl MessageWriter for StalledWriter {
    async fn append_text(&self, text: &str) {
        use std::sync::atomic::Ordering;
        if !self.stalled.swap(true, Ordering::SeqCst) {
            let deadline = std::time::Instant::now() + WAIT;
            while !self.release.load(Ordering::SeqCst) {
                assert!(std::time::Instant::now() < deadline, "never released");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.inner.append_text(text).await;
    }

    async fn finalize(&self) {
        self.inner.finalize().await;
    }

    async fn record_tool_call(
        &self,
        tool_call_id: &str,
        title: &str,
        raw_input: Option<&serde_json::Value>,
    ) {
        self.inner
            .record_tool_call(tool_call_id, title, raw_input)
            .await;
    }

    async fn update_tool_call_title(
        &self,
        tool_call_id: &str,
        title: Option<&str>,
        raw_input: Option<&serde_json::Value>,
    ) {
        self.inner
            .update_tool_call_title(tool_call_id, title, raw_input)
            .await;
    }

    async fn record_tool_result(&self, tool_call_id: &str, content: &str) {
        self.inner.record_tool_result(tool_call_id, content).await;
    }
}

/// A session whose writer blocks its thread while the shared agent streams
/// it output falls behind and is cut off: the proxy, on a thread of its own,
/// cancels its turn on the agent, and once the session's thread is free its
/// run ends with the cut-off error rather than the queue having grown with
/// everything the agent wrote.
#[tokio::test(flavor = "current_thread")]
async fn a_session_that_stops_taking_its_output_is_cut_off_and_its_turn_cancelled() {
    LocalSet::new()
        .run_until(async {
            let sq = FakeSq::new("cut-off");
            let driver = sq.driver();
            let store = Arc::new(RecordingStore::default());
            let dyn_store: Arc<dyn Store> = Arc::clone(&store) as Arc<dyn Store>;
            let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let writer: Arc<dyn MessageWriter> = Arc::new(StalledWriter {
                inner: BasicMessageWriter::new(),
                stalled: std::sync::atomic::AtomicBool::new(false),
                release: Arc::clone(&release),
            });
            let cancel = CancellationToken::new();
            let mut connection = timeout(
                WAIT,
                driver.connect(
                    "stalled",
                    &sq.dir,
                    &dyn_store,
                    &writer,
                    &cancel,
                    None,
                    &[],
                    None,
                ),
            )
            .await
            .expect("connect should not hang")
            .expect("the fake sq should spawn");

            // This thread is about to block: release it from another once the
            // proxy has cancelled the turn.
            let received = sq.dir.join("received");
            let releaser = std::thread::spawn({
                let release = Arc::clone(&release);
                move || {
                    let deadline = std::time::Instant::now() + WAIT;
                    loop {
                        let frames = std::fs::read_to_string(&received).unwrap_or_default();
                        if frames.contains("\"method\":\"session/cancel\"") {
                            break;
                        }
                        assert!(std::time::Instant::now() < deadline, "no cancel arrived");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    release.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            });

            let outcome = timeout(WAIT, connection.prompt("flood", &[]))
                .await
                .expect("the prompt should resolve");
            releaser.join().unwrap();

            let error = outcome.expect_err("the session was cut off");
            assert!(error.starts_with("Remote session cut off: "), "{error}");
            assert!(error.contains("cancelled"), "{error}");
            assert!(!is_remote_agent_lost_error(&error), "{error}");
            assert!(!is_remote_agent_detached_error(&error), "{error}");
            let agent_session = store.agent_session_ids.lock().unwrap()[0].1.clone();
            let cancels = sq.received("session/cancel");
            assert_eq!(cancels.len(), 1, "{cancels:?}");
            assert!(cancels[0].contains(&agent_session), "{cancels:?}");
        })
        .await;
}
