//! Structured rate-limit observability.
//!
//! Staged does not have a metrics backend today, so these counters live in
//! process and are emitted as JSON log records with stable event names. The
//! logs give support/debugging flows a grep-friendly event stream while the
//! counters show cumulative state since app startup.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use serde_json::json;

static STATE: OnceLock<Mutex<RateLimitMetricState>> = OnceLock::new();

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitMetricSnapshot {
    counters: BTreeMap<String, u64>,
    rate_limit_hits_by_agent_kind: BTreeMap<String, u64>,
    max_scheduled_resume_backlog: usize,
}

#[derive(Debug, Default)]
struct RateLimitMetricState {
    snapshot: RateLimitMetricSnapshot,
    /// Last backlog size handed to [`observe_backlog_if_changed`], logged or
    /// not. The scheduler observes on every 15s tick, so an unchanged backlog
    /// (e.g. one permanently parked session) must not log a record per tick.
    last_scheduled_resume_backlog: usize,
}

impl RateLimitMetricState {
    fn increment_counter(&mut self, name: &str) {
        *self.snapshot.counters.entry(name.to_string()).or_default() += 1;
    }

    fn record_hit(&mut self, provider: &str, kind: &str) {
        self.increment_counter("rate_limit_hits_total");
        *self
            .snapshot
            .rate_limit_hits_by_agent_kind
            .entry(format!("{provider}/{kind}"))
            .or_default() += 1;
    }

    /// Record the backlog and return a snapshot to log, or `None` when the
    /// size is unchanged since the last observation.
    fn observe_backlog_if_changed(
        &mut self,
        actionable_sessions: usize,
    ) -> Option<RateLimitMetricSnapshot> {
        if actionable_sessions == self.last_scheduled_resume_backlog {
            return None;
        }
        self.last_scheduled_resume_backlog = actionable_sessions;
        self.increment_counter("scheduled_resume_backlog_observations_total");
        self.snapshot.max_scheduled_resume_backlog = self
            .snapshot
            .max_scheduled_resume_backlog
            .max(actionable_sessions);
        Some(self.snapshot.clone())
    }
}

pub(crate) fn record_rate_limit_hit(
    session_id: &str,
    provider: Option<&str>,
    kind: Option<&str>,
    reset_known: bool,
) {
    let provider = metric_label(provider);
    let kind = metric_label(kind);
    let snapshot = with_state(|state| {
        state.record_hit(&provider, &kind);
        state.snapshot.clone()
    });

    log_metric(
        "rate_limit_hit",
        json!({
            "sessionId": session_id,
            "provider": provider,
            "kind": kind,
            "resetKnown": reset_known,
        }),
        snapshot,
    );
}

/// Observe the count of paused sessions the scheduler can still act on.
///
/// Sessions that gave up or need a manual resume are excluded by the caller,
/// and only a change in the count emits a log record — the size is otherwise
/// re-observed every tick, and a stable backlog (including its drop back to
/// zero) is one event, not thousands.
pub(crate) fn record_scheduled_resume_backlog(actionable_sessions: usize) {
    let Some(snapshot) = with_state(|state| state.observe_backlog_if_changed(actionable_sessions))
    else {
        return;
    };

    log_metric(
        "scheduled_resume_backlog",
        json!({
            "pausedSessions": actionable_sessions,
        }),
        snapshot,
    );
}

pub(crate) fn record_recovery_scheduled(
    session_id: &str,
    provider: Option<&str>,
    kind: Option<&str>,
    attempt: u32,
    max_attempts: u32,
    next_retry_at: i64,
    reason: Option<&str>,
) {
    let snapshot = with_state(|state| {
        state.increment_counter("scheduled_resumes_total");
        state.snapshot.clone()
    });

    log_metric(
        "recovery_scheduled",
        json!({
            "sessionId": session_id,
            "provider": metric_label(provider),
            "kind": metric_label(kind),
            "attempt": attempt,
            "maxAttempts": max_attempts,
            "nextRetryAt": next_retry_at,
            "reason": metric_label(reason),
        }),
        snapshot,
    );
}

pub(crate) fn record_recovery_retry_attempt(
    session_id: &str,
    provider: Option<&str>,
    kind: Option<&str>,
    attempt: u32,
    max_attempts: u32,
) {
    let snapshot = with_state(|state| {
        state.increment_counter("retry_attempts_total");
        state.snapshot.clone()
    });

    log_metric(
        "recovery_retry_attempt",
        json!({
            "sessionId": session_id,
            "provider": metric_label(provider),
            "kind": metric_label(kind),
            "attempt": attempt,
            "maxAttempts": max_attempts,
        }),
        snapshot,
    );
}

pub(crate) fn record_recovery_give_up(
    session_id: &str,
    provider: Option<&str>,
    kind: Option<&str>,
    attempt: u32,
    reason: Option<&str>,
) {
    let snapshot = with_state(|state| {
        state.increment_counter("give_ups_total");
        state.snapshot.clone()
    });

    log_metric(
        "recovery_give_up",
        json!({
            "sessionId": session_id,
            "provider": metric_label(provider),
            "kind": metric_label(kind),
            "attempt": attempt,
            "reason": metric_label(reason),
        }),
        snapshot,
    );
}

pub(crate) fn record_recovery_manual_required(
    session_id: &str,
    provider: Option<&str>,
    kind: Option<&str>,
    reason: Option<&str>,
) {
    let snapshot = with_state(|state| {
        state.increment_counter("manual_resume_required_total");
        state.snapshot.clone()
    });

    log_metric(
        "recovery_manual_required",
        json!({
            "sessionId": session_id,
            "provider": metric_label(provider),
            "kind": metric_label(kind),
            "reason": metric_label(reason),
        }),
        snapshot,
    );
}

pub(crate) fn record_recovery_start_failed(session_id: &str, reason: &str) {
    let snapshot = with_state(|state| {
        state.increment_counter("resume_start_failures_total");
        state.snapshot.clone()
    });

    log_metric(
        "recovery_start_failed",
        json!({
            "sessionId": session_id,
            "reason": reason,
        }),
        snapshot,
    );
}

fn with_state<T>(f: impl FnOnce(&mut RateLimitMetricState) -> T) -> T {
    let mut state = STATE
        .get_or_init(|| Mutex::new(RateLimitMetricState::default()))
        .lock()
        .unwrap();
    f(&mut state)
}

fn metric_label(value: Option<&str>) -> String {
    let value = value.unwrap_or("unknown").trim();
    if value.is_empty() {
        return "unknown".to_string();
    }

    value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' => ch,
            _ => '_',
        })
        .collect()
}

fn log_metric(event: &str, fields: serde_json::Value, snapshot: RateLimitMetricSnapshot) {
    let payload = json!({
        "metric": "staged.rate_limit",
        "event": event,
        "fields": fields,
        "snapshot": snapshot,
    });
    log::info!(target: "staged::rate_limit_metrics", "{payload}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_stable_for_metric_keys() {
        assert_eq!(metric_label(None), "unknown");
        assert_eq!(metric_label(Some(" claude pro ")), "claude_pro");
        assert_eq!(metric_label(Some("rate-limit")), "rate-limit");
    }

    #[test]
    fn backlog_observations_log_only_on_change() {
        let mut state = RateLimitMetricState::default();

        // Startup with nothing paused: nothing worth logging.
        assert!(state.observe_backlog_if_changed(0).is_none());

        let snapshot = state.observe_backlog_if_changed(2).unwrap();
        assert_eq!(snapshot.max_scheduled_resume_backlog, 2);

        // A stable backlog re-observed every tick is one event, not one per tick.
        assert!(state.observe_backlog_if_changed(2).is_none());
        assert!(state.observe_backlog_if_changed(2).is_none());

        // Draining back to zero is a change and logs once.
        let snapshot = state.observe_backlog_if_changed(0).unwrap();
        assert_eq!(snapshot.max_scheduled_resume_backlog, 2);
        assert!(state.observe_backlog_if_changed(0).is_none());
    }

    #[test]
    fn state_counts_hits_by_provider_and_kind() {
        let mut state = RateLimitMetricState::default();

        state.record_hit("claude", "subscription");
        state.record_hit("claude", "subscription");
        state.record_hit("codex", "usage_limit");

        assert_eq!(
            state
                .snapshot
                .counters
                .get("rate_limit_hits_total")
                .copied(),
            Some(3)
        );
        assert_eq!(
            state
                .snapshot
                .rate_limit_hits_by_agent_kind
                .get("claude/subscription")
                .copied(),
            Some(2)
        );
        assert_eq!(
            state
                .snapshot
                .rate_limit_hits_by_agent_kind
                .get("codex/usage_limit")
                .copied(),
            Some(1)
        );
    }
}
