//! Server-side completion side effects for pipeline (pr/push) sessions.
//!
//! When a PR or push session reaches its terminal transition, the session
//! runner calls [`run_completion_side_effects`] from whichever thread won the
//! `transition_from_running` write. It parses the session's outcome (PR URL
//! from the transcript / pipeline step outputs, push result from the
//! non-fast-forward markers), persists it (branch PR number, cleared PR
//! status), and emits the `pr-created` / `push-completed` domain events —
//! before the terminal `session-status-changed` event, so clients can render
//! outcomes from the ordered event stream alone.
//!
//! This used to live in the frontend completion handlers, where every
//! connected client raced to perform the same writes; frontends are now
//! idempotent renderers of these events.

use std::sync::Arc;

use serde::Serialize;
use tauri::AppHandle;

use crate::store::{
    MessageRole, PipelineExecution, Session, SessionMessage, SessionStatus, StepStatus, Store,
};

// =============================================================================
// Domain events
// =============================================================================

/// Emitted when a completed PR session produced a pull request.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrCreatedEvent {
    pub branch_id: String,
    pub session_id: String,
    pub pr_url: String,
    pub pr_number: u64,
}

/// Emitted when a push session completes, carrying the classified outcome.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushCompletedEvent {
    pub branch_id: String,
    pub session_id: String,
    pub outcome: PushOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PushOutcome {
    Succeeded,
    RejectedNonFastForward,
}

/// What a completed pr/push session resolved to. Pure decision value so the
/// parsing/classification can be tested without an app handle.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CompletionEffect {
    PrCreated { pr_url: String, pr_number: u64 },
    PrUrlMissing,
    PushCompleted { outcome: PushOutcome },
}

// =============================================================================
// Entry point
// =============================================================================

/// Run completion side effects for a session that just reached a terminal
/// state.
///
/// Callers must only invoke this after winning the `transition_from_running`
/// write, so exactly one thread performs the side effects, and before emitting
/// the terminal `session-status-changed` event, so the domain events reach
/// clients first.
///
/// Only sessions launched via a pipeline are considered (pr/push sessions are
/// always pipeline sessions); their kind is inferred from the stored prompt
/// exactly like the resume path and busy-state snapshot do. Non-completed
/// terminal states have no outcome to parse — clients render those directly
/// from the terminal status event.
///
/// Outcomes are delivered at most once per session, recorded by the
/// `completion_effects_at` marker: a resumed pr/push session completes again,
/// but its pipeline and transcript still describe the *original* run, so
/// re-evaluating them would re-emit `pr-created` or — destructively — re-clear
/// the branch's PR status. See [`pending_completion_effect`] for what the
/// marker does and doesn't claim.
pub fn run_completion_side_effects(
    store: &Arc<Store>,
    app_handle: &AppHandle,
    session_id: &str,
    branch_id: Option<&str>,
    status: &str,
) {
    // Cheap pre-check so ordinary terminal transitions don't read the session
    // row; `pending_completion_effect` re-applies the gate as part of the
    // decision it owns.
    if status != "completed" {
        return;
    }

    let session = match store.get_session(session_id) {
        Ok(Some(session)) => session,
        Ok(None) => return,
        Err(e) => {
            log::error!("Completion side effects: failed to load session {session_id}: {e}");
            return;
        }
    };
    let Some((kind, effect)) = pending_completion_effect(&session, status, || {
        store.get_session_messages(session_id).unwrap_or_default()
    }) else {
        return;
    };

    let Some(branch_id) = branch_id
        .map(str::to_string)
        .or_else(|| session.branch_id.clone())
    else {
        log::warn!(
            "Completion side effects: {kind} session {session_id} has no branch attribution"
        );
        return;
    };

    if should_record_effects(&effect) {
        // Mark *before* emitting/persisting. Dying in between loses one
        // emission, which existing recovery already covers (`recover_branch_pr`
        // re-derives PR numbers, the PR refresh loop repopulates status,
        // clients keep a read-only fallback classification). Dying the other
        // way round would let a later resume replay the destructive push
        // re-clear — the bug this marker exists to prevent. A failed marker
        // write is logged but doesn't suppress the events: that leaves today's
        // status quo rather than dropping a real outcome.
        if let Err(e) = store.mark_completion_effects_ran(session_id) {
            log::error!(
                "Failed to mark completion effects for {kind} session {session_id} as delivered: {e}"
            );
        }
    }

    match effect {
        CompletionEffect::PrCreated { pr_url, pr_number } => {
            // Persist first so any refresh triggered by the event finds the
            // number. A failed write is logged but doesn't suppress the event:
            // the PR exists on GitHub, and `recover_branch_pr` re-derives the
            // number later.
            if let Err(e) = store.update_branch_pr_number(&branch_id, Some(pr_number)) {
                log::error!(
                    "Failed to persist PR #{pr_number} for branch {branch_id} after session {session_id}: {e}"
                );
            }
            crate::web_server::emit_to_all(
                app_handle,
                "pr-created",
                PrCreatedEvent {
                    branch_id: branch_id.clone(),
                    session_id: session_id.to_string(),
                    pr_url,
                    pr_number,
                },
            );

            // Fetch checks/mergeability in the background; results arrive via
            // the existing `pr-status-changed` event.
            let store = Arc::clone(store);
            let app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) =
                    crate::prs::refresh_pr_status_impl(store, app_handle, branch_id.clone()).await
                {
                    log::warn!(
                        "Failed to refresh PR status for branch {branch_id} after PR creation: {e}"
                    );
                }
            });
        }
        CompletionEffect::PrUrlMissing => {
            // No event: clients infer "completed but no PR URL" from a
            // terminal status event that wasn't preceded by `pr-created`.
            log::warn!("PR session {session_id} completed but no PR URL was found in the output");
        }
        CompletionEffect::PushCompleted { outcome } => {
            if outcome == PushOutcome::Succeeded {
                // The old PR head/checks no longer describe the branch.
                if let Err(e) =
                    crate::prs::clear_branch_pr_status_impl(store, app_handle, &branch_id)
                {
                    log::warn!("Failed to clear PR status for branch {branch_id} after push: {e}");
                }
            }
            crate::web_server::emit_to_all(
                app_handle,
                "push-completed",
                PushCompletedEvent {
                    branch_id,
                    session_id: session_id.to_string(),
                    outcome,
                },
            );
        }
    }
}

/// Decide what outcome, if any, a session that just reached a terminal state
/// still owes its clients.
///
/// Folds every gate — terminal status, the one-shot marker, pipeline
/// provenance, pr/push kind inference — and the outcome evaluation into one
/// pure decision, so the whole thing is testable without an `AppHandle`.
///
/// The marker means "outcome events for this session were delivered once", not
/// "the session finished once": error and cancelled terminal states never reach
/// evaluation (status gate) and never mark, so resuming a pipeline session that
/// failed or was cancelled before it finished its work still fires its outcome
/// on the first real completion.
///
/// `load_messages` is lazy because most completions are ordinary AI sessions
/// that fail the pipeline gate, and there is no reason to read their transcript.
fn pending_completion_effect(
    session: &Session,
    status: &str,
    load_messages: impl FnOnce() -> Vec<SessionMessage>,
) -> Option<(&'static str, CompletionEffect)> {
    if status != SessionStatus::Completed.as_str()
        || session.completion_effects_at.is_some()
        || session.pipeline.is_none()
    {
        return None;
    }
    let kind = crate::session_commands::infer_branch_resume_session_type(&session.prompt)?;
    let effect = evaluate_completed_session(kind, session, &load_messages())?;
    Some((kind, effect))
}

/// Whether an effect counts as "outcomes delivered" for the one-shot marker.
///
/// `PrUrlMissing` emits and persists nothing, and leaving it unmarked preserves
/// the recovery turn: a PR session that completed without producing a URL can
/// be resumed ("you didn't create the PR — do it now"), and the next completion
/// scans the new transcript and fires `pr-created` for real.
fn should_record_effects(effect: &CompletionEffect) -> bool {
    !matches!(effect, CompletionEffect::PrUrlMissing)
}

fn evaluate_completed_session(
    kind: &str,
    session: &Session,
    messages: &[SessionMessage],
) -> Option<CompletionEffect> {
    match kind {
        "pr" => Some(
            extract_pr_url(messages)
                .or_else(|| extract_pr_url_from_pipeline(session.pipeline.as_ref()?))
                .map_or(CompletionEffect::PrUrlMissing, |(pr_url, pr_number)| {
                    CompletionEffect::PrCreated { pr_url, pr_number }
                }),
        ),
        "push" => Some(CompletionEffect::PushCompleted {
            outcome: classify_completed_push_session(session.pipeline.as_ref(), messages),
        }),
        _ => None,
    }
}

// =============================================================================
// PR URL extraction
// =============================================================================

fn pr_url_regex() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"^https://github\.com/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)/pull/(\d+)([/?#].*)?$",
        )
        .unwrap()
    })
}

/// Canonicalize a PR URL candidate, stripping wrapping punctuation
/// (`<url>`, `(url)`, trailing `.` etc.) and any query/fragment suffix.
/// Returns the normalized URL and the PR number.
fn normalize_pr_url(candidate: &str) -> Option<(String, u64)> {
    let trimmed = candidate
        .trim()
        .trim_start_matches(['<', '`', '\'', '"', '[', '('])
        .trim_end_matches(['>', '`', '\'', '"', ']', ')', ',', '.', '?', '!', ';', ':']);
    let captures = pr_url_regex().captures(trimmed)?;
    let number: u64 = captures[3].parse().ok()?;
    Some((
        format!(
            "https://github.com/{}/{}/pull/{}",
            &captures[1], &captures[2], number
        ),
        number,
    ))
}

/// Find the PR URL in a session transcript.
///
/// First pass looks for the explicit `PR_URL: <url>` marker the PR session
/// prompt asks the agent to output; the second pass falls back to any GitHub
/// PR URL in the transcript. Both passes only consider assistant /
/// tool-result messages: a URL in a user message (e.g. pasted into a queued
/// follow-up) is not evidence the session created that PR.
fn extract_pr_url(messages: &[SessionMessage]) -> Option<(String, u64)> {
    static MARKER_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static URL_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let marker_re = MARKER_RE.get_or_init(|| regex::Regex::new(r"PR_URL:\s*(\S+)").unwrap());
    let url_re = URL_RE.get_or_init(|| regex::Regex::new(r"https?://\S+").unwrap());

    for msg in messages {
        if !matches!(msg.role, MessageRole::Assistant | MessageRole::ToolResult) {
            continue;
        }
        if let Some(captures) = marker_re.captures(&msg.content) {
            if let Some(normalized) = normalize_pr_url(&captures[1]) {
                return Some(normalized);
            }
        }
    }

    for msg in messages {
        if !matches!(msg.role, MessageRole::Assistant | MessageRole::ToolResult) {
            continue;
        }
        for candidate in url_re.find_iter(&msg.content) {
            if let Some(normalized) = normalize_pr_url(candidate.as_str()) {
                return Some(normalized);
            }
        }
    }

    None
}

/// Fallback for PR sessions whose pipeline steps produced the URL without an
/// AI handoff (or where the transcript was lost): scan step outputs.
fn extract_pr_url_from_pipeline(pipeline: &PipelineExecution) -> Option<(String, u64)> {
    static STEP_URL_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let step_url_re = STEP_URL_RE.get_or_init(|| {
        regex::Regex::new(r"https://github\.com/[^\s/]+/[^\s/]+/pull/\d+").unwrap()
    });

    pipeline
        .steps
        .iter()
        .filter_map(|step| step.output.as_deref())
        .find_map(|output| {
            step_url_re
                .find(output)
                .and_then(|m| normalize_pr_url(m.as_str()))
        })
}

// =============================================================================
// Push outcome classification
// =============================================================================

fn contains_non_fast_forward_marker(content: &str) -> bool {
    content.contains("PUSH_REJECTED: NON_FAST_FORWARD")
        || content.to_lowercase().contains("non-fast-forward")
}

/// Classify a completed push session.
///
/// The deterministic pipeline is consulted first: a failed step whose output
/// carries the non-fast-forward marker means the push was rejected — unless an
/// AI turn ran afterwards (e.g. a failed `--force-with-lease` whose error
/// happened to mention "non-fast-forward"), in which case the AI handled
/// recovery. When the pipeline is inconclusive, the transcript markers decide;
/// the default is success.
fn classify_completed_push_session(
    pipeline: Option<&PipelineExecution>,
    messages: &[SessionMessage],
) -> PushOutcome {
    if let Some(pipeline) = pipeline {
        let has_non_fast_forward = pipeline.steps.iter().any(|step| {
            step.status == StepStatus::Failed
                && step
                    .output
                    .as_deref()
                    .is_some_and(contains_non_fast_forward_marker)
        });
        if has_non_fast_forward {
            let ai_ran = messages
                .iter()
                .any(|msg| msg.role == MessageRole::Assistant);
            return if ai_ran {
                PushOutcome::Succeeded
            } else {
                PushOutcome::RejectedNonFastForward
            };
        }

        let all_steps_passed_or_skipped = pipeline
            .steps
            .iter()
            .all(|step| matches!(step.status, StepStatus::Succeeded | StepStatus::Skipped));
        if pipeline.completed_without_ai || all_steps_passed_or_skipped {
            return PushOutcome::Succeeded;
        }
    }

    let rejected_in_transcript = messages.iter().any(|msg| {
        matches!(msg.role, MessageRole::Assistant | MessageRole::ToolResult)
            && contains_non_fast_forward_marker(&msg.content)
    });
    if rejected_in_transcript {
        PushOutcome::RejectedNonFastForward
    } else {
        PushOutcome::Succeeded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{PipelineStepStatus, StepType};

    fn message(role: MessageRole, content: &str) -> SessionMessage {
        SessionMessage {
            id: 0,
            session_id: "session-1".to_string(),
            role,
            content: content.to_string(),
            created_at: 0,
            image_ids: vec![],
            acp: Default::default(),
        }
    }

    fn step(status: StepStatus, output: Option<&str>) -> PipelineStepStatus {
        PipelineStepStatus {
            label: "step".to_string(),
            step_type: StepType::Command,
            status,
            output: output.map(str::to_string),
            error: None,
            started_at: None,
            completed_at: None,
        }
    }

    fn pipeline(steps: Vec<PipelineStepStatus>) -> PipelineExecution {
        PipelineExecution {
            kind: None,
            rebase_target: None,
            push_force: false,
            steps,
            current_step: 0,
            completed_without_ai: false,
        }
    }

    fn pr_session(pipeline: PipelineExecution) -> Session {
        let mut session = Session::new_running(
            "Create a pull request for the current branch",
            std::path::Path::new("/tmp"),
        )
        .with_branch("branch-1");
        session.pipeline = Some(pipeline);
        session
    }

    fn push_session(pipeline: PipelineExecution) -> Session {
        let mut session = Session::new_running(
            "Push the current branch to the remote",
            std::path::Path::new("/tmp"),
        )
        .with_branch("branch-1");
        session.pipeline = Some(pipeline);
        session
    }

    fn succeeded_push_pipeline() -> PipelineExecution {
        pipeline(vec![step(StepStatus::Succeeded, Some("pushed"))])
    }

    #[test]
    fn extracts_pr_url_from_marker() {
        let messages = vec![
            message(MessageRole::User, "Create a PR"),
            message(
                MessageRole::Assistant,
                "Done!\nPR_URL: https://github.com/org/repo/pull/42",
            ),
        ];
        assert_eq!(
            extract_pr_url(&messages),
            Some(("https://github.com/org/repo/pull/42".to_string(), 42))
        );
    }

    #[test]
    fn marker_pass_ignores_user_messages_and_prefers_marker_over_earlier_urls() {
        let messages = vec![
            message(
                MessageRole::User,
                "PR_URL: https://github.com/org/repo/pull/1",
            ),
            message(
                MessageRole::Assistant,
                "see https://github.com/org/repo/actions first",
            ),
            message(
                MessageRole::ToolResult,
                "PR_URL: <https://github.com/org/repo/pull/7>.",
            ),
        ];
        assert_eq!(
            extract_pr_url(&messages),
            Some(("https://github.com/org/repo/pull/7".to_string(), 7))
        );
    }

    #[test]
    fn falls_back_to_any_pr_url_with_wrapping_punctuation() {
        let messages = vec![message(
            MessageRole::Assistant,
            "Created the PR (https://github.com/org/repo/pull/123?diff=split).",
        )];
        assert_eq!(
            extract_pr_url(&messages),
            Some(("https://github.com/org/repo/pull/123".to_string(), 123))
        );
    }

    #[test]
    fn fallback_pass_ignores_user_messages() {
        let messages = vec![
            message(
                MessageRole::User,
                "see https://github.com/org/repo/pull/3 for prior art",
            ),
            message(MessageRole::Assistant, "Working on it."),
        ];
        assert_eq!(extract_pr_url(&messages), None);
    }

    #[test]
    fn ignores_non_pr_github_urls() {
        let messages = vec![message(
            MessageRole::Assistant,
            "See https://github.com/org/repo/issues/9 and https://example.com/pull/3",
        )];
        assert_eq!(extract_pr_url(&messages), None);
    }

    #[test]
    fn extracts_pr_url_from_pipeline_step_output() {
        let execution = pipeline(vec![
            step(StepStatus::Succeeded, Some("pushed")),
            step(
                StepStatus::Succeeded,
                Some("https://github.com/org/repo/pull/55\n"),
            ),
        ]);
        assert_eq!(
            extract_pr_url_from_pipeline(&execution),
            Some(("https://github.com/org/repo/pull/55".to_string(), 55))
        );
    }

    #[test]
    fn evaluate_pr_session_without_url_reports_missing() {
        let session = pr_session(pipeline(vec![step(StepStatus::Succeeded, Some("ok"))]));
        assert_eq!(
            evaluate_completed_session("pr", &session, &[]),
            Some(CompletionEffect::PrUrlMissing)
        );
    }

    #[test]
    fn evaluate_pr_session_with_url_reports_created() {
        let session = pr_session(pipeline(vec![step(StepStatus::Succeeded, Some("ok"))]));
        let messages = vec![message(
            MessageRole::Assistant,
            "PR_URL: https://github.com/org/repo/pull/8",
        )];
        assert_eq!(
            evaluate_completed_session("pr", &session, &messages),
            Some(CompletionEffect::PrCreated {
                pr_url: "https://github.com/org/repo/pull/8".to_string(),
                pr_number: 8,
            })
        );
    }

    #[test]
    fn push_rejected_when_failed_step_has_marker_and_no_ai_ran() {
        let execution = pipeline(vec![step(
            StepStatus::Failed,
            Some("! [rejected] main -> main (non-fast-forward)"),
        )]);
        assert_eq!(
            classify_completed_push_session(Some(&execution), &[]),
            PushOutcome::RejectedNonFastForward
        );
    }

    #[test]
    fn push_succeeds_when_ai_recovered_after_marker() {
        let execution = pipeline(vec![step(
            StepStatus::Failed,
            Some("error: failed to push (non-fast-forward)"),
        )]);
        let messages = vec![message(MessageRole::Assistant, "Retried and pushed.")];
        assert_eq!(
            classify_completed_push_session(Some(&execution), &messages),
            PushOutcome::Succeeded
        );
    }

    #[test]
    fn push_succeeds_when_all_steps_passed_or_skipped() {
        let execution = pipeline(vec![
            step(StepStatus::Succeeded, Some("pushed")),
            step(StepStatus::Skipped, None),
        ]);
        assert_eq!(
            classify_completed_push_session(Some(&execution), &[]),
            PushOutcome::Succeeded
        );
    }

    #[test]
    fn push_falls_back_to_transcript_markers_when_pipeline_inconclusive() {
        let execution = pipeline(vec![
            step(StepStatus::Failed, Some("some unrelated failure")),
            step(StepStatus::Skipped, None),
        ]);
        let messages = vec![message(
            MessageRole::ToolResult,
            "PUSH_REJECTED: NON_FAST_FORWARD",
        )];
        assert_eq!(
            classify_completed_push_session(Some(&execution), &messages),
            PushOutcome::RejectedNonFastForward
        );
        assert_eq!(
            classify_completed_push_session(None, &messages),
            PushOutcome::RejectedNonFastForward
        );
    }

    #[test]
    fn push_defaults_to_succeeded_without_markers() {
        assert_eq!(
            classify_completed_push_session(None, &[message(MessageRole::Assistant, "pushed")]),
            PushOutcome::Succeeded
        );
    }

    #[test]
    fn unmarked_completed_push_session_has_a_pending_effect() {
        let session = push_session(succeeded_push_pipeline());
        assert_eq!(
            pending_completion_effect(&session, "completed", Vec::new),
            Some((
                "push",
                CompletionEffect::PushCompleted {
                    outcome: PushOutcome::Succeeded
                }
            ))
        );
    }

    #[test]
    fn marked_session_has_no_pending_effect() {
        // A resumed push session: the old all-succeeded pipeline still
        // classifies as a fresh success, so only the marker stops the
        // destructive re-clear of the branch's PR status.
        let mut session = push_session(succeeded_push_pipeline());
        session.completion_effects_at = Some(1_700_000_000_000);
        assert_eq!(
            pending_completion_effect(&session, "completed", Vec::new),
            None
        );
    }

    #[test]
    fn non_completed_terminal_states_have_no_pending_effect() {
        let session = push_session(succeeded_push_pipeline());
        for status in ["error", "cancelled", "running"] {
            assert_eq!(
                pending_completion_effect(&session, status, Vec::new),
                None,
                "status {status} should not evaluate an outcome"
            );
        }
    }

    #[test]
    fn non_pipeline_and_non_pipeline_kind_sessions_have_no_pending_effect() {
        let plain = Session::new_running("Fix the login flow", std::path::Path::new("/tmp"));
        assert_eq!(
            pending_completion_effect(&plain, "completed", Vec::new),
            None
        );

        let mut ai_with_pipeline = plain.clone();
        ai_with_pipeline.pipeline = Some(succeeded_push_pipeline());
        assert_eq!(
            pending_completion_effect(&ai_with_pipeline, "completed", Vec::new),
            None
        );
    }

    #[test]
    fn only_pr_url_missing_skips_the_marker() {
        assert!(should_record_effects(&CompletionEffect::PrCreated {
            pr_url: "https://github.com/org/repo/pull/8".to_string(),
            pr_number: 8,
        }));
        assert!(should_record_effects(&CompletionEffect::PushCompleted {
            outcome: PushOutcome::Succeeded
        }));
        assert!(should_record_effects(&CompletionEffect::PushCompleted {
            outcome: PushOutcome::RejectedNonFastForward
        }));
        assert!(!should_record_effects(&CompletionEffect::PrUrlMissing));
    }

    #[test]
    fn unmarked_pr_url_missing_session_fires_on_a_later_completion() {
        let session = pr_session(pipeline(vec![step(StepStatus::Succeeded, Some("ok"))]));

        // First completion produced no URL, so nothing was delivered and the
        // session stays eligible.
        let first = pending_completion_effect(&session, "completed", Vec::new);
        assert_eq!(first, Some(("pr", CompletionEffect::PrUrlMissing)));
        assert!(!should_record_effects(&first.unwrap().1));

        // Resumed with "you didn't create the PR — do it now": the next
        // completion scans the new transcript and fires for real.
        let messages = vec![message(
            MessageRole::Assistant,
            "PR_URL: https://github.com/org/repo/pull/9",
        )];
        assert_eq!(
            pending_completion_effect(&session, "completed", || messages),
            Some((
                "pr",
                CompletionEffect::PrCreated {
                    pr_url: "https://github.com/org/repo/pull/9".to_string(),
                    pr_number: 9,
                }
            ))
        );
    }
}
