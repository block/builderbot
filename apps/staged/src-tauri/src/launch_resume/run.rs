//! The two-phase runner: repo sessions first, then project sessions.
//!
//! Phase 1 restarts every repo session (resume, or re-queue when there is no
//! conversation to load or the branch is busy — a re-queued row that has a
//! conversation is resumed by the drain when its turn comes, see
//! `session_commands::QueuedStartKind`) and waits, within a bound, until each
//! is `running` and live, `queued`, or terminal. Phase 2 then
//! resumes the project sessions, so a parent that polls
//! `wait_for_repo_session` on relaunch finds its children queued or running
//! rather than dead. Every claim goes through `continue_resume_gate`, the
//! same check the MCP `continue_repo_session` path uses, so two exclusive
//! sessions on one branch cannot both claim. The one wait in a resume's
//! preparation, the pre-run HEAD read, runs under
//! `session_commands::RESUME_HEAD_READ_CAP` and degrades to no snapshot, so a
//! claim cannot hold its branch on a slow checkout and a slow checkout does
//! not fail the resume.
//!
//! Phase 1 is sequential in plan order, so its worst case is the cap times
//! the number of slow HEAD reads before phase 2 starts. Resumes on different
//! branches are independent (ordering only matters within a branch), so
//! running branches concurrently is a possible follow-up if that sum shows
//! up on multi-branch remote setups.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::session_commands::{
    continue_resume_gate, resume_claimed_session_for_store, ClaimedResume, ContinueOutcome,
};
use crate::session_queue::KickSource;
use crate::session_runner::{self, SessionRegistry};
use crate::store::{CompletionReason, Session, SessionStatus, Store};

use super::plan::{
    classify_candidate, plan_launch_resume, LaunchAction, PlannedSession,
    PROJECT_SESSION_RESUME_PROMPT, REPO_SESSION_RESUME_PROMPT,
};
use super::LaunchResumeSummary;

/// How long phase 1 waits for its sessions to settle before phase 2 starts
/// anyway. What settlement waits on is the drain's queued → running →
/// registered window: a re-queued row the kick claims is `running` under our
/// pid from the claim until `start_session` registers it with a liveness
/// hold, and `is_settled` only counts a `running` row once it is live.
/// `queued` already counts as settled, so a row the drain has not reached
/// never holds phase 2 up; this cap bounds the ones it has.
pub(crate) const REPO_PHASE_SETTLE_CAP: Duration = Duration::from_secs(15);
const SETTLE_POLL: Duration = Duration::from_millis(200);

/// What one phase did, for the log and the Doctor.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PhaseOutcome {
    pub(crate) resumed: Vec<String>,
    pub(crate) requeued: Vec<String>,
    pub(crate) skipped: Vec<(String, String)>,
    pub(crate) failed: Vec<(String, String)>,
}

impl PhaseOutcome {
    fn touched(&self) -> impl Iterator<Item = &String> {
        self.resumed.iter().chain(self.requeued.iter())
    }
}

/// A row is still what launch resume was offered: terminal with the quit's
/// reason. Anything else (resumed by hand, re-queued by another instance,
/// deleted) is left alone.
fn still_interrupted(session: &Session) -> bool {
    matches!(
        session.status,
        SessionStatus::Cancelled | SessionStatus::Error
    ) && session.completion_reason == Some(CompletionReason::AppQuit)
}

fn read_interrupted(store: &Store, session_id: &str) -> Result<Option<Session>, String> {
    Ok(store
        .get_session(session_id)
        .map_err(|e| e.to_string())?
        .filter(still_interrupted))
}

/// Flip the row back to `queued` and tell clients. The branch is kicked by
/// the caller once every row of the phase is in place, so the drain reads
/// the whole re-queued set in `created_at` order.
fn requeue<R: tauri::Runtime>(
    store: &Store,
    app_handle: &tauri::AppHandle<R>,
    session: &Session,
    branch_id: Option<&str>,
) -> Result<bool, String> {
    let requeued = store
        .transition_app_quit_to_queued(&session.id)
        .map_err(|e| e.to_string())?;
    if requeued {
        let project_id = store.get_project_id_for_session(&session.id).ok().flatten();
        session_runner::emit_status(
            app_handle,
            &session.id,
            SessionStatus::Queued.as_str(),
            None,
            None,
            None,
            branch_id.map(str::to_string),
            project_id,
        );
    }
    Ok(requeued)
}

/// Phase 1 over the planned repo sessions, in plan order. `resume` finishes a
/// claim the gate made: in production `resume_claimed_session_for_store`,
/// whose preparation bounds its pre-run HEAD read (`RESUME_HEAD_READ_CAP`,
/// degrading to no snapshot rather than failing) and which ends the row in
/// `error` (releasing the claim and draining the branch) when the
/// preparation or launch fails, so the phase simply awaits it and counts an
/// `Err` as failed. A claim the gate blocks — the branch is busy, provisioning, or
/// has rows queued ahead — falls back to re-queue; the row keeps its ACP
/// session id, so the drain resumes its conversation in turn rather than
/// starting it over. One the gate refuses outright (the worktree is gone) is
/// left terminal.
pub(crate) async fn run_repo_phase<R, F, Fut>(
    store: &Arc<Store>,
    registry: &Arc<SessionRegistry>,
    app_handle: &tauri::AppHandle<R>,
    planned: &[PlannedSession],
    resume: F,
) -> PhaseOutcome
where
    R: tauri::Runtime,
    F: Fn(ClaimedResume) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let mut outcome = PhaseOutcome::default();
    let mut branches_to_kick = BTreeSet::new();

    for item in planned {
        let session = match read_interrupted(store, &item.session_id) {
            Ok(Some(session)) => session,
            Ok(None) => {
                log::info!(
                    "[launch_resume] session {} is no longer interrupted; skipping",
                    item.session_id
                );
                continue;
            }
            Err(e) => {
                outcome.failed.push((item.session_id.clone(), e));
                continue;
            }
        };

        let requeue_now = match &item.action {
            LaunchAction::Skip { reason } => {
                outcome
                    .skipped
                    .push((item.session_id.clone(), (*reason).to_string()));
                continue;
            }
            LaunchAction::Requeue => true,
            LaunchAction::Resume => match continue_resume_gate(store, registry, session.clone()) {
                Ok(Ok(claim)) => match resume(claim).await {
                    Ok(()) => {
                        outcome.resumed.push(item.session_id.clone());
                        continue;
                    }
                    Err(e) => {
                        // Recorded on the row by the resume itself (it is
                        // `error` now, and its branch has been kicked).
                        log::warn!(
                            "[launch_resume] resume of session {} failed: {e}",
                            item.session_id
                        );
                        outcome.failed.push((item.session_id.clone(), e));
                        continue;
                    }
                },
                Ok(Err(ContinueOutcome::Blocked { reason })) => {
                    // The row keeps its agent id through the re-queue, so the
                    // drain resumes its conversation when its turn comes
                    // (`QueuedStartKind::Resume`): the dialog's `resume` holds.
                    log::info!(
                        "[launch_resume] session {} cannot resume now ({reason}); re-queued to \
                         resume in turn",
                        item.session_id
                    );
                    true
                }
                Ok(Err(refusal)) => {
                    let reason = refusal.reason().unwrap_or("not resumable").to_string();
                    log::warn!(
                        "[launch_resume] session {} left as it is: {reason}",
                        item.session_id
                    );
                    outcome.skipped.push((item.session_id.clone(), reason));
                    continue;
                }
                Err(e) => {
                    outcome.failed.push((item.session_id.clone(), e));
                    continue;
                }
            },
        };

        if requeue_now {
            match requeue(store, app_handle, &session, item.branch_id.as_deref()) {
                Ok(true) => {
                    outcome.requeued.push(item.session_id.clone());
                    if let Some(branch_id) = &item.branch_id {
                        branches_to_kick.insert(branch_id.clone());
                    }
                }
                Ok(false) => log::info!(
                    "[launch_resume] session {} was not re-queued; it moved under us",
                    item.session_id
                ),
                Err(e) => outcome.failed.push((item.session_id.clone(), e)),
            }
        }
    }

    for branch_id in branches_to_kick {
        crate::session_queue::kick(app_handle, &branch_id, KickSource::LaunchResume);
    }
    outcome
}

/// Whether a phase-1 session has settled: `running` and owned by something in
/// this process, `queued` (the drain owns it from here), `disconnected` (its
/// turn runs on its workstation with no local owner to wait for), `paused` (a
/// rate limit parked it; its scheduler owns the resume), terminal, or gone.
pub(crate) fn is_settled(store: &Store, registry: &SessionRegistry, session_id: &str) -> bool {
    match store.get_session(session_id) {
        Ok(Some(session)) => match session.status {
            SessionStatus::Running => registry.is_live(session_id),
            SessionStatus::Queued
            | SessionStatus::Disconnected
            | SessionStatus::Paused
            | SessionStatus::Completed
            | SessionStatus::Error
            | SessionStatus::Cancelled => true,
        },
        Ok(None) => true,
        Err(_) => true,
    }
}

/// Poll until every id is settled or `cap` has passed. Returns the ids still
/// unsettled at the end, for the log.
pub(crate) async fn wait_until_settled(
    store: &Store,
    registry: &SessionRegistry,
    session_ids: &[String],
    cap: Duration,
) -> Vec<String> {
    let deadline = Instant::now() + cap;
    loop {
        let pending: Vec<String> = session_ids
            .iter()
            .filter(|id| !is_settled(store, registry, id))
            .cloned()
            .collect();
        if pending.is_empty() || Instant::now() >= deadline {
            return pending;
        }
        tokio::time::sleep(SETTLE_POLL).await;
    }
}

/// Phase 2 over the planned project sessions. No branch schedule applies, so
/// the gate is the status CAS alone; a session that is blocked (it started
/// again under us) or refused is left as it is.
pub(crate) async fn run_project_phase<F, Fut>(
    store: &Arc<Store>,
    registry: &Arc<SessionRegistry>,
    planned: &[PlannedSession],
    resume: F,
) -> PhaseOutcome
where
    F: Fn(ClaimedResume) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let mut outcome = PhaseOutcome::default();
    for item in planned {
        let session = match read_interrupted(store, &item.session_id) {
            Ok(Some(session)) => session,
            Ok(None) => continue,
            Err(e) => {
                outcome.failed.push((item.session_id.clone(), e));
                continue;
            }
        };
        match &item.action {
            LaunchAction::Resume => {}
            LaunchAction::Requeue => {
                // Never planned for a project session; left terminal if it
                // ever were, since a project session has no queue.
                outcome.skipped.push((
                    item.session_id.clone(),
                    "project sessions cannot be re-queued".to_string(),
                ));
                continue;
            }
            LaunchAction::Skip { reason } => {
                outcome
                    .skipped
                    .push((item.session_id.clone(), (*reason).to_string()));
                continue;
            }
        }
        match continue_resume_gate(store, registry, session) {
            Ok(Ok(claim)) => match resume(claim).await {
                Ok(()) => outcome.resumed.push(item.session_id.clone()),
                Err(e) => {
                    log::warn!(
                        "[launch_resume] resume of project session {} failed: {e}",
                        item.session_id
                    );
                    outcome.failed.push((item.session_id.clone(), e));
                }
            },
            Ok(Err(refusal)) => {
                let reason = refusal.reason().unwrap_or("not resumable").to_string();
                log::warn!(
                    "[launch_resume] project session {} left as it is: {reason}",
                    item.session_id
                );
                outcome.skipped.push((item.session_id.clone(), reason));
            }
            Err(e) => outcome.failed.push((item.session_id.clone(), e)),
        }
    }
    outcome
}

/// Everything the production resume needs besides the claim.
#[derive(Clone)]
pub(crate) struct ResumeDeps {
    pub(crate) store: Arc<Store>,
    pub(crate) registry: Arc<SessionRegistry>,
    pub(crate) action_executor: Arc<ActionExecutor>,
    pub(crate) action_registry: Arc<ActionRegistry>,
    pub(crate) app_handle: tauri::AppHandle,
}

impl ResumeDeps {
    async fn resume(&self, claim: ClaimedResume, prompt: &'static str) -> Result<(), String> {
        resume_claimed_session_for_store(
            Arc::clone(&self.store),
            Arc::clone(&self.registry),
            Arc::clone(&self.action_executor),
            Arc::clone(&self.action_registry),
            self.app_handle.clone(),
            claim,
            prompt.to_string(),
            Vec::new(),
        )
        .await
    }
}

/// The whole run for the ids the user chose. Ids whose row is no longer
/// terminal with the quit's reason are ignored, so a second window or a
/// reload cannot double-resume.
pub(crate) async fn run_launch_resume(
    deps: ResumeDeps,
    session_ids: Vec<String>,
) -> LaunchResumeSummary {
    let store = Arc::clone(&deps.store);
    let registry = Arc::clone(&deps.registry);

    let mut candidates = Vec::new();
    for session_id in &session_ids {
        match read_interrupted(&store, session_id) {
            Ok(Some(session)) => candidates.push(classify_candidate(&store, &session)),
            Ok(None) => {}
            Err(e) => log::warn!("[launch_resume] could not read session {session_id}: {e}"),
        }
    }
    let plan = plan_launch_resume(candidates);
    log::info!(
        "[launch_resume] resuming {} repo and {} project session(s)",
        plan.repo.len(),
        plan.project.len()
    );

    let repo_outcome = run_repo_phase(&store, &registry, &deps.app_handle, &plan.repo, |claim| {
        let deps = deps.clone();
        async move { deps.resume(claim, REPO_SESSION_RESUME_PROMPT).await }
    })
    .await;

    let touched: Vec<String> = repo_outcome.touched().cloned().collect();
    let unsettled = wait_until_settled(&store, &registry, &touched, REPO_PHASE_SETTLE_CAP).await;
    if !unsettled.is_empty() {
        log::warn!(
            "[launch_resume] {} repo session(s) had not settled after {:?}; starting project \
             sessions anyway: {}",
            unsettled.len(),
            REPO_PHASE_SETTLE_CAP,
            unsettled.join(", ")
        );
    }

    let project_outcome = if crate::app_lifecycle::is_quitting(&deps.app_handle) {
        log::info!("[launch_resume] app is quitting; project sessions are not resumed");
        PhaseOutcome::default()
    } else {
        run_project_phase(&store, &registry, &plan.project, |claim| {
            let deps = deps.clone();
            async move { deps.resume(claim, PROJECT_SESSION_RESUME_PROMPT).await }
        })
        .await
    };

    let summary = LaunchResumeSummary {
        requested: session_ids.len(),
        resumed: repo_outcome.resumed.len() + project_outcome.resumed.len(),
        requeued: repo_outcome.requeued.len(),
        skipped: repo_outcome.skipped.len() + project_outcome.skipped.len(),
        failed: repo_outcome.failed.len() + project_outcome.failed.len(),
    };
    log::info!(
        "[launch_resume] done: requested {}, resumed {}, re-queued {}, skipped {}, failed {}",
        summary.requested,
        summary.resumed,
        summary.requeued,
        summary.skipped,
        summary.failed
    );
    for (session_id, reason) in repo_outcome
        .failed
        .iter()
        .chain(project_outcome.failed.iter())
    {
        log::warn!("[launch_resume] session {session_id} failed: {reason}");
    }
    summary
}
