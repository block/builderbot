//! A session waiting for a provider-reported reset keeps waiting for it when
//! a pause in between reports none (see `pending_reset_retry`).

use std::path::Path;
use std::sync::Arc;

use super::{
    auto_recovery_from_metadata, build_recovery_state, claim_for_auto_resume, is_due,
    parse_rfc3339_millis, pause_for_rate_limit, AutoResumeClaim, PauseAccounting,
    RateLimitAutoRecovery, BASE_BACKOFF_MS, GIVE_UP_AFTER_MS, MAX_AUTO_RETRY_ATTEMPTS,
    MIN_RETRY_DELAY_MS, RESET_GRACE_MS, RESET_JITTER_MS,
};
use crate::store::{CompletionReason, Session, SessionStatus, Store};

const MINUTE_MS: i64 = 60_000;
const RESETS_AT: &str = "2026-06-24T17:30:00.000Z";
const LATER_RESETS_AT: &str = "2026-06-24T19:00:00.000Z";
const JITTER_MS: i64 = 12_000;

/// 2026-06-24T15:00:00Z, two and a half hours before [`RESETS_AT`].
fn paused_at() -> i64 {
    parse_rfc3339_millis("2026-06-24T15:00:00Z").unwrap()
}

fn millis(timestamp: &str) -> i64 {
    parse_rfc3339_millis(timestamp).unwrap()
}

/// The bookkeeping of a first pause that reported [`RESETS_AT`].
fn waiting_for_reset() -> RateLimitAutoRecovery {
    build_recovery_state(
        None,
        Some("five_hour"),
        PauseAccounting::Counted,
        Some(RESETS_AT),
        paused_at(),
        None,
        JITTER_MS,
    )
}

/// One more API-limit pause on top of `existing`.
fn pause_again(
    existing: &RateLimitAutoRecovery,
    resets_at: Option<&str>,
    now: i64,
) -> RateLimitAutoRecovery {
    build_recovery_state(
        Some(existing),
        Some("rate_limit"),
        PauseAccounting::Counted,
        resets_at,
        now,
        None,
        JITTER_MS,
    )
}

#[test]
fn a_pause_with_no_reset_keeps_a_pending_reset_retry_uncounted() {
    let waiting = waiting_for_reset();
    assert_eq!(waiting.reason.as_deref(), Some("reset_at"));
    let now = paused_at() + MINUTE_MS;

    let state = pause_again(&waiting, None, now);

    assert_eq!(
        state,
        RateLimitAutoRecovery {
            updated_at: now,
            ..waiting
        }
    );
    assert_eq!(state.attempt, 1);
    assert_eq!(
        state.next_retry_at,
        Some(millis(RESETS_AT) + RESET_GRACE_MS + JITTER_MS)
    );
}

#[test]
fn a_reset_that_cannot_be_used_keeps_a_pending_reset_retry_too() {
    let waiting = waiting_for_reset();
    let now = paused_at() + MINUTE_MS;

    // A reset already in the past, and one that does not parse.
    for unusable in ["2026-06-24T14:00:00Z", "5:30pm"] {
        let state = pause_again(&waiting, Some(unusable), now);
        assert_eq!(state.attempt, 1, "{unusable}");
        assert_eq!(state.next_retry_at, waiting.next_retry_at, "{unusable}");
    }
}

#[test]
fn a_pending_reset_retry_is_not_charged_to_the_cap_or_the_window() {
    // Out of attempts and first limited more than a day ago, but the reset
    // the session waits for is still ahead.
    let waiting = RateLimitAutoRecovery {
        attempt: MAX_AUTO_RETRY_ATTEMPTS,
        first_limited_at: paused_at() - GIVE_UP_AFTER_MS - MINUTE_MS,
        ..waiting_for_reset()
    };

    let state = pause_again(&waiting, None, paused_at() + MINUTE_MS);

    assert_eq!(state.attempt, MAX_AUTO_RETRY_ATTEMPTS);
    assert_eq!(state.gave_up_at, None);
    assert_eq!(state.next_retry_at, waiting.next_retry_at);
}

#[test]
fn a_pause_after_the_reset_retry_came_due_is_counted_on_the_backoff() {
    let waiting = waiting_for_reset();
    // The scheduler resumes a session once its retry is due, so this is
    // where a refused auto-resume lands: the reset has passed.
    let now = waiting.next_retry_at.unwrap() + 15_000;

    let state = pause_again(&waiting, None, now);

    assert_eq!(state.attempt, 2);
    assert_eq!(state.reason.as_deref(), Some("backoff"));
    assert_eq!(state.next_retry_at, Some(now + JITTER_MS));
}

#[test]
fn a_new_reset_replaces_a_pending_reset_retry() {
    let waiting = waiting_for_reset();
    let now = paused_at() + MINUTE_MS;

    let state = pause_again(&waiting, Some(LATER_RESETS_AT), now);

    assert_eq!(state.attempt, 2);
    assert_eq!(state.reason.as_deref(), Some("reset_at"));
    assert_eq!(
        state.next_retry_at,
        Some(millis(LATER_RESETS_AT) + RESET_GRACE_MS + JITTER_MS)
    );
}

#[test]
fn a_pending_backoff_retry_is_never_kept() {
    let backing_off = build_recovery_state(
        None,
        Some("rate_limit"),
        PauseAccounting::Counted,
        None,
        paused_at(),
        None,
        JITTER_MS,
    );
    assert_eq!(backing_off.reason.as_deref(), Some("backoff"));
    let now = paused_at() + 1_000;
    assert!(backing_off.next_retry_at.unwrap() > now);

    let state = pause_again(&backing_off, None, now);

    assert_eq!(state.attempt, 2);
    assert_eq!(state.next_retry_at, Some(now + JITTER_MS));
}

#[test]
fn a_blocked_pause_still_needs_a_manual_resume() {
    let waiting = waiting_for_reset();

    let state = build_recovery_state(
        Some(&waiting),
        Some("rate_limit"),
        PauseAccounting::Counted,
        None,
        paused_at() + MINUTE_MS,
        Some("pending_tool_call"),
        JITTER_MS,
    );

    assert!(state.unsafe_to_auto_resume);
    assert_eq!(state.next_retry_at, None);
}

fn running_session(store: &Store) -> Session {
    let session = Session::new_running("work", Path::new("/tmp")).with_agent("agent-1");
    store.create_session(&session).unwrap();
    session
}

fn recovery(store: &Store, session_id: &str) -> RateLimitAutoRecovery {
    let row = store.get_session(session_id).unwrap().unwrap();
    auto_recovery_from_metadata(row.provider_metadata.as_ref()).expect("retry bookkeeping")
}

/// A session paused with [`RESETS_AT`] and then claimed for an auto-resume.
///
/// Returns the bookkeeping the pause wrote. The claim leaves the row
/// `running` with that bookkeeping and no reset column, which is all a pause
/// of the resumed run has to plan from.
fn paused_for_reset_then_claimed(store: &Arc<Store>, session_id: &str) -> RateLimitAutoRecovery {
    assert!(pause_for_rate_limit(
        store,
        None,
        session_id,
        Some("You've hit your session limit · resets 5:30pm (UTC)"),
        Some(RESETS_AT),
        Some("five_hour"),
        paused_at(),
    )
    .unwrap());
    let scheduled = recovery(store, session_id);
    assert_eq!(scheduled.attempt, 1);
    assert_eq!(scheduled.reason.as_deref(), Some("reset_at"));
    let retry_at = scheduled.next_retry_at.unwrap();
    let earliest = millis(RESETS_AT) + RESET_GRACE_MS + MIN_RETRY_DELAY_MS;
    let latest = millis(RESETS_AT) + RESET_GRACE_MS + RESET_JITTER_MS;
    assert!((earliest..=latest).contains(&retry_at), "{retry_at}");

    assert!(matches!(
        claim_for_auto_resume(store, session_id).unwrap(),
        AutoResumeClaim::Claimed(_)
    ));
    let claimed = store.get_session(session_id).unwrap().unwrap();
    assert_eq!(claimed.status, SessionStatus::Running);
    assert_eq!(claimed.rate_limit_resets_at, None);
    assert_eq!(recovery(store, session_id), scheduled);
    scheduled
}

/// The resumed run is refused before its first model response, with an
/// error that carries no reset.
fn refuse_without_a_reset(store: &Store, session_id: &str, now: i64) {
    assert!(pause_for_rate_limit(
        store,
        None,
        session_id,
        Some("You've hit your session limit"),
        None,
        Some("rate_limit"),
        now,
    )
    .unwrap());
}

/// The claim itself does not ask whether the retry is due (the tick does),
/// so this refuses a resume that started while the reset was still ahead.
#[test]
fn an_auto_resume_refused_before_the_reset_keeps_waiting_for_it() {
    let store = Arc::new(Store::in_memory().unwrap());
    let session = running_session(&store);
    let scheduled = paused_for_reset_then_claimed(&store, &session.id);
    let refused_at = paused_at() + MINUTE_MS;

    refuse_without_a_reset(&store, &session.id, refused_at);

    let waiting = recovery(&store, &session.id);
    assert_eq!(
        waiting,
        RateLimitAutoRecovery {
            updated_at: refused_at,
            ..scheduled.clone()
        }
    );
    let row = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(row.status, SessionStatus::Paused);
    assert_eq!(row.completion_reason, Some(CompletionReason::RateLimited));
    assert_eq!(row.rate_limit_resets_at, None);
    assert_eq!(store.get_rate_limited_paused_sessions().unwrap().len(), 1);

    // The scheduler leaves it alone until the retry the reset set, and
    // resumes it then.
    let retry_at = scheduled.next_retry_at.unwrap();
    assert!(!is_due(&waiting, refused_at + 30 * MINUTE_MS));
    assert!(!is_due(&waiting, retry_at - 1));
    assert!(is_due(&waiting, retry_at));
}

/// What the scheduler's own retry meets: it resumes only once the retry is
/// due, by when the reset has passed, so a refusal with no reset is a new
/// limit and takes the next attempt on the backoff.
#[test]
fn an_auto_resume_refused_after_the_reset_is_counted_on_the_backoff() {
    let store = Arc::new(Store::in_memory().unwrap());
    let session = running_session(&store);
    let scheduled = paused_for_reset_then_claimed(&store, &session.id);
    let refused_at = scheduled.next_retry_at.unwrap() + 15_000;

    refuse_without_a_reset(&store, &session.id, refused_at);

    let state = recovery(&store, &session.id);
    assert_eq!(state.attempt, 2);
    assert_eq!(state.reason.as_deref(), Some("backoff"));
    // The wait for the reset is not time on the backoff: the 24 hour window
    // starts again with this refusal.
    assert_eq!(state.first_limited_at, refused_at);
    let retry_at = state.next_retry_at.unwrap();
    let earliest = refused_at + MIN_RETRY_DELAY_MS;
    let latest = refused_at + 2 * BASE_BACKOFF_MS;
    assert!((earliest..=latest).contains(&retry_at), "{retry_at}");
}

#[test]
fn an_auto_resume_refused_with_a_new_reset_waits_for_that_one() {
    let store = Arc::new(Store::in_memory().unwrap());
    let session = running_session(&store);
    let scheduled = paused_for_reset_then_claimed(&store, &session.id);
    let refused_at = scheduled.next_retry_at.unwrap() + 15_000;

    assert!(pause_for_rate_limit(
        &store,
        None,
        &session.id,
        Some("You've hit your session limit · resets 7pm (UTC)"),
        Some(LATER_RESETS_AT),
        Some("five_hour"),
        refused_at,
    )
    .unwrap());

    let state = recovery(&store, &session.id);
    assert_eq!(state.attempt, 2);
    assert_eq!(state.reason.as_deref(), Some("reset_at"));
    assert!(state.next_retry_at.unwrap() > millis(LATER_RESETS_AT));
    let row = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(row.rate_limit_resets_at.as_deref(), Some(LATER_RESETS_AT));
}
