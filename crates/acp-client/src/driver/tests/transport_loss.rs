//! Transport-loss classification and detaching: what a connection settles with
//! when its byte stream to the agent goes away, as opposed to the agent
//! answering with an error, and the SIGKILL-only teardown that leaves a remote
//! turn running.

use super::*;
use crate::driver::{
    is_transport_loss_error, DetachHandle, ProxyStop, TransportLoss, TransportLossPhase,
    TransportMonitor, TransportSignal,
};

fn turn_loss() -> Option<TransportLoss> {
    Some(TransportLoss {
        phase: TransportLossPhase::Turn,
    })
}

// =============================================================================
// Phrase fallback
// =============================================================================

#[test]
fn transport_loss_wording_matches_closed_pipes_only() {
    for error in [
        "ACP protocol failed: Error { code: -32603, message: \"Incoming transport closed\" }",
        "Prompt failed: connection closed before message completed",
        "Connection reset by peer (os error 54)",
        "Failed to write: Broken pipe (os error 32)",
        "unexpected EOF while reading",
        "early eof",
        "Unexpected end of file",
    ] {
        assert!(is_transport_loss_error(error), "{error}");
    }
    // An agent that answered: none of these may read as a lost transport.
    for error in [
        // A bridge relaying its own subprocess dying, over a live transport.
        "Prompt failed: Error { code: -32603, message: \"Internal error\", data: \
         \"Claude Code process exited with code 1\" }",
        "Prompt failed: subprocess exited with status 1",
        "Child exited gracefully",
        // Sign-in, a refused prompt, a startup that never answered.
        "Authentication required to run the prompt. Sign in to Claude.",
        "Prompt failed: Error { code: -32602, message: \"Invalid params\" }",
        "Timed out waiting for ACP protocol startup after 90s",
        "Selected ACP model 'opus' is no longer available",
    ] {
        assert!(!is_transport_loss_error(error), "{error}");
    }
}

// =============================================================================
// The monitor's verdict
// =============================================================================

#[test]
fn a_connection_that_ended_on_its_own_has_no_verdict() {
    let monitor = TransportMonitor::new();
    monitor.session_named();
    assert!(monitor.begin_turn());
    assert_eq!(monitor.verdict(true, None), None);
    // Teardown's own EOF arrives after the loop returned: still nothing.
    monitor.signal(TransportSignal::StdoutClosed);
    assert_eq!(monitor.verdict(true, Some("Prompt failed: refused")), None);
}

#[test]
fn a_confirmed_loss_keeps_the_phase_of_the_request_it_failed() {
    let monitor = TransportMonitor::new();
    monitor.session_named();
    assert!(monitor.begin_turn());
    monitor.signal(TransportSignal::OrchestratorEof);
    monitor.confirm(TransportLossPhase::Turn);
    assert_eq!(monitor.verdict(true, None), turn_loss());

    // A completed turn whose hold was cut short is a hold loss, even though
    // the signal landed while the turn was still current.
    let monitor = TransportMonitor::new();
    monitor.session_named();
    assert!(monitor.begin_turn());
    monitor.signal(TransportSignal::StdoutClosed);
    monitor.begin_hold();
    monitor.confirm(TransportLossPhase::Hold);
    assert_eq!(
        monitor.verdict(true, None),
        Some(TransportLoss {
            phase: TransportLossPhase::Hold
        })
    );
}

#[test]
fn nothing_is_lost_before_the_session_is_named() {
    let monitor = TransportMonitor::new();
    monitor.signal(TransportSignal::StdoutClosed);
    monitor.confirm(TransportLossPhase::Setup);
    assert_eq!(monitor.verdict(true, None), None);
    assert_eq!(
        monitor.verdict(false, Some("Incoming transport closed")),
        None
    );
}

#[test]
fn an_sdk_abort_falls_back_to_signals_then_wording() {
    // A signal stands in for the request that never got to fail.
    let monitor = TransportMonitor::new();
    monitor.session_named();
    assert!(monitor.begin_turn());
    monitor.signal(TransportSignal::StdoutClosed);
    assert_eq!(
        monitor.verdict(false, Some("ACP protocol failed")),
        turn_loss()
    );

    // No signal: the wording decides, both ways.
    let monitor = TransportMonitor::new();
    monitor.session_named();
    assert!(monitor.begin_turn());
    assert_eq!(
        monitor.verdict(false, Some("ACP protocol failed: broken pipe")),
        turn_loss()
    );
    assert_eq!(
        monitor.verdict(
            false,
            Some("Prompt failed: Claude Code process exited with code 1")
        ),
        None
    );
}

#[test]
fn a_prompt_is_refused_once_the_transport_is_known_lost() {
    let monitor = TransportMonitor::new();
    monitor.session_named();
    monitor.signal(TransportSignal::OrchestratorEof);
    assert!(!monitor.begin_turn(), "no turn on a dead transport");
    monitor.confirm(TransportLossPhase::Setup);
    assert_eq!(
        monitor.verdict(true, None),
        Some(TransportLoss {
            phase: TransportLossPhase::Setup
        })
    );
    assert!(monitor.lost().is_cancelled());
}

#[test]
fn teardown_skips_sigint_once_detached_or_lost() {
    let monitor = TransportMonitor::new();
    assert_eq!(monitor.begin_teardown(), ProxyStop::Graceful);

    for signal in [
        TransportSignal::StdoutClosed,
        TransportSignal::OrchestratorEof,
    ] {
        let monitor = TransportMonitor::new();
        monitor.signal(signal);
        assert_eq!(monitor.begin_teardown(), ProxyStop::KillOnly, "{signal}");
    }
}

/// Another session's turn may be running on a shared process: even a normal
/// end of this one must not SIGINT it.
#[test]
fn teardown_of_a_shared_process_never_sends_sigint() {
    let monitor = TransportMonitor::new();
    monitor.mark_shared_process();
    monitor.session_named();
    assert_eq!(monitor.verdict(true, None), None);
    assert_eq!(monitor.begin_teardown(), ProxyStop::KillOnly);
}

#[test]
fn proxy_stderr_markers_are_classified() {
    let monitor = TransportMonitor::new();
    monitor.observe_proxy_stderr("INF Found existing ACP process, reusing process_id=27294439");
    assert!(!monitor.process_replaced());
    assert!(!monitor.lost().is_cancelled());

    for line in [
        "INF Requested ACP process has exited, matching its recorded command",
        "INF Requested ACP process is no longer active, falling back to command matching",
        "INF Started new ACP process process_id=27300001 command=claude-agent-acp",
    ] {
        let monitor = TransportMonitor::new();
        monitor.observe_proxy_stderr(line);
        assert!(monitor.process_replaced(), "{line}");
        assert!(
            !monitor.lost().is_cancelled(),
            "a replacement is not a loss: {line}"
        );
    }

    let monitor = TransportMonitor::new();
    monitor.observe_proxy_stderr("INF Received EOF signal from orchestrator, connection closed");
    assert_eq!(
        monitor.first_signal(),
        Some(TransportSignal::OrchestratorEof)
    );
    assert!(monitor.lost().is_cancelled());
}

#[test]
fn a_detach_with_no_lease_armed_does_nothing() {
    // A local session: nothing armed.
    let monitor = TransportMonitor::new();
    let handle = DetachHandle::new(Arc::clone(&monitor));
    assert!(!handle.detach());
    assert!(!monitor.detach_requested());
    assert!(!monitor.lost().is_cancelled());
}

#[tokio::test(flavor = "current_thread")]
async fn owns_session_admits_only_the_named_session() {
    let handler = AcpNotificationHandler::new(
        Arc::new(BasicMessageWriter::new()),
        false,
        vec![],
        CancellationToken::new(),
    );
    assert!(!handler.owns_session("mine").await, "unnamed owns nothing");
    handler.name_session("mine").await;
    assert!(handler.owns_session("mine").await);
    assert!(!handler.owns_session("other").await);
}

// =============================================================================
// SIGKILL-only teardown of a real process group
// =============================================================================

#[cfg(unix)]
mod proxy_group {
    use super::*;
    use crate::driver::remote_process;
    use crate::driver::transport_loss::kill_without_interrupt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, BufReader};

    /// A wrapper that exits 130 from its INT trap, like a healthy `sq`, over
    /// a grandchild in the same group. Prints the grandchild's pid.
    async fn spawn_sigint_honouring_group() -> (tokio::process::Child, i32) {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg("trap 'exit 130' INT; sleep 300 & echo $!; wait $!")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        cmd.process_group(0);
        // A test binary started from a background job inherits SIGINT
        // ignored, and a non-interactive `sh` cannot trap a signal ignored on
        // entry; the trap must mean what it says.
        // SAFETY: resetting a signal disposition is async-signal-safe and the
        // closure touches nothing else before `exec`.
        unsafe {
            cmd.pre_exec(|| {
                nix::sys::signal::signal(
                    nix::sys::signal::Signal::SIGINT,
                    nix::sys::signal::SigHandler::SigDfl,
                )
                .map(|_| ())
                .map_err(std::io::Error::other)
            });
        }
        let mut child = cmd.spawn().expect("sh spawns");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut lines = BufReader::new(stdout).lines();
        let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await
            .expect("the grandchild pid is printed promptly")
            .expect("stdout readable")
            .expect("one line");
        (child, line.trim().parse().expect("a pid"))
    }

    fn process_exists(pid: i32) -> bool {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
    }

    async fn wait_until_gone(pid: i32) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while process_exists(pid) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "process {pid} is still alive (or an unreaped zombie) after 10 s"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The wrapper would exit 130 on a SIGINT; dying of signal 9 instead is
    /// what proves none was sent — SIGINT to the CLI that started the remote
    /// process stops that process and every turn on it.
    #[tokio::test(flavor = "current_thread")]
    async fn kill_without_interrupt_kills_the_whole_group_with_no_sigint() {
        let (mut child, grandchild) = spawn_sigint_honouring_group().await;
        kill_without_interrupt(&mut child, true).await;
        let status = child
            .try_wait()
            .expect("waitable")
            .expect("the wrapper is reaped");
        assert_eq!(
            status.signal(),
            Some(9),
            "killed, not interrupted: {status:?}"
        );
        wait_until_gone(grandchild).await;
    }

    /// A stand-in for `sq` on a shared proxy: logs any SIGINT to `dir/log` —
    /// a healthy `sq` stops the agent process it started on one — and writes
    /// its own pid and a grandchild's, in its process group, to `dir/pids`.
    fn sigint_logging_proxy(dir: &Path) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(
                r#"trap 'echo SIGINT >> "$1/log"' INT
sleep 300 &
echo "$$ $!" > "$1/pids.tmp" && mv "$1/pids.tmp" "$1/pids"
wait $!"#,
            )
            .arg("fake-sq")
            .arg(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd.process_group(0);
        // As in `spawn_sigint_honouring_group`: the trap must see a SIGINT.
        // SAFETY: resetting a signal disposition is async-signal-safe and the
        // closure touches nothing else before `exec`.
        unsafe {
            cmd.pre_exec(|| {
                nix::sys::signal::signal(
                    nix::sys::signal::Signal::SIGINT,
                    nix::sys::signal::SigHandler::SigDfl,
                )
                .map(|_| ())
                .map_err(std::io::Error::other)
            });
        }
        cmd
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "acp-lease-detach-{}-{name}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// The fake `sq`'s pid (its process group's id) and its grandchild's.
    async fn read_pids(dir: &Path) -> (i32, i32) {
        let path = dir.join("pids");
        let text = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    return text;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the fake `sq` writes its pids promptly");
        let mut pids = text
            .split_whitespace()
            .map(|pid| pid.parse().expect("a pid"));
        (
            pids.next().expect("its own pid"),
            pids.next().expect("its grandchild's pid"),
        )
    }

    fn sigints_logged(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("log"))
            .unwrap_or_default()
            .matches("SIGINT")
            .count()
    }

    /// An attach spawns its proxy itself: a detach SIGKILLs that group at
    /// once, and once teardown has begun it is disarmed, so it can never
    /// signal a reaped (and reusable) pgid.
    #[tokio::test(flavor = "current_thread")]
    async fn detach_kills_an_armed_proxy_group_and_is_disarmed_by_teardown() {
        let (mut child, grandchild) = spawn_sigint_honouring_group().await;
        let monitor = TransportMonitor::new();
        monitor.arm_proxy_group(child.id().expect("running") as i32);
        let handle = DetachHandle::new(Arc::clone(&monitor));

        assert!(handle.detach());
        assert!(monitor.detach_requested());
        assert!(monitor.lost().is_cancelled());
        assert_eq!(monitor.first_signal(), Some(TransportSignal::Detached));
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("the wrapper dies promptly")
            .expect("waitable");
        assert_eq!(
            status.signal(),
            Some(9),
            "killed, not interrupted: {status:?}"
        );
        wait_until_gone(grandchild).await;

        assert_eq!(monitor.begin_teardown(), ProxyStop::KillOnly);
        assert!(!handle.detach(), "disarmed once teardown began");
    }

    /// A lease released as usual interrupts the proxy it was the last to
    /// hold — the fake `sq` does log a SIGINT — while one that spares it (a
    /// shared process) only SIGKILLs it.
    #[tokio::test(flavor = "current_thread")]
    async fn a_lease_released_kill_only_never_interrupts_its_proxy() {
        for kill_only in [false, true] {
            let dir = scratch_dir(if kill_only { "kill-only" } else { "graceful" });
            let (lease, _streams) = remote_process::attach(sigint_logging_proxy(&dir), "Test")
                .await
                .expect("the proxy spawns");
            let (proxy, grandchild) = read_pids(&dir).await;
            if kill_only {
                lease.never_interrupt();
            }
            tokio::time::timeout(Duration::from_secs(10), lease.release())
                .await
                .expect("the proxy stops promptly");
            wait_until_gone(proxy).await;
            wait_until_gone(grandchild).await;
            assert_eq!(
                sigints_logged(&dir),
                usize::from(!kill_only),
                "kill_only: {kill_only}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The quit path's detach of the only session on a proxy: its group is
    /// SIGKILLed at once, with no SIGINT, and the detach is disarmed once
    /// teardown begins, so it can never signal a reaped (and reusable) pgid.
    #[tokio::test(flavor = "current_thread")]
    async fn detaching_the_only_lease_kills_the_proxy_group_with_no_sigint() {
        let dir = scratch_dir("sole");
        let (lease, _streams) = remote_process::attach(sigint_logging_proxy(&dir), "Test")
            .await
            .expect("the proxy spawns");
        let (proxy, grandchild) = read_pids(&dir).await;
        let monitor = TransportMonitor::new();
        monitor.arm_lease(lease.detacher());
        let handle = DetachHandle::new(Arc::clone(&monitor));

        assert!(handle.detach());
        assert!(monitor.detach_requested());
        assert!(monitor.lost().is_cancelled());
        assert_eq!(monitor.first_signal(), Some(TransportSignal::Detached));
        tokio::time::timeout(Duration::from_secs(10), lease.exited().cancelled())
            .await
            .expect("the proxy is killed promptly");
        wait_until_gone(proxy).await;
        wait_until_gone(grandchild).await;

        assert_eq!(monitor.begin_teardown(), ProxyStop::KillOnly);
        assert!(!handle.detach(), "disarmed once teardown began");
        tokio::time::timeout(Duration::from_secs(5), lease.release())
            .await
            .expect("nothing is left to wait for");
        assert_eq!(sigints_logged(&dir), 0, "killed, not interrupted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Detaching one of two sessions on a proxy leaves it running for the
    /// other; once that one lets go too, the proxy is SIGKILLed rather than
    /// interrupted, since a SIGINT would stop the detached session's turn.
    #[tokio::test(flavor = "current_thread")]
    async fn a_proxy_a_session_detached_from_is_never_interrupted() {
        let dir = scratch_dir("shared");
        let (detached, _detached_streams) =
            remote_process::attach(sigint_logging_proxy(&dir), "Test")
                .await
                .expect("the proxy spawns");
        let (other, _other_streams) = remote_process::attach(sigint_logging_proxy(&dir), "Test")
            .await
            .expect("the second session joins the proxy");
        let (proxy, grandchild) = read_pids(&dir).await;
        let monitor = TransportMonitor::new();
        monitor.arm_lease(detached.detacher());

        assert!(DetachHandle::new(Arc::clone(&monitor)).detach());
        assert_eq!(monitor.begin_teardown(), ProxyStop::KillOnly);
        tokio::time::timeout(Duration::from_secs(5), detached.release())
            .await
            .expect("another session still holds the proxy");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(process_exists(proxy), "still serving the other session");
        assert!(!other.exited().is_cancelled());

        tokio::time::timeout(Duration::from_secs(10), other.release())
            .await
            .expect("the proxy stops promptly");
        wait_until_gone(proxy).await;
        wait_until_gone(grandchild).await;
        assert_eq!(sigints_logged(&dir), 0, "killed, not interrupted");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// =============================================================================
// End to end through `AcpDriver::connect`, against a fake Blox proxy
// =============================================================================

/// A remote ACP proxy in `sh`: answers `initialize` and `session/new`, logs
/// every line it reads (and any SIGINT) to `$1`, and does what `$2` says once
/// the prompt arrives. Request ids are the SDK's UUID strings.
#[cfg(unix)]
const FAKE_PROXY: &str = r#"
log=$1; mode=$2
trap 'echo SIGINT >> "$log"' INT
reply() { printf '{"jsonrpc":"2.0","id":"%s","result":%s}\n' "$1" "$2"; }
fail() { printf '{"jsonrpc":"2.0","id":"%s","error":%s}\n' "$1" "$2"; }
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$log"
  id=$(printf '%s' "$line" | sed -n 's/^.*"id":"\([^"]*\)".*$/\1/p')
  case "$line" in
    *'"method":"initialize"'*) reply "$id" '{"protocolVersion":1}' ;;
    *'"method":"session/new"'*)
      [ "$mode" = hang-setup ] && { echo SETUP >> "$log"; continue; }
      reply "$id" '{"sessionId":"sess-1"}' ;;
    *'"method":"session/prompt"'*)
      prompt_id=$id
      echo PROMPT >> "$log"
      case "$mode" in
        hang) ;;
        orchestrator-eof)
          echo 'INF Received EOF signal from orchestrator, connection closed' >&2 ;;
        exit) exit 0 ;;
        process-exited)
          fail "$id" '{"code":-32603,"message":"Internal error","data":"Claude Code process exited with code 1"}'
          exit 0 ;;
        auth) fail "$id" '{"code":-32000,"message":"Authentication required"}' ;;
        refused) fail "$id" '{"code":-32602,"message":"Invalid params"}' ;;
        permissions)
          opts='"options":[{"optionId":"allow","name":"Yes","kind":"allow_once"},{"optionId":"reject","name":"No","kind":"reject_once"}]'
          printf '{"jsonrpc":"2.0","id":"perm-foreign","method":"session/request_permission","params":{"sessionId":"other","toolCall":{"toolCallId":"tc-foreign","title":"x"},%s}}\n' "$opts"
          printf '{"jsonrpc":"2.0","id":"perm-own","method":"session/request_permission","params":{"sessionId":"sess-1","toolCall":{"toolCallId":"tc-own","title":"y"},%s}}\n' "$opts" ;;
      esac ;;
    *'"id":"perm-own"'*) reply "$prompt_id" '{"stopReason":"end_turn"}' ;;
  esac
done
"#;

#[cfg(unix)]
mod fake_proxy {
    use super::*;

    pub(super) struct Run {
        pub(super) settled: SessionSettled,
        pub(super) turn: Result<AgentRunOutcome, String>,
        pub(super) log: String,
        pub(super) elapsed: Duration,
    }

    fn driver(log: &Path, mode: &str) -> AcpDriver {
        AcpDriver {
            binary_path: PathBuf::from("/bin/sh"),
            acp_args: vec![
                "-c".to_string(),
                FAKE_PROXY.to_string(),
                "fake-proxy".to_string(),
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
            blox_process: None,
        }
    }

    fn log_path(mode: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "acp-transport-loss-{}-{mode}-{nanos}.log",
            std::process::id()
        ))
    }

    async fn wait_for_log(log: &Path, needle: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if std::fs::read_to_string(log).is_ok_and(|text| text.contains(needle)) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the fake proxy never logged {needle}"));
    }

    /// Connect, prompt, and (when `detach_on` names a log marker) detach once
    /// the proxy logs it; returns what the connection settled with.
    pub(super) async fn run(mode: &str, detach_on: Option<&str>) -> Run {
        let log = log_path(mode);
        let local = tokio::task::LocalSet::new();
        let run = local
            .run_until(async {
                let driver = driver(&log, mode);
                let store: Arc<dyn Store> = Arc::new(RecordingStore::default());
                let writer: Arc<dyn MessageWriter> = Arc::new(BasicMessageWriter::new());
                let cancel = CancellationToken::new();
                let started = tokio::time::Instant::now();
                let mut connection = driver
                    .connect(
                        "local-session",
                        &std::env::temp_dir(),
                        &store,
                        &writer,
                        &cancel,
                        None,
                        &[],
                        None,
                    )
                    .await
                    .expect("the proxy spawns");
                let settled_rx = connection.take_settled_receiver().expect("settled rx");
                let detach = connection.detach_handle();
                let detacher = async {
                    if let Some(marker) = detach_on {
                        wait_for_log(&log, marker).await;
                        assert!(detach.detach(), "the proxy group is armed");
                    }
                };
                let (turn, ()) = tokio::join!(connection.prompt("hello", &[]), detacher);
                let settled = tokio::time::timeout(Duration::from_secs(30), settled_rx)
                    .await
                    .expect("the connection settles")
                    .expect("settled event sent");
                Run {
                    settled,
                    turn,
                    log: String::new(),
                    elapsed: started.elapsed(),
                }
            })
            .await;
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_file(&log);
        Run { log: text, ..run }
    }
}

/// The quit path's detach: the proxy group is SIGKILLed with the prompt in
/// flight, and the remote agent sees neither a `session/cancel` nor a
/// SIGINT — its turn keeps running on the workstation.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn detaching_mid_turn_sends_no_cancel_and_no_sigint() {
    let run = fake_proxy::run("hang", Some("PROMPT")).await;
    assert_eq!(run.settled.reason, SessionSettleReason::Detached);
    assert_eq!(run.settled.transport_loss, turn_loss());
    assert!(run.settled.outcome.is_err(), "{:?}", run.settled.outcome);
    assert!(run.turn.is_err(), "{:?}", run.turn);
    assert!(run.log.contains("PROMPT"), "{}", run.log);
    assert!(!run.log.contains("session/cancel"), "{}", run.log);
    assert!(!run.log.contains("SIGINT"), "{}", run.log);
    assert!(
        run.elapsed < Duration::from_secs(5),
        "no SIGINT grace was waited out: {:?}",
        run.elapsed
    );
}

/// Detached before any prompt went out: nothing was running remotely, so the
/// session simply stopped.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn detaching_before_the_prompt_settles_cancelled() {
    let run = fake_proxy::run("hang-setup", Some("SETUP")).await;
    assert_eq!(run.settled.reason, SessionSettleReason::Cancelled);
    assert_eq!(run.settled.outcome, Ok(AgentRunOutcome::Cancelled));
    assert_eq!(run.settled.transport_loss, None);
    assert!(!run.log.contains("PROMPT"), "{}", run.log);
    assert!(!run.log.contains("SIGINT"), "{}", run.log);
}

/// A reusing proxy whose remote process died: stdout stays open, and the
/// stderr line is the only sign. The turn is a transport loss, not a hang
/// and not an agent error, and teardown sends no SIGINT.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn the_proxys_orchestrator_eof_line_is_a_turn_loss() {
    let run = fake_proxy::run("orchestrator-eof", None).await;
    assert_eq!(run.settled.reason, SessionSettleReason::Failed);
    assert_eq!(run.settled.transport_loss, turn_loss());
    assert!(run.settled.outcome.is_err());
    assert!(!run.log.contains("SIGINT"), "{}", run.log);
    assert!(!run.log.contains("session/cancel"), "{}", run.log);
}

/// The proxy's stdout closing under the prompt: the SDK fails it with its
/// structured transport-closed error.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn stdout_closing_mid_turn_is_a_turn_loss() {
    let run = fake_proxy::run("exit", None).await;
    assert_eq!(run.settled.reason, SessionSettleReason::Failed);
    assert_eq!(run.settled.transport_loss, turn_loss());
}

/// Errors the agent *answered* stay plain failures, even when the bridge
/// exits right behind its answer.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn errors_the_agent_answered_are_not_transport_loss() {
    for mode in ["process-exited", "auth", "refused"] {
        let run = fake_proxy::run(mode, None).await;
        assert_eq!(run.settled.reason, SessionSettleReason::Failed, "{mode}");
        assert_eq!(run.settled.transport_loss, None, "{mode}");
        assert!(run.settled.outcome.is_err(), "{mode}");
    }
}

/// A shared bridge relays another session's permission request: it is left
/// unanswered, while this session's own request is answered.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn another_sessions_permission_request_is_left_unanswered() {
    let run = fake_proxy::run("permissions", None).await;
    assert_eq!(run.settled.outcome, Ok(AgentRunOutcome::Completed));
    assert_eq!(run.settled.transport_loss, None);
    assert!(
        run.log
            .lines()
            .any(|line| line.contains("\"id\":\"perm-own\"")),
        "the own request is answered: {}",
        run.log
    );
    assert!(
        !run.log.contains("perm-foreign"),
        "the foreign request got an answer: {}",
        run.log
    );
}
