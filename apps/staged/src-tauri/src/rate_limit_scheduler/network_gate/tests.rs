use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::super::{
    admit_for_resume, auto_recovery_from_metadata, claim_for_auto_resume, network_rows_waiting,
    pause_for_rate_limit, reschedule_after_start_failure, AutoResumeClaim, PauseAccounting,
    RateLimitAutoRecovery, RateLimitScheduler, MAX_AUTO_RETRY_ATTEMPTS, MAX_RESUMES_PER_TICK,
    NETWORK_PAUSE_KIND,
};
use super::*;

mod triggers;
use crate::store::{
    AcpMessageMetadata, CompletionReason, MessageRole, Session, SessionStatus, Store,
};

const HOUR_MS: i64 = 60 * 60 * 1_000;

/// The store stamps rows with the wall clock, and the scheduler lists paused
/// rows by that stamp. A short sleep keeps two writes in order.
fn step() {
    std::thread::sleep(Duration::from_millis(2));
}

fn running_session(store: &Store) -> Session {
    let session = Session::new_running("work", Path::new("/tmp")).with_agent("agent-1");
    store.create_session(&session).unwrap();
    session
}

/// Park a running session the way the runner parks a network failure.
fn fail_on_network(store: &Store, scheduler: &RateLimitScheduler, session_id: &str, now: i64) {
    step();
    assert!(pause_for_rate_limit(
        store,
        Some(scheduler),
        session_id,
        Some("Can't reach the API server"),
        None,
        Some(NETWORK_PAUSE_KIND),
        now,
    )
    .unwrap());
}

fn network_paused_sessions(
    store: &Store,
    scheduler: &RateLimitScheduler,
    count: usize,
    now: i64,
) -> Vec<String> {
    (0..count)
        .map(|_| {
            let session = running_session(store);
            fail_on_network(store, scheduler, &session.id, now);
            session.id
        })
        .collect()
}

/// The synchronous part of `run_tick` without the wake and network-change
/// triggers: settle the probe, then admit and claim rows oldest pause first.
/// Returns the sessions it started.
///
/// The tests in this file jump the clock freely to exercise the gate's own
/// rules; `triggers.rs` drives [`tick_with_triggers`] on a real cadence.
fn tick(scheduler: &RateLimitScheduler, store: &Arc<Store>, now: i64) -> Vec<String> {
    scheduler.settle_network_probe(store, now);
    admit_and_claim(scheduler, store, now)
}

/// The whole synchronous part of `run_tick`, triggers included.
fn tick_with_triggers(scheduler: &RateLimitScheduler, store: &Arc<Store>, now: i64) -> Vec<String> {
    let sessions = store.get_rate_limited_paused_sessions().unwrap();
    scheduler.settle_network_probe(store, now);
    scheduler.observe_network_tick(now, network_rows_waiting(&sessions));
    admit_and_claim(scheduler, store, now)
}

fn admit_and_claim(scheduler: &RateLimitScheduler, store: &Arc<Store>, now: i64) -> Vec<String> {
    let mut started = Vec::new();
    for session in store.get_rate_limited_paused_sessions().unwrap() {
        if started.len() >= MAX_RESUMES_PER_TICK {
            break;
        }
        let Some(admission) = admit_for_resume(scheduler, store, &session, now) else {
            continue;
        };
        if let AutoResumeClaim::Claimed(_) = claim_for_auto_resume(store, &session.id).unwrap() {
            if admission.probe {
                scheduler.network_probe_started(&session.id, now);
            }
            started.push(session.id);
        }
    }
    started
}

fn recovery(store: &Store, session_id: &str) -> RateLimitAutoRecovery {
    let session = store.get_session(session_id).unwrap().unwrap();
    auto_recovery_from_metadata(session.provider_metadata.as_ref()).unwrap()
}

/// The agent's turn reaches its provider: the model asks for a tool, and
/// the tool answers.
fn agent_replies(store: &Store, session_id: &str) {
    for (role, content) in [
        (MessageRole::ToolCall, "Read"),
        (MessageRole::ToolResult, "contents"),
    ] {
        store
            .add_session_message(session_id, role, content)
            .unwrap();
    }
}

const OFFLINE_ERROR: &str =
    "API Error: Can't reach the API server \u{2014} check your internet or DNS (ENOTFOUND)";

fn acp_event(store: &Store, session_id: &str, kind: &str, content: serde_json::Value) {
    store
        .add_acp_metadata_message(
            session_id,
            &AcpMessageMetadata {
                acp_event_kind: Some(kind.to_string()),
                acp_content: Some(content),
                ..Default::default()
            },
        )
        .unwrap();
}

/// The rows a Claude turn records when it cannot reach the provider, as
/// stored by real failed turns: the CLI's error as a message chunk, the
/// SDK's result marked as an error (its `subtype` still reads `success`), a
/// usage update, the prompt error, and the error text as the visible
/// assistant message.
fn claude_turn_fails_offline(store: &Store, session_id: &str) {
    acp_event(
        store,
        session_id,
        "agent_message_chunk",
        serde_json::json!({
            "content": { "type": "text", "text": OFFLINE_ERROR },
            "messageId": "a6922e42-33a1-4c9b-95d6-45c748f7e168",
        }),
    );
    acp_event(
        store,
        session_id,
        "response_result",
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": true,
            "terminal_reason": "api_error",
            "api_error": "connection_failed",
            "api_error_status": null,
            "stop_reason": "stop_sequence",
            "num_turns": 32,
            "result": OFFLINE_ERROR,
        }),
    );
    acp_event(
        store,
        session_id,
        "usage_update",
        serde_json::json!({ "used": 175_506, "size": 1_000_000 }),
    );
    acp_event(
        store,
        session_id,
        "prompt_error",
        serde_json::json!({
            "code": -32603,
            "message": format!("Internal error: {OFFLINE_ERROR}"),
            "data": { "errorKind": "server_error" },
        }),
    );
    store
        .add_session_message(session_id, MessageRole::Assistant, OFFLINE_ERROR)
        .unwrap();
}

/// A gate opened by `sessions[0]` probing and getting an answer.
fn open_gate(scheduler: &RateLimitScheduler, store: &Arc<Store>, sessions: &[String], now: i64) {
    assert_eq!(tick(scheduler, store, now), [sessions[0].clone()]);
    agent_replies(store, &sessions[0]);
    scheduler.settle_network_probe(store, now);
    assert_eq!(scheduler.network_turn(now), NetworkTurn::Resume);
}

#[test]
fn a_fresh_scheduler_starts_closed_with_an_immediate_probe() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    assert_eq!(scheduler.network_turn(0), NetworkTurn::Probe);

    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 1, now);

    // The first failure does not push the first probe out.
    assert_eq!(tick(&scheduler, &store, now), sessions);
    assert_eq!(scheduler.network_turn(now), NetworkTurn::Hold);
}

#[test]
fn only_one_probe_starts_for_eleven_paused_rows() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 11, now);

    assert_eq!(tick(&scheduler, &store, now), [sessions[0].clone()]);
    // While the probe runs, later ticks start nothing.
    assert!(tick(&scheduler, &store, now + 15_000).is_empty());
    assert!(tick(&scheduler, &store, now + PROBE_TIMEOUT_MS - 1).is_empty());
    assert_eq!(store.get_rate_limited_paused_sessions().unwrap().len(), 10);
}

#[test]
fn a_failed_probe_spends_no_attempt_and_rotates_to_the_next_row() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 3, now);

    assert_eq!(tick(&scheduler, &store, now), [sessions[0].clone()]);
    let failed_at = now + 1_000;
    fail_on_network(&store, &scheduler, &sessions[0], failed_at);

    let state = recovery(&store, &sessions[0]);
    assert_eq!(state.attempt, 0);
    assert_eq!(state.gave_up_at, None);
    assert_eq!(state.next_retry_at, Some(failed_at));
    assert_eq!(state.reason.as_deref(), Some("network_wait"));

    // The next probe waits out the delay, then goes to the row that has been
    // paused longest, which is no longer the one that just failed.
    assert!(tick(&scheduler, &store, failed_at + BASE_PROBE_DELAY_MS - 1).is_empty());
    let second_at = failed_at + BASE_PROBE_DELAY_MS;
    assert_eq!(tick(&scheduler, &store, second_at), [sessions[1].clone()]);

    // A second failed probe doubles the delay.
    fail_on_network(&store, &scheduler, &sessions[1], second_at);
    assert!(tick(&scheduler, &store, second_at + 2 * BASE_PROBE_DELAY_MS - 1).is_empty());
    assert_eq!(
        tick(&scheduler, &store, second_at + 2 * BASE_PROBE_DELAY_MS),
        [sessions[2].clone()]
    );
}

#[test]
fn a_probe_with_agent_output_opens_the_gate_and_the_rest_resume_two_per_tick() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    // Output from before the claim says nothing about the network now.
    let first = running_session(&store);
    agent_replies(&store, &first.id);
    step();
    let now = crate::store::now_timestamp();
    fail_on_network(&store, &scheduler, &first.id, now);
    let rest = network_paused_sessions(&store, &scheduler, 4, now);

    assert_eq!(
        tick(&scheduler, &store, now),
        std::slice::from_ref(&first.id)
    );
    assert!(tick(&scheduler, &store, now).is_empty());

    agent_replies(&store, &first.id);
    assert_eq!(tick(&scheduler, &store, now), rest[..2]);
    assert_eq!(tick(&scheduler, &store, now), rest[2..]);
    for session_id in &rest {
        assert_eq!(recovery(&store, session_id).attempt, 0);
    }
}

#[test]
fn a_probe_that_completes_opens_the_gate() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 2, now);

    assert_eq!(tick(&scheduler, &store, now), [sessions[0].clone()]);
    store
        .update_session_status(
            &sessions[0],
            SessionStatus::Completed,
            None,
            Some(&CompletionReason::TurnComplete),
        )
        .unwrap();

    assert_eq!(tick(&scheduler, &store, now), [sessions[1].clone()]);
    assert_eq!(scheduler.network_turn(now), NetworkTurn::Resume);
}

#[test]
fn a_stopped_probe_hands_the_probe_to_the_next_row_at_once() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 2, now);

    assert_eq!(tick(&scheduler, &store, now), [sessions[0].clone()]);
    store
        .update_session_status(
            &sessions[0],
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::Interrupted),
        )
        .unwrap();

    assert_eq!(tick(&scheduler, &store, now), [sessions[1].clone()]);
    // It is a probe, not a normal resume: the gate is still shut.
    assert_eq!(scheduler.network_turn(now), NetworkTurn::Hold);
}

#[test]
fn a_failure_while_open_advances_the_attempt_and_closes_the_gate() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 4, now);
    open_gate(&scheduler, &store, &sessions, now);
    assert_eq!(tick(&scheduler, &store, now), sessions[1..3]);

    let failed_at = now + 5_000;
    fail_on_network(&store, &scheduler, &sessions[1], failed_at);

    let state = recovery(&store, &sessions[1]);
    assert_eq!(state.attempt, 1);
    assert_eq!(state.reason.as_deref(), Some("backoff"));
    assert!(state.next_retry_at.unwrap() > failed_at);

    // Closed: the row still waiting does not resume until a probe is due.
    assert!(tick(&scheduler, &store, failed_at).is_empty());
    assert_eq!(
        tick(&scheduler, &store, failed_at + BASE_PROBE_DELAY_MS),
        [sessions[3].clone()]
    );

    // The other session resumed in the open period fails with the gate shut,
    // so it is not counted.
    fail_on_network(&store, &scheduler, &sessions[2], failed_at + 1_000);
    assert_eq!(recovery(&store, &sessions[2]).attempt, 0);
}

#[test]
fn a_probe_that_answers_and_then_fails_is_counted() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 1, now);

    assert_eq!(tick(&scheduler, &store, now), sessions);
    agent_replies(&store, &sessions[0]);
    // No tick has seen the output yet; the pause path settles the probe.
    fail_on_network(&store, &scheduler, &sessions[0], now + 1_000);

    assert_eq!(recovery(&store, &sessions[0]).attempt, 1);
    assert_eq!(scheduler.network_turn(now + 1_000), NetworkTurn::Hold);
}

#[test]
fn a_row_past_24_hours_still_resumes() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 1, now);

    // Two failed probes a day apart: neither is counted, and neither gives up.
    let mut at = now;
    for _ in 0..2 {
        at += 25 * HOUR_MS;
        assert_eq!(tick(&scheduler, &store, at), sessions);
        fail_on_network(&store, &scheduler, &sessions[0], at);
        let state = recovery(&store, &sessions[0]);
        assert_eq!(state.attempt, 0);
        assert_eq!(state.gave_up_at, None);
        assert_eq!(state.first_limited_at, now);
    }
    assert_eq!(tick(&scheduler, &store, at + 25 * HOUR_MS), sessions);
}

#[test]
fn api_limit_rows_are_unaffected_by_the_gate() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let network = network_paused_sessions(&store, &scheduler, 2, now);
    assert_eq!(tick(&scheduler, &store, now), [network[0].clone()]);

    // An API limit while a probe runs: counted as ever, and no word to the
    // gate.
    let limited = running_session(&store);
    step();
    assert!(pause_for_rate_limit(
        &store,
        Some(&scheduler),
        &limited.id,
        Some("rate limit"),
        None,
        Some("claude_pro"),
        now,
    )
    .unwrap());
    let state = recovery(&store, &limited.id);
    assert_eq!(state.attempt, 1);
    assert_eq!(state.reason.as_deref(), Some("backoff"));
    assert_eq!(scheduler.network_turn(now), NetworkTurn::Hold);

    // It resumes on its own schedule, past the network row the gate holds.
    let due = state.next_retry_at.unwrap();
    assert!(tick(&scheduler, &store, due - 1).is_empty());
    assert_eq!(
        tick(&scheduler, &store, due),
        std::slice::from_ref(&limited.id)
    );
    assert_eq!(scheduler.network_turn(due), NetworkTurn::Hold);

    // Nor does an API limit close an open gate.
    agent_replies(&store, &network[0]);
    scheduler.settle_network_probe(&store, due);
    assert!(pause_for_rate_limit(
        &store,
        Some(&scheduler),
        &limited.id,
        Some("rate limit"),
        None,
        Some("claude_pro"),
        due,
    )
    .unwrap());
    assert_eq!(recovery(&store, &limited.id).attempt, 2);
    assert_eq!(scheduler.network_turn(due), NetworkTurn::Resume);
}

/// The failed start re-parks the row, then reschedules it.
fn fail_to_start(store: &Store, scheduler: &RateLimitScheduler, session_id: &str, now: i64) {
    assert!(store
        .transition_running_to_rate_limited(
            session_id,
            Some("Can't reach the API server"),
            None,
            Some(NETWORK_PAUSE_KIND),
        )
        .unwrap());
    reschedule_after_start_failure(store, scheduler, session_id, now);
}

/// A start failure says nothing about the network: the probe is abandoned,
/// so the delay does not grow and another row probes at once, and the
/// failure is counted against the session that could not start.
#[test]
fn a_probe_that_fails_to_start_is_counted_and_hands_the_probe_over() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 2, now);
    assert_eq!(tick(&scheduler, &store, now), [sessions[0].clone()]);

    fail_to_start(&store, &scheduler, &sessions[0], now);

    let state = recovery(&store, &sessions[0]);
    assert_eq!(state.attempt, 1);
    assert_eq!(state.reason.as_deref(), Some("backoff"));
    assert!(state.next_retry_at.unwrap() > now);

    // The next row probes on the same tick time, with no delay added.
    assert_eq!(tick(&scheduler, &store, now), [sessions[1].clone()]);
    // And when that probe fails on the network, the delay is still the
    // base one: the failed start doubled nothing.
    fail_on_network(&store, &scheduler, &sessions[1], now);
    let due = state.next_retry_at.unwrap().max(now + BASE_PROBE_DELAY_MS);
    assert!(tick(&scheduler, &store, now + BASE_PROBE_DELAY_MS - 1).is_empty());
    assert!(!tick(&scheduler, &store, due).is_empty());
}

/// A lone network-paused session whose resume never starts is bounded by
/// the attempt cap, as it was before the gate existed.
#[test]
fn a_lone_session_that_never_starts_gives_up_at_the_attempt_cap() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let mut now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 1, now);

    for attempt in 1..=MAX_AUTO_RETRY_ATTEMPTS {
        assert_eq!(tick(&scheduler, &store, now), sessions, "attempt {attempt}");
        fail_to_start(&store, &scheduler, &sessions[0], now);
        let state = recovery(&store, &sessions[0]);
        assert_eq!(state.attempt, attempt);
        assert_eq!(state.gave_up_at, None);
        now = state.next_retry_at.unwrap();
    }

    assert_eq!(tick(&scheduler, &store, now), sessions);
    fail_to_start(&store, &scheduler, &sessions[0], now);
    let state = recovery(&store, &sessions[0]);
    assert_eq!(state.gave_up_at, Some(now));
    assert_eq!(state.reason.as_deref(), Some("retry_cap_exceeded"));
    assert!(tick(&scheduler, &store, now + HOUR_MS).is_empty());
}

/// What a real failed Claude turn records is not a sign of the network: the
/// error text, the error result and the visible message all follow the
/// claim, and none of them may open the gate or get the failure counted.
#[test]
fn a_failed_claude_probe_is_not_taken_for_a_success() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 3, now);
    assert_eq!(tick(&scheduler, &store, now), [sessions[0].clone()]);

    claude_turn_fails_offline(&store, &sessions[0]);
    // A tick that lands between those rows and the pause write resumes
    // nobody into the outage.
    assert!(tick(&scheduler, &store, now).is_empty());
    assert_eq!(scheduler.network_turn(now), NetworkTurn::Hold);

    let failed_at = now + 1_000;
    fail_on_network(&store, &scheduler, &sessions[0], failed_at);

    let state = recovery(&store, &sessions[0]);
    assert_eq!(state.attempt, 0);
    assert_eq!(state.gave_up_at, None);
    assert_eq!(state.reason.as_deref(), Some("network_wait"));

    // The gate closed as after any failed probe, and the delay doubles.
    assert!(tick(&scheduler, &store, failed_at + BASE_PROBE_DELAY_MS - 1).is_empty());
    let second_at = failed_at + BASE_PROBE_DELAY_MS;
    assert_eq!(tick(&scheduler, &store, second_at), [sessions[1].clone()]);
    claude_turn_fails_offline(&store, &sessions[1]);
    fail_on_network(&store, &scheduler, &sessions[1], second_at);
    assert_eq!(recovery(&store, &sessions[1]).attempt, 0);
    assert!(tick(&scheduler, &store, second_at + 2 * BASE_PROBE_DELAY_MS - 1).is_empty());
    assert_eq!(
        tick(&scheduler, &store, second_at + 2 * BASE_PROBE_DELAY_MS),
        [sessions[2].clone()]
    );
}

/// The signs that do open the gate: a result the agent did not mark as an
/// error, and a prompt response. Message text alone does not.
#[test]
fn a_successful_result_or_prompt_response_opens_the_gate_and_text_does_not() {
    for kind in ["response_result", "prompt_response"] {
        let store = Arc::new(Store::in_memory().unwrap());
        let scheduler = RateLimitScheduler::new();
        let now = crate::store::now_timestamp();
        let sessions = network_paused_sessions(&store, &scheduler, 2, now);
        assert_eq!(tick(&scheduler, &store, now), [sessions[0].clone()]);

        store
            .add_session_message(&sessions[0], MessageRole::Assistant, "Checking my work.")
            .unwrap();
        acp_event(
            &store,
            &sessions[0],
            "agent_message_chunk",
            serde_json::json!({ "content": { "type": "text", "text": "Checking my work." } }),
        );
        assert!(tick(&scheduler, &store, now).is_empty(), "{kind}");

        let content = if kind == "response_result" {
            serde_json::json!({ "type": "result", "subtype": "success", "is_error": false })
        } else {
            serde_json::json!({ "stopReason": "end_turn" })
        };
        acp_event(&store, &sessions[0], kind, content);
        assert_eq!(
            tick(&scheduler, &store, now),
            [sessions[1].clone()],
            "{kind}"
        );
        assert_eq!(scheduler.network_turn(now), NetworkTurn::Resume);
    }
}

/// After an outage longer than 24 hours, a session's first counted failure
/// schedules a retry: the window starts there, not at the first pause.
#[test]
fn a_counted_failure_after_a_long_outage_does_not_give_up() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    // The ticks run in the past so that the probe's reply, stamped with
    // the wall clock, lands after the probe's claim.
    let paused_at = crate::store::now_timestamp() - 40 * HOUR_MS;
    let sessions = network_paused_sessions(&store, &scheduler, 1, paused_at);

    let resumed_at = paused_at + 30 * HOUR_MS;
    assert_eq!(tick(&scheduler, &store, resumed_at), sessions);
    // The probe gets through, then the connection drops again.
    agent_replies(&store, &sessions[0]);
    let failed_at = resumed_at + 60_000;
    fail_on_network(&store, &scheduler, &sessions[0], failed_at);

    let state = recovery(&store, &sessions[0]);
    assert_eq!(state.attempt, 1);
    assert_eq!(state.gave_up_at, None);
    assert_eq!(state.reason.as_deref(), Some("backoff"));
    assert_eq!(state.first_limited_at, failed_at);
}

/// A probe that stays `running` with no sign of the provider stops holding
/// the other rows after the timeout.
#[test]
fn a_probe_that_hangs_times_out_and_another_row_probes() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 2, now);
    assert_eq!(tick(&scheduler, &store, now), [sessions[0].clone()]);

    assert!(tick(&scheduler, &store, now + PROBE_TIMEOUT_MS - 1).is_empty());
    assert_eq!(
        scheduler.network_gate.lock().unwrap().probe(),
        Some((sessions[0].clone(), now))
    );

    // The timeout closes the gate like a failed probe: the next one waits
    // the probe delay.
    let timed_out_at = now + PROBE_TIMEOUT_MS;
    assert!(tick(&scheduler, &store, timed_out_at).is_empty());
    assert_eq!(scheduler.network_gate.lock().unwrap().probe(), None);
    assert!(tick(&scheduler, &store, timed_out_at + BASE_PROBE_DELAY_MS - 1).is_empty());
    assert_eq!(
        tick(&scheduler, &store, timed_out_at + BASE_PROBE_DELAY_MS),
        [sessions[1].clone()]
    );

    // The session that hung is still running. When it fails at last, it is
    // not counted and does not disturb the probe now running.
    let late = timed_out_at + BASE_PROBE_DELAY_MS + 1_000;
    fail_on_network(&store, &scheduler, &sessions[0], late);
    assert_eq!(recovery(&store, &sessions[0]).attempt, 0);
    assert!(scheduler
        .network_gate
        .lock()
        .unwrap()
        .is_probing(&sessions[1]));
}

/// The user's "Resume now" on a network-paused row: the claim the manual
/// resume makes, with the word it gives the scheduler.
fn resume_manually(store: &Store, scheduler: &RateLimitScheduler, session_id: &str, now: i64) {
    let paused = store.get_session(session_id).unwrap().unwrap();
    assert!(store
        .transition_to_running_clearing_agent_id(session_id, None)
        .unwrap());
    scheduler.manual_resume_started(&paused, now);
}

#[test]
fn a_manual_resume_that_gets_through_opens_the_gate() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 4, now);
    // A failed probe leaves the gate closed with the next probe a delay away.
    assert_eq!(tick(&scheduler, &store, now), [sessions[0].clone()]);
    fail_on_network(&store, &scheduler, &sessions[0], now);
    assert!(tick(&scheduler, &store, now).is_empty());

    resume_manually(&store, &scheduler, &sessions[1], now);
    // The gate watches that turn; nothing else starts meanwhile, even once
    // the probe delay is up.
    assert!(tick(&scheduler, &store, now + BASE_PROBE_DELAY_MS).is_empty());

    agent_replies(&store, &sessions[1]);
    assert_eq!(tick(&scheduler, &store, now + 1_000).len(), 2);
    assert_eq!(scheduler.network_turn(now + 1_000), NetworkTurn::Resume);
}

#[test]
fn a_manual_resume_that_fails_closes_the_gate_uncounted() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 2, now);
    assert_eq!(tick(&scheduler, &store, now), [sessions[0].clone()]);
    fail_on_network(&store, &scheduler, &sessions[0], now);

    resume_manually(&store, &scheduler, &sessions[1], now);
    claude_turn_fails_offline(&store, &sessions[1]);
    fail_on_network(&store, &scheduler, &sessions[1], now);

    assert_eq!(recovery(&store, &sessions[1]).attempt, 0);
    // It failed as the second probe in a row: the next waits twice the base.
    assert!(tick(&scheduler, &store, now + 2 * BASE_PROBE_DELAY_MS - 1).is_empty());
    assert_eq!(
        tick(&scheduler, &store, now + 2 * BASE_PROBE_DELAY_MS).len(),
        1
    );
}

/// Only a network pause is watched, and only by a closed gate.
#[test]
fn a_manual_resume_of_another_row_tells_the_gate_nothing() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();

    let limited = running_session(&store);
    assert!(pause_for_rate_limit(
        &store,
        Some(&scheduler),
        &limited.id,
        Some("rate limit"),
        None,
        Some("claude_pro"),
        now,
    )
    .unwrap());
    resume_manually(&store, &scheduler, &limited.id, now);
    assert_eq!(scheduler.network_turn(now), NetworkTurn::Probe);

    // A probe is already running: it keeps the gate.
    let sessions = network_paused_sessions(&store, &scheduler, 2, now);
    assert_eq!(tick(&scheduler, &store, now), [sessions[0].clone()]);
    resume_manually(&store, &scheduler, &sessions[1], now);
    assert!(scheduler
        .network_gate
        .lock()
        .unwrap()
        .is_probing(&sessions[0]));
}

#[test]
fn the_probe_delay_doubles_to_a_ten_minute_cap() {
    let mut gate = NetworkGate::default();
    let mut now = 1_000;
    let mut delays = Vec::new();
    for _ in 0..8 {
        assert_eq!(gate.turn(now), NetworkTurn::Probe);
        gate.probe_started("probe", now);
        assert_eq!(gate.record_failure("probe", now), PauseAccounting::Waiting);
        delays.push(gate.next_probe_at - now);
        now = gate.next_probe_at;
    }
    assert_eq!(
        delays,
        [15_000, 30_000, 60_000, 120_000, 240_000, 480_000, 600_000, 600_000]
    );

    // An open period starts the doubling again.
    gate.probe_started("probe", now);
    gate.probe_succeeded("probe");
    assert_eq!(gate.record_failure("other", now), PauseAccounting::Counted);
    assert_eq!(gate.next_probe_at - now, BASE_PROBE_DELAY_MS);
    now = gate.next_probe_at;
    gate.probe_started("probe", now);
    assert_eq!(gate.record_failure("probe", now), PauseAccounting::Waiting);
    assert_eq!(gate.next_probe_at - now, 2 * BASE_PROBE_DELAY_MS);
}

#[test]
fn another_sessions_failure_leaves_a_running_probe_alone() {
    let mut gate = NetworkGate::default();
    gate.probe_started("probe", 1_000);

    assert_eq!(
        gate.record_failure("other", 2_000),
        PauseAccounting::Waiting
    );
    assert!(gate.is_probing("probe"));

    // Nor does a stale answer about a session that is not the probe move it.
    gate.probe_succeeded("other");
    gate.probe_abandoned("other", 2_000);
    assert!(gate.is_probing("probe"));
}

/// A closed gate that has read a fingerprint with one address.
fn gate_with_a_network() -> NetworkGate {
    let mut gate = NetworkGate::default();
    assert!(!gate.observe_fingerprint(BTreeSet::from(["192.0.2.10".parse().unwrap()])));
    gate
}

fn fail_a_probe(gate: &mut NetworkGate, session_id: &str, now: i64) -> PauseAccounting {
    gate.probe_started(session_id, now);
    gate.record_failure(session_id, now)
}

#[test]
fn a_sessions_twentieth_failed_probe_in_a_row_exhausts_it() {
    let mut gate = gate_with_a_network();
    for failure in 1..MAX_CONSECUTIVE_PROBE_FAILURES {
        assert_eq!(
            fail_a_probe(&mut gate, "lone", 1_000),
            PauseAccounting::Waiting,
            "failure {failure}"
        );
        // Another session's failed probes are its own.
        assert_eq!(
            fail_a_probe(&mut gate, "other", 1_000),
            PauseAccounting::Waiting
        );
    }
    assert_eq!(
        fail_a_probe(&mut gate, "lone", 1_000),
        PauseAccounting::ProbesExhausted
    );
    // The count starts over for a session that was given up on.
    assert_eq!(
        fail_a_probe(&mut gate, "lone", 1_000),
        PauseAccounting::Waiting
    );
}

#[test]
fn a_probe_failure_counts_only_awake_with_a_network_present() {
    let mut gate = gate_with_a_network();
    for _ in 1..MAX_CONSECUTIVE_PROBE_FAILURES {
        fail_a_probe(&mut gate, "lone", 1_000);
    }

    // The machine slept while the probe ran.
    gate.observe_tick(1_000);
    gate.probe_started("lone", 1_000);
    assert!(gate.observe_tick(1_000 + WAKE_GAP_MS + 1));
    assert_eq!(
        gate.record_failure("lone", 2_000_000),
        PauseAccounting::Waiting
    );

    // No interface holds an address.
    assert!(gate.observe_fingerprint(BTreeSet::new()));
    assert_eq!(
        fail_a_probe(&mut gate, "lone", 3_000_000),
        PauseAccounting::Waiting
    );

    // Nothing is known about the network.
    gate.fingerprint_unreadable();
    assert_eq!(
        fail_a_probe(&mut gate, "lone", 4_000_000),
        PauseAccounting::Waiting
    );

    // None of those broke the run or added to it: the next failure awake
    // with a network is the twentieth.
    gate.observe_fingerprint(BTreeSet::from(["192.0.2.10".parse().unwrap()]));
    assert_eq!(
        fail_a_probe(&mut gate, "lone", 5_000_000),
        PauseAccounting::ProbesExhausted
    );
}

#[test]
fn a_probe_that_gets_through_clears_the_failure_counts() {
    let mut gate = gate_with_a_network();
    for _ in 1..MAX_CONSECUTIVE_PROBE_FAILURES {
        fail_a_probe(&mut gate, "lone", 1_000);
    }
    gate.probe_started("other", 1_000);
    gate.probe_succeeded("other");
    assert_eq!(gate.record_failure("lone", 1_000), PauseAccounting::Counted);

    // Closed again: the fingerprint is read anew, and "lone" starts at one.
    gate.observe_fingerprint(BTreeSet::from(["192.0.2.10".parse().unwrap()]));
    assert_eq!(
        fail_a_probe(&mut gate, "lone", 1_000),
        PauseAccounting::Waiting
    );
}

#[test]
fn a_timed_out_probe_closes_the_gate_like_a_failed_one_but_is_not_counted() {
    let mut gate = gate_with_a_network();
    gate.probe_started("probe", 1_000);
    gate.probe_timed_out("other", 2_000);
    assert!(gate.is_probing("probe"));

    gate.probe_timed_out("probe", 2_000);
    assert!(gate.is_closed());
    assert_eq!(gate.next_probe_at, 2_000 + BASE_PROBE_DELAY_MS);
    assert_eq!(gate.probe_delay_ms, 2 * BASE_PROBE_DELAY_MS);
    assert!(gate.probe_failures.is_empty());
}
