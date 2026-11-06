//! Keeping one connection's state its own: frames relayed for another ACP
//! session are dropped, frames that arrive before the connection's session is
//! named wait for it, and stopping a remote proxy takes its whole process
//! group with it.

use super::real_bridge::{install_logger, logged};
use super::*;
use crate::driver::UNNAMED_GATE_FRAME_LIMIT;
use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandsUpdate, InitializeRequest, InitializeResponse,
};

fn handler_with_writer() -> (Arc<BasicMessageWriter>, AcpNotificationHandler) {
    let writer = Arc::new(BasicMessageWriter::new());
    let handler = AcpNotificationHandler::new(
        Arc::clone(&writer) as Arc<dyn MessageWriter>,
        false,
        vec![],
        CancellationToken::new(),
    );
    (writer, handler)
}

fn agent_text(session_id: &str, text: &str) -> SessionNotification {
    SessionNotification::new(
        session_id.to_string(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(AcpContentBlock::Text(
            TextContent::new(text),
        ))),
    )
}

fn async_task_spawned(session_id: &str, task_id: &str) -> AsyncTaskNotification {
    AsyncTaskNotification::from_params(&serde_json::json!({
        "sessionId": session_id,
        "update": {
            "sessionUpdate": "async_task_spawned",
            "asyncTaskId": task_id,
            "name": "Investigate",
        },
    }))
    .expect("a spawned async task parses")
}

fn sdk_task_started(session_id: &str, task_id: &str) -> ExtNotification {
    let params = serde_json::value::to_raw_value(&serde_json::json!({
        "sessionId": session_id,
        "message": {
            "type": "system",
            "subtype": "task_started",
            "task_id": task_id,
        },
    }))
    .expect("params serialize");
    ExtNotification::new(CLAUDE_SDK_MESSAGE_METHOD, params.into())
}

/// Feed one of each gated frame kind stamped `session_id`.
async fn feed_each_kind(handler: &AcpNotificationHandler, session_id: &str, text: &str) {
    handler
        .session_notification(agent_text(session_id, text))
        .await
        .expect("a session update is never rejected");
    handler
        .async_task_update(async_task_spawned(
            session_id,
            &format!("{session_id}-typed"),
        ))
        .await
        .expect("a typed task update is never rejected");
    handler
        .ext_notification(sdk_task_started(session_id, &format!("{session_id}-raw")))
        .await
        .expect("a raw SDK frame is never rejected");
}

/// The Blox proxy reuses one long-lived workstation bridge, which relays
/// every session's `session/update` frames down every open connection. Once
/// the connection knows its own session, frames for any other session must
/// not reach the writer, the task sets or the activity signal — the holding
/// debounce reads that signal, and a stranger's trailing turn would keep it
/// busy.
#[tokio::test(flavor = "current_thread")]
async fn frames_for_another_session_are_dropped_once_the_own_session_is_named() {
    let (writer, handler) = handler_with_writer();
    let mut activity_rx = handler.subscribe_background_activity();
    handler.name_session("mine").await;
    handler.transition_to_live().await;
    let baseline = activity_rx.borrow_and_update().clone();

    feed_each_kind(&handler, "other", "a stranger's answer").await;

    assert_eq!(
        writer.get_text().await,
        "",
        "nothing of the stranger's was written"
    );
    let after_foreign = activity_rx.borrow_and_update().clone();
    assert_eq!(
        after_foreign.activity_seq, baseline.activity_seq,
        "foreign frames are not activity"
    );
    assert_eq!(after_foreign.live_tasks, 0, "foreign tasks are not tracked");
    assert!(!after_foreign.sdk_frames_seen);
    assert!(handler.typed_tasks.lock().await.live_ids().is_empty());

    handler
        .session_notification(agent_text("mine", "my answer"))
        .await
        .expect("own update accepted");
    assert_eq!(writer.get_text().await, "my answer");
    assert!(
        activity_rx.borrow_and_update().activity_seq > baseline.activity_seq,
        "own frames are activity"
    );
}

/// Before `session/new` answers, the connection cannot tell its own frames
/// from a stranger's: a reused workstation bridge relays an older session's
/// still-streaming turn from the moment the connection is attached, while
/// the agent's first frames for the new session (the Claude bridge's
/// `available_commands_update`) can beat the answer through the dispatch
/// task. Everything is held, nothing counts as activity yet, and naming the
/// session releases exactly its own frames, in arrival order, ahead of the
/// next live one.
#[tokio::test(flavor = "current_thread")]
async fn frames_before_the_session_is_named_are_held_and_only_the_own_ones_released() {
    let (writer, handler) = handler_with_writer();
    let mut activity_rx = handler.subscribe_background_activity();
    handler.transition_to_live().await;
    let baseline = activity_rx.borrow_and_update().clone();

    feed_each_kind(&handler, "older", "a stranger's early chunk ").await;
    feed_each_kind(&handler, "mine", "my early chunk ").await;
    handler
        .session_notification(agent_text("older", "more of the stranger's turn "))
        .await
        .expect("a held update is not rejected");

    assert_eq!(writer.get_text().await, "", "nothing is written unnamed");
    let while_unnamed = activity_rx.borrow_and_update().clone();
    assert_eq!(
        while_unnamed.activity_seq, baseline.activity_seq,
        "held frames are not activity yet"
    );
    assert!(!while_unnamed.sdk_frames_seen);

    handler.name_session("mine").await;

    assert_eq!(writer.get_text().await, "my early chunk ");
    let released = activity_rx.borrow_and_update().clone();
    assert!(
        released.activity_seq > baseline.activity_seq,
        "released frames are activity"
    );
    assert!(released.sdk_frames_seen, "the own raw frame was released");
    assert_eq!(
        handler.background_tasks.lock().await.sorted_ids(),
        vec!["mine-raw"],
        "only the own raw task is tracked"
    );
    assert_eq!(
        handler.typed_tasks.lock().await.live_ids(),
        vec!["mine-typed"],
        "only the own typed task is tracked"
    );

    handler
        .session_notification(agent_text("mine", "then the live answer"))
        .await
        .expect("own update accepted");
    handler
        .session_notification(agent_text("older", "the stranger again"))
        .await
        .expect("a foreign update is dropped, not rejected");
    assert_eq!(
        writer.get_text().await,
        "my early chunk then the live answer"
    );
}

/// A stranger's turn streaming down an unnamed connection is bounded: past
/// [`UNNAMED_GATE_FRAME_LIMIT`] the oldest held frame goes, so the own
/// session's frames, which only race the `session/new` answer and so arrive
/// last, survive any amount of foreign traffic before them.
#[tokio::test(flavor = "current_thread")]
async fn the_unnamed_gate_drops_its_oldest_frames_past_the_limit() {
    let (writer, handler) = handler_with_writer();
    handler.transition_to_live().await;

    handler
        .session_notification(agent_text("mine", "pushed out "))
        .await
        .expect("held");
    for index in 0..UNNAMED_GATE_FRAME_LIMIT * 2 {
        handler
            .session_notification(agent_text("older", &format!("stranger {index} ")))
            .await
            .expect("held");
    }
    handler
        .session_notification(agent_text("mine", "kept"))
        .await
        .expect("held");

    handler.name_session("mine").await;
    assert_eq!(writer.get_text().await, "kept");
}

/// A reattach is routed its running turn's frames while `initialize` is
/// answered, so its own frames can be the ones pushed out: the oldest go,
/// the rest are released in order, and naming the session says how many
/// were lost.
#[tokio::test]
async fn an_overflowing_gate_drops_the_sessions_oldest_frames_and_says_so() {
    install_logger();
    let (writer, handler) = handler_with_writer();
    handler.transition_to_live().await;

    for index in 0..UNNAMED_GATE_FRAME_LIMIT + 3 {
        handler
            .session_notification(agent_text("overflowing", &format!("{index} ")))
            .await
            .expect("held");
    }
    handler.name_session("overflowing").await;

    let expected: String = (3..UNNAMED_GATE_FRAME_LIMIT + 3)
        .map(|index| format!("{index} "))
        .collect();
    assert_eq!(writer.get_text().await, expected);
    assert!(
        logged(&format!(
            "ACP session overflowing: dropped the oldest 3 of its frames received before it was \
             named, past the {UNNAMED_GATE_FRAME_LIMIT}-frame limit"
        )),
        "the overflow is logged"
    );
}

#[derive(Default)]
struct EventLogWriter {
    log: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl MessageWriter for EventLogWriter {
    async fn append_text(&self, text: &str) {
        self.log.lock().unwrap().push(format!("text:{text}"));
    }

    async fn finalize(&self) {}

    async fn record_tool_call(
        &self,
        _tool_call_id: &str,
        _title: &str,
        _raw_input: Option<&serde_json::Value>,
    ) {
    }

    async fn update_tool_call_title(
        &self,
        _tool_call_id: &str,
        _title: Option<&str>,
        _raw_input: Option<&serde_json::Value>,
    ) {
    }

    async fn record_tool_result(&self, _tool_call_id: &str, _content: &str) {}

    async fn record_acp_event_metadata(&self, metadata: AcpEventMetadata) {
        if let Some(kind) = metadata.event_kind {
            self.log.lock().unwrap().push(format!("event:{kind}"));
        }
    }
}

/// End to end through `setup_acp_session` and the SDK's dispatch: the agent
/// sends a stranger's chunk and the new session's command list *before*
/// answering `session/new`, the worst case of a reused bridge. The new
/// session keeps its commands (the composer's slash-command list reads them)
/// and stores nothing of the stranger's.
#[tokio::test(flavor = "current_thread")]
async fn session_setup_keeps_own_frames_that_beat_the_session_new_answer() {
    let agent = agent_client_protocol::Agent
        .builder()
        .on_receive_request(
            async |request: InitializeRequest, responder, _cx| {
                responder.respond(InitializeResponse::new(request.protocol_version))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |_request: NewSessionRequest, responder, cx| {
                cx.send_notification(agent_text("older", "a stranger's chunk"))?;
                cx.send_notification(SessionNotification::new(
                    "sess-new",
                    SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(vec![
                        AvailableCommand::new("review", "Review the branch"),
                    ])),
                ))?;
                responder.respond(NewSessionResponse::new("sess-new"))
            },
            agent_client_protocol::on_receive_request!(),
        );
    let writer = Arc::new(EventLogWriter::default());
    let handler = Arc::new(AcpNotificationHandler::new(
        Arc::clone(&writer) as Arc<dyn MessageWriter>,
        false,
        vec![],
        CancellationToken::new(),
    ));
    let store: Arc<dyn Store> = Arc::new(RecordingStore::default());
    let notification_handler = Arc::clone(&handler);

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: IncomingSessionUpdate, _cx| match notification {
                IncomingSessionUpdate::Standard(notification) => {
                    notification_handler
                        .session_notification(notification)
                        .await
                }
                IncomingSessionUpdate::AsyncTask(notification) => {
                    notification_handler.async_task_update(notification).await
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(agent, async |connection| {
            let setup = setup_acp_session(AcpSessionSetupContext {
                connection: &connection,
                working_dir: Path::new("/tmp"),
                store: &store,
                handler: &handler,
                our_session_id: "local-session",
                acp_session_id: None,
                config_options: &[],
                mcp_servers: &[],
                agent_label: "Fake",
                auth_selection: None,
                unattended_session_mode: None,
            })
            .await
            .map_err(agent_client_protocol::util::internal_error)?;
            assert_eq!(setup.agent_session_id, "sess-new");
            // Frames are dispatched in order, so once the command list is in
            // the stranger's earlier chunk has been dealt with too.
            tokio::time::timeout(Duration::from_secs(5), async {
                while writer.log.lock().unwrap().is_empty() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the own command list arrives");
            Ok(())
        })
        .await
        .expect("setup succeeds");

    assert_eq!(
        writer.log.lock().unwrap().as_slice(),
        &["event:available_commands_update"]
    );
}

#[cfg(unix)]
mod proxy_group {
    use super::super::*;
    use crate::driver::graceful_stop_with_grace;
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, BufReader};

    /// The `blox.exoskeleton` stand-in: ignores HUP and INT like the real
    /// one, so the SIGHUP+SIGCONT the kernel sends an orphaned stopped group
    /// would only revive it, prints its own pid once the traps are
    /// installed, then stops itself.
    const FROZEN_GRANDCHILD: &str =
        "sh -c 'trap \"\" HUP INT; echo $$; kill -STOP $$; exec sleep 300' &";

    /// A remote proxy process group whose wrapper ignores SIGINT (as a frozen
    /// `sq` would) and waits on its frozen grandchild.
    async fn spawn_frozen_proxy_group() -> (tokio::process::Child, i32) {
        spawn_proxy_group(&format!("trap '' INT; {FROZEN_GRANDCHILD} wait $!")).await
    }

    /// A remote proxy process group whose wrapper honours SIGINT and exits at
    /// once (as a healthy `sq` does), leaving its frozen grandchild behind.
    async fn spawn_sigint_honouring_proxy_group() -> (tokio::process::Child, i32) {
        spawn_proxy_group(&format!("trap 'exit 130' INT; {FROZEN_GRANDCHILD} wait $!")).await
    }

    async fn spawn_proxy_group(script: &str) -> (tokio::process::Child, i32) {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        cmd.process_group(0);
        // A test binary started from a background job inherits SIGINT
        // ignored, and a non-interactive `sh` cannot trap a signal ignored on
        // entry; the wrapper's `trap ... INT` must mean what it says.
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
        let grandchild: i32 = line.trim().parse().expect("a pid");
        (child, grandchild)
    }

    fn process_exists(pid: i32) -> bool {
        // kill(pid, 0) probes for existence; a zombie still counts, so callers
        // poll until the orphan has been reaped.
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
    }

    /// SIGKILLs the grandchild when a test fails before it is gone, so a
    /// failure leaves no stopped `sleep` behind. Only while panicking: a
    /// passing test has already watched the pid disappear, and it could have
    /// been reused since.
    struct ReapOnFailure(i32);

    impl Drop for ReapOnFailure {
        fn drop(&mut self) {
            if std::thread::panicking() {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(self.0),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
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

    /// The teardown of a remote proxy that does not honour SIGINT: after the
    /// grace, SIGKILL goes to the *group* — not just the wrapper pid, which
    /// would leave the stopped child behind.
    #[tokio::test(flavor = "current_thread")]
    async fn a_remote_proxy_group_that_ignores_sigint_is_killed_whole_after_the_grace() {
        let (mut child, grandchild) = spawn_frozen_proxy_group().await;
        let _reap = ReapOnFailure(grandchild);
        assert!(process_exists(grandchild));

        let started = tokio::time::Instant::now();
        graceful_stop_with_grace(&mut child, true, Duration::from_millis(300)).await;
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "the SIGINT grace was offered first: {:?}",
            started.elapsed()
        );
        assert!(
            child.try_wait().expect("waitable").is_some(),
            "the wrapper is reaped"
        );
        wait_until_gone(grandchild).await;
    }

    /// The wrapper honours the SIGINT and exits within the grace, but its
    /// grandchild is frozen: the group must still be SIGKILLed after the
    /// wrapper is reaped, or the stopped child is orphaned and continued by
    /// the kernel along the ordinary teardown path (transport loss, normal
    /// turn end).
    #[tokio::test(flavor = "current_thread")]
    async fn a_remote_proxy_group_whose_wrapper_exits_on_sigint_is_still_killed_whole() {
        let (mut child, grandchild) = spawn_sigint_honouring_proxy_group().await;
        let _reap = ReapOnFailure(grandchild);
        assert!(process_exists(grandchild));

        graceful_stop_with_grace(&mut child, true, Duration::from_secs(30)).await;
        let status = child
            .try_wait()
            .expect("waitable")
            .expect("the wrapper is reaped");
        assert_eq!(
            status.code(),
            Some(130),
            "the wrapper exited from its INT trap, not from the SIGKILL: {status:?}"
        );
        wait_until_gone(grandchild).await;
    }
}
