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

use crate::store::{MessageRole, PipelineExecution, Session, SessionMessage, StepStatus, Store};

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
pub fn run_completion_side_effects(
    store: &Arc<Store>,
    app_handle: &AppHandle,
    session_id: &str,
    branch_id: Option<&str>,
    status: &str,
) {
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
    if session.pipeline.is_none() {
        return;
    }
    let Some(kind) = crate::session_commands::infer_branch_resume_session_type(&session.prompt)
    else {
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

    let messages = store.get_session_messages(session_id).unwrap_or_default();
    let Some(effect) = evaluate_completed_session(kind, &session, &messages) else {
        return;
    };

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
}
