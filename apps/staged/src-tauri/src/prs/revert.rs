//! Revert: a run-now pipeline that creates a signed-off revert commit for a
//! completed commit on the branch.
//!
//! Unlike rebase and squash it never queues: it is refused while the branch
//! has work in flight, and runs `git revert --signoff --no-edit <sha>` with an
//! AI handoff only if git stops on a conflict. A pending-commit stub is linked
//! to the session so completion can record the revert commit's SHA.

use std::sync::Arc;

use crate::session_runner;
use crate::store::{FailureStrategy, PipelineExecution, PipelineStep, Store};

use super::commit::insert_running_pending_commit_session;
use super::context::{resolve_branch_pipeline_context, BranchPipelineContext};
use super::pipeline::{
    branch_has_work_in_flight, launch_running_pipeline_session, RunningPipelineSession,
};

pub(super) fn validate_commit_sha(commit_sha: &str) -> Result<String, String> {
    let sha = commit_sha.trim();
    if (7..=64).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(sha.to_string())
    } else {
        Err("Commit SHA must be 7-64 hexadecimal characters".to_string())
    }
}

fn revert_handoff_prompt(commit_sha: &str) -> String {
    format!(
        r#"<action>
`git revert --signoff --no-edit {commit_sha}` failed.

Inspect the git state and command output, then complete the revert safely.

Requirements:
- Resolve conflicts if present, preserving unrelated local changes.
- If git is in a revert sequence and the resolution is correct, run `git revert --continue`.
- Use the user's global git identity for author and committer.
- Ensure the resulting revert commit is signed off.
- Do not push the branch.
- If the revert should not continue, explain why and leave the repository in a clear state.

Context from prior steps:

{{step_outputs}}
</action>"#
    )
}

pub(super) fn build_revert_pipeline_steps(commit_sha: &str) -> Result<Vec<PipelineStep>, String> {
    let commit_sha = validate_commit_sha(commit_sha)?;
    Ok(vec![PipelineStep::Command {
        label: "Revert commit".to_string(),
        command: format!("git revert --signoff --no-edit {commit_sha}"),
        on_failure: FailureStrategy::HandoffToAi {
            prompt_template: revert_handoff_prompt(&commit_sha),
        },
    }])
}

pub(super) const REVERT_BRANCH_BUSY: &str =
    "Revert is unavailable while this branch has queued or running work";
pub(super) const REVERT_WORKTREE_BUSY: &str =
    "Revert is unavailable while this branch is being reset or its changes discarded";

/// Insert the rows for a revert that runs now, or refuse it on a busy branch.
///
/// A revert is never queued: a queued row would have to carry the target SHA
/// for the drain to rebuild its step from. So where a rebase or squash would
/// queue behind work in flight, this errors — under the branch launch lock,
/// with the insert in the same critical section, so two rapid clicks (or a
/// click racing a session start) cannot both observe an idle branch. This
/// stays synchronous on purpose: the lock must not be held across an await.
///
/// A reset-to-origin or discard has no session row for that check to find,
/// so the marker they set is read here as well, the way the forced start reads
/// it: a revert started under one would race its `git reset --hard`, and if it
/// lost on `index.lock` the AI handoff would be asked to finish reverting a
/// commit the reset may have just removed from the branch.
///
/// HEAD is not read here. The runner reads it before the revert step (see
/// [`session_runner::PipelineConfig::capture_pre_head`]), by which point this
/// row has made the branch busy, so nothing that goes through the branch queue
/// can move HEAD in between. Reset-to-origin, discard and delete-commit run
/// outside the queue: the marker above only stops a revert starting under a
/// reset or discard, only the frontend's busy-branch gates keep those two from
/// starting beside a running revert, and nothing gates a delete-commit then.
pub(super) fn insert_running_revert_pipeline_session_if_idle(
    store: &Arc<Store>,
    registry: &Arc<session_runner::SessionRegistry>,
    ctx: &BranchPipelineContext,
    steps: &[PipelineStep],
    commit_sha: &str,
    provider: Option<&str>,
) -> Result<RunningPipelineSession, String> {
    let launch_lock = crate::session_commands::branch_session_launch_lock_for(&ctx.branch.id);
    let _guard = launch_lock.lock().unwrap();
    if crate::session_commands::branch_worktree_operation_in_flight(&ctx.branch.id) {
        return Err(REVERT_WORKTREE_BUSY.to_string());
    }
    if branch_has_work_in_flight(store, &ctx.branch.id)? {
        return Err(REVERT_BRANCH_BUSY.to_string());
    }

    let prompt = format!("Revert commit {}", &commit_sha[..7.min(commit_sha.len())]);
    insert_running_pending_commit_session(
        store,
        registry,
        ctx,
        PipelineExecution::from_steps(steps),
        prompt,
        provider,
        true,
    )
}

/// Revert a completed commit by creating a new signed-off revert commit.
///
/// Runs `git revert --signoff --no-edit <sha>` directly. If git needs conflict
/// resolution, the pipeline hands off to AI with the pending commit row still
/// linked so completion can record the final HEAD.
///
/// Shared by the Tauri command and the web dispatch arm so both gates behave
/// identically.
pub(crate) fn revert_commit_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    commit_sha: String,
    provider: Option<String>,
) -> Result<String, String> {
    let commit_sha = validate_commit_sha(&commit_sha)?;
    let steps = build_revert_pipeline_steps(&commit_sha)?;
    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;

    let running = insert_running_revert_pipeline_session_if_idle(
        &store,
        &registry,
        &ctx,
        &steps,
        &commit_sha,
        provider.as_deref(),
    )?;
    launch_running_pipeline_session(
        ctx,
        running,
        steps,
        "commit",
        provider,
        store,
        &app_handle,
        &registry,
    )
}
