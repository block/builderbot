//! The session-row plumbing every branch pipeline shares.
//!
//! A pipeline session is a `sessions` row linked to the branch (never to an
//! artifact: a push or pull produces no commit, and a pending-commit stub
//! would render as a failed commit), so the branch queue sees it as work in
//! flight. This module holds what the actions have in common: the
//! start-or-queue response, the prompts that double as the rows' timeline
//! labels, the busy and dedupe checks that run under the branch launch lock,
//! and the run-now row with its launch into the session runner.

use serde::Serialize;
use std::sync::Arc;

use crate::session_commands::BranchSessionLaunchStatus;
use crate::session_runner;
use crate::store::{self, PipelineExecution, PipelineKind, PipelineStep, Store};

use super::context::BranchPipelineContext;
use super::push::PushVariant;

/// Result of a start-or-queue branch pipeline command.
///
/// Rebase and squash can be requested while the branch already has work in
/// flight, so the caller needs to know whether the returned session started
/// running or is waiting its turn on the branch queue.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BranchPipelineResponse {
    pub session_id: String,
    pub session_status: BranchSessionLaunchStatus,
    /// The queued session that runs after this one, and only if it succeeds
    /// (the force push of "Rebase and force push"). Always queued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_up_session_id: Option<String>,
}

impl BranchPipelineResponse {
    pub(super) fn running(session_id: String) -> Self {
        Self {
            session_id,
            session_status: BranchSessionLaunchStatus::Running,
            follow_up_session_id: None,
        }
    }

    pub(super) fn queued(session_id: String) -> Self {
        Self {
            session_id,
            session_status: BranchSessionLaunchStatus::Queued,
            follow_up_session_id: None,
        }
    }
}

/// Create a session with a pipeline and start it in the background.
///
/// Handles session creation, pipeline persistence, the "running" event emission,
/// and spawning the pipeline runner. Returns the session ID.
#[allow(clippy::too_many_arguments)]
pub(super) fn start_pipeline_for_branch(
    ctx: BranchPipelineContext,
    steps: Vec<PipelineStep>,
    prompt: &str,
    session_type: &str,
    provider: Option<String>,
    store: Arc<Store>,
    app_handle: &tauri::AppHandle,
    registry: &Arc<session_runner::SessionRegistry>,
) -> Result<String, String> {
    crate::project_transfer::ensure_branch_not_transferring(&ctx.branch.id)?;
    let pipeline = PipelineExecution::from_steps(&steps);

    // pr/push pipeline sessions link no artifact row, so the session row
    // itself carries the branch linkage — busy-state snapshots and terminal
    // completion side-effects resolve the branch from it.
    let mut session =
        store::Session::new_running(prompt, &ctx.working_dir).with_branch(&ctx.branch.id);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline.clone());
    // Live from before the `running` write until the runner has registered
    // (see `SessionRegistry::hold`); released when this function returns.
    let _liveness = registry.hold(&session.id);
    store.create_session(&session).map_err(|e| e.to_string())?;

    // Emit "running" event *before* returning so the global session listener
    // registers this session atomically — avoiding the race where the session
    // completes before the frontend `.then()` callback fires.
    let branch_id = ctx.branch.id.clone();
    let project_id = ctx.branch.project_id.clone();

    session_runner::emit_session_running(
        app_handle,
        &session.id,
        &branch_id,
        &project_id,
        session_type,
    );

    session_runner::start_pipeline_session(
        session_runner::PipelineConfig {
            session_id: session.id.clone(),
            prompt: prompt.to_string(),
            steps,
            pipeline,
            working_dir: ctx.working_dir,
            pre_head_sha: None,
            provider,
            workspace_name: ctx.workspace_name,
            remote_working_dir: ctx.remote_working_dir,
            branch_id: Some(branch_id),
            project_id: Some(project_id),
        },
        store,
        app_handle.clone(),
        Arc::clone(registry),
    )?;

    Ok(session.id)
}

// Every push prompt must contain "Push the current branch to the remote" or
// "Force push the current branch to the remote":
// `infer_branch_resume_session_type` matches on those to recognise a push
// session, and without a match no `push-completed` event fires.
pub(super) const PUSH_PROMPT: &str = "Push the current branch to the remote with a normal push. If the push fails for a recoverable reason, diagnose and fix it, then retry with a normal push. Do not force push.";
pub(super) const FORCE_PUSH_PROMPT: &str = "Force push the current branch to the remote";
pub(super) const NO_VERIFY_PUSH_PROMPT: &str = "Push the current branch to the remote with a normal push, skipping the pre-push hook (`--no-verify`). If the push fails for a recoverable reason, diagnose and fix it, then retry with a normal push that keeps `--no-verify`. Do not force push.";
pub(super) const NO_VERIFY_FORCE_PUSH_PROMPT: &str =
    "Force push the current branch to the remote, skipping the pre-push hook (`--no-verify`)";
/// A pull never hands off to an agent (every step aborts), so this is purely the
/// session's label.
pub(super) const PULL_PROMPT: &str = "Pull from origin";

/// Prompt for a pipeline session, which doubles as its timeline label.
///
/// `push` only matters for [`PipelineKind::Push`]; the other kinds ignore it.
/// Queued and running paths share this so a session's label doesn't change
/// when it is drained.
pub(super) fn pipeline_prompt(kind: &PipelineKind, push: PushVariant) -> &'static str {
    match kind {
        PipelineKind::Rebase => "Rebase branch",
        PipelineKind::Squash => "Squash commits",
        PipelineKind::Push => match (push.force, push.no_verify) {
            (false, false) => PUSH_PROMPT,
            (true, false) => FORCE_PUSH_PROMPT,
            (false, true) => NO_VERIFY_PUSH_PROMPT,
            (true, true) => NO_VERIFY_FORCE_PUSH_PROMPT,
        },
        PipelineKind::Pull => PULL_PROMPT,
    }
}

/// Find a queued pipeline session on this branch that already performs exactly
/// the work being requested, so a second click doesn't stack a duplicate.
///
/// `matches` has to compare every persisted field that changes what the pipeline
/// does, not just the kind: "rebase onto base" vs "rebase onto origin" and
/// "push" vs "force push" are different operations that share a [`PipelineKind`].
pub(super) fn find_queued_pipeline(
    store: &Arc<Store>,
    branch_id: &str,
    matches: impl Fn(&PipelineExecution) -> bool,
) -> Result<Option<String>, String> {
    let queued = store
        .get_queued_sessions_for_branch(branch_id)
        .map_err(|e| e.to_string())?;

    Ok(queued
        .into_iter()
        .find(|session| session.pipeline.as_ref().is_some_and(&matches))
        .map(|session| session.id))
}

/// The work a branch has in flight, for callers that need to tell work which
/// finishes on its own (running) from work that only moves when something
/// drains it (queued).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchWorkInFlight {
    None,
    Queued,
    Running,
}

/// What the branch is busy with, if anything.
///
/// Callers must hold the branch launch lock: the answer is only meaningful for
/// as long as no session can start or be enqueued underneath them.
pub(crate) fn branch_work_in_flight(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<BranchWorkInFlight, String> {
    if store
        .has_running_session_for_branch(branch_id)
        .map_err(|e| e.to_string())?
    {
        return Ok(BranchWorkInFlight::Running);
    }
    if !store
        .get_queued_sessions_for_branch(branch_id)
        .map_err(|e| e.to_string())?
        .is_empty()
    {
        return Ok(BranchWorkInFlight::Queued);
    }
    Ok(BranchWorkInFlight::None)
}

/// Whether the branch already has work in flight, and a new request therefore
/// has to join the queue instead of starting now.
pub(super) fn branch_has_work_in_flight(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<bool, String> {
    Ok(branch_work_in_flight(store, branch_id)? != BranchWorkInFlight::None)
}

/// The rows a run-now pipeline needs before the session runner can pick it up.
///
/// Inserted under the branch launch lock and consumed by
/// [`launch_running_pipeline_session`] once the lock is released.
///
/// Carries the row's liveness hold (see `SessionRegistry::hold`), taken
/// before the insert and released when this is dropped — after the runner
/// has registered the session, for a launch; after the terminal write, for a
/// rebase that failed before launching.
pub(super) struct RunningPipelineSession {
    pub(super) session_id: String,
    pub(super) pipeline: PipelineExecution,
    pub(super) prompt: String,
    /// HEAD captured before the insert, for a pipeline whose completion is
    /// judged against it up front (a revert). A rebase leaves this `None` and
    /// the runner captures it at start.
    pub(super) pre_head_sha: Option<String>,
    pub(super) _liveness: session_runner::LivenessHold,
}

/// Announce a run-now pipeline session and hand it to the session runner.
///
/// Runs after the branch launch lock is released: the session row already exists,
/// so anything racing this already sees the branch as busy.
#[allow(clippy::too_many_arguments)]
pub(super) fn launch_running_pipeline_session(
    ctx: BranchPipelineContext,
    running: RunningPipelineSession,
    steps: Vec<PipelineStep>,
    session_type: &str,
    provider: Option<String>,
    store: Arc<Store>,
    app_handle: &tauri::AppHandle,
    registry: &Arc<session_runner::SessionRegistry>,
) -> Result<String, String> {
    let branch_id = ctx.branch.id.clone();
    let project_id = ctx.branch.project_id.clone();

    session_runner::emit_session_running(
        app_handle,
        &running.session_id,
        &branch_id,
        &project_id,
        session_type,
    );

    let RunningPipelineSession {
        session_id,
        pipeline,
        prompt,
        pre_head_sha,
        _liveness: liveness,
    } = running;
    let started = session_runner::start_pipeline_session(
        session_runner::PipelineConfig {
            session_id: session_id.clone(),
            prompt,
            steps,
            pipeline,
            working_dir: ctx.working_dir,
            pre_head_sha,
            provider,
            workspace_name: ctx.workspace_name,
            remote_working_dir: ctx.remote_working_dir,
            branch_id: Some(branch_id.clone()),
            project_id: Some(project_id.clone()),
        },
        Arc::clone(&store),
        app_handle.clone(),
        Arc::clone(registry),
    );
    if let Err(error) = started {
        // The row is `running` under our pid and the hold is still ours, so
        // the failure goes on the row *before* the hold is released: left
        // `running` with no owner, the sweep would end it within a minute as
        // "Staged lost track of this session" in place of the real cause.
        // The kick is the terminal-state drain a runner would have owed.
        if session_runner::finish_failed_before_run(
            &session_id,
            Some(branch_id.clone()),
            Some(project_id),
            &store,
            app_handle,
            &error,
        ) {
            crate::session_queue::kick(
                app_handle,
                &branch_id,
                crate::session_queue::KickSource::StartFailed,
            );
        }
        drop(liveness);
        return Err(error);
    }
    // The runner registered the session at entry; the row is its now.
    drop(liveness);

    Ok(session_id)
}
