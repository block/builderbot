//! Sessions sharing one remote agent process through one `sq` proxy.

use super::*;
use crate::driver::SessionConnection;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use std::os::unix::fs::PermissionsExt;
use tokio::task::LocalSet;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(30);

/// Stands in for `sq blox acp`: a fake ACP agent. Each launch records its
/// pid, every frame it reads, and the SIGINT that stops it. It answers setup
/// and prompts at once, except a prompt saying "hold me", which it answers
/// only once cancelled.
const FAKE_SQ: &str = r#"#!/bin/sh
dir='@DIR@'
echo $$ >> "$dir/launches"
trap 'echo interrupted >> "$dir/signals"; exit 0' INT
n=0
held=
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$dir/received"
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":"\(staged-[^"]*\)".*/\1/p')
  result='{}'
  case "$line" in
    *'"method":"initialize"'*) result='{"protocolVersion":1}' ;;
    *'"method":"session/new"'*) n=$((n+1)); result="{\"sessionId\":\"sess-$$-$n\"}" ;;
    *'"method":"session/prompt"'*'hold me'*) held=$id; continue ;;
    *'"method":"session/prompt"'*) result='{"stopReason":"end_turn"}' ;;
    *'"method":"session/cancel"'*)
      if [ -n "$held" ]; then
        printf '{"jsonrpc":"2.0","id":"%s","result":{"stopReason":"cancelled"}}\n' "$held"
        held=
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

    /// A remote driver whose `sq` is the fake, as `AcpDriver::for_workspace`
    /// builds it.
    fn driver(&self) -> AcpDriver {
        AcpDriver {
            binary_path: self.dir.join("sq"),
            acp_args: vec![],
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
