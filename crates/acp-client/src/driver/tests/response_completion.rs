//! Caller-directed completion at response boundaries, independent of cancellation.
use super::*;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;

const ARTIFACT: &str = "complete artifact with next steps";

#[derive(Default)]
struct ArtifactWriter {
    text: Mutex<String>,
    metadata: Mutex<Vec<super::super::AcpEventMetadata>>,
    acceptance_checks: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl MessageWriter for ArtifactWriter {
    async fn append_text(&self, text: &str) {
        self.text.lock().unwrap().push_str(text);
    }
    async fn finalize(&self) {
        self.text.lock().unwrap().clear();
    }
    async fn record_acp_event_metadata(&self, metadata: super::super::AcpEventMetadata) {
        self.metadata.lock().unwrap().push(metadata);
    }
    async fn should_finish_after_response(&self) -> bool {
        self.acceptance_checks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    handler.response.lock().await.start_prompt("sess-1".into());
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

fn successful_result(id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "result", "subtype": "success", "is_error": false,
        "num_turns": 1, "stop_reason": "end_turn", "uuid": id, "result": text,
        "usage": { "output_tokens": 10 },
    })
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
        feed_sdk_frame(&handler, successful_result("background", ARTIFACT)).await;
        feed_sdk_frame(&handler, successful_result("background", ARTIFACT)).await;
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

    for mode in [TaskTrackingMode::Raw, TaskTrackingMode::Typed] {
        // 0: prompt reply; 1: hold; 2: pending prompt; 3: autonomous
        // pending continuation; 4: result-only; 5: late chunk; 6: user Stop;
        // 7: background holding disabled.
        for scenario in 0..8 {
            let continuation = scenario == 1;
            let writer = Arc::new(ArtifactWriter::default());
            let cancel = CancellationToken::new();
            let handler = Arc::new(AcpNotificationHandler::new(
                writer.clone(),
                false,
                vec![],
                cancel.clone(),
            ));

            let user_cancel = cancel.clone();
            let agent = agent_client_protocol::Agent
            .builder()
            .on_receive_request(
                async move |_: InitializeRequest, responder, _| {
                    let response = InitializeResponse::new(ProtocolVersion::V1);
                    responder.respond(if mode == TaskTrackingMode::Typed { response.meta(super::super::air_client_capabilities_meta()) } else { response })
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
                async move |_: PromptRequest, responder, connection| {
                    let send_result = |message| {
                        let params = serde_json::value::to_raw_value(&serde_json::json!({
                            "sessionId": "sess-1", "message": message,
                        })).unwrap();
                        connection.send_notification(agent_client_protocol::schema::v1::AgentNotification::ExtNotification(ExtNotification::new(
                            "_claude/sdkMessage", params.into(),
                        ))).unwrap();
                    };
                    if mode == TaskTrackingMode::Typed {
                        let notification = super::super::IncomingSessionUpdate::parse_message(
                            "session/update", &serde_json::json!({
                                "sessionId": "sess-1", "update": {
                                    "sessionUpdate": "async_task_spawned", "asyncTaskId": "live-task",
                                },
                            }),
                        ).unwrap();
                        connection.send_notification(notification).unwrap();
                    } else {
                        send_result(task_started("live-task"));
                    }
                    if scenario == 3 {
                        connection.send_notification(agent_text(None, "working")).unwrap();
                        send_result(successful_result("initial", "working"));
                    }
                    if scenario != 4 {
                        connection.send_notification(agent_text(None,
                            if continuation { "working" }
                            else if scenario == 5 { "complete artifact" }
                            else { ARTIFACT },
                        )).unwrap();
                    }
                    if scenario == 6 {
                        user_cancel.cancel();
                        return responder.respond(PromptResponse::new(StopReason::Cancelled));
                    }
                    if scenario >= 2 {
                        let mut result = successful_result("final", ARTIFACT);
                        if scenario == 3 {
                            result["origin"] = serde_json::json!({"kind": "task-notification"});
                        }
                        send_result(result);
                        // The bridge sends a result before its fallback text. The
                        // complete artifact must already be persisted at settlement.
                        if scenario == 4 {
                            connection.send_notification(agent_text(None, ARTIFACT)).unwrap();
                        } else if scenario == 5 {
                            connection.send_notification(agent_text(None, " with next steps")).unwrap();
                        }
                        // Neither idle nor the prompt reply is sent, even though
                        // the subagent is still live. Only a raw result can finish.
                        if scenario != 7 {
                            std::future::pending::<()>().await;
                        }
                    }
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
            agent_client_protocol::Client.builder()
                .on_receive_notification({
                    let handler = handler.clone();
                    async move |notification: super::super::IncomingSessionUpdate, _| {
                        match notification {
                            super::super::IncomingSessionUpdate::Standard(update) => handler.session_notification(update).await,
                            super::super::IncomingSessionUpdate::AsyncTask(update) => handler.async_task_update(update).await,
                        }
                    }
                }, agent_client_protocol::on_receive_notification!())
                .on_receive_notification({
                    let handler = handler.clone();
                    async move |notification: agent_client_protocol::schema::v1::AgentNotification, _| {
                        if let agent_client_protocol::schema::v1::AgentNotification::ExtNotification(ext) = notification {
                            handler.ext_notification(ext).await?;
                        }
                        Ok(())
                    }
                }, agent_client_protocol::on_receive_notification!())
                .connect_with(agent, async |connection| {
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
                    if scenario == 7 { None } else { Some(&QUIESCENCE_PROBE) },
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
                        feed_sdk_frame(&handler, successful_result("background", ARTIFACT)).await;
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
                    if scenario == 6 {
                        AgentRunOutcome::Cancelled
                    } else {
                        AgentRunOutcome::Completed
                    },
                    if scenario == 6 {
                        SessionSettleReason::Cancelled
                    } else if scenario == 7 {
                        SessionSettleReason::Immediate
                    } else {
                        SessionSettleReason::ResponseComplete
                    }
                )
            );
            assert_eq!(
                *writer.text.lock().unwrap(),
                ARTIFACT,
                "scenario {scenario}, mode {mode:?}"
            );
            assert_eq!(
                handler.subscribe_background_activity().borrow().live_tasks,
                1,
                "scenario {scenario}, mode {mode:?}"
            );
            assert_eq!(cancel.is_cancelled(), scenario == 6);
            assert!(!active.load(std::sync::atomic::Ordering::Relaxed));
        }
    }
}

#[tokio::test]
async fn unsuccessful_unrelated_and_historical_results_do_not_accept_notes() {
    for mutation in [
        "error",
        "refusal",
        "max_tokens",
        "cancelled",
        "zero",
        "subtype",
        "foreign",
        "unrelated",
    ] {
        let writer = Arc::new(ArtifactWriter::default());
        let handler = Arc::new(AcpNotificationHandler::new(
            writer,
            false,
            vec![],
            CancellationToken::new(),
        ));
        start_task(&handler, TaskTrackingMode::Raw).await;
        feed(&handler, agent_text(None, ARTIFACT)).await;
        let mut result = successful_result("bad", ARTIFACT);
        match mutation {
            "error" => result["is_error"] = true.into(),
            "zero" => result["num_turns"] = 0.into(),
            "subtype" => result["subtype"] = "error_during_execution".into(),
            "foreign" => result["parent_tool_use_id"] = "subagent".into(),
            "unrelated" => result["result"] = "different response".into(),
            reason => result["stop_reason"] = reason.into(),
        }
        feed_sdk_frame(&handler, result).await;
        assert!(
            !handler
                .subscribe_background_activity()
                .borrow()
                .finish_requested,
            "{mutation}"
        );
    }
    let handler = Arc::new(AcpNotificationHandler::new(
        Arc::new(ArtifactWriter::default()),
        false,
        vec![],
        CancellationToken::new(),
    ));
    start_task(&handler, TaskTrackingMode::Raw).await;
    feed(&handler, agent_text(None, ARTIFACT)).await;
    handler.transition_to_background_holding().await;
    feed_sdk_frame(&handler, successful_result("old", "")).await;
    assert!(
        !handler
            .subscribe_background_activity()
            .borrow()
            .finish_requested
    );
}

#[tokio::test]
async fn subagent_tool_call_mid_message_does_not_split_result_certification() {
    let writer = Arc::new(ArtifactWriter::default());
    let handler = Arc::new(AcpNotificationHandler::new(
        writer.clone(),
        false,
        vec![],
        CancellationToken::new(),
    ));
    start_task(&handler, TaskTrackingMode::Raw).await;
    feed(&handler, agent_text(Some("msg-1"), "complete artifact")).await;
    feed(&handler, tool_call_notification("subagent-grep")).await;
    feed(&handler, agent_text(Some("msg-1"), " with next steps")).await;
    feed_sdk_frame(&handler, successful_result("interrupted", ARTIFACT)).await;
    assert!(
        handler
            .subscribe_background_activity()
            .borrow()
            .finish_requested
    );
    assert!(writer
        .metadata
        .lock()
        .unwrap()
        .iter()
        .any(|event| event.event_kind.as_deref() == Some("response_result")));
}

#[tokio::test]
async fn result_only_fallback_and_duplicate_results_are_recorded_once() {
    let writer = Arc::new(ArtifactWriter::default());
    let handler = Arc::new(AcpNotificationHandler::new(
        writer.clone(),
        false,
        vec![],
        CancellationToken::new(),
    ));
    start_task(&handler, TaskTrackingMode::Raw).await;
    handler.transition_to_background_holding().await;
    feed_sdk_frame(&handler, successful_result("only", ARTIFACT)).await;
    feed(
        &handler,
        agent_text(Some("fallback-id"), "complete artifact"),
    )
    .await;
    feed(
        &handler,
        agent_text(Some("fallback-id"), " with next steps"),
    )
    .await;
    feed_sdk_frame(&handler, successful_result("only", ARTIFACT)).await;
    assert_eq!(*writer.text.lock().unwrap(), ARTIFACT);
    assert!(
        handler
            .subscribe_background_activity()
            .borrow()
            .finish_requested
    );
    assert_eq!(
        writer
            .acceptance_checks
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    let metadata = writer.metadata.lock().unwrap();
    let result = metadata
        .iter()
        .find(|event| event.event_kind.as_deref() == Some("response_result"))
        .unwrap();
    assert_eq!(result.usage.as_ref().unwrap()["output_tokens"], 10);
    assert_eq!(result.content.as_ref().unwrap()["result"], ARTIFACT);
}

#[tokio::test]
async fn replay_and_pre_prompt_results_cannot_finish_or_be_reused() {
    for replaying in [false, true] {
        let writer = Arc::new(ArtifactWriter::default());
        let handler = Arc::new(AcpNotificationHandler::new(
            writer.clone(),
            replaying,
            vec![],
            CancellationToken::new(),
        ));
        feed_sdk_frame(&handler, successful_result("historical", ARTIFACT)).await;
        assert!(
            !handler
                .subscribe_background_activity()
                .borrow()
                .finish_requested
        );
        handler.transition_to_live().await;
        handler.response.lock().await.start_prompt("sess-1".into());
        feed(&handler, agent_text(None, ARTIFACT)).await;
        feed_sdk_frame(&handler, successful_result("historical", ARTIFACT)).await;
        assert!(
            !handler
                .subscribe_background_activity()
                .borrow()
                .finish_requested
        );
        assert_eq!(
            writer
                .acceptance_checks
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }
}

#[tokio::test]
async fn ordinary_writer_does_not_request_finish_for_successful_results() {
    let handler = hold_test_handler();
    handler.response.lock().await.start_prompt("sess-1".into());
    feed(&handler, agent_text(None, ARTIFACT)).await;
    feed_sdk_frame(&handler, successful_result("ordinary", ARTIFACT)).await;
    assert!(
        !handler
            .subscribe_background_activity()
            .borrow()
            .finish_requested
    );
}
