//! Bounded recovery for Claude's `Couldn't confirm model` failure while the
//! saved model selection is applied before the first prompt.
//!
//! The scenarios drive `run_acp_session` end to end against a scripted
//! in-process agent, so the setup timeout, the cancellation boundary, and
//! the prompt queue under test are the production ones. Time is paused: the
//! one-second retry delay and the 90-second setup deadline elapse instantly
//! once nothing else can make progress.

use super::*;
use crate::driver::{
    describe_protocol_failure, is_model_confirmation_error, run_acp_session,
    supports_model_confirmation_recovery, ACP_SETUP_TIMEOUT, MODEL_CONFIRMATION_EXHAUSTED_MESSAGE,
};
use agent_client_protocol::schema::v1::{
    Implementation, InitializeRequest, InitializeResponse, PromptResponse,
};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

const CLAUDE_ADAPTER: &str = "@agentclientprotocol/claude-agent-acp";
const FABLE: &str = "claude-fable-5-1[1m]";
const SONNET: &str = "claude-sonnet-5-5";

/// The exact shape the installed Claude bridge returns: the SDK's sentence
/// under `details` of a plain internal error.
fn confirmation_error(model: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::internal_error().data(serde_json::json!({
        "details": format!(
            "Couldn't confirm model \"{model}\" with the API. Try again, or run /model to see available models."
        ),
    }))
}

fn model_option(current: &str) -> SessionConfigOption {
    SessionConfigOption::select(
        "model",
        "Model",
        current.to_string(),
        vec![
            SessionConfigSelectOption::new(SONNET, "Sonnet"),
            SessionConfigSelectOption::new(FABLE, "Fable"),
        ],
    )
    .category(SessionConfigOptionCategory::Model)
}

fn effort_option(current: &str, values: &[&str]) -> SessionConfigOption {
    SessionConfigOption::select(
        "reasoning",
        "Reasoning",
        current.to_string(),
        values
            .iter()
            .map(|value| SessionConfigSelectOption::new(value.to_string(), value.to_string()))
            .collect::<Vec<_>>(),
    )
    .category(SessionConfigOptionCategory::ThoughtLevel)
}

fn model_selection(value_id: &str) -> AcpSessionConfigOptionSelection {
    AcpSessionConfigOptionSelection {
        category: SessionConfigOptionCategory::Model,
        config_id: "model".to_string(),
        value_id: value_id.to_string(),
    }
}

fn effort_selection(value_id: &str) -> AcpSessionConfigOptionSelection {
    AcpSessionConfigOptionSelection {
        category: SessionConfigOptionCategory::ThoughtLevel,
        config_id: "reasoning".to_string(),
        value_id: value_id.to_string(),
    }
}

fn call(config_id: &str, value_id: &str) -> (String, String) {
    (config_id.to_string(), value_id.to_string())
}

/// What the scripted agent does with the next `session/set_config_option`.
enum ConfigReply {
    Accept(Vec<SessionConfigOption>),
    Fail(agent_client_protocol::Error),
    /// Never answer; the request stays in flight until the connection drops.
    Hang,
}

#[derive(Default)]
struct AgentLog {
    config_calls: Mutex<Vec<(String, String)>>,
    prompts: AtomicUsize,
}

impl AgentLog {
    fn config_calls(&self) -> Vec<(String, String)> {
        self.config_calls.lock().unwrap().clone()
    }

    fn prompts(&self) -> usize {
        self.prompts.load(Ordering::SeqCst)
    }
}

struct Script {
    adapter: &'static str,
    initial_options: Vec<SessionConfigOption>,
    replies: Vec<ConfigReply>,
    /// Cancel this token when the n-th (1-based) config request arrives,
    /// before its scripted reply — a user Stop racing the recovery.
    cancel_on_config_call: Option<(usize, CancellationToken)>,
}

impl Script {
    fn claude(initial_options: Vec<SessionConfigOption>, replies: Vec<ConfigReply>) -> Self {
        Self {
            adapter: CLAUDE_ADAPTER,
            initial_options,
            replies,
            cancel_on_config_call: None,
        }
    }
}

struct Run {
    outcome: Result<(AgentRunOutcome, SessionSettleReason), String>,
    agent: Arc<AgentLog>,
    writer: Arc<RecordingMessageWriter>,
}

/// One queued prompt through the real `run_acp_session` against `script`.
async fn run_session(
    script: Script,
    selections: Vec<AcpSessionConfigOptionSelection>,
    cancel: CancellationToken,
) -> Run {
    let log = Arc::new(AgentLog::default());
    let log_for_config = Arc::clone(&log);
    let log_for_prompt = Arc::clone(&log);
    let adapter = script.adapter;
    let initial_options = script.initial_options;
    let replies = Arc::new(Mutex::new(VecDeque::from(script.replies)));
    let cancel_on_config_call = script.cancel_on_config_call;
    let agent = agent_client_protocol::Agent
        .builder()
        .on_receive_request(
            async move |request: InitializeRequest, responder, _cx| {
                responder.respond(
                    InitializeResponse::new(request.protocol_version)
                        .agent_info(Implementation::new(adapter, "0.0.0-test")),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: NewSessionRequest, responder, _cx| {
                responder.respond(
                    NewSessionResponse::new("session-1").config_options(initial_options.clone()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionConfigOptionRequest, responder, _cx| {
                let call_index = {
                    let mut calls = log_for_config.config_calls.lock().unwrap();
                    calls.push((
                        request.config_id.to_string(),
                        request
                            .value
                            .as_value_id()
                            .expect("selected config value should be a value ID")
                            .to_string(),
                    ));
                    calls.len()
                };
                if let Some((at, token)) = &cancel_on_config_call {
                    if *at == call_index {
                        token.cancel();
                    }
                }
                let reply = replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("more config requests than scripted replies");
                match reply {
                    ConfigReply::Accept(options) => {
                        responder.respond(SetSessionConfigOptionResponse::new(options))
                    }
                    ConfigReply::Fail(error) => responder.respond_with_error(error),
                    ConfigReply::Hang => {
                        std::future::pending::<()>().await;
                        unreachable!("a hung request never resolves")
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: PromptRequest, responder, _cx| {
                log_for_prompt.prompts.fetch_add(1, Ordering::SeqCst);
                responder.respond(PromptResponse::new(StopReason::EndTurn))
            },
            agent_client_protocol::on_receive_request!(),
        );

    let writer = Arc::new(RecordingMessageWriter::default());
    let message_writer: Arc<dyn MessageWriter> = writer.clone();
    let handler = Arc::new(AcpNotificationHandler::new(
        message_writer,
        false,
        vec![],
        cancel.clone(),
    ));
    let store: Arc<dyn Store> = Arc::new(RecordingStore::default());
    let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel();
    let (_hold_control_tx, mut hold_control_rx) = mpsc::unbounded_channel();
    let hold_active = AtomicBool::new(false);
    let pending_reply = RefCell::new(None);
    let child_exited = CancellationToken::new();
    let (reply_tx, _reply_rx) = oneshot::channel();
    prompt_tx
        .send(QueuedSessionTurn {
            prompt: "hello".to_string(),
            images: vec![],
            reply: reply_tx,
        })
        .expect("queued turn should send");

    let outcome = agent_client_protocol::Client
        .connect_with(agent, async |connection| {
            Ok(run_acp_session(
                &connection,
                Path::new("/tmp"),
                &store,
                "local-session",
                None,
                &selections,
                &handler,
                &[],
                "Claude Code",
                &cancel,
                None,
                None,
                None,
                &child_exited,
                &mut prompt_rx,
                &mut hold_control_rx,
                &hold_active,
                &pending_reply,
            )
            .await)
        })
        .await
        .expect("the connection itself stays healthy");

    Run {
        outcome,
        agent: log,
        writer,
    }
}

#[test]
fn matcher_accepts_only_claudes_confirmation_failure_for_the_requested_model() {
    assert!(is_model_confirmation_error(
        &confirmation_error(FABLE),
        FABLE
    ));
    // A bridge that flattens `details` to a bare string still matches.
    assert!(is_model_confirmation_error(
        &agent_client_protocol::Error::internal_error()
            .data(format!("Couldn't confirm model \"{FABLE}\" with the API.")),
        FABLE,
    ));

    // A different model, another error code with the same words, an internal
    // error with other details, and one with no data at all are not it.
    assert!(!is_model_confirmation_error(
        &confirmation_error("claude-opus-5-5"),
        FABLE
    ));
    let mut auth_with_details = agent_client_protocol::Error::auth_required();
    auth_with_details.data = confirmation_error(FABLE).data;
    assert!(!is_model_confirmation_error(&auth_with_details, FABLE));
    assert!(!is_model_confirmation_error(
        &agent_client_protocol::Error::internal_error()
            .data(serde_json::json!({ "details": format!("Invalid value for config option model: {FABLE}") })),
        FABLE,
    ));
    assert!(!is_model_confirmation_error(
        &agent_client_protocol::Error::internal_error(),
        FABLE
    ));
}

#[test]
fn recovery_is_keyed_to_claude_bridges_only() {
    assert!(supports_model_confirmation_recovery(CLAUDE_ADAPTER));
    assert!(supports_model_confirmation_recovery(
        "@zed-industries/claude-code-acp"
    ));
    assert!(supports_model_confirmation_recovery("Claude Code"));
    assert!(!supports_model_confirmation_recovery("@openai/codex-acp"));
    assert!(!supports_model_confirmation_recovery("Goose"));
    assert!(!supports_model_confirmation_recovery("Blox"));
}

/// The terminal sentence must keep the saved selection — the runner clears a
/// selection only when `is_config_selection_unavailable_error` fires — and it
/// must reach the session alert as the sentence itself: `run` unwraps the
/// `internal_error` the connection closure wrapped it in, while every other
/// setup failure keeps the Debug shape the auth-recovery fixtures pin.
#[test]
fn exhausted_message_is_not_an_unavailable_selection() {
    assert!(!is_config_selection_unavailable_error(
        MODEL_CONFIRMATION_EXHAUSTED_MESSAGE
    ));
    // Exactly as the `connect_with` closure wraps a setup failure before
    // `run` describes it for the session alert.
    let shown = describe_protocol_failure(agent_client_protocol::util::internal_error(
        MODEL_CONFIRMATION_EXHAUSTED_MESSAGE,
    ));
    assert_eq!(shown, MODEL_CONFIRMATION_EXHAUSTED_MESSAGE);
    assert!(!is_config_selection_unavailable_error(&shown));

    // Any other setup failure, and a bare transport error, keep the shape
    // `isAuthenticationError` and the driver's auth tests are keyed to.
    let auth = agent_client_protocol::util::internal_error(
        "ACP authentication is required to create ACP session. Sign this agent in, then retry.",
    );
    assert_eq!(
        describe_protocol_failure(auth.clone()),
        format!("ACP protocol failed: {auth:?}")
    );
    let transport = agent_client_protocol::Error::internal_error();
    assert_eq!(
        describe_protocol_failure(transport.clone()),
        format!("ACP protocol failed: {transport:?}")
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_failed_confirmation_is_retried_once_then_effort_applies_and_one_prompt_runs() {
    let after_model = vec![
        model_option(FABLE),
        effort_option("medium", &["low", "medium", "high"]),
    ];
    let after_effort = vec![
        model_option(FABLE),
        effort_option("high", &["low", "medium", "high"]),
    ];
    let run = run_session(
        Script::claude(
            vec![
                model_option(SONNET),
                effort_option("medium", &["low", "medium", "high"]),
            ],
            vec![
                ConfigReply::Fail(confirmation_error(FABLE)),
                ConfigReply::Accept(after_model),
                ConfigReply::Accept(after_effort.clone()),
            ],
        ),
        vec![model_selection(FABLE), effort_selection("high")],
        CancellationToken::new(),
    )
    .await;

    assert_eq!(
        run.outcome
            .expect("recovered setup should complete the turn"),
        (AgentRunOutcome::Completed, SessionSettleReason::Immediate)
    );
    assert_eq!(
        run.agent.config_calls(),
        vec![
            call("model", FABLE),
            call("model", FABLE),
            call("reasoning", "high")
        ],
        "the identical model request goes out exactly twice, then effort"
    );
    assert_eq!(run.agent.prompts(), 1, "the queued prompt is released once");
    let updates = run.writer.config_option_updates.lock().unwrap();
    assert_eq!(updates.as_slice(), &[after_effort]);
    assert!(run
        .writer
        .config_option_fallbacks
        .lock()
        .unwrap()
        .is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn two_failed_confirmations_fail_setup_plainly_without_prompting() {
    let run = run_session(
        Script::claude(
            vec![model_option(SONNET)],
            vec![
                ConfigReply::Fail(confirmation_error(FABLE)),
                ConfigReply::Fail(confirmation_error(FABLE)),
            ],
        ),
        vec![model_selection(FABLE)],
        CancellationToken::new(),
    )
    .await;

    let error = run.outcome.expect_err("exhausted recovery fails the setup");
    assert_eq!(error, MODEL_CONFIRMATION_EXHAUSTED_MESSAGE);
    assert!(
        !is_config_selection_unavailable_error(&error),
        "the saved selection must survive a confirmation failure"
    );
    assert_eq!(
        run.agent.config_calls(),
        vec![call("model", FABLE), call("model", FABLE)]
    );
    assert_eq!(run.agent.prompts(), 0);
    assert!(run.writer.config_option_updates.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancelling_during_the_retry_delay_sends_nothing_further() {
    let cancel = CancellationToken::new();
    let run = run_session(
        Script {
            cancel_on_config_call: Some((1, cancel.clone())),
            ..Script::claude(
                vec![model_option(SONNET)],
                vec![ConfigReply::Fail(confirmation_error(FABLE))],
            )
        },
        vec![model_selection(FABLE)],
        cancel,
    )
    .await;

    assert_eq!(
        run.outcome.expect("a cancel is a clean outcome"),
        (AgentRunOutcome::Cancelled, SessionSettleReason::Cancelled)
    );
    assert_eq!(run.agent.config_calls(), vec![call("model", FABLE)]);
    assert_eq!(run.agent.prompts(), 0);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancelling_during_the_second_attempt_sends_no_prompt() {
    let cancel = CancellationToken::new();
    let run = run_session(
        Script {
            cancel_on_config_call: Some((2, cancel.clone())),
            ..Script::claude(
                vec![model_option(SONNET)],
                vec![
                    ConfigReply::Fail(confirmation_error(FABLE)),
                    ConfigReply::Hang,
                ],
            )
        },
        vec![model_selection(FABLE)],
        cancel,
    )
    .await;

    assert_eq!(
        run.outcome.expect("a cancel is a clean outcome"),
        (AgentRunOutcome::Cancelled, SessionSettleReason::Cancelled)
    );
    assert_eq!(
        run.agent.config_calls(),
        vec![call("model", FABLE), call("model", FABLE)]
    );
    assert_eq!(run.agent.prompts(), 0);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_setup_deadline_still_bounds_a_hung_second_attempt() {
    let run = run_session(
        Script::claude(
            vec![model_option(SONNET)],
            vec![
                ConfigReply::Fail(confirmation_error(FABLE)),
                ConfigReply::Hang,
            ],
        ),
        vec![model_selection(FABLE)],
        CancellationToken::new(),
    )
    .await;

    let error = run.outcome.expect_err("the setup deadline fails the run");
    assert_eq!(
        error,
        format!(
            "Timed out waiting for ACP protocol startup after {}s",
            ACP_SETUP_TIMEOUT.as_secs()
        )
    );
    assert_eq!(
        run.agent.config_calls(),
        vec![call("model", FABLE), call("model", FABLE)]
    );
    assert_eq!(run.agent.prompts(), 0, "no late prompt after the deadline");
}

/// The retry carries the *resolved* value — here the hint-stripped rename a
/// resumed pin falls back to — and the fallback is announced once, only
/// after the agent has accepted it.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_retried_fallback_value_is_announced_once_after_acceptance() {
    let renamed = || {
        vec![SessionConfigOption::select(
            "model",
            "Model",
            "claude-fable-5-1",
            vec![
                SessionConfigSelectOption::new("claude-fable-5-1", "Fable"),
                SessionConfigSelectOption::new("claude-opus-5[1m]", "Opus"),
            ],
        )
        .category(SessionConfigOptionCategory::Model)]
    };
    let run = run_session(
        Script::claude(
            renamed(),
            vec![
                ConfigReply::Fail(confirmation_error("claude-fable-5-1")),
                ConfigReply::Accept(renamed()),
            ],
        ),
        vec![model_selection("claude-fable-5[1m]")],
        CancellationToken::new(),
    )
    .await;

    assert!(run.outcome.is_ok());
    assert_eq!(
        run.agent.config_calls(),
        vec![
            call("model", "claude-fable-5-1"),
            call("model", "claude-fable-5-1")
        ]
    );
    assert_eq!(run.agent.prompts(), 1);
    assert_eq!(
        run.writer
            .config_option_fallbacks
            .lock()
            .unwrap()
            .as_slice(),
        &[String::from(
            "Selected model claude-fable-5[1m] was not available, falling back to \
             claude-fable-5-1 (Fable)."
        )]
    );
}

/// A model accepted on the retry can still narrow the effort options; the
/// existing stale-effort skip applies to the options the retry returned.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn effort_options_changed_by_the_retried_model_keep_the_stale_effort_skip() {
    let after_model = vec![
        model_option(FABLE),
        effort_option("medium", &["low", "medium"]),
    ];
    let run = run_session(
        Script::claude(
            vec![
                model_option(SONNET),
                effort_option("medium", &["low", "medium", "high"]),
            ],
            vec![
                ConfigReply::Fail(confirmation_error(FABLE)),
                ConfigReply::Accept(after_model.clone()),
            ],
        ),
        vec![model_selection(FABLE), effort_selection("high")],
        CancellationToken::new(),
    )
    .await;

    assert!(run.outcome.is_ok());
    assert_eq!(
        run.agent.config_calls(),
        vec![call("model", FABLE), call("model", FABLE)],
        "the now-unavailable effort is skipped, not sent"
    );
    assert_eq!(run.agent.prompts(), 1);
    assert_eq!(
        run.writer.config_option_updates.lock().unwrap().as_slice(),
        &[after_model]
    );
}

/// The bridge re-applies an already-current model on purpose (to repair SDK
/// drift), so an advertised match must not short-circuit `setModel`.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_already_current_model_is_still_applied() {
    let run = run_session(
        Script::claude(
            vec![model_option(FABLE)],
            vec![ConfigReply::Accept(vec![model_option(FABLE)])],
        ),
        vec![model_selection(FABLE)],
        CancellationToken::new(),
    )
    .await;

    assert!(run.outcome.is_ok());
    assert_eq!(run.agent.config_calls(), vec![call("model", FABLE)]);
    assert_eq!(run.agent.prompts(), 1);
}

/// Everything that is not Claude failing to confirm the selected model keeps
/// the single attempt and the existing Debug-shaped error.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn other_failures_and_other_adapters_keep_the_single_attempt() {
    struct Case {
        name: &'static str,
        adapter: &'static str,
        selection: AcpSessionConfigOptionSelection,
        error: agent_client_protocol::Error,
        expected_call: (String, String),
        expected_prefix: &'static str,
    }
    let cases = vec![
        Case {
            name: "authentication required",
            adapter: CLAUDE_ADAPTER,
            selection: model_selection(FABLE),
            error: agent_client_protocol::Error::auth_required(),
            expected_call: call("model", FABLE),
            expected_prefix: "Failed to set selected ACP model config option: Error { code: -32000",
        },
        Case {
            name: "unrelated internal error",
            adapter: CLAUDE_ADAPTER,
            selection: model_selection(FABLE),
            error: agent_client_protocol::Error::internal_error(),
            expected_call: call("model", FABLE),
            expected_prefix: "Failed to set selected ACP model config option: Error { code: -32603",
        },
        Case {
            name: "confirmation failure for a different model",
            adapter: CLAUDE_ADAPTER,
            selection: model_selection(FABLE),
            error: confirmation_error("claude-opus-5-5"),
            expected_call: call("model", FABLE),
            expected_prefix: "Failed to set selected ACP model config option: Error { code: -32603",
        },
        Case {
            name: "the same words from another adapter",
            adapter: "@openai/codex-acp",
            selection: model_selection(FABLE),
            error: confirmation_error(FABLE),
            expected_call: call("model", FABLE),
            expected_prefix: "Failed to set selected ACP model config option: Error { code: -32603",
        },
        Case {
            name: "the same words on the effort option",
            adapter: CLAUDE_ADAPTER,
            selection: effort_selection("high"),
            error: confirmation_error("high"),
            expected_call: call("reasoning", "high"),
            expected_prefix:
                "Failed to set selected ACP effort config option: Error { code: -32603",
        },
    ];

    for case in cases {
        let run = run_session(
            Script {
                adapter: case.adapter,
                ..Script::claude(
                    vec![
                        model_option(SONNET),
                        effort_option("medium", &["low", "medium", "high"]),
                    ],
                    vec![ConfigReply::Fail(case.error)],
                )
            },
            vec![case.selection],
            CancellationToken::new(),
        )
        .await;

        let error = run
            .outcome
            .expect_err(&format!("{}: should fail the setup", case.name));
        assert!(
            error.starts_with(case.expected_prefix),
            "{}: unexpected error {error:?}",
            case.name
        );
        assert_eq!(
            run.agent.config_calls(),
            vec![case.expected_call],
            "{}: exactly one attempt",
            case.name
        );
        assert_eq!(run.agent.prompts(), 0, "{}: no prompt", case.name);
    }
}

/// A stale pin is still reported as unavailable before any request goes out,
/// so the runner keeps clearing it exactly as before.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_unavailable_selection_is_still_reported_without_a_request() {
    let run = run_session(
        Script::claude(vec![model_option(SONNET)], vec![]),
        vec![model_selection("claude-haiku-4")],
        CancellationToken::new(),
    )
    .await;

    let error = run.outcome.expect_err("a stale pin fails the setup");
    assert!(is_config_selection_unavailable_error(&error), "{error}");
    assert!(run.agent.config_calls().is_empty());
    assert_eq!(run.agent.prompts(), 0);
}
