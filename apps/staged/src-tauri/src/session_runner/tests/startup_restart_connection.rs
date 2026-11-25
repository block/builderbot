//! The startup restart end to end: real connections to a fake local agent go
//! through the runner's connection turn (`connection_turn`) and its
//! failed-connection decision (`StartupRestarts::after_turn`), the way
//! `start_session`'s turn loop runs them, against a real store.
//!
//! The defect this guards against: the decision matched the error text with
//! `starts_with`, but a setup timeout reaches the runner wrapped by the SDK
//! ("ACP protocol failed: Error { … data: Some(String(\"Timed out …\")) }").
//! No timeout was ever retried, and sessions whose agent would have answered
//! a second process at once ended in `error`.

use super::*;
use crate::session_runner::connection_turn::{run_connection_turn, ConnectionTurnRequest};
use crate::session_runner::startup_restart::{StartupRestart, StartupRestarts};

/// A local ACP agent in `sh`. Logs `SPAWN <pid>` to `$1` as it starts, and
/// which requests it answered. On its first spawn in mode `hang-first-new`,
/// and on every spawn in `hang-new`, it never answers `session/new`. In
/// `load-not-found` it refuses every `session/load` with -32002. A prompt is
/// answered with one text chunk. Request ids are the SDK's UUID strings.
const FAKE_AGENT: &str = r#"
log=$1; mode=$2
echo "SPAWN $$" >> "$log"
spawn=$(grep -c '^SPAWN' "$log")
reply() { printf '{"jsonrpc":"2.0","id":"%s","result":%s}\n' "$1" "$2"; }
fail() { printf '{"jsonrpc":"2.0","id":"%s","error":%s}\n' "$1" "$2"; }
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/^.*"id":"\([^"]*\)".*$/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      reply "$id" '{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}' ;;
    *'"method":"session/new"'*)
      if [ "$mode" = hang-new ] || { [ "$mode" = hang-first-new ] && [ "$spawn" = 1 ]; }; then
        echo "HUNG $$" >> "$log"; continue
      fi
      echo "NEW $$" >> "$log"
      reply "$id" "{\"sessionId\":\"sess-$$\"}" ;;
    *'"method":"session/load"'*)
      echo "LOAD $$" >> "$log"
      fail "$id" '{"code":-32002,"message":"Resource not found"}' ;;
    *'"method":"session/prompt"'*)
      echo "PROMPT $$" >> "$log"
      printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-%s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}}}\n' "$$"
      reply "$id" '{"stopReason":"end_turn"}' ;;
  esac
done
"#;

const PROMPT: &str = "do the thing";

fn log_path(mode: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "staged-startup-restart-{}-{mode}-{nanos}.log",
        std::process::id()
    ))
}

fn fake_agent_driver(log: &std::path::Path, mode: &str) -> AcpDriver {
    AcpDriver::from_agent(&acp_client::AcpAgent {
        binary_path: PathBuf::from("/bin/sh"),
        acp_args: vec![
            "-c".to_string(),
            FAKE_AGENT.to_string(),
            "fake-agent".to_string(),
            log.display().to_string(),
            mode.to_string(),
        ],
        label: "Fake".to_string(),
    })
    // Spawned directly, not through the user's login shell.
    .with_env_snapshot(vec![("PATH".to_string(), "/usr/bin:/bin".to_string())])
    // A setup step's 90s, shortened so the first spawn times out quickly.
    .with_setup_step_timeout(Duration::from_millis(500))
}

struct Run {
    result: Result<AgentRunOutcome, String>,
    /// Each failed connection's start-over decision, in order.
    decisions: Vec<Option<StartupRestart>>,
    /// The fake's log: what each spawned process did.
    log: String,
}

impl Run {
    /// The pid of each agent process spawned, in order.
    fn spawned(&self) -> Vec<&str> {
        self.lines("SPAWN")
    }

    /// The pids that logged `event`, in order.
    fn lines(&self, event: &str) -> Vec<&str> {
        self.log
            .lines()
            .filter_map(|line| line.strip_prefix(event)?.strip_prefix(' '))
            .collect()
    }
}

/// The turn loop of `start_session`, cut down to what a startup restart
/// touches: the prompt row is stored once, before the loop; each pass makes
/// one connection on a fresh writer, waits for the store's queue, and asks
/// the runner's own decision whether to start over.
fn run_turn_loop(
    store: &Arc<Store>,
    session_id: &str,
    mode: &str,
    stored_agent_session_id: Option<&str>,
) -> Run {
    store
        .add_session_message_with_images(session_id, MessageRole::User, PROMPT, &[])
        .expect("prompt row");
    let log = log_path(mode);
    let driver = fake_agent_driver(&log, mode);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let local = tokio::task::LocalSet::new();
    let (result, decisions) = local.block_on(&runtime, async {
        let mut agent_session_id = stored_agent_session_id.map(str::to_string);
        let mut startup_restarts = StartupRestarts::default();
        let mut decisions = Vec::new();
        loop {
            assert!(decisions.len() < 4, "the run keeps starting over");
            let writer = Arc::new(MessageWriter::new(
                session_id.to_string(),
                Arc::clone(store),
            ));
            let store_trait: Arc<dyn acp_client::Store> = store.clone();
            let writer_trait: Arc<dyn acp_client::MessageWriter> = writer;
            let cancel = CancellationToken::new();
            let turn = tokio::time::timeout(
                Duration::from_secs(30),
                run_connection_turn(
                    &driver,
                    ConnectionTurnRequest {
                        session_id,
                        working_dir: &std::env::temp_dir(),
                        store: &store_trait,
                        writer: &writer_trait,
                        cancel: &cancel,
                        agent_session_id: agent_session_id.as_deref(),
                        config_options: &[],
                        prompt: PROMPT,
                        images: &[],
                    },
                    |_| {},
                    None::<std::future::Pending<()>>,
                    |()| {},
                ),
            )
            .await
            .expect("the connection ends");
            store.flush_writes().await;
            let setup_failure = turn.setup_failure();
            let turn_result = turn.result;
            if turn_result.is_ok() {
                return (turn_result, decisions);
            }
            let restart = startup_restarts.after_turn(
                session_id,
                &turn_result,
                setup_failure,
                false,
                &mut agent_session_id,
                store,
            );
            decisions.push(restart);
            if restart.is_none() {
                return (turn_result, decisions);
            }
        }
    });
    let log_text = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    Run {
        result,
        decisions,
        log: log_text,
    }
}

fn new_session(store: &Store) -> String {
    let session = crate::store::Session::new_running(PROMPT, std::path::Path::new("/tmp"));
    store.create_session(&session).expect("create session");
    session.id
}

/// The rows of `session_id` with `role`, by content.
fn rows(store: &Store, session_id: &str, role: MessageRole) -> Vec<String> {
    store
        .get_session_messages(session_id)
        .expect("transcript")
        .into_iter()
        .filter(|message| message.role == role)
        .map(|message| message.content)
        .collect()
}

#[test]
fn a_timed_out_setup_is_retried_once_on_a_fresh_process() {
    let store = Arc::new(Store::in_memory().expect("store"));
    let session_id = new_session(&store);

    let run = run_turn_loop(&store, &session_id, "hang-first-new", None);

    assert_eq!(run.result, Ok(AgentRunOutcome::Completed), "{}", run.log);
    assert_eq!(
        run.decisions,
        vec![Some(StartupRestart::RetryTimedOutSetup)],
        "{}",
        run.log
    );
    let spawned = run.spawned();
    assert_eq!(spawned.len(), 2, "one retry: {}", run.log);
    assert_ne!(spawned[0], spawned[1], "on a fresh process: {}", run.log);
    assert_eq!(run.lines("HUNG"), vec![spawned[0]], "{}", run.log);
    // Only the retry's process got the prompt.
    assert_eq!(run.lines("PROMPT"), vec![spawned[1]], "{}", run.log);
    // The prompt row is the one stored before the loop; the retry wrote no
    // second one.
    assert_eq!(rows(&store, &session_id, MessageRole::User), vec![PROMPT]);
    assert_eq!(
        rows(&store, &session_id, MessageRole::Assistant),
        vec!["done"]
    );
    // The timed-out setup stored no agent session id; the retry's did.
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(
        session.agent_id.as_deref(),
        Some(format!("sess-{}", spawned[1]).as_str())
    );
}

#[test]
fn a_second_setup_timeout_is_the_runs_error() {
    let store = Arc::new(Store::in_memory().expect("store"));
    let session_id = new_session(&store);

    let run = run_turn_loop(&store, &session_id, "hang-new", None);

    let error = run.result.clone().expect_err("every session/new hangs");
    assert!(
        error.contains("Timed out waiting for ACP protocol startup: session/new did not answer"),
        "{error}"
    );
    assert_eq!(
        run.decisions,
        vec![Some(StartupRestart::RetryTimedOutSetup), None],
        "{}",
        run.log
    );
    assert_eq!(run.spawned().len(), 2, "{}", run.log);
    assert!(run.lines("PROMPT").is_empty(), "{}", run.log);
    assert_eq!(rows(&store, &session_id, MessageRole::User), vec![PROMPT]);
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.agent_id, None);
}

/// The same wrapping hid the agent refusing a stored id that it never kept:
/// the run starts a fresh agent session on a new process, with the same
/// prompt row.
#[test]
fn a_refused_load_with_no_agent_output_starts_fresh_once() {
    let store = Arc::new(Store::in_memory().expect("store"));
    let session_id = new_session(&store);
    store
        .set_agent_session_id(&session_id, "never-prompted")
        .expect("stored id");

    let run = run_turn_loop(
        &store,
        &session_id,
        "load-not-found",
        Some("never-prompted"),
    );

    assert_eq!(run.result, Ok(AgentRunOutcome::Completed), "{}", run.log);
    assert_eq!(
        run.decisions,
        vec![Some(StartupRestart::StartFresh)],
        "{}",
        run.log
    );
    let spawned = run.spawned();
    assert_eq!(spawned.len(), 2, "{}", run.log);
    assert_eq!(run.lines("LOAD"), vec![spawned[0]], "{}", run.log);
    assert_eq!(run.lines("NEW"), vec![spawned[1]], "{}", run.log);
    assert_eq!(run.lines("PROMPT"), vec![spawned[1]], "{}", run.log);
    assert_eq!(rows(&store, &session_id, MessageRole::User), vec![PROMPT]);
    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(
        session.agent_id.as_deref(),
        Some(format!("sess-{}", spawned[1]).as_str())
    );
}
