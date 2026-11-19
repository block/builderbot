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
/// records none.
pub(super) fn decide_failed_run(
    session: Option<&Session>,
    fallback_provider: Option<&str>,
    driver_error: &str,
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
        classify_network_failure(provider, driver_error, provider_metadata)
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

        let decision = decide_failed_run(Some(&session), None, &driver_error(DNS_MESSAGE), NOW_MS);

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

        let decision = decide_failed_run(Some(&row), None, &driver_error(DNS_MESSAGE), NOW_MS);
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

    #[test]
    fn a_pipeline_session_with_the_same_error_fails_as_crashed() {
        for kind in [PipelineKind::Push, PipelineKind::Pull] {
            let mut session = session_with_dns_failure();
            session.pipeline = Some(PipelineExecution::from_steps(&[]).with_kind(kind));

            let decision =
                decide_failed_run(Some(&session), None, &driver_error(DNS_MESSAGE), NOW_MS);

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

        let with_fallback = decide_failed_run(Some(&session), Some("claude"), &error, NOW_MS);
        assert_eq!(with_fallback.status(), "paused");

        // Codex is never classified, and neither is an unknown agent.
        for provider in [Some("codex"), None] {
            let decision = decide_failed_run(Some(&session), provider, &error, NOW_MS);
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

    #[test]
    fn other_failures_still_fail_the_session() {
        // A real 5xx: the same structured kind, no network token.
        let mut session = session_with_dns_failure();
        let message = "Internal error: API Error: 500 Internal server error";
        session.provider_metadata = Some(prompt_error_metadata("server_error", message));
        let decision = decide_failed_run(Some(&session), None, &driver_error(message), NOW_MS);
        assert_eq!(decision.status(), "error");
        assert_eq!(decision.completion_reason, CompletionReason::Crashed);

        // A row that could not be read has nothing to classify.
        let decision = decide_failed_run(None, Some("claude"), &driver_error(DNS_MESSAGE), NOW_MS);
        assert_eq!(decision.status(), "error");
        assert_eq!(decision.completion_reason, CompletionReason::Crashed);

        // A lost remote agent keeps its own reason.
        let decision = decide_failed_run(
            None,
            Some("claude"),
            "ACP protocol failed: server shut down unexpectedly",
            NOW_MS,
        );
        assert_eq!(decision.status(), "error");
        assert_eq!(
            decision.completion_reason,
            failed_run_completion_reason("ACP protocol failed: server shut down unexpectedly")
        );
    }
}
