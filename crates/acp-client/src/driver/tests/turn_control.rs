//! Timely message delivery to a running connection: steering into the live
//! turn, interrupt-and-reprompt on the same connection, and the hold-time and
//! teardown behaviour of the turn-control channel.

use super::*;
use crate::driver::{
    reject_queued_turn_control_requests, run_acp_session, steer_outcome, steering_message,
    steering_supported_from_initialize, HandlerPhase, IncomingSessionUpdate, SessionTurnResult,
    SteerOutcome, TurnControlHandle, TurnControlReply, TurnControlRequest, TurnStep,
    STEERING_META_KEY, STEERING_METHOD,
};
use agent_client_protocol::schema::v1::{
    CancelNotification, InitializeRequest, InitializeResponse, Meta, PromptResponse,
};
use agent_client_protocol::{Responder, UntypedMessage};
use std::cell::RefCell;
use std::future::Future;
use std::sync::atomic::Ordering;

/// Everything a fake agent saw, plus the prompt responder it is holding back.
/// ACP request handlers run inline in the agent's dispatch loop, so a handler
/// that waited for the test would deadlock the connection: the prompt handler
/// stashes its responder here and the test (or a later `session/cancel`)
/// answers it.
#[derive(Default)]
struct FakeAgent {
    prompts: Mutex<Vec<String>>,
    steers: Mutex<Vec<serde_json::Value>>,
    cancels: Mutex<usize>,
    pending_prompt: Mutex<Option<Responder<PromptResponse>>>,
    prompt_arrived: tokio::sync::Notify,
    /// What the agent answers to `_session/steering`; `None` leaves the
    /// method unregistered (an agent without the extension).
    steer_response: Option<serde_json::Value>,
    /// Whether to announce steering support at initialize.
    advertise_steering: bool,
    /// Text the agent streams at the start of each prompt, in order.
    stream_on_prompt: Vec<&'static str>,
    /// Ask permission for a tool call during the Nth (1-based) prompt, and
    /// record the client's answer.
    permission_on_prompt: Option<usize>,
    permission_outcomes: Mutex<Vec<RequestPermissionOutcome>>,
    permission_answered: tokio::sync::Notify,
}

impl FakeAgent {
    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    fn steers(&self) -> Vec<serde_json::Value> {
        self.steers.lock().unwrap().clone()
    }

    fn cancels(&self) -> usize {
        *self.cancels.lock().unwrap()
    }

    async fn await_prompt(&self) {
        tokio::time::timeout(SETTLE_TIMEOUT, self.prompt_arrived.notified())
            .await
            .expect("agent should receive a prompt");
    }

    /// Answer the prompt the agent is holding back.
    fn finish_prompt(&self, stop_reason: StopReason) {
        let responder = self
            .pending_prompt
            .lock()
            .unwrap()
            .take()
            .expect("a prompt should be pending");
        responder
            .respond(PromptResponse::new(stop_reason))
            .expect("prompt response should send");
    }

    fn build(
        self: &Arc<Self>,
    ) -> impl agent_client_protocol::ConnectTo<agent_client_protocol::Client> + use<> {
        let init_agent = Arc::clone(self);
        let prompt_agent = Arc::clone(self);
        let cancel_agent = Arc::clone(self);
        let steer_agent = Arc::clone(self);
        let steer_response = self.steer_response.clone();
        let builder = agent_client_protocol::Agent
            .builder()
            .on_receive_request(
                async move |request: InitializeRequest, responder, _cx| {
                    let mut response = InitializeResponse::new(request.protocol_version);
                    if init_agent.advertise_steering {
                        let mut meta = Meta::new();
                        meta.insert(
                            STEERING_META_KEY.to_string(),
                            serde_json::json!({ "supported": true }),
                        );
                        response = response.meta(meta);
                    }
                    responder.respond(response)
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async |_request: NewSessionRequest, responder, _cx| {
                    responder.respond(NewSessionResponse::new("session-1"))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: PromptRequest, responder, cx| {
                    let text = request
                        .prompt
                        .iter()
                        .filter_map(|block| match block {
                            AcpContentBlock::Text(text) => Some(text.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    let ordinal = {
                        let mut prompts = prompt_agent.prompts.lock().unwrap();
                        prompts.push(text);
                        prompts.len()
                    };
                    if let Some(chunk) = prompt_agent.stream_on_prompt.get(ordinal - 1) {
                        cx.send_notification(agent_text(Some(&format!("msg-{ordinal}")), chunk))?;
                    }
                    if prompt_agent.permission_on_prompt == Some(ordinal) {
                        let tool_call_id = format!("tc-{ordinal}");
                        cx.send_notification(tool_call_notification(&tool_call_id))?;
                        let agent = Arc::clone(&prompt_agent);
                        cx.send_request(permission_request(&tool_call_id))
                            .on_receiving_result(move |result| async move {
                                let response = result?;
                                agent
                                    .permission_outcomes
                                    .lock()
                                    .unwrap()
                                    .push(response.outcome);
                                agent.permission_answered.notify_one();
                                Ok(())
                            })?;
                    }
                    *prompt_agent.pending_prompt.lock().unwrap() = Some(responder);
                    prompt_agent.prompt_arrived.notify_one();
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                async move |_notification: CancelNotification, _cx| {
                    *cancel_agent.cancels.lock().unwrap() += 1;
                    // The bridge aborts the turn and answers its prompt.
                    cancel_agent.finish_prompt(StopReason::Cancelled);
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            );
        // Registered last: `UntypedMessage` matches every method, so it must
        // not shadow the typed handlers above. Left out entirely for an agent
        // without the extension, which then answers method-not-found.
        builder.on_receive_request(
            async move |request: UntypedMessage, responder, _cx| {
                assert_eq!(request.method(), STEERING_METHOD, "unexpected request");
                steer_agent
                    .steers
                    .lock()
                    .unwrap()
                    .push(request.params().clone());
                match &steer_response {
                    Some(response) => responder.respond(response.clone()),
                    None => responder
                        .respond_with_error(agent_client_protocol::Error::method_not_found()),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
    }
}

fn steering_setup(steering_supported: bool) -> AcpSessionSetup {
    AcpSessionSetup {
        agent_session_id: "session-1".to_string(),
        agent_capabilities: AgentCapabilities::default(),
        task_tracking_mode: TaskTrackingMode::Raw,
        auth_methods: vec![],
        steering_supported,
    }
}

fn turn_control_handle(
    turn_control_tx: &mpsc::UnboundedSender<TurnControlRequest>,
    alive: bool,
) -> TurnControlHandle {
    TurnControlHandle {
        turn_control_tx: turn_control_tx.clone(),
        connection_alive: Arc::new(AtomicBool::new(alive)),
    }
}

/// The client side of `session/update`, as [`AcpDriver::connect`] wires it.
async fn dispatch_session_update(
    handler: &Arc<AcpNotificationHandler>,
    notification: IncomingSessionUpdate,
) -> agent_client_protocol::Result<()> {
    match notification {
        IncomingSessionUpdate::Standard(notification) => {
            handler.session_notification(notification).await
        }
        IncomingSessionUpdate::AsyncTask(notification) => {
            handler.async_task_update(notification).await
        }
    }
}

/// Whether the handler is in its holding phase right now. `false` while the
/// lock is momentarily held elsewhere — the poll simply tries again.
fn holding(handler: &AcpNotificationHandler) -> bool {
    handler
        .phase
        .try_lock()
        .map(|phase| matches!(&*phase, HandlerPhase::BackgroundHolding { .. }))
        .unwrap_or(false)
}

/// Poll until `done` holds — for ordering that depends on the client's
/// dispatch loop catching up with a frame the agent already sent.
async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    tokio::time::timeout(SETTLE_TIMEOUT, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

// =============================================================================
// Capability
// =============================================================================

#[test]
fn steering_capability_is_a_top_level_meta_flag() {
    let supported = |meta: serde_json::Value| {
        let serde_json::Value::Object(meta) = meta else {
            panic!("test meta must be an object");
        };
        steering_supported_from_initialize(Some(&meta))
    };

    assert!(supported(
        serde_json::json!({ "steering": { "supported": true } })
    ));
    // Sibling namespaces (the asyncTasks mirror, the bridge's own) don't
    // interfere either way.
    assert!(supported(serde_json::json!({
        "claudeCode": { "version": "0.98.0" },
        "jetbrains": { "air": { "version": 1, "capabilities": ["asyncTasks"] } },
        "steering": { "supported": true },
    })));

    assert!(!steering_supported_from_initialize(None));
    for (label, meta) in [
        ("empty meta", serde_json::json!({})),
        (
            "no steering key",
            serde_json::json!({ "claudeCode": { "version": "0.70.0" } }),
        ),
        (
            "explicitly unsupported",
            serde_json::json!({ "steering": { "supported": false } }),
        ),
        (
            "supported is not a bool",
            serde_json::json!({ "steering": { "supported": "true" } }),
        ),
        (
            "steering is not an object",
            serde_json::json!({ "steering": true }),
        ),
        // Inside agentCapabilities' meta would be a different field; the
        // flag is a top-level sibling.
        (
            "nested under another key",
            serde_json::json!({ "agentCapabilities": { "steering": { "supported": true } } }),
        ),
    ] {
        assert!(!supported(meta), "{label} must read as unsupported");
    }
}

#[test]
fn steering_frame_carries_the_prompt_blocks_and_prompt_required_idle_behavior() {
    let message = steering_message(
        "session-1",
        build_prompt_content_blocks("look at the failing test first", &[], true),
    )
    .expect("frame should build");
    assert_eq!(message.method(), "_session/steering");
    assert_eq!(
        message.params(),
        &serde_json::json!({
            "sessionId": "session-1",
            "prompt": [{ "type": "text", "text": "look at the failing test first" }],
            "_meta": { "steering": { "idleBehavior": "promptRequired" } },
        })
    );
}

#[test]
fn steer_outcome_only_counts_injected_as_delivered() {
    assert_eq!(
        steer_outcome(Ok(serde_json::json!({ "outcome": "injected" }))),
        Ok(SteerOutcome::Injected)
    );
    for (response, needle) in [
        (
            serde_json::json!({ "outcome": "promptRequired", "reason": "noRunningTurn" }),
            "promptRequired' (noRunningTurn)",
        ),
        (
            serde_json::json!({ "outcome": "startedNewTurn" }),
            "startedNewTurn",
        ),
        (serde_json::json!({ "outcome": "failed" }), "failed"),
        (serde_json::json!({}), "malformed"),
    ] {
        let error = steer_outcome(Ok(response)).expect_err("not a delivery");
        assert!(error.contains(needle), "got: {error}");
    }
    let error = steer_outcome(Err(agent_client_protocol::Error::method_not_found()))
        .expect_err("an agent without the extension is not a delivery");
    assert!(error.contains("Steering request failed"), "got: {error}");
}

// =============================================================================
// Steering inside a live turn
// =============================================================================

/// Run one `run_prompt_turn` against the fake agent while `drive` exercises
/// the turn-control handle; returns the turn's step and the writer.
async fn live_turn_with<F, Fut>(
    agent: &Arc<FakeAgent>,
    steering_supported: bool,
    drive: F,
) -> (Result<TurnStep, String>, Arc<TranscriptWriter>)
where
    F: FnOnce(TurnControlHandle, Arc<FakeAgent>, Arc<TranscriptWriter>) -> Fut,
    Fut: Future<Output = ()>,
{
    let writer = Arc::new(TranscriptWriter::default());
    let handler = Arc::new(transcript_handler(&writer));
    let setup = steering_setup(steering_supported);
    let (turn_control_tx, mut turn_control_rx) = mpsc::unbounded_channel();
    let handle = turn_control_handle(&turn_control_tx, true);
    let cancel = CancellationToken::new();
    let notification_handler = Arc::clone(&handler);
    let step = agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: IncomingSessionUpdate, _cx| {
                dispatch_session_update(&notification_handler, notification).await
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(agent.build(), async |connection| {
            let (step, ()) = tokio::join!(
                run_prompt_turn(
                    &connection,
                    &setup,
                    &handler,
                    "local-session",
                    &cancel,
                    &mut turn_control_rx,
                    "first",
                    &[],
                ),
                drive(handle, Arc::clone(agent), Arc::clone(&writer)),
            );
            Ok(step)
        })
        .await
        .expect("the connection itself stays healthy");
    (step, writer)
}

#[tokio::test(flavor = "current_thread")]
async fn steer_in_a_live_turn_sends_the_extension_request_and_persists_the_user_row() {
    let agent = Arc::new(FakeAgent {
        steer_response: Some(serde_json::json!({ "outcome": "injected" })),
        advertise_steering: true,
        stream_on_prompt: vec!["working on it"],
        ..FakeAgent::default()
    });

    let (step, writer) = live_turn_with(&agent, true, |handle, agent, writer| async move {
        agent.await_prompt().await;
        wait_until("the first chunk", || {
            writer
                .text_flow()
                .contains(&WriterCall::Append("working on it".to_string()))
        })
        .await;

        let outcome = handle
            .steer(
                "also run the linter",
                &[PromptImage::new("aGk=", "image/png")],
            )
            .await;
        assert_eq!(outcome, Ok(SteerOutcome::Injected));
        // The user row is written in the ordered response callback: after
        // the open assistant message is closed, before anything else from the
        // agent can be dispatched. The turn itself is still in flight.
        assert_eq!(
            writer.text_flow(),
            vec![
                WriterCall::Append("working on it".to_string()),
                WriterCall::Finalize,
                WriterCall::UserMessage("also run the linter".to_string()),
            ]
        );
        assert!(agent.pending_prompt.lock().unwrap().is_some());
        agent.finish_prompt(StopReason::EndTurn);
    })
    .await;

    assert!(matches!(
        step,
        Ok(TurnStep::Settled(AgentRunOutcome::Completed))
    ));
    assert_eq!(agent.cancels(), 0, "steering never cancels the turn");
    let steers = agent.steers();
    assert_eq!(steers.len(), 1);
    assert_eq!(steers[0]["sessionId"], "session-1");
    assert_eq!(steers[0]["prompt"][0]["text"], "also run the linter");
    // No image capability on the fake agent: the attachment is noted, not sent.
    assert!(steers[0]["prompt"][1]["text"]
        .as_str()
        .is_some_and(|text| text.contains("1 image block(s) omitted")));
    assert_eq!(
        steers[0]["_meta"]["steering"]["idleBehavior"],
        "promptRequired"
    );
    // Nothing more than one row: the agent's echo of the steered message is
    // not a session update this client records.
    assert_eq!(
        writer
            .calls()
            .iter()
            .filter(|call| matches!(call, WriterCall::UserMessage(_)))
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn steer_answered_prompt_required_is_an_error_without_a_user_row() {
    let agent = Arc::new(FakeAgent {
        steer_response: Some(
            serde_json::json!({ "outcome": "promptRequired", "reason": "noRunningTurn" }),
        ),
        advertise_steering: true,
        ..FakeAgent::default()
    });

    let (step, writer) = live_turn_with(&agent, true, |handle, agent, _writer| async move {
        agent.await_prompt().await;
        let error = handle
            .steer("too late", &[])
            .await
            .expect_err("promptRequired is not a delivery");
        assert!(error.contains("promptRequired"), "got: {error}");
        assert!(error.contains("noRunningTurn"), "got: {error}");
        agent.finish_prompt(StopReason::EndTurn);
    })
    .await;

    assert!(matches!(
        step,
        Ok(TurnStep::Settled(AgentRunOutcome::Completed))
    ));
    assert_eq!(agent.steers().len(), 1);
    // Leaves nothing behind for the caller's fallback delivery to duplicate.
    assert_eq!(writer.text_flow(), vec![]);
}

#[tokio::test(flavor = "current_thread")]
async fn steer_to_an_agent_without_the_method_is_an_error() {
    // Advertises steering but doesn't serve the method: the request itself
    // fails (method-not-found), which must reach the caller as an error
    // rather than hang the turn's select.
    let agent = Arc::new(FakeAgent {
        steer_response: None,
        advertise_steering: true,
        ..FakeAgent::default()
    });

    let (step, writer) = live_turn_with(&agent, true, |handle, agent, _writer| async move {
        agent.await_prompt().await;
        let error = handle
            .steer("hello?", &[])
            .await
            .expect_err("method-not-found is not a delivery");
        assert!(error.contains("Steering request failed"), "got: {error}");
        agent.finish_prompt(StopReason::EndTurn);
    })
    .await;

    assert!(matches!(
        step,
        Ok(TurnStep::Settled(AgentRunOutcome::Completed))
    ));
    assert_eq!(writer.text_flow(), vec![]);
}

#[tokio::test(flavor = "current_thread")]
async fn steer_is_refused_locally_when_the_agent_did_not_advertise_it() {
    let agent = Arc::new(FakeAgent {
        steer_response: Some(serde_json::json!({ "outcome": "injected" })),
        advertise_steering: false,
        ..FakeAgent::default()
    });

    let (step, _writer) = live_turn_with(&agent, false, |handle, agent, _writer| async move {
        agent.await_prompt().await;
        let error = handle
            .steer("hello?", &[])
            .await
            .expect_err("no capability, no steer");
        assert!(
            error.contains("does not advertise steering"),
            "got: {error}"
        );
        agent.finish_prompt(StopReason::EndTurn);
    })
    .await;

    assert!(matches!(
        step,
        Ok(TurnStep::Settled(AgentRunOutcome::Completed))
    ));
    assert!(
        agent.steers().is_empty(),
        "the gate must stop the frame before the wire"
    );
}

// =============================================================================
// Interrupt-and-reprompt on the same connection
// =============================================================================

/// Drive a whole `run_acp_session` (setup, prompt loop) against the fake
/// agent. `drive` runs alongside it once setup has completed; it receives the
/// handle, the agent, the writer and the handler. Returns the session's
/// result, the reply receiver of the first `prompt()`, and whatever reply
/// sender is still stashed when the loop returns — the one the connection
/// task would resolve after teardown.
#[allow(clippy::type_complexity)]
async fn session_with<F, Fut>(
    agent: &Arc<FakeAgent>,
    background_hold: Option<&BackgroundHoldConfig>,
    drive: F,
) -> (
    Result<(AgentRunOutcome, SessionSettleReason), String>,
    oneshot::Receiver<SessionTurnResult>,
    Option<oneshot::Sender<SessionTurnResult>>,
    Arc<TranscriptWriter>,
    Arc<AcpNotificationHandler>,
)
where
    F: FnOnce(
        TurnControlHandle,
        Arc<FakeAgent>,
        Arc<TranscriptWriter>,
        Arc<AcpNotificationHandler>,
    ) -> Fut,
    Fut: Future<Output = ()>,
{
    let writer = Arc::new(TranscriptWriter::default());
    let handler = Arc::new(transcript_handler(&writer));
    let store: Arc<dyn Store> = Arc::new(RecordingStore::default());
    let cancel = CancellationToken::new();
    let child_exited = CancellationToken::new();
    let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
    let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
    let (turn_control_tx, mut turn_control_rx) = mpsc::unbounded_channel();
    let hold_active = AtomicBool::new(false);
    let pending_reply = RefCell::new(None);
    let handle = turn_control_handle(&turn_control_tx, true);

    // The first turn, queued the way `SessionConnection::prompt` queues it.
    let (reply_tx, reply_rx) = oneshot::channel();
    prompt_tx
        .send(QueuedSessionTurn {
            prompt: "first".to_string(),
            images: vec![],
            reply: Some(reply_tx),
            control: None,
        })
        .expect("queue should accept");

    let permission_handler = Arc::clone(&handler);
    let notification_handler = Arc::clone(&handler);
    let result = agent_client_protocol::Client
        .builder()
        .on_receive_request(
            async move |args: RequestPermissionRequest, responder, _connection| {
                let response = permission_handler.request_permission(args).await?;
                responder.respond(response)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: IncomingSessionUpdate, _cx| {
                dispatch_session_update(&notification_handler, notification).await
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(agent.build(), async |connection| {
            let (result, ()) = tokio::join!(
                run_acp_session(
                    &connection,
                    Path::new("/tmp"),
                    &store,
                    "local-session",
                    None,
                    &[],
                    &handler,
                    &[],
                    "Fake",
                    &cancel,
                    None,
                    background_hold,
                    None,
                    false,
                    &child_exited,
                    &mut prompt_rx,
                    &mut hold_control_rx,
                    &mut turn_control_rx,
                    &hold_active,
                    &pending_reply,
                ),
                drive(
                    handle,
                    Arc::clone(agent),
                    Arc::clone(&writer),
                    Arc::clone(&handler)
                ),
            );
            Ok(result)
        })
        .await
        .expect("the connection itself stays healthy");
    let stashed_reply = pending_reply.borrow_mut().take();
    (result, reply_rx, stashed_reply, writer, handler)
}

#[tokio::test(flavor = "current_thread")]
async fn interrupt_cancels_the_turn_and_reprompts_on_the_same_connection() {
    let agent = Arc::new(FakeAgent {
        advertise_steering: false,
        stream_on_prompt: vec!["half an answer", "the new answer"],
        // Turn 2 asks permission: the connection-wide permission token must
        // have survived the interrupt for the handler to answer it.
        permission_on_prompt: Some(2),
        ..FakeAgent::default()
    });

    let (result, mut reply_rx, stashed_reply, writer, handler) =
        session_with(&agent, None, |handle, agent, writer, _handler| async move {
            agent.await_prompt().await;
            wait_until("the first chunk", || {
                writer
                    .text_flow()
                    .contains(&WriterCall::Append("half an answer".to_string()))
            })
            .await;

            // The agent's cancel handler answers the pending prompt
            // `cancelled`; the interrupt then sends the new message.
            handle
                .interrupt_and_reprompt("actually, do this instead", &[])
                .await
                .expect("the agent acknowledged the cancel");
            agent.await_prompt().await;
            assert_eq!(agent.prompts(), ["first", "actually, do this instead"]);
            assert_eq!(agent.cancels(), 1);

            tokio::time::timeout(SETTLE_TIMEOUT, agent.permission_answered.notified())
                .await
                .expect("turn 2's permission request should be answered");
            wait_until("the second chunk", || {
                writer
                    .text_flow()
                    .contains(&WriterCall::Append("the new answer".to_string()))
            })
            .await;
            agent.finish_prompt(StopReason::EndTurn);
        })
        .await;

    // Without a background hold the session settles right after the *final*
    // turn, as a completed turn — the interrupted one never produced an
    // outcome of its own.
    assert_eq!(
        result,
        Ok((AgentRunOutcome::Completed, SessionSettleReason::Immediate))
    );
    // The transcript: the interrupted assistant message is closed, the new
    // user message recorded, and only then does the next answer stream.
    assert_eq!(
        writer.text_flow(),
        vec![
            WriterCall::Append("half an answer".to_string()),
            WriterCall::Finalize,
            WriterCall::UserMessage("actually, do this instead".to_string()),
            WriterCall::Append("the new answer".to_string()),
        ]
    );
    // The first prompt()'s reply was never resolved by the interrupt: it is
    // still stashed for the connection task to answer at teardown with the
    // final turn's result — from the caller's side the session simply kept
    // running.
    assert!(matches!(
        reply_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let stashed_reply = stashed_reply.expect("the first turn's reply is still pending");
    stashed_reply
        .send(result.clone().map(|(outcome, _)| outcome))
        .expect("the prompt() caller is still waiting");
    assert_eq!(reply_rx.await, Ok(Ok(AgentRunOutcome::Completed)));
    // The interrupt did not burn the connection's permission token: turn 2's
    // request reached the writer (which auto-approves) instead of being
    // auto-answered `cancelled`.
    assert!(!handler.permission_cancel_token.is_cancelled());
    let outcomes = agent.permission_outcomes.lock().unwrap();
    assert_eq!(outcomes.len(), 1);
    assert!(
        matches!(outcomes[0], RequestPermissionOutcome::Selected(_)),
        "got: {:?}",
        outcomes[0]
    );
}

/// A control request queued next to an already-queued `prompt()` (both land
/// while setup is still running, say) does not jump it: the prompt is the
/// turn, and the request is served inside it — here an interrupt, which
/// cancels that turn and follows it. Seen against a real bridge, where the
/// unbiased select sometimes made the message the first turn and the
/// prompt the second.
#[tokio::test(flavor = "current_thread")]
async fn a_prompt_queued_before_a_control_request_goes_first() {
    let agent = Arc::new(FakeAgent {
        advertise_steering: false,
        ..FakeAgent::default()
    });

    let (result, _reply_rx, stashed_reply, writer, _handler) = session_with(
        &agent,
        None,
        |handle, agent, _writer, _handler| async move {
            // No await before this: it is queued while setup is in flight,
            // behind the prompt `session_with` queued before connecting.
            let outcome = handle.interrupt_and_reprompt("second", &[]).await;
            assert_eq!(outcome, Ok(()));
            wait_until("both prompts", || agent.prompts().len() == 2).await;
            assert_eq!(agent.prompts(), ["first", "second"]);
            assert_eq!(
                agent.cancels(),
                1,
                "the interrupt cancelled the queued prompt's turn"
            );
            agent.finish_prompt(StopReason::EndTurn);
        },
    )
    .await;

    assert_eq!(
        result,
        Ok((AgentRunOutcome::Completed, SessionSettleReason::Immediate))
    );
    assert_eq!(
        writer
            .calls()
            .iter()
            .filter(|call| matches!(call, WriterCall::UserMessage(_)))
            .count(),
        1
    );
    assert!(
        stashed_reply.is_some(),
        "the prompt() for the first turn is still pending"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn interrupt_during_a_hold_is_served_as_a_new_turn_without_a_cancel() {
    let agent = Arc::new(FakeAgent {
        advertise_steering: true,
        steer_response: Some(serde_json::json!({ "outcome": "injected" })),
        ..FakeAgent::default()
    });

    let (result, _reply_rx, _stashed_reply, writer, _handler) = session_with(
        &agent,
        Some(&QUIESCENCE_PROBE),
        |handle, agent, writer, handler| async move {
            agent.await_prompt().await;
            // A live background task keeps the hold open after turn 1.
            feed_sdk_frame(&handler, task_started("task-1")).await;
            agent.finish_prompt(StopReason::EndTurn);
            wait_until("the hold", || holding(&handler)).await;

            // Nothing is in flight to steer: the message becomes a turn of
            // its own, with no `_session/steering` frame on the wire.
            let outcome = handle.steer("while you wait", &[]).await;
            assert_eq!(outcome, Ok(SteerOutcome::NewTurnStarted));
            agent.await_prompt().await;
            assert_eq!(agent.prompts(), ["first", "while you wait"]);
            assert!(agent.steers().is_empty());
            assert_eq!(
                writer.text_flow(),
                vec![
                    WriterCall::Finalize,
                    WriterCall::UserMessage("while you wait".to_string())
                ]
            );
            agent.finish_prompt(StopReason::EndTurn);

            // And an interrupt with nothing to interrupt is the same.
            wait_until("the second hold", || holding(&handler)).await;
            handle
                .interrupt_and_reprompt("and this", &[])
                .await
                .expect("nothing to cancel, served as a turn");
            agent.await_prompt().await;
            assert_eq!(agent.cancels(), 0);
            assert_eq!(agent.prompts().len(), 3);
            agent.finish_prompt(StopReason::EndTurn);

            // Drain the task set so the hold settles on its own.
            wait_until("the third hold", || holding(&handler)).await;
            feed_sdk_frame(&handler, empty_tasks_frame()).await;
        },
    )
    .await;

    assert_eq!(
        result,
        Ok((AgentRunOutcome::Completed, SessionSettleReason::Quiescent))
    );
    assert_eq!(
        writer
            .calls()
            .iter()
            .filter(|call| matches!(call, WriterCall::UserMessage(_)))
            .count(),
        2
    );
}

#[tokio::test]
async fn holding_wait_turns_a_steer_into_a_new_turn_without_rejecting_it_at_boundaries() {
    let handler = hold_test_handler();
    let (_prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
    let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
    let (turn_control_tx, mut turn_control_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let child_exited = CancellationToken::new();
    let hold_active = AtomicBool::new(false);
    let handle = turn_control_handle(&turn_control_tx, true);

    feed_sdk_frame(&handler, task_started("task-1")).await;

    // Queued before the hold exists. Unlike a per-task stop, a message is not
    // aimed at one hold: the entry boundary must not reject it.
    let (early_reply_tx, early_reply_rx) = oneshot::channel();
    turn_control_tx
        .send(TurnControlRequest::Steer {
            prompt: "queued early".to_string(),
            images: vec![],
            reply: early_reply_tx,
        })
        .expect("queue should accept");

    let outcome = tokio::time::timeout(
        SETTLE_TIMEOUT,
        hold_for_background_quiescence(
            &QUIESCENCE_PROBE,
            TaskTrackingMode::Raw,
            &handler,
            &cancel,
            &child_exited,
            &mut prompt_rx,
            &mut hold_control_rx,
            &mut turn_control_rx,
            &hold_active,
            &|_| {},
            "sess-1",
            None,
        ),
    )
    .await
    .expect("a queued message ends the hold at once");
    let HoldOutcome::NewTurn(turn) = outcome else {
        panic!("expected NewTurn, got {outcome:?}");
    };
    assert_eq!(turn.prompt, "queued early");
    assert!(turn.reply.is_none(), "no prompt() is waiting on this turn");
    // Answered by the session loop once the turn goes out, not by the hold.
    assert!(matches!(turn.control, Some(TurnControlReply::Steer(_))));
    let mut early_reply_rx = early_reply_rx;
    assert!(matches!(
        early_reply_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    turn.control.expect("control reply").turn_started();
    assert_eq!(
        early_reply_rx.await.expect("answered"),
        Ok(SteerOutcome::NewTurnStarted)
    );

    // A handle-issued interrupt during the hold reads the same way.
    let served = async {
        let outcome = hold_for_background_quiescence(
            &QUIESCENCE_PROBE,
            TaskTrackingMode::Raw,
            &handler,
            &cancel,
            &child_exited,
            &mut prompt_rx,
            &mut hold_control_rx,
            &mut turn_control_rx,
            &hold_active,
            &|_| {},
            "sess-1",
            None,
        )
        .await;
        let HoldOutcome::NewTurn(turn) = outcome else {
            panic!("expected NewTurn, got {outcome:?}");
        };
        turn.control.expect("control reply").turn_started();
        turn.prompt
    };
    let (prompt, answer) = tokio::join!(served, handle.interrupt_and_reprompt("now", &[]));
    assert_eq!(prompt, "now");
    assert_eq!(answer, Ok(()));
}

// =============================================================================
// Teardown and handle failure mapping
// =============================================================================

#[tokio::test]
async fn teardown_answers_every_queued_message_and_the_handle_fails_fast_after() {
    let (turn_control_tx, mut turn_control_rx) = mpsc::unbounded_channel();
    let alive = Arc::new(AtomicBool::new(true));
    let handle = TurnControlHandle {
        turn_control_tx: turn_control_tx.clone(),
        connection_alive: Arc::clone(&alive),
    };

    let (steer_reply_tx, steer_reply_rx) = oneshot::channel();
    let (interrupt_reply_tx, interrupt_reply_rx) = oneshot::channel();
    turn_control_tx
        .send(TurnControlRequest::Steer {
            prompt: "a".to_string(),
            images: vec![],
            reply: steer_reply_tx,
        })
        .expect("queue should accept");
    turn_control_tx
        .send(TurnControlRequest::Interrupt {
            prompt: "b".to_string(),
            images: vec![],
            reply: interrupt_reply_tx,
        })
        .expect("queue should accept");

    // The connection task's teardown sequence.
    alive.store(false, Ordering::Relaxed);
    reject_queued_turn_control_requests(&mut turn_control_rx, "the ACP connection closed");

    let steer = steer_reply_rx
        .await
        .expect("every queued request is answered");
    assert!(
        steer
            .as_ref()
            .is_err_and(|e| e.contains("connection closed")),
        "got: {steer:?}"
    );
    let interrupt = interrupt_reply_rx
        .await
        .expect("every queued request is answered");
    assert!(
        interrupt
            .as_ref()
            .is_err_and(|e| e.contains("connection closed")),
        "got: {interrupt:?}"
    );
    assert!(
        turn_control_rx.try_recv().is_err(),
        "the queue must be drained"
    );

    // After settle the handle refuses without queueing.
    let error = handle
        .steer("c", &[])
        .await
        .expect_err("connection is gone");
    assert!(error.contains("no longer running"), "got: {error}");
    let error = handle
        .interrupt_and_reprompt("d", &[])
        .await
        .expect_err("connection is gone");
    assert!(error.contains("no longer running"), "got: {error}");
    assert!(turn_control_rx.try_recv().is_err());
}

#[tokio::test]
async fn a_turn_control_handle_maps_channel_failures_to_connection_state() {
    let (turn_control_tx, mut turn_control_rx) = mpsc::unbounded_channel();
    let handle = turn_control_handle(&turn_control_tx, true);

    // A reply dropped unanswered (teardown discarded the scheduled callback)
    // reads as the connection closing mid-request.
    let drop_unanswered = async {
        let request = turn_control_rx.recv().await.expect("request should arrive");
        drop(request);
    };
    let (steered, ()) = tokio::join!(handle.steer("a", &[]), drop_unanswered);
    let error = steered.expect_err("a dropped reply is not an answer");
    assert!(error.contains("closed before"), "got: {error}");

    // With the receiver gone the connection itself is gone, whatever the
    // alive flag still says.
    drop(turn_control_rx);
    let error = handle
        .interrupt_and_reprompt("b", &[])
        .await
        .expect_err("no connection to serve the message");
    assert!(error.contains("no longer running"), "got: {error}");
}
