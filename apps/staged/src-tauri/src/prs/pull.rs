//! Fast-forwarding the branch to origin.
//!
//! An idle branch pulls right away, outside the pipeline runner, behind a
//! running marker session that keeps other work off the worktree for the
//! duration; a busy branch queues a pull pipeline that the drain rebuilds and
//! runs later (see [`super::git_pipeline`]). A pull never hands off to an
//! agent: a branch that diverged while the pull waited is the user's call.

use std::path::PathBuf;
use std::sync::Arc;

use crate::session_runner;
use crate::store::{self, FailureStrategy, PipelineExecution, PipelineKind, PipelineStep, Store};

use super::git_commands::git_fetch_with_fallback;
use super::pipeline::{branch_has_work_in_flight, find_queued_pipeline, PULL_PROMPT};

/// Build the steps for a queued fast-forward pull.
///
/// Both steps abort rather than handing off to an agent: a failed `--ff-only`
/// merge means the branch diverged while the pull waited, and the fix is a user
/// decision (rebase onto origin, or reset to origin) rather than something an
/// agent should pick. `session_runner` turns that abort into a session error the
/// frontend toasts, since a drained pull has no one watching it.
///
/// Rebuilt from the branch's current remote branch on dequeue, like the push
/// steps.
pub(super) fn build_pull_pipeline_steps(remote_branch: &str) -> Vec<PipelineStep> {
    vec![
        PipelineStep::Command {
            label: format!("Fetch origin/{remote_branch}"),
            command: git_fetch_with_fallback(remote_branch),
            on_failure: FailureStrategy::Abort { marker: None },
        },
        PipelineStep::Command {
            label: format!("Fast-forward to origin/{remote_branch}"),
            command: format!("git merge --ff-only origin/{remote_branch}"),
            on_failure: FailureStrategy::Abort { marker: None },
        },
    ]
}

/// What the branch queue decided to do with a pull request.
pub(super) enum PullDisposition {
    /// Waiting its turn: either a freshly queued session, or the queued pull that
    /// already covers this request.
    Queued(String),
    /// The branch was idle, so the pull runs now. The id is the session that marks
    /// the branch busy for its duration, and the hold is what says this process
    /// will finish it (see `SessionRegistry::hold`) — it never goes through the
    /// runner, so nothing else registers it.
    RunningNow {
        session_id: String,
        liveness: session_runner::LivenessHold,
    },
}

/// Decide between pulling now and queueing behind in-flight branch work, and
/// record that decision.
///
/// Mirrors [`super::push::queue_push_pipeline_if_branch_busy`]: the busy check, the dedupe
/// scan, and the insert all run under the branch launch lock, and stay
/// synchronous because the lock must not be held across an await. A pull has no
/// variants, so the dedupe keys on the kind alone.
///
/// The run-now case still gets a session row — a running `PipelineKind::Pull`
/// linked to the branch with no artifact, exactly like a running push. It is what
/// makes the fetch-and-merge visible to `has_running_session_for_branch` and the
/// drain scan; without it the one mutating git operation that skips the pipeline
/// runner would look idle for its whole duration, and a commit session or another
/// git action could start against the same worktree mid-merge. No
/// `session-status-changed` event is emitted for it: the caller awaits the pull and
/// reports the outcome itself, so an event would double-report a failure and spin
/// the project tile for what is usually an instant operation.
///
/// No provider is recorded either: every pull step aborts on failure, so the
/// pipeline never hands off to an agent.
pub(super) fn claim_or_queue_pull_for_branch(
    store: &Arc<Store>,
    registry: &Arc<session_runner::SessionRegistry>,
    branch_id: &str,
) -> Result<PullDisposition, String> {
    let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
    let _guard = launch_lock.lock().unwrap();
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;

    let busy = branch_has_work_in_flight(store, branch_id)?;
    if busy {
        if let Some(existing) = find_queued_pipeline(store, branch_id, |pipeline| {
            pipeline.kind.as_ref() == Some(&PipelineKind::Pull)
        })? {
            return Ok(PullDisposition::Queued(existing));
        }
    }

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    // A queued pull is rebuilt with a full resolution on dequeue, and a pull
    // that runs now goes through `pull_branch_ff_only_impl`, which resolves
    // the remote branch itself; these steps are the queue marker, so the
    // branch on origin is not looked up under the launch lock here.
    let steps = build_pull_pipeline_steps(&branch.branch_name);
    let pipeline = PipelineExecution::from_steps(&steps).with_kind(PipelineKind::Pull);
    // Like a push session, this one carries no artifact — `branch_id` is what
    // keeps it on the branch queue.
    let mut session = if busy {
        store::Session::new_queued(PULL_PROMPT)
    } else {
        store::Session::new_running(PULL_PROMPT, &immediate_pull_working_dir(store, branch_id))
    }
    .with_branch(branch_id);
    session.pipeline = Some(pipeline);
    // Before the write, so the row is never `running` without it.
    let liveness = (!busy).then(|| registry.hold(&session.id));
    store.create_session(&session).map_err(|e| e.to_string())?;

    Ok(match liveness {
        None => PullDisposition::Queued(session.id),
        Some(liveness) => PullDisposition::RunningNow {
            session_id: session.id,
            liveness,
        },
    })
}

/// Best-effort working directory for the session row of an immediate pull.
///
/// The pull resolves its own path (a remote branch fast-forwards through its
/// workspace shell), so this only decides what the session row displays — not
/// where anything runs, and not whether the pull can proceed.
fn immediate_pull_working_dir(store: &Arc<Store>, branch_id: &str) -> PathBuf {
    store
        .get_workdir_for_branch(branch_id)
        .ok()
        .flatten()
        .map(|workdir| PathBuf::from(workdir.path))
        .unwrap_or_default()
}

/// Release the session that marked the branch busy for an immediate pull.
///
/// The marker never went through the session runner, so nothing else will end it.
/// The completion reason matches the pipeline path: the work ran to its conclusion
/// either way, only the outcome differs.
pub(super) fn finish_immediate_pull_session(
    store: &Arc<Store>,
    session_id: &str,
    error: Option<&str>,
) {
    let status = if error.is_some() {
        store::SessionStatus::Error
    } else {
        store::SessionStatus::Completed
    };

    if let Err(e) = store.transition_from_running(
        session_id,
        status,
        error,
        Some(&store::CompletionReason::TurnComplete),
    ) {
        log::warn!("[prs] Failed to finish immediate pull session {session_id}: {e}");
    }
}

/// Fast-forward the branch to origin now, or queue the pull behind in-flight
/// branch work.
///
/// An idle branch pulls directly rather than through the pipeline runner, which is
/// why this returns an `Option` rather than the [`super::pipeline::BranchPipelineResponse`] the
/// pipeline-only actions use: `None` means the pull already happened (and its
/// failure, if any, is this call's error), `Some(session_id)` means it is waiting
/// its turn on the branch queue.
pub(crate) async fn pull_or_queue_branch_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<Option<String>, String> {
    let pull_store = Arc::clone(&store);
    let pull_branch_id = branch_id.clone();
    let kick_branch_id = branch_id.clone();
    pull_or_queue_branch_with(
        store,
        registry,
        branch_id,
        move || crate::timeline::pull_branch_ff_only_impl(&pull_store, &pull_branch_id),
        move || {
            // Anything the user requested while the pull held the branch is
            // queued behind the marker session, and the marker bypassed the
            // session runner that would normally drain it.
            crate::session_queue::kick(
                &app_handle,
                &kick_branch_id,
                crate::session_queue::KickSource::ImmediatePull,
            );
        },
    )
    .await
}

/// [`pull_or_queue_branch_for_branch`] with the pull itself and the
/// after-release hook injected, so a test can stand in a pull that blocks.
pub(super) async fn pull_or_queue_branch_with(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    branch_id: String,
    pull: impl FnOnce() -> Result<(), String> + Send + 'static,
    on_released: impl FnOnce() + Send + 'static,
) -> Result<Option<String>, String> {
    let (session_id, liveness) =
        match claim_or_queue_pull_for_branch(&store, &registry, &branch_id)? {
            PullDisposition::Queued(session_id) => return Ok(Some(session_id)),
            PullDisposition::RunningNow {
                session_id,
                liveness,
            } => (session_id, liveness),
        };

    // From here on the marker row is the branch's busy state, and this future
    // belongs to whoever asked for the pull: in web mode that is a request
    // handler hyper drops at its next `.await` the moment the client closes
    // the connection. The work that releases the marker must therefore run on
    // its own task, with this future only waiting on the handle — dropping a
    // `JoinHandle` detaches the task, it does not cancel it.
    //
    // A join failure (the pull task panicking, or runtime shutdown) folds into
    // the pull's own error path rather than returning early: the marker
    // session is what marks the branch busy, and nothing else ever finishes it.
    let released = tauri::async_runtime::spawn(async move {
        let pulled = tauri::async_runtime::spawn_blocking(pull)
            .await
            .unwrap_or_else(|e| Err(format!("Pull task failed: {e}")));

        finish_immediate_pull_session(
            &store,
            &session_id,
            pulled.as_ref().err().map(String::as_str),
        );
        // Released after the terminal write, on the task that owns the marker
        // — not with the request future, which may already be gone.
        drop(liveness);
        on_released();

        pulled
    });

    released
        .await
        .unwrap_or_else(|e| Err(format!("Pull task failed: {e}")))
        .map(|()| None)
}
