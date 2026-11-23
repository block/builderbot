//! How a run that ended in an agent error is recorded.
//!
//! Three outcomes, tried in this order:
//!
//! 1. **A provider limit** pauses the session until the limit resets.
//! 2. **A network failure** pauses it too, under the same pause with the
//!    kind [`NETWORK_PAUSE_KIND`], so the scheduler resumes it. Pipeline
//!    sessions are left out: a push or pull that fails on the network fails.
//! 3. **Anything else** fails the session, as a lost remote agent when that
//!    is what the error says.
//!
//! The limit check runs first so a limit is never taken for the network.

use super::network_failure::{classify_network_failure, NetworkFailure};
use super::{
    completion_reason_for_driver_error, failed_run_completion_reason, rate_limit_recovery_details,
    RateLimitRecovery,
};
use crate::rate_limit_scheduler::NETWORK_PAUSE_KIND;
use crate::store::{CompletionReason, Session};

/// What [`decide_failed_run`] concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FailedRunDecision {
    pub(super) completion_reason: CompletionReason,
    /// Set exactly when the session is to be paused for an automatic resume.
    pub(super) recovery: Option<RateLimitRecovery>,
    /// The signal that marked the failure as the network, when it paused the
    /// session for that reason.
    pub(super) network_failure: Option<NetworkFailure>,
}

impl FailedRunDecision {
    /// The session status this decision is written as.
    pub(super) fn status(&self) -> &'static str {
        if self.recovery.is_some() {
            "paused"
        } else {
            "error"
        }
    }
}

/// Decide how a run that failed with `driver_error` ends.
///
/// `session` is the row as it stands after the run, when it could be read;
/// `fallback_provider` is the run's configured provider, for a row that
/// records none; `prompt_failed` is the driver's flag that the agent answered
/// the prompt with an error (`acp_client::SessionSettled::prompt_failed`).
pub(super) fn decide_failed_run(
    session: Option<&Session>,
    fallback_provider: Option<&str>,
    driver_error: &str,
    prompt_failed: bool,
    now_millis: i64,
) -> FailedRunDecision {
    let provider_metadata = session.and_then(|session| session.provider_metadata.as_ref());
    if completion_reason_for_driver_error(driver_error, provider_metadata)
        == CompletionReason::RateLimited
    {
        return FailedRunDecision {
            completion_reason: CompletionReason::RateLimited,
            recovery: Some(rate_limit_recovery_details(
                driver_error,
                provider_metadata,
                now_millis,
            )),
            network_failure: None,
        };
    }

    let provider = session
        .and_then(|session| session.provider.as_deref())
        .or(fallback_provider);
    let is_pipeline = session.is_some_and(|session| session.pipeline.is_some());
    let network_failure = if is_pipeline {
        None
    } else {
        classify_network_failure(provider, prompt_failed, provider_metadata)
    };
    if network_failure.is_some() {
        return FailedRunDecision {
            // The pause shared with provider limits; the kind tells them
            // apart. No reset time: nothing says when the network returns.
            completion_reason: CompletionReason::RateLimited,
            recovery: Some(RateLimitRecovery {
                resets_at: None,
                kind: Some(NETWORK_PAUSE_KIND.to_string()),
            }),
            network_failure,
        };
    }

    FailedRunDecision {
        completion_reason: failed_run_completion_reason(driver_error),
        recovery: None,
        network_failure: None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::store::{PipelineExecution, PipelineKind, SessionStatus, Store};

    const NOW_MS: i64 = 1_794_470_400_000;
    const DNS_MESSAGE: &str = "Internal error: API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)";

    /// The driver error as the runner receives it: the prompt failure wrapped
    /// once by the connection that carried it.
    fn driver_error(message: &str) -> String {
        let inner = format!(
            r#"Prompt failed: Error {{ code: -32603: Internal error, message: {message:?}, data: None }}"#
        );
        format!(
            r#"ACP protocol failed: Error {{ code: -32603: Internal error, message: "Internal error", data: Some(String({inner:?})) }}"#
        )
    }

    fn prompt_error_metadata(kind: &str, message: &str) -> serde_json::Value {
        json!({ "acp": { "promptError": {
            "code": -32603,
            "message": message,
            "data": { "errorKind": kind },
        } } })
    }

    /// A Claude session whose prompt failed on a DNS lookup.
    fn session_with_dns_failure() -> Session {
        let mut session = Session::new_running("work", Path::new("/tmp")).with_provider("claude");
        session.provider_metadata = Some(prompt_error_metadata("server_error", DNS_MESSAGE));
        session
    }

    #[test]
    fn a_network_prompt_error_pauses_with_the_network_kind_and_no_reset() {
        let session = session_with_dns_failure();

        let decision = decide_failed_run(
            Some(&session),
            None,
            &driver_error(DNS_MESSAGE),
            true,
            NOW_MS,
        );

        assert_eq!(decision.status(), "paused");
        assert_eq!(decision.completion_reason, CompletionReason::RateLimited);
        assert_eq!(
            decision.recovery,
            Some(RateLimitRecovery {
                resets_at: None,
                kind: Some("network".to_string()),
            })
        );
        assert_eq!(
            decision.network_failure,
            Some(NetworkFailure::ClaudeErrno("ENOTFOUND"))
        );
    }

    /// The decision carried through the write `finish_session_run` makes with
    /// it: the row is parked for the scheduler with kind `network`.
    #[test]
    fn a_network_prompt_error_parks_the_row_with_kind_network() {
        let store = Store::in_memory().unwrap();
        let session = session_with_dns_failure().with_agent("agent-1");
        store.create_session(&session).unwrap();
        let row = store.get_session(&session.id).unwrap().unwrap();

        let decision =
            decide_failed_run(Some(&row), None, &driver_error(DNS_MESSAGE), true, NOW_MS);
        let recovery = decision.recovery.expect("a network failure pauses");
        assert!(crate::rate_limit_scheduler::pause_for_rate_limit(
            &store,
            None,
            &session.id,
            Some("Can't reach the API server"),
            recovery.resets_at.as_deref(),
            recovery.kind.as_deref(),
            NOW_MS,
        )
        .unwrap());

        let parked = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(parked.status, SessionStatus::Paused);
        assert_eq!(
            parked.completion_reason,
            Some(CompletionReason::RateLimited)
        );
        assert_eq!(parked.rate_limit_kind.as_deref(), Some("network"));
        assert_eq!(parked.rate_limit_resets_at, None);
        let listed = store.get_rate_limited_paused_sessions().unwrap();
        assert_eq!(listed.len(), 1, "the scheduler lists the row");
        let bookkeeping = crate::rate_limit_scheduler::auto_recovery_from_metadata(
            parked.provider_metadata.as_ref(),
        )
        .expect("the pause writes retry bookkeeping");
        // With the network gate not open, the pause spends no attempt and
        // waits for the gate.
        assert_eq!(bookkeeping.attempt, 0);
        assert_eq!(bookkeeping.next_retry_at, Some(NOW_MS));
        assert_eq!(bookkeeping.reason.as_deref(), Some("network_wait"));
        assert!(!bookkeeping.unsafe_to_auto_resume);
    }

    const LIMIT_MESSAGE: &str = "Internal error: You've hit your session limit · resets 11am (UTC)";
    /// 2026-11-12T11:00:00Z in epoch seconds, three hours after [`NOW_MS`].
    const LIMIT_RESETS_AT_SECS: i64 = 1_794_481_200;

    /// A Claude session refused before its first model response: the limit
    /// was already reached when the prompt was sent, as it is on a refused
    /// auto-resume. The bridge sends no `usage_update` for such a turn, so
    /// the row holds the prompt error alone. `rate_limit` is the error's
    /// `data.rateLimit`, which a bridge older than claude-agent-acp #1610
    /// does not send.
    fn session_refused_before_usage(rate_limit: Option<serde_json::Value>) -> Session {
        let mut metadata = prompt_error_metadata("rate_limit", LIMIT_MESSAGE);
        if let Some(rate_limit) = rate_limit {
            metadata["acp"]["promptError"]["data"]["rateLimit"] = rate_limit;
        }
        let mut session = Session::new_running("work", Path::new("/tmp")).with_provider("claude");
        session.provider_metadata = Some(metadata);
        session
    }

    fn rejected_five_hour_limit() -> serde_json::Value {
        json!({
            "status": "rejected",
            "resetsAt": LIMIT_RESETS_AT_SECS,
            "rateLimitType": "five_hour",
            "overageStatus": "rejected",
            "overageDisabledReason": "org_level_disabled",
            "isUsingOverage": false,
        })
    }

    #[test]
    fn a_turn_refused_before_usage_pauses_until_the_reset_in_its_prompt_error() {
        let session = session_refused_before_usage(Some(rejected_five_hour_limit()));

        let decision = decide_failed_run(
            Some(&session),
            None,
            &driver_error(LIMIT_MESSAGE),
            true,
            NOW_MS,
        );

        assert_eq!(decision.status(), "paused");
        assert_eq!(decision.completion_reason, CompletionReason::RateLimited);
        assert_eq!(
            decision.recovery,
            Some(RateLimitRecovery {
                resets_at: Some("2026-11-12T11:00:00.000Z".to_string()),
                kind: Some("five_hour".to_string()),
            })
        );
        assert_eq!(decision.network_failure, None);
    }

    /// The decision carried through the write `finish_session_run` makes with
    /// it: the retry is scheduled for the reset, not on the backoff.
    #[test]
    fn a_turn_refused_before_usage_parks_the_row_for_its_reset() {
        let store = Store::in_memory().unwrap();
        let session =
            session_refused_before_usage(Some(rejected_five_hour_limit())).with_agent("agent-1");
        store.create_session(&session).unwrap();
        let row = store.get_session(&session.id).unwrap().unwrap();

        let decision =
            decide_failed_run(Some(&row), None, &driver_error(LIMIT_MESSAGE), true, NOW_MS);
        let recovery = decision.recovery.expect("a usage limit pauses");
        assert!(crate::rate_limit_scheduler::pause_for_rate_limit(
            &store,
            None,
            &session.id,
            Some("You've hit your session limit · resets 11am (UTC)"),
            recovery.resets_at.as_deref(),
            recovery.kind.as_deref(),
            NOW_MS,
        )
        .unwrap());

        let parked = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(parked.status, SessionStatus::Paused);
        assert_eq!(parked.rate_limit_kind.as_deref(), Some("five_hour"));
        assert_eq!(
            parked.rate_limit_resets_at.as_deref(),
            Some("2026-11-12T11:00:00.000Z")
        );
        let bookkeeping = crate::rate_limit_scheduler::auto_recovery_from_metadata(
            parked.provider_metadata.as_ref(),
        )
        .expect("the pause writes retry bookkeeping");
        assert_eq!(bookkeeping.attempt, 1);
        assert_eq!(bookkeeping.reason.as_deref(), Some("reset_at"));
        let retry_at = bookkeeping.next_retry_at.expect("a retry is scheduled");
        assert!(
            retry_at > LIMIT_RESETS_AT_SECS * 1_000,
            "the retry waits for the reset: {retry_at}"
        );
    }

    /// Park `session` the way `finish_session_run` does with the decision
    /// for `message`, and return the retry bookkeeping the pause wrote.
    fn park_failed_run(
        store: &Store,
        session: &Session,
        message: &str,
    ) -> crate::rate_limit_scheduler::RateLimitAutoRecovery {
        store.create_session(session).unwrap();
        let row = store.get_session(&session.id).unwrap().unwrap();
        let decision = decide_failed_run(Some(&row), None, &driver_error(message), true, NOW_MS);
        let recovery = decision.recovery.expect("the failure pauses");
        assert!(crate::rate_limit_scheduler::pause_for_rate_limit(
            store,
            None,
            &session.id,
            Some(message),
            recovery.resets_at.as_deref(),
            recovery.kind.as_deref(),
            NOW_MS,
        )
        .unwrap());
        let parked = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(parked.status, SessionStatus::Paused);
        assert_eq!(parked.rate_limit_kind, recovery.kind);
        assert_eq!(parked.rate_limit_resets_at, recovery.resets_at);
        crate::rate_limit_scheduler::auto_recovery_from_metadata(parked.provider_metadata.as_ref())
            .expect("the pause writes retry bookkeeping")
    }

    const MINUTE_MS: i64 = 60_000;

    /// The subscription case with no structured reset: a bridge older than
    /// claude-agent-acp #1610 sends the error kind and the message, nothing
    /// else. The wording names it a usage limit.
    #[test]
    fn a_turn_refused_before_usage_with_no_reset_is_named_a_usage_limit() {
        let session = session_refused_before_usage(None);

        let decision = decide_failed_run(
            Some(&session),
            None,
            &driver_error(LIMIT_MESSAGE),
            true,
            NOW_MS,
        );

        assert_eq!(decision.status(), "paused");
        assert_eq!(
            decision.recovery,
            Some(RateLimitRecovery {
                resets_at: None,
                kind: Some("usage_limit".to_string()),
            })
        );
    }

    /// And it is parked on the slow schedule, not the minute-scale backoff
    /// that used to spend its attempts long before the limit reset.
    #[test]
    fn a_usage_limit_with_no_reset_parks_the_row_on_the_slow_schedule() {
        let store = Store::in_memory().unwrap();
        let session = session_refused_before_usage(None).with_agent("agent-1");

        let bookkeeping = park_failed_run(&store, &session, LIMIT_MESSAGE);

        assert_eq!(bookkeeping.attempt, 1);
        assert_eq!(bookkeeping.reason.as_deref(), Some("usage_limit_backoff"));
        let retry_at = bookkeeping.next_retry_at.expect("a retry is scheduled");
        let waits = (NOW_MS + 40 * MINUTE_MS)..=(NOW_MS + 50 * MINUTE_MS);
        assert!(waits.contains(&retry_at), "{}", retry_at - NOW_MS);
    }

    /// 2026-11-18T08:00:00Z in epoch seconds, six days after [`NOW_MS`].
    const WEEKLY_RESETS_AT_SECS: i64 = 1_794_988_800;
    const WEEKLY_LIMIT_MESSAGE: &str =
        "Internal error: You've hit your weekly limit · resets Nov 18, 8am (UTC)";

    /// A weekly limit: the reset is days ahead, further than a reset used to
    /// be trusted for, and the row waits for it.
    #[test]
    fn a_weekly_limit_parks_the_row_for_its_reset_days_ahead() {
        let store = Store::in_memory().unwrap();
        let mut session = session_refused_before_usage(Some(json!({
            "status": "rejected",
            "resetsAt": WEEKLY_RESETS_AT_SECS,
            "rateLimitType": "seven_day",
            "overageStatus": "rejected",
            "overageDisabledReason": "org_level_disabled",
            "isUsingOverage": false,
        })))
        .with_agent("agent-1");
        session.provider_metadata.as_mut().unwrap()["acp"]["promptError"]["message"] =
            json!(WEEKLY_LIMIT_MESSAGE);

        let bookkeeping = park_failed_run(&store, &session, WEEKLY_LIMIT_MESSAGE);

        let parked = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(parked.rate_limit_kind.as_deref(), Some("seven_day"));
        assert_eq!(
            parked.rate_limit_resets_at.as_deref(),
            Some("2026-11-18T08:00:00.000Z")
        );
        assert_eq!(bookkeeping.attempt, 1);
        assert_eq!(bookkeeping.reason.as_deref(), Some("reset_at"));
        let retry_at = bookkeeping.next_retry_at.expect("a retry is scheduled");
        let reset_at = WEEKLY_RESETS_AT_SECS * 1_000;
        assert!(
            (reset_at..=reset_at + 6 * MINUTE_MS).contains(&retry_at),
            "the retry follows the reset: {retry_at}"
        );
    }

    /// API-style failures keep the minute-scale backoff: a 429 and an
    /// overloaded provider clear in seconds to minutes. A usage update from
    /// earlier in the turn changes nothing, whether it was sent while a
    /// subscription window was still open or for an account on extra usage,
    /// whose weekly window stays `rejected` while its turns run.
    #[test]
    fn api_rate_limits_and_overloads_park_the_row_on_the_minute_backoff() {
        const API_429: &str = r#"Internal error: API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed your organization's rate limit of 400,000 input tokens per minute."}}"#;
        const OVERLOADED: &str = r#"Internal error: API Error: 529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let open_window = json!({
            "sessionUpdate": "usage_update",
            "used": 51_234,
            "size": 200_000,
            "_meta": { "_claude/rateLimit": {
                "status": "allowed_warning",
                "resetsAt": LIMIT_RESETS_AT_SECS,
                "rateLimitType": "five_hour",
                "utilization": 0.93,
            } },
        });

        let on_extra_usage = json!({
            "sessionUpdate": "usage_update",
            "used": 51_234,
            "size": 200_000,
            "_meta": { "_claude/rateLimit": {
                "status": "rejected",
                "resetsAt": WEEKLY_RESETS_AT_SECS,
                "rateLimitType": "seven_day",
                "overageStatus": "allowed",
                "isUsingOverage": true,
            } },
        });

        for (error_kind, message) in [("rate_limit", API_429), ("overloaded", OVERLOADED)] {
            for usage_update in [None, Some(&open_window), Some(&on_extra_usage)] {
                let store = Store::in_memory().unwrap();
                let mut metadata = prompt_error_metadata(error_kind, message);
                if let Some(usage_update) = usage_update {
                    metadata["acp"]["usageUpdate"] = usage_update.clone();
                }
                let mut session = Session::new_running("work", Path::new("/tmp"))
                    .with_provider("claude")
                    .with_agent("agent-1");
                session.provider_metadata = Some(metadata);

                let bookkeeping = park_failed_run(&store, &session, message);

                let parked = store.get_session(&session.id).unwrap().unwrap();
                assert_eq!(parked.rate_limit_kind.as_deref(), Some(error_kind));
                assert_eq!(parked.rate_limit_resets_at, None, "{error_kind}");
                assert_eq!(bookkeeping.attempt, 1);
                assert_eq!(bookkeeping.reason.as_deref(), Some("backoff"));
                let retry_at = bookkeeping.next_retry_at.expect("a retry is scheduled");
                assert!(
                    (NOW_MS..=NOW_MS + MINUTE_MS).contains(&retry_at),
                    "{error_kind}: {}",
                    retry_at - NOW_MS
                );
            }
        }

        // An overloaded prompt takes nothing from a usage update that says
        // the weekly limit was reached, extra usage or not.
        let store = Store::in_memory().unwrap();
        let mut metadata = prompt_error_metadata("overloaded", OVERLOADED);
        metadata["acp"]["usageUpdate"] = on_extra_usage.clone();
        metadata["acp"]["usageUpdate"]["_meta"]["_claude/rateLimit"]["isUsingOverage"] =
            json!(false);
        let mut session = Session::new_running("work", Path::new("/tmp"))
            .with_provider("claude")
            .with_agent("agent-1");
        session.provider_metadata = Some(metadata);

        let bookkeeping = park_failed_run(&store, &session, OVERLOADED);

        let parked = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(parked.rate_limit_kind.as_deref(), Some("overloaded"));
        assert_eq!(parked.rate_limit_resets_at, None);
        assert_eq!(bookkeeping.reason.as_deref(), Some("backoff"));
        let retry_at = bookkeeping.next_retry_at.expect("a retry is scheduled");
        assert!((NOW_MS..=NOW_MS + MINUTE_MS).contains(&retry_at));
    }

    #[test]
    fn a_pipeline_session_with_the_same_error_fails_as_crashed() {
        for kind in [PipelineKind::Push, PipelineKind::Pull] {
            let mut session = session_with_dns_failure();
            session.pipeline = Some(PipelineExecution::from_steps(&[]).with_kind(kind));

            let decision = decide_failed_run(
                Some(&session),
                None,
                &driver_error(DNS_MESSAGE),
                true,
                NOW_MS,
            );

            assert_eq!(decision.status(), "error");
            assert_eq!(decision.completion_reason, CompletionReason::Crashed);
            assert_eq!(decision.recovery, None);
            assert_eq!(decision.network_failure, None);
        }
    }

    #[test]
    fn the_configured_provider_stands_in_for_a_row_that_records_none() {
        let mut session = session_with_dns_failure();
        session.provider = None;
        let error = driver_error(DNS_MESSAGE);

        let with_fallback = decide_failed_run(Some(&session), Some("claude"), &error, true, NOW_MS);
        assert_eq!(with_fallback.status(), "paused");

        // Codex is never classified, and neither is an unknown agent.
        for provider in [Some("codex"), None] {
            let decision = decide_failed_run(Some(&session), provider, &error, true, NOW_MS);
            assert_eq!(decision.status(), "error", "{provider:?}");
            assert_eq!(decision.completion_reason, CompletionReason::Crashed);
        }
    }

    /// The limit check runs first and keeps its own kind and reset handling.
    #[test]
    fn a_rate_limit_is_still_a_rate_limit() {
        let mut session = session_with_dns_failure();
        session.provider_metadata = Some(prompt_error_metadata(
            "rate_limit",
            "Internal error: rate limit exceeded",
        ));

        let decision = decide_failed_run(
            Some(&session),
            None,
            &driver_error("Internal error: rate limit exceeded"),
            true,
            NOW_MS,
        );

        assert_eq!(decision.status(), "paused");
        assert_eq!(decision.completion_reason, CompletionReason::RateLimited);
        assert_eq!(
            decision.recovery.and_then(|recovery| recovery.kind),
            Some("rate_limit".to_string())
        );
        assert_eq!(decision.network_failure, None);
    }

    /// The same row and error text, but the driver did not report a failed
    /// prompt: the stored `promptError` is stale and the run fails.
    #[test]
    fn a_run_that_did_not_fail_on_the_prompt_is_not_paused() {
        let session = session_with_dns_failure();

        let decision = decide_failed_run(
            Some(&session),
            None,
            &driver_error(DNS_MESSAGE),
            false,
            NOW_MS,
        );

        assert_eq!(decision.status(), "error");
        assert_eq!(decision.completion_reason, CompletionReason::Crashed);
        assert_eq!(decision.network_failure, None);
    }

    #[test]
    fn other_failures_still_fail_the_session() {
        // A real 5xx: the same structured kind, no network token.
        let mut session = session_with_dns_failure();
        let message = "Internal error: API Error: 500 Internal server error";
        session.provider_metadata = Some(prompt_error_metadata("server_error", message));
        let decision =
            decide_failed_run(Some(&session), None, &driver_error(message), true, NOW_MS);
        assert_eq!(decision.status(), "error");
        assert_eq!(decision.completion_reason, CompletionReason::Crashed);

        // A row that could not be read has nothing to classify.
        let decision = decide_failed_run(
            None,
            Some("claude"),
            &driver_error(DNS_MESSAGE),
            true,
            NOW_MS,
        );
        assert_eq!(decision.status(), "error");
        assert_eq!(decision.completion_reason, CompletionReason::Crashed);

        // A lost remote agent keeps its own reason.
        let decision = decide_failed_run(
            None,
            Some("claude"),
            "ACP protocol failed: server shut down unexpectedly",
            false,
            NOW_MS,
        );
        assert_eq!(decision.status(), "error");
        assert_eq!(
            decision.completion_reason,
            failed_run_completion_reason("ACP protocol failed: server shut down unexpectedly")
        );
    }
}
