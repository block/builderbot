//! How the retry after a counted pause is timed: for the reset the provider
//! reported, on the slow schedule of a subscription usage limit that
//! reported none, or on the minute-scale backoff.

use super::{
    parse_reset_timestamp_millis, plausible_reset_millis, MAX_BACKOFF_MS, RESET_JITTER_MS,
};

/// The `rate_limit_kind` of a subscription usage limit known only from its
/// message, as an agent that reports no limit type words it ("You've hit
/// your session limit"). A limit the agent names keeps that name; see
/// [`is_usage_limit_kind`].
pub(crate) const USAGE_LIMIT_KIND: &str = "usage_limit";

/// The bookkeeping `reason` of a retry on the slow schedule
/// ([`RetrySchedule::UsageLimit`]).
pub(super) const USAGE_LIMIT_BACKOFF_REASON: &str = "usage_limit_backoff";

/// The fixed part of the wait between retries of a subscription usage limit
/// that reported no reset, and the most jitter added to it.
///
/// Such a limit clears when its window resets, up to five hours on for a
/// session limit, and nothing says when. The eight attempts of
/// `MAX_AUTO_RETRY_ATTEMPTS` are spread over 5 h 20 min at least (6 h 40 min
/// at most), so a session limit is outlasted from its first pause, and a
/// retry follows the reset by 50 minutes at most. A refused retry is cheap:
/// the turn is turned away before its first model response.
pub(super) const USAGE_LIMIT_RETRY_INTERVAL_MS: i64 = 40 * 60_000;
pub(super) const USAGE_LIMIT_RETRY_JITTER_MS: i64 = 10 * 60_000;

/// How the retry after a counted pause is timed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RetrySchedule {
    /// For the reset the provider reported, in UTC millis.
    Reset(i64),
    /// The slow schedule of a subscription usage limit that reported no
    /// reset: [`USAGE_LIMIT_RETRY_INTERVAL_MS`] and jitter, every attempt.
    UsageLimit,
    /// The minute-scale backoff, doubling from a minute up to
    /// [`MAX_BACKOFF_MS`]: API rate limits (429), overloaded providers and
    /// network pauses, which clear in seconds to minutes.
    Backoff,
}

/// The schedule for a pause of `kind` that reported `resets_at`.
///
/// A usable reset always wins. Without one, only a subscription usage limit
/// leaves the minute-scale backoff, and only if it reported no reset to act
/// on: one whose reset has already passed is retried on the backoff, because
/// its window is due to have reopened.
pub(super) fn retry_schedule(
    kind: Option<&str>,
    resets_at: Option<&str>,
    now: i64,
) -> RetrySchedule {
    if let Some(reset_at) = plausible_reset_millis(resets_at, now) {
        RetrySchedule::Reset(reset_at)
    } else if is_usage_limit_kind(kind) && !reset_has_passed(resets_at, now) {
        RetrySchedule::UsageLimit
    } else {
        RetrySchedule::Backoff
    }
}

/// Whether `kind` names a subscription usage limit: one recognised from its
/// message ([`USAGE_LIMIT_KIND`]), or a limit type the Claude Agent SDK
/// reports for claude.ai subscriptions (`SDKRateLimitInfo.rateLimitType`:
/// `five_hour`, `seven_day` and its variants, `overage`).
///
/// Every other kind is not one, an API `rate_limit` or `overloaded` among
/// them, and so is a kind this list does not know: it keeps the backoff.
fn is_usage_limit_kind(kind: Option<&str>) -> bool {
    kind.is_some_and(|kind| {
        kind == USAGE_LIMIT_KIND
            || kind == "five_hour"
            || kind == "overage"
            || kind.starts_with("seven_day")
    })
}

/// Whether a stored reset is a time that has already passed.
fn reset_has_passed(resets_at: Option<&str>, now: i64) -> bool {
    resets_at
        .and_then(parse_reset_timestamp_millis)
        .is_some_and(|reset_at| reset_at <= now)
}

/// The most jitter a retry on the schedule of this pause is given.
pub(super) fn jitter_bound_for(kind: Option<&str>, resets_at: Option<&str>, now: i64) -> i64 {
    match retry_schedule(kind, resets_at, now) {
        RetrySchedule::Reset(_) => RESET_JITTER_MS,
        RetrySchedule::UsageLimit => USAGE_LIMIT_RETRY_JITTER_MS,
        RetrySchedule::Backoff => MAX_BACKOFF_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        build_recovery_state, parse_rfc3339_millis, PauseAccounting, RateLimitAutoRecovery,
        BASE_BACKOFF_MS, GIVE_UP_AFTER_MS, MAX_AUTO_RETRY_ATTEMPTS, MAX_PLAUSIBLE_RESET_LEAD_MS,
        MIN_RETRY_DELAY_MS, NETWORK_PAUSE_KIND, RESET_GRACE_MS,
    };
    use super::*;

    const MINUTE_MS: i64 = 60_000;
    const HOUR_MS: i64 = 60 * MINUTE_MS;
    const DAY_MS: i64 = 24 * HOUR_MS;
    const JITTER_MS: i64 = 12_000;

    /// 2026-06-24T15:00:00Z.
    fn now() -> i64 {
        parse_rfc3339_millis("2026-06-24T15:00:00Z").unwrap()
    }

    /// One more counted pause of `kind` on top of `existing`.
    fn pause(
        existing: Option<&RateLimitAutoRecovery>,
        kind: &str,
        resets_at: Option<&str>,
        now: i64,
        jitter_ms: i64,
    ) -> RateLimitAutoRecovery {
        build_recovery_state(
            existing,
            Some(kind),
            PauseAccounting::Counted,
            resets_at,
            now,
            None,
            jitter_ms,
        )
    }

    #[test]
    fn a_usage_limit_with_no_reset_takes_the_slow_schedule() {
        for kind in [
            "usage_limit",
            "five_hour",
            "seven_day",
            "seven_day_opus",
            "seven_day_sonnet",
            "seven_day_overage_included",
            "overage",
        ] {
            assert_eq!(
                retry_schedule(Some(kind), None, now()),
                RetrySchedule::UsageLimit,
                "{kind}"
            );
            assert_eq!(
                jitter_bound_for(Some(kind), None, now()),
                USAGE_LIMIT_RETRY_JITTER_MS,
                "{kind}"
            );
        }
    }

    #[test]
    fn api_limits_and_other_kinds_keep_the_backoff() {
        let kinds = [
            Some("rate_limit"),
            Some("overloaded"),
            Some("rejected"),
            Some("subscription"),
            Some(NETWORK_PAUSE_KIND),
            None,
        ];
        for kind in kinds {
            assert_eq!(
                retry_schedule(kind, None, now()),
                RetrySchedule::Backoff,
                "{kind:?}"
            );
            assert_eq!(
                jitter_bound_for(kind, None, now()),
                MAX_BACKOFF_MS,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_usable_reset_wins_for_every_kind() {
        let reset = "2026-06-24T17:30:00Z";
        for kind in [Some("usage_limit"), Some("rate_limit"), None] {
            assert_eq!(
                retry_schedule(kind, Some(reset), now()),
                RetrySchedule::Reset(parse_rfc3339_millis(reset).unwrap()),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_usage_limit_whose_reset_has_passed_is_retried_on_the_backoff() {
        // The window is due to have reopened, so the next try is soon.
        let passed = "2026-06-24T14:59:00Z";
        assert_eq!(
            retry_schedule(Some("five_hour"), Some(passed), now()),
            RetrySchedule::Backoff
        );

        // A reset that says nothing usable is no reset to act on.
        let beyond_the_allowance = (now() + MAX_PLAUSIBLE_RESET_LEAD_MS + MINUTE_MS).to_string();
        for unusable in ["5:30pm", beyond_the_allowance.as_str()] {
            assert_eq!(
                retry_schedule(Some("five_hour"), Some(unusable), now()),
                RetrySchedule::UsageLimit,
                "{unusable}"
            );
        }
    }

    #[test]
    fn the_slow_schedule_waits_the_interval_and_bounded_jitter() {
        let state = pause(None, "usage_limit", None, now(), JITTER_MS);

        assert_eq!(state.attempt, 1);
        assert_eq!(state.reason.as_deref(), Some(USAGE_LIMIT_BACKOFF_REASON));
        assert_eq!(
            state.next_retry_at,
            Some(now() + USAGE_LIMIT_RETRY_INTERVAL_MS + JITTER_MS)
        );

        // Jitter drawn for another schedule is cut to this one's bound.
        let state = pause(None, "usage_limit", None, now(), MAX_BACKOFF_MS);
        assert_eq!(
            state.next_retry_at,
            Some(now() + USAGE_LIMIT_RETRY_INTERVAL_MS + USAGE_LIMIT_RETRY_JITTER_MS)
        );
    }

    /// Each refused retry pauses the session again when it comes due. Returns
    /// the time of every retry, and the state once recovery gave up.
    fn retries_until_give_up(kind: &str, jitter_ms: i64) -> (Vec<i64>, RateLimitAutoRecovery) {
        let mut retries = Vec::new();
        let mut state = pause(None, kind, None, now(), jitter_ms);
        while let Some(retry_at) = state.next_retry_at {
            retries.push(retry_at);
            state = pause(Some(&state), kind, None, retry_at, jitter_ms);
        }
        (retries, state)
    }

    #[test]
    fn the_slow_schedule_outlasts_a_five_hour_window() {
        // The least and the most jitter a retry can draw.
        for jitter_ms in [MIN_RETRY_DELAY_MS, USAGE_LIMIT_RETRY_JITTER_MS] {
            let (retries, gave_up) = retries_until_give_up("usage_limit", jitter_ms);

            assert_eq!(retries.len(), MAX_AUTO_RETRY_ATTEMPTS as usize);
            let last = *retries.last().unwrap();
            assert!(last - now() > 5 * HOUR_MS, "{jitter_ms}: {last}");
            assert!(last - now() <= 7 * HOUR_MS, "{jitter_ms}: {last}");
            // No two retries are more than 50 minutes apart, so the first one
            // after the window resets is at most that late.
            let mut previous = now();
            for retry_at in retries {
                assert!(retry_at - previous <= 50 * MINUTE_MS);
                previous = retry_at;
            }
            assert_eq!(gave_up.reason.as_deref(), Some("retry_cap_exceeded"));
            assert_eq!(gave_up.attempt, MAX_AUTO_RETRY_ATTEMPTS + 1);
        }
    }

    #[test]
    fn an_api_limit_still_gives_up_on_the_minute_scale() {
        // The same refusals as an API `rate_limit`, with the most jitter the
        // backoff can draw: 1 + 2 + 4 + 8 + 16 + 30 + 30 + 30 minutes.
        let (retries, gave_up) = retries_until_give_up("rate_limit", MAX_BACKOFF_MS);

        assert_eq!(retries.len(), MAX_AUTO_RETRY_ATTEMPTS as usize);
        assert_eq!(retries[0], now() + BASE_BACKOFF_MS);
        assert_eq!(*retries.last().unwrap() - now(), 121 * MINUTE_MS);
        assert_eq!(gave_up.reason.as_deref(), Some("retry_cap_exceeded"));

        let overloaded = pause(None, "overloaded", None, now(), MAX_BACKOFF_MS);
        assert_eq!(overloaded.reason.as_deref(), Some("backoff"));
        assert_eq!(overloaded.next_retry_at, Some(now() + BASE_BACKOFF_MS));
    }

    #[test]
    fn a_weekly_reset_days_ahead_is_waited_for() {
        // Six days and a few hours on: past the 48 hours a reset used to be
        // trusted for.
        let reset = "2026-06-30T19:00:00.000Z";
        let state = pause(None, "seven_day", Some(reset), now(), JITTER_MS);

        assert_eq!(state.attempt, 1);
        assert_eq!(state.reason.as_deref(), Some("reset_at"));
        assert_eq!(
            state.next_retry_at,
            Some(parse_rfc3339_millis(reset).unwrap() + RESET_GRACE_MS + JITTER_MS)
        );
        assert_eq!(
            jitter_bound_for(Some("seven_day"), Some(reset), now()),
            RESET_JITTER_MS
        );
    }

    #[test]
    fn a_reset_past_the_allowance_is_not_waited_for() {
        let nine_days_on = (now() + 9 * DAY_MS).to_string();
        // An epoch in microseconds, read as millis.
        let wrong_scale = ((now() + HOUR_MS) * 1_000).to_string();

        for reset in [nine_days_on.as_str(), wrong_scale.as_str()] {
            let weekly = pause(None, "seven_day", Some(reset), now(), JITTER_MS);
            assert_eq!(
                weekly.reason.as_deref(),
                Some(USAGE_LIMIT_BACKOFF_REASON),
                "{reset}"
            );
            let api = pause(None, "rate_limit", Some(reset), now(), JITTER_MS);
            assert_eq!(api.reason.as_deref(), Some("backoff"), "{reset}");
        }
    }

    #[test]
    fn a_pause_with_a_reset_is_not_held_to_the_window() {
        // On the backoff for more than a day, then a pause that reports a
        // reset: it is waited for, where a pause without one gives up.
        let first = pause(None, "rate_limit", None, now(), JITTER_MS);
        let late = now() + GIVE_UP_AFTER_MS + MINUTE_MS;
        let reset = (late + 3 * DAY_MS).to_string();

        let with_reset = pause(Some(&first), "seven_day", Some(&reset), late, JITTER_MS);
        assert_eq!(with_reset.attempt, 2);
        assert_eq!(with_reset.gave_up_at, None);
        assert_eq!(with_reset.reason.as_deref(), Some("reset_at"));

        let without = pause(Some(&first), "rate_limit", None, late, JITTER_MS);
        assert_eq!(without.gave_up_at, Some(late));
        assert_eq!(without.reason.as_deref(), Some("retry_window_elapsed"));
    }

    #[test]
    fn the_window_starts_again_after_a_wait_for_a_reset() {
        // A weekly limit waited out over six days. The resume at its reset is
        // refused with no reset: the first refusal of a new run, not the end
        // of a window that the wait used up.
        let reset = "2026-06-30T19:00:00.000Z";
        let waited = pause(None, "seven_day", Some(reset), now(), JITTER_MS);
        let refused_at = waited.next_retry_at.unwrap() + 15_000;
        assert!(refused_at - waited.first_limited_at > GIVE_UP_AFTER_MS);

        for kind in ["rate_limit", "overloaded", "usage_limit"] {
            let state = pause(Some(&waited), kind, None, refused_at, JITTER_MS);
            assert_eq!(state.attempt, 2, "{kind}");
            assert_eq!(state.gave_up_at, None, "{kind}");
            assert_eq!(state.first_limited_at, refused_at, "{kind}");
            assert!(state.next_retry_at.is_some(), "{kind}");
        }

        // From there the window runs as before: a refusal more than a day
        // into the new run gives up.
        let second = pause(Some(&waited), "rate_limit", None, refused_at, JITTER_MS);
        let late = refused_at + GIVE_UP_AFTER_MS + MINUTE_MS;
        let state = pause(Some(&second), "rate_limit", None, late, JITTER_MS);
        assert_eq!(state.gave_up_at, Some(late));
        assert_eq!(state.reason.as_deref(), Some("retry_window_elapsed"));
    }

    #[test]
    fn a_wait_for_a_reset_still_counts_toward_the_attempt_cap() {
        let reset = "2026-06-30T19:00:00.000Z";
        let out_of_attempts = RateLimitAutoRecovery {
            attempt: MAX_AUTO_RETRY_ATTEMPTS,
            ..pause(None, "rate_limit", None, now(), JITTER_MS)
        };

        let state = pause(
            Some(&out_of_attempts),
            "seven_day",
            Some(reset),
            now() + MINUTE_MS,
            JITTER_MS,
        );

        assert_eq!(state.gave_up_at, Some(now() + MINUTE_MS));
        assert_eq!(state.reason.as_deref(), Some("retry_cap_exceeded"));
    }
}
