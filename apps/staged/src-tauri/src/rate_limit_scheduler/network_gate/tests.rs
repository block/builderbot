use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::super::{
    admit_for_resume, auto_recovery_from_metadata, claim_for_auto_resume, pause_for_rate_limit,
    reschedule_after_start_failure, AutoResumeClaim, RateLimitAutoRecovery, RateLimitScheduler,
    MAX_RESUMES_PER_TICK, NETWORK_PAUSE_KIND,
};
use super::*;
use crate::store::{CompletionReason, MessageRole, Session, SessionStatus, Store};

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

/// The synchronous part of `run_tick`: settle the probe, then admit and
/// claim rows oldest pause first. Returns the sessions it started.
fn tick(scheduler: &RateLimitScheduler, store: &Arc<Store>, now: i64) -> Vec<String> {
    scheduler.settle_network_probe(store, now);
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

fn agent_replies(store: &Store, session_id: &str) {
    store
        .add_session_message(session_id, MessageRole::Assistant, "Back online.")
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
    assert!(tick(&scheduler, &store, now + HOUR_MS).is_empty());
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

    assert_eq!(tick(&scheduler, &store, now), [first.id.clone()]);
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
    assert_eq!(tick(&scheduler, &store, due), [limited.id.clone()]);
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

#[test]
fn a_probe_that_fails_to_start_closes_the_gate_uncounted() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = RateLimitScheduler::new();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 1, now);
    assert_eq!(tick(&scheduler, &store, now), sessions);

    // The failed start re-parks the row, then reschedules it.
    assert!(store
        .transition_running_to_rate_limited(
            &sessions[0],
            Some("Can't reach the API server"),
            None,
            Some(NETWORK_PAUSE_KIND),
        )
        .unwrap());
    reschedule_after_start_failure(&store, &scheduler, &sessions[0], now);

    assert_eq!(recovery(&store, &sessions[0]).attempt, 0);
    assert!(tick(&scheduler, &store, now + BASE_PROBE_DELAY_MS - 1).is_empty());
    assert_eq!(
        tick(&scheduler, &store, now + BASE_PROBE_DELAY_MS),
        sessions
    );
}

#[test]
fn the_probe_delay_doubles_to_a_ten_minute_cap() {
    let mut gate = NetworkGate::default();
    let mut now = 1_000;
    let mut delays = Vec::new();
    for _ in 0..8 {
        assert_eq!(gate.turn(now), NetworkTurn::Probe);
        gate.probe_started("probe", now);
        assert!(!gate.record_failure("probe", now));
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
    assert!(gate.record_failure("other", now));
    assert_eq!(gate.next_probe_at - now, BASE_PROBE_DELAY_MS);
    now = gate.next_probe_at;
    gate.probe_started("probe", now);
    assert!(!gate.record_failure("probe", now));
    assert_eq!(gate.next_probe_at - now, 2 * BASE_PROBE_DELAY_MS);
}

#[test]
fn another_sessions_failure_leaves_a_running_probe_alone() {
    let mut gate = NetworkGate::default();
    gate.probe_started("probe", 1_000);

    assert!(!gate.record_failure("other", 2_000));
    assert!(gate.is_probing("probe"));

    // Nor does a stale answer about a session that is not the probe move it.
    gate.probe_succeeded("other");
    gate.probe_abandoned("other", 2_000);
    assert!(gate.is_probing("probe"));
}
