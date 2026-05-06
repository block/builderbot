//! Revert: a run-now pipeline that creates a signed-off revert commit for a
//! completed commit on the branch.
//!
//! Unlike rebase and squash it never queues: it is refused while the branch
//! has work in flight, and runs `git revert --signoff --no-edit <sha>` with an
//! AI handoff only if git stops on a conflict. A pending-commit stub is linked
//! to the session so completion can record the revert commit's SHA.

use std::sync::Arc;

use crate::session_runner;
use crate::store::{self, FailureStrategy, PipelineExecution, PipelineStep, Store};

use super::context::{resolve_branch_pipeline_context, BranchPipelineContext};

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

/// HEAD of the checkout the pipeline will run in, read off a blocking thread
/// because on a remote branch it is a `ws exec` round trip.
async fn current_head_for_pipeline_context(ctx: &BranchPipelineContext) -> Result<String, String> {
    let working_dir = ctx.working_dir.clone();
    let workspace_name = ctx.workspace_name.clone();
    let remote_dir = ctx
        .remote_working_dir
        .as_deref()
        .and_then(|dir| dir.to_str())
        .map(str::to_string);
    tauri::async_runtime::spawn_blocking(move || {
        let git = crate::branches::git_runner(&working_dir, workspace_name.as_deref(), remote_dir);
        crate::branches::head_sha(&git)
    })
    .await
    .map_err(|e| format!("HEAD lookup task failed: {e}"))?
    .map_err(|e| format!("Failed to get HEAD SHA: {e}"))
}

async fn start_running_revert_pipeline_for_branch(
    ctx: BranchPipelineContext,
    commit_sha: String,
    provider: Option<String>,
    store: Arc<Store>,
    app_handle: &tauri::AppHandle,
    registry: &Arc<session_runner::SessionRegistry>,
) -> Result<String, String> {
    let steps = build_revert_pipeline_steps(&commit_sha)?;
    let prompt = format!("Revert commit {}", &commit_sha[..7.min(commit_sha.len())]);
    let pre_head_sha = Some(current_head_for_pipeline_context(&ctx).await?);
    let pipeline = PipelineExecution::from_steps(&steps);

    let mut session = store::Session::new_running(&prompt, &ctx.working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline.clone());
    store.create_session(&session).map_err(|e| e.to_string())?;

    let branch_id = ctx.branch.id.clone();
    let project_id = ctx.branch.project_id.clone();

    let commit = store::Commit::new_pending(&branch_id).with_session(&session.id);
    store.create_commit(&commit).map_err(|e| e.to_string())?;

    session_runner::emit_session_running(
        app_handle,
        &session.id,
        &branch_id,
        &project_id,
        "commit",
    );

    session_runner::start_pipeline_session(
        session_runner::PipelineConfig {
            session_id: session.id.clone(),
            prompt,
            steps,
            pipeline,
            working_dir: ctx.working_dir,
            pre_head_sha,
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

/// Revert a completed commit by creating a new signed-off revert commit.
///
/// Runs `git revert --signoff --no-edit <sha>` directly. If git needs conflict
/// resolution, the pipeline hands off to AI with the pending commit row still
/// linked so completion can record the final HEAD.
///
/// Shared by the Tauri command and the web dispatch arm so both gates behave
/// identically.
pub(crate) async fn revert_commit_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    commit_sha: String,
    provider: Option<String>,
) -> Result<String, String> {
    let commit_sha = validate_commit_sha(&commit_sha)?;
    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;

    let branch_has_running_session = store
        .has_running_session_for_branch(&branch_id)
        .map_err(|e| e.to_string())?;
    let branch_has_queued_session = !store
        .get_queued_sessions_for_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .is_empty();

    if branch_has_running_session || branch_has_queued_session {
        return Err(
            "Revert is unavailable while this branch has queued or running work".to_string(),
        );
    }

    start_running_revert_pipeline_for_branch(
        ctx,
        commit_sha,
        provider,
        store,
        &app_handle,
        &registry,
    )
    .await
}
