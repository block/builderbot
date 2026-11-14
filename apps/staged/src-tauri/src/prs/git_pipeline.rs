//! Starting a queued push or pull once it reaches the front of the branch
//! queue.
//!
//! The queued row is only a marker: its steps are rebuilt here from the
//! branch's current names and the persisted push flags, so a branch renamed
//! while the work waited still acts on the right ref.

use std::sync::Arc;

use crate::session_commands::{fail_claimed_queued_start, QueuedSessionClaim, QueuedSessionStart};
use crate::session_runner;
use crate::store::{self, PipelineExecution, PipelineKind, PipelineStep, Store};

use super::context::resolve_branch_pipeline_context;
use super::pipeline::pipeline_prompt;
use super::pull::build_pull_pipeline_steps;
use super::push::{build_push_pipeline_steps, PushVariant};

/// A queued push or pull, rebuilt for the run that is about to start.
pub(super) struct RebuiltGitPipeline {
    pub(super) steps: Vec<PipelineStep>,
    pub(super) pipeline: PipelineExecution,
    pub(super) prompt: &'static str,
    pub(super) session_type: &'static str,
}

/// Rebuild a queued push or pull from its persisted flags and the branch's
/// current names.
///
/// The [`PushVariant`] carries over and is persisted again on the running
/// pipeline, so a no-verify push that waited in the queue still skips the
/// `pre-push` hook. `depends_on_session_id` carries over too, so the push still
/// records what it waited on.
pub(super) fn rebuild_queued_git_pipeline(
    session_id: &str,
    queued: &PipelineExecution,
    local_branch: &str,
    remote_branch: &str,
) -> Result<RebuiltGitPipeline, String> {
    let kind = queued
        .kind
        .clone()
        .ok_or_else(|| format!("Queued session {session_id} has no pipeline kind"))?;
    let variant = PushVariant::of(queued);
    let (steps, session_type) = match kind {
        PipelineKind::Push => (
            build_push_pipeline_steps(local_branch, remote_branch, variant),
            "push",
        ),
        PipelineKind::Pull => (build_pull_pipeline_steps(remote_branch), "pull"),
        PipelineKind::Rebase | PipelineKind::Squash => {
            return Err(format!(
                "Queued git pipeline session {session_id} has non-git kind {kind:?}"
            ));
        }
    };
    let prompt = pipeline_prompt(&kind, variant);
    let pipeline = PipelineExecution::from_steps(&steps)
        .with_kind(kind)
        .with_push_force(variant.force)
        .with_push_no_verify(variant.no_verify)
        .with_depends_on_session_id(queued.depends_on_session_id.clone());
    Ok(RebuiltGitPipeline {
        steps,
        pipeline,
        prompt,
        session_type,
    })
}

/// Start a queued push or pull pipeline that reached the front of the branch
/// queue.
///
/// Steps are rebuilt from the branch's current name and the persisted push
/// flags rather than replayed from the queued pipeline (see
/// [`rebuild_queued_git_pipeline`]), matching
/// [`super::commit::start_queued_commit_pipeline_for_branch`] so a branch renamed while the work
/// waited still acts on the right ref. `claim` takes the row the same way too.
pub(crate) async fn start_queued_git_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    session: store::Session,
    provider: Option<String>,
    claim: QueuedSessionClaim,
) -> Result<QueuedSessionStart, String> {
    let queued_pipeline = session
        .pipeline
        .as_ref()
        .ok_or_else(|| format!("Queued session {} has no pipeline", session.id))?;

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let remote_branch = crate::branches::resolve_remote_branch(&store, &ctx.branch).await;
    let RebuiltGitPipeline {
        steps,
        pipeline,
        prompt,
        session_type,
    } = rebuild_queued_git_pipeline(
        &session.id,
        queued_pipeline,
        &ctx.branch.branch_name,
        &remote_branch,
    )?;
    let effective_provider = session.provider.clone().or(provider);

    // Live from before the claim flips the row to `running` until
    // `start_pipeline_session` has registered it (see
    // `SessionRegistry::hold`); released when this function returns.
    let _liveness = registry.hold(&session.id);

    let claimed = claim.claim(&store)?;
    if claimed != QueuedSessionStart::Started {
        return Ok(claimed);
    }

    let branch_id = ctx.branch.id.clone();
    let project_id = ctx.branch.project_id.clone();

    // No `mark_session_artifact_started` call: a git pipeline has no queued
    // artifact stub whose timestamp needs restamping when the work actually
    // starts.
    //
    // The row is ours and `running` from here: a failure before the handoff
    // ends it itself (see `fail_claimed_queued_start`).
    let prepared = (|| -> Result<(), String> {
        store
            .prepare_queued_session(&session.id, &ctx.working_dir.to_string_lossy(), prompt)
            .map_err(|e| e.to_string())?;
        store
            .update_session_pipeline(&session.id, &pipeline)
            .map_err(|e| e.to_string())
    })();
    if let Err(error) = prepared {
        return Err(fail_claimed_queued_start(
            &store,
            &app_handle,
            &branch_id,
            &project_id,
            &session.id,
            error,
        ));
    }

    session_runner::emit_session_running(
        &app_handle,
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
            capture_pre_head: false,
            provider: effective_provider,
            workspace_name: ctx.workspace_name,
            remote_working_dir: ctx.remote_working_dir,
            branch_id: Some(branch_id),
            project_id: Some(project_id),
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )
    .map(QueuedSessionStart::from_runner)
}
