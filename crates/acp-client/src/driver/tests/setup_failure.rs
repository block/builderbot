//! [`SessionSettled::setup_failure`] end to end through `AcpDriver::connect`:
//! the failures a caller starts over from are told apart by the settle, not
//! by the error text, which reaches the caller wrapped by the SDK ("ACP
//! protocol failed: Error { … data: Some(String(\"Timed out …\")) }"). A
//! runner matching the text with `starts_with` never saw a timeout.

use super::*;
use crate::driver::{SetupFailure, ACP_SETUP_TIMEOUT_HEADLINE, SESSION_NOT_FOUND_PREFIX};
use crate::types::AcpAgent;

/// A local ACP agent in `sh`: answers `initialize` (with `loadSession`), and
/// what `$1` says for `session/new` and `session/load`. Request ids are the
/// SDK's UUID strings.
const FAKE_AGENT: &str = r#"
mode=$1
reply() { printf '{"jsonrpc":"2.0","id":"%s","result":%s}\n' "$1" "$2"; }
fail() { printf '{"jsonrpc":"2.0","id":"%s","error":%s}\n' "$1" "$2"; }
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/^.*"id":"\([^"]*\)".*$/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      reply "$id" '{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}' ;;
    *'"method":"session/new"'*)
      [ "$mode" = hang-new ] && continue
      reply "$id" '{"sessionId":"sess-1"}' ;;
    *'"method":"session/load"'*)
      fail "$id" '{"code":-32002,"message":"Resource not found"}' ;;
    *'"method":"session/prompt"'*)
      printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}}}\n'
      reply "$id" '{"stopReason":"end_turn"}' ;;
  esac
done
"#;

struct Run {
    turn: Result<AgentRunOutcome, String>,
    settled: SessionSettled,
}

/// Connect to the fake in `mode` (loading `agent_session_id` if given),
/// prompt once, and wait for the connection to settle.
async fn run(mode: &str, agent_session_id: Option<&str>) -> Run {
    let driver = AcpDriver::from_agent(&AcpAgent {
        binary_path: PathBuf::from("/bin/sh"),
        acp_args: vec![
            "-c".to_string(),
            FAKE_AGENT.to_string(),
            "fake-agent".to_string(),
            mode.to_string(),
        ],
        label: "Fake".to_string(),
    })
    // Spawned directly, not through the user's login shell.
    .with_env_snapshot(vec![("PATH".to_string(), "/usr/bin:/bin".to_string())])
    .with_setup_step_timeout(Duration::from_millis(500));
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let store: Arc<dyn Store> = Arc::new(RecordingStore::default());
            let writer: Arc<dyn MessageWriter> = Arc::new(BasicMessageWriter::new());
            let mut connection = driver
                .connect(
                    "local-session",
                    &std::env::temp_dir(),
                    &store,
                    &writer,
                    &CancellationToken::new(),
                    agent_session_id,
                    &[],
                    None,
                )
                .await
                .expect("the fake agent spawns");
            let settled_rx = connection.take_settled_receiver().expect("settled rx");
            let turn = tokio::time::timeout(Duration::from_secs(20), connection.prompt("go", &[]))
                .await
                .expect("the turn ends");
            let settled = settled_rx.await.expect("the connection settles");
            Run { turn, settled }
        })
        .await
}

#[tokio::test(flavor = "current_thread")]
async fn a_timed_out_setup_settles_as_timed_out_under_the_sdk_wrapping() {
    let run = run("hang-new", None).await;

    let error = run.turn.expect_err("session/new never answers");
    // The text as the runner gets it: no longer starting with the headline.
    assert!(error.starts_with("ACP protocol failed: "), "{error}");
    assert!(error.contains(ACP_SETUP_TIMEOUT_HEADLINE), "{error}");
    assert!(
        error.contains("session/new did not answer within"),
        "{error}"
    );
    assert_eq!(run.settled.reason, SessionSettleReason::Failed);
    assert_eq!(run.settled.setup_failure, Some(SetupFailure::TimedOut));
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_load_settles_as_session_not_found_under_the_sdk_wrapping() {
    let run = run("ok", Some("never-prompted")).await;

    let error = run.turn.expect_err("the agent has no such session");
    assert!(error.starts_with("ACP protocol failed: "), "{error}");
    assert!(error.contains(SESSION_NOT_FOUND_PREFIX), "{error}");
    assert_eq!(run.settled.reason, SessionSettleReason::Failed);
    assert_eq!(
        run.settled.setup_failure,
        Some(SetupFailure::SessionNotFound)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_completed_setup_settles_without_a_setup_failure() {
    let run = run("ok", None).await;

    assert_eq!(run.turn, Ok(AgentRunOutcome::Completed));
    assert_eq!(run.settled.setup_failure, None);
}
