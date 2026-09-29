//! Caller-directed completion at response boundaries, independent of cancellation.
use super::*;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;

const ARTIFACT: &str = "complete artifact with next steps";

#[derive(Default)]
struct ArtifactWriter {
    text: Mutex<String>,
}

#[async_trait::async_trait]
impl MessageWriter for ArtifactWriter {
    async fn append_text(&self, text: &str) {
        self.text.lock().unwrap().push_str(text);
    }
    async fn finalize(&self) {
        self.text.lock().unwrap().clear();
    }
    async fn should_finish_after_response(&self) -> bool {
        *self.text.lock().unwrap() == ARTIFACT
    }
    async fn record_tool_call(&self, _: &str, _: &str, _: Option<&serde_json::Value>) {}
    async fn update_tool_call_title(
        &self,
        _: &str,
        _: Option<&str>,
        _: Option<&serde_json::Value>,
    ) {
    }
    async fn record_tool_result(&self, _: &str, _: &str) {}
}

async fn start_task(handler: &Arc<AcpNotificationHandler>, mode: TaskTrackingMode) {
    handler.set_task_tracking_mode(mode);
    match mode {
        TaskTrackingMode::Raw => feed_sdk_frame(handler, task_started("live-task")).await,
        TaskTrackingMode::Typed => {
            feed_async_task_update(
                handler,
                serde_json::json!({
                    "sessionUpdate": "async_task_spawned", "asyncTaskId": "live-task",
                }),
            )
            .await
        }
    }
}

async fn idle(handler: &Arc<AcpNotificationHandler>) {
    feed_sdk_frame(
        handler,
        serde_json::json!({
            "type": "system", "subtype": "session_state_changed", "state": "idle",
        }),
    )
    .await;
}

async fn assert_holding(hold: Pin<&mut impl Future<Output = HoldOutcome>>) {
    let mut hold = hold;
    poll_fn(|cx| {
        assert!(
            hold.as_mut().poll(cx).is_pending(),
            "response has not finished"
        );
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn initial_response_requests_successful_finish_with_active_tasks() {
    for mode in [TaskTrackingMode::Raw, TaskTrackingMode::Typed] {
        let writer = Arc::new(ArtifactWriter::default());
        let cancel = CancellationToken::new();
        let handler = Arc::new(AcpNotificationHandler::new(
            writer.clone(),
            false,
            vec![],
            cancel.clone(),
        ));
        start_task(&handler, mode).await;
        feed(&handler, agent_text(None, ARTIFACT)).await;
        // Streaming, and even an idle frame during the live turn, must not
        // finish it. The prompt must return successfully first.
        idle(&handler).await;
        assert!(
            !handler
                .subscribe_background_activity()
                .borrow()
                .finish_requested
        );
        handler.finish_completed_response().await;
        let activity = handler.subscribe_background_activity().borrow().clone();
        assert!(activity.finish_requested);
        assert_eq!(activity.live_tasks, 1);
        assert_eq!(*writer.text.lock().unwrap(), ARTIFACT);
        assert!(!cancel.is_cancelled());
    }
}

#[tokio::test]
async fn background_response_releases_wait_only_at_completion_and_clears_indicator() {
    for mode in [TaskTrackingMode::Raw, TaskTrackingMode::Typed] {
        let writer = Arc::new(ArtifactWriter::default());
        let cancel = CancellationToken::new();
        let handler = Arc::new(AcpNotificationHandler::new(
            writer.clone(),
            false,
            vec![],
            cancel.clone(),
        ));
        start_task(&handler, mode).await;
        feed(&handler, agent_text(None, "initial response")).await;
        handler.finish_completed_response().await;
        handler.transition_to_background_holding().await;
        let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (_control_tx, mut control_rx) = mpsc::unbounded_channel();
        let child_exited = CancellationToken::new();
        let active = AtomicBool::new(false);
        let (observer, reported) = recording_hold_observer();
        let mut hold = std::pin::pin!(hold_for_background_quiescence(
            &QUIESCENCE_PROBE,
            mode,
            &handler,
            &cancel,
            &child_exited,
            &mut prompt_rx,
            &mut control_rx,
            &active,
            &|_| {},
            "sess-1",
            Some(&observer),
        ));
        assert_holding(hold.as_mut()).await;
        feed(&handler, agent_text(None, "complete artifact")).await;
        assert_holding(hold.as_mut()).await;
        feed(&handler, agent_text(None, " with next steps")).await;
        // The caller would accept this text, but it is still streaming.
        assert_holding(hold.as_mut()).await;
        idle(&handler).await;
        // A later nonterminal update can coalesce with the finish notification.
        // It must not erase the completed response's request to end the hold.
        match mode {
            TaskTrackingMode::Raw => feed_sdk_frame(&handler, task_started("another-task")).await,
            TaskTrackingMode::Typed => {
                feed_async_task_update(
                    &handler,
                    serde_json::json!({
                        "sessionUpdate": "async_task_spawned", "asyncTaskId": "another-task",
                    }),
                )
                .await
            }
        }
        let outcome = tokio::time::timeout(SETTLE_TIMEOUT, hold).await.unwrap();
        assert!(matches!(outcome, HoldOutcome::ResponseComplete));
        assert_eq!(
            handler.subscribe_background_activity().borrow().live_tasks,
            2
        );
        assert_eq!(*writer.text.lock().unwrap(), ARTIFACT);
        assert!(!cancel.is_cancelled());
        assert_eq!(
            reported.lock().unwrap().last(),
            Some(&BackgroundHoldStatus::default())
        );
    }
}

#[tokio::test]
async fn idle_without_a_new_artifact_keeps_waiting() {
    let writer = Arc::new(ArtifactWriter::default());
    let cancel = CancellationToken::new();
    let handler = Arc::new(AcpNotificationHandler::new(
        writer.clone(),
        false,
        vec![],
        cancel.clone(),
    ));
    start_task(&handler, TaskTrackingMode::Raw).await;
    // A prior response must not satisfy an idle edge before continuation text.
    feed(&handler, agent_text(None, ARTIFACT)).await;
    handler.transition_to_background_holding().await;
    idle(&handler).await;
    assert!(
        !handler
            .subscribe_background_activity()
            .borrow()
            .finish_requested
    );
    feed(&handler, agent_text(None, "still working")).await;
    idle(&handler).await;
    let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
    let (_control_tx, mut control_rx) = mpsc::unbounded_channel();
    let child_exited = CancellationToken::new();
    let active = AtomicBool::new(false);
    let mut hold = std::pin::pin!(hold_for_background_quiescence(
        &QUIESCENCE_PROBE,
        TaskTrackingMode::Raw,
        &handler,
        &cancel,
        &child_exited,
        &mut prompt_rx,
        &mut control_rx,
        &active,
        &|_| {},
        "sess-1",
        None,
    ));
    assert_holding(hold.as_mut()).await;
    cancel.cancel();
    assert!(matches!(hold.await, HoldOutcome::Cancelled));
}

#[tokio::test(flavor = "current_thread")]
async fn session_loop_finishes_successfully_for_initial_and_background_artifacts() {
    use agent_client_protocol::schema::v1::{
        InitializeRequest, InitializeResponse, PromptResponse,
    };
    use agent_client_protocol::schema::ProtocolVersion;
    use std::cell::RefCell;

    for continuation in [false, true] {
        let writer = Arc::new(ArtifactWriter::default());
        let cancel = CancellationToken::new();
        let handler = Arc::new(AcpNotificationHandler::new(
            writer.clone(),
            false,
            vec![],
            cancel.clone(),
        ));
        let prompt_handler = handler.clone();
        let agent = agent_client_protocol::Agent
            .builder()
            .on_receive_request(
                async |_: InitializeRequest, responder, _| {
                    responder.respond(InitializeResponse::new(ProtocolVersion::V1))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async |_: NewSessionRequest, responder, _| {
                    responder.respond(NewSessionResponse::new("sess-1"))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |_: PromptRequest, responder, _| {
                    feed_sdk_frame(&prompt_handler, task_started("live-task")).await;
                    feed(
                        &prompt_handler,
                        agent_text(None, if continuation { "working" } else { ARTIFACT }),
                    )
                    .await;
                    responder.respond(PromptResponse::new(StopReason::EndTurn))
                },
                agent_client_protocol::on_receive_request!(),
            );
        let store: Arc<dyn Store> = Arc::new(RecordingStore::default());
        let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
        let (_control_tx, mut control_rx) = mpsc::unbounded_channel();
        let (reply, _reply_rx) = oneshot::channel();
        prompt_tx
            .send(QueuedSessionTurn {
                prompt: "write".into(),
                images: vec![],
                reply,
            })
            .unwrap();
        let child_exited = CancellationToken::new();
        let active = AtomicBool::new(false);
        let pending_reply = RefCell::new(None);
        let holding = CancellationToken::new();
        let holding_for_observer = holding.clone();
        let observer: BackgroundHoldObserver = Arc::new(move |status| {
            if status.holding {
                holding_for_observer.cancel();
            }
        });
        let outcome = tokio::time::timeout(
            SETTLE_TIMEOUT,
            agent_client_protocol::Client.connect_with(agent, async |connection| {
                let run = super::super::run_acp_session(
                    &connection,
                    Path::new("/tmp"),
                    &store,
                    "sess-1",
                    None,
                    &[],
                    &handler,
                    &[],
                    "test",
                    &cancel,
                    None,
                    Some(&QUIESCENCE_PROBE),
                    Some(&observer),
                    &child_exited,
                    &mut prompt_rx,
                    &mut control_rx,
                    &active,
                    &pending_reply,
                );
                let complete_background = async {
                    if continuation {
                        holding.cancelled().await;
                        feed(&handler, agent_text(None, ARTIFACT)).await;
                        idle(&handler).await;
                    }
                };
                let (result, ()) = tokio::join!(run, complete_background);
                result.map_err(agent_client_protocol::util::internal_error)
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            outcome,
            (
                AgentRunOutcome::Completed,
                SessionSettleReason::ResponseComplete
            )
        );
        assert_eq!(*writer.text.lock().unwrap(), ARTIFACT);
        assert!(!cancel.is_cancelled());
        assert!(!active.load(std::sync::atomic::Ordering::Relaxed));
    }
}
