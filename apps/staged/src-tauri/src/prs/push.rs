//! The push pipeline and its variants.
//!
//! A push is one step, built from the branch's current local and remote names
//! and a [`PushVariant`] (force, `--no-verify`). Each variant is its own
//! operation: queued pushes dedupe per variant, and a queued push is rebuilt
//! from its persisted flags when it is dequeued (see [`super::git_pipeline`]).
//! "Rebase and force push" queues its push here too, gated on the rebase.

use std::path::Path;
use std::sync::Arc;

use crate::git;
use crate::session_runner;
use crate::store::{self, FailureStrategy, PipelineExecution, PipelineKind, PipelineStep, Store};

use super::context::{resolve_branch_pipeline_context, BranchPipelineContext};
use super::git_commands::{git_push_with_fallback, push_refspec};
use super::pipeline::{
    branch_has_work_in_flight, find_queued_pipeline, launch_running_pipeline_session,
    pipeline_prompt, BranchPipelineResponse, RunningPipelineSession,
};

/// What a push pipeline does: whether it force-pushes (`--force-with-lease`),
/// and whether it skips git's `pre-push` hook (`--no-verify`).
///
/// Persisted on the pipeline as `push_force` / `push_no_verify`, so a queued
/// push re-derives the same command on dequeue. Every combination is its own
/// operation: the queue never folds one variant into another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PushVariant {
    pub force: bool,
    pub no_verify: bool,
}

impl PushVariant {
    /// The variant a persisted pipeline records. Non-push pipelines record
    /// neither flag, so they read as the default.
    pub(super) fn of(pipeline: &PipelineExecution) -> Self {
        Self {
            force: pipeline.push_force,
            no_verify: pipeline.push_no_verify,
        }
    }

    /// The variant a `push_branch` request asks for; an omitted flag is off.
    pub(crate) fn from_request(force: Option<bool>, no_verify: Option<bool>) -> Self {
        Self {
            force: force.unwrap_or(false),
            no_verify: no_verify.unwrap_or(false),
        }
    }

    /// Flags shared by both halves of [`git_push_with_fallback`].
    fn git_flags(self) -> &'static str {
        if self.no_verify {
            "--no-verify "
        } else {
            ""
        }
    }
}

/// Build the single step that pushes the branch to its remote.
///
/// Rebuilt from the persisted `push_force` / `push_no_verify` flags on dequeue
/// rather than replayed from the queued pipeline, so a queued push always
/// pushes the branch's current name with the command the user asked for.
///
/// The refspec is always explicit (`local:remote`) so a de-duplicated local
/// name (`feature-x-2`) still lands on the PR's branch (`feature-x`) instead of
/// creating a new remote branch; `-u` then tracks `origin/{remote}`.
///
/// `--no-verify` skips only git's `pre-push` hook; server-side checks still
/// apply. It goes into both halves of the HTTPS fallback, so the retry doesn't
/// run the hook either.
pub(crate) fn build_push_pipeline_steps(
    local_branch: &str,
    remote_branch: &str,
    variant: PushVariant,
) -> Vec<PipelineStep> {
    let refspec = push_refspec(local_branch, remote_branch);
    let flags = variant.git_flags();
    let push_command = if variant.force {
        git_push_with_fallback(&format!("{flags}-u origin {refspec} --force-with-lease"))
    } else {
        git_push_with_fallback(&format!("{flags}-u origin {refspec}"))
    };

    let on_failure = if variant.force {
        // Spell the refspec out: with the upstream on `origin/{remote}` a
        // bare `git push` is refused under `push.default=simple` when the
        // names differ, and the obvious fallback `git push origin {local}`
        // would recreate the stray remote branch the refspec avoids. The
        // flags are spelled out too, or the agent's retry runs the hook the
        // user asked to skip.
        FailureStrategy::HandoffToAi {
            prompt_template: format!(
                "The force push failed. Diagnose and fix the issue, then retry the force push with exactly `git push {flags}-u origin {refspec} --force-with-lease`. Do not push `{local_branch}` under any other remote name.\n\n{{step_outputs}}"
            ),
        }
    } else {
        // For normal push, abort on non-fast-forward so the frontend can show
        // the force-push dialog. The marker matches git's actual stderr output
        // (e.g. "! [rejected] main -> main (non-fast-forward)").
        //
        // If the push fails for a *different* reason (e.g. auth error, network
        // timeout), the marker won't match and the pipeline falls through to an
        // AI handoff for generic diagnosis — this is intentional.
        FailureStrategy::Abort {
            marker: Some("non-fast-forward".to_string()),
        }
    };

    vec![PipelineStep::Command {
        label: "Push to remote".to_string(),
        command: push_command,
        on_failure,
    }]
}

/// Insert the session row for a push that runs right now.
///
/// Unlike [`super::pipeline::start_pipeline_for_branch`], the session records its pipeline kind
/// and `branch_id`. A push creates no artifact, so without that link the branch
/// queue could not see it and a commit session could start mid-push.
///
/// Like [`super::commit::insert_running_commit_pipeline_session`], callers must hold the branch
/// launch lock: this row is what makes the branch look busy.
pub(super) fn insert_running_push_pipeline_session(
    store: &Arc<Store>,
    registry: &Arc<session_runner::SessionRegistry>,
    ctx: &BranchPipelineContext,
    variant: PushVariant,
    steps: &[PipelineStep],
    provider: Option<&str>,
) -> Result<RunningPipelineSession, String> {
    let prompt = pipeline_prompt(&PipelineKind::Push, variant);
    let pipeline = PipelineExecution::from_steps(steps)
        .with_kind(PipelineKind::Push)
        .with_push_force(variant.force)
        .with_push_no_verify(variant.no_verify);

    let mut session =
        store::Session::new_running(prompt, &ctx.working_dir).with_branch(&ctx.branch.id);
    if let Some(p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline.clone());
    let liveness = registry.hold(&session.id);
    store.create_session(&session).map_err(|e| e.to_string())?;

    Ok(RunningPipelineSession {
        session_id: session.id,
        pipeline,
        prompt: prompt.to_string(),
        pre_head_sha: None,
        _liveness: liveness,
    })
}

/// Queue a push pipeline when the branch has work in flight.
///
/// Returns the queued session id — either a freshly created one, or an existing
/// queued push that already covers this request — and `None` when the branch is
/// idle so the caller should push immediately.
///
/// Mirrors [`super::commit::queue_commit_pipeline_if_branch_busy`]: the busy check, the dedupe
/// scan, and the insert all run under the branch launch lock, and stay
/// synchronous because the lock must not be held across an await. Each
/// [`PushVariant`] dedupes separately — they are different operations, so a
/// queued normal push must not swallow a force push request, and a push that
/// runs the `pre-push` hook must not swallow one that skips it (or the
/// reverse).
pub(super) fn queue_push_pipeline_if_branch_busy(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    variant: PushVariant,
) -> Result<Option<String>, String> {
    let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
    let _guard = launch_lock.lock().unwrap();
    queue_push_pipeline_locked(store, branch_id, provider, variant)
}

/// The body of [`queue_push_pipeline_if_branch_busy`], for the run-now path,
/// which re-checks while already holding the branch launch lock.
pub(super) fn queue_push_pipeline_locked(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    variant: PushVariant,
) -> Result<Option<String>, String> {
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;
    if !branch_has_work_in_flight(store, branch_id)? {
        return Ok(None);
    }

    // A push gated on a rebase is a different request from an unconditional
    // one: folding a plain push into it would make the user's push depend on a
    // rebase they didn't ask about.
    if let Some(existing) = find_queued_pipeline(store, branch_id, |pipeline| {
        pipeline.kind.as_ref() == Some(&PipelineKind::Push)
            && PushVariant::of(pipeline) == variant
            && pipeline.depends_on_session_id.is_none()
    })? {
        return Ok(Some(existing));
    }

    insert_queued_push_session(store, branch_id, provider, variant, None).map(Some)
}

/// The push "Rebase and force push" queues behind its rebase. It always runs
/// the `pre-push` hook: the menu offers no variant that skips it.
const FOLLOW_UP_FORCE_PUSH: PushVariant = PushVariant {
    force: true,
    no_verify: false,
};

/// Queue the force push that runs after `dependency_id` — the rebase of a
/// "Rebase and force push" — and only if that rebase succeeds.
///
/// Always queues, even on an idle branch: the push must not start before the
/// rebase has run, and the drain that follows the rebase is what decides
/// whether it starts at all (see `pipeline_dependency`). A repeat request for
/// the same rebase dedupes onto the push already waiting on it.
///
/// Callers must hold the branch launch lock.
pub(super) fn queue_force_push_after_locked(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    dependency_id: &str,
) -> Result<String, String> {
    if let Some(existing) = find_queued_pipeline(store, branch_id, |pipeline| {
        pipeline.kind.as_ref() == Some(&PipelineKind::Push)
            && PushVariant::of(pipeline) == FOLLOW_UP_FORCE_PUSH
            && pipeline.depends_on_session_id.as_deref() == Some(dependency_id)
    })? {
        return Ok(existing);
    }

    insert_queued_push_session(
        store,
        branch_id,
        provider,
        FOLLOW_UP_FORCE_PUSH,
        Some(dependency_id),
    )
}

/// Insert a queued push row, optionally gated on another session.
fn insert_queued_push_session(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    variant: PushVariant,
    depends_on: Option<&str>,
) -> Result<String, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    // A queue marker only: the steps are rebuilt with a full resolution on
    // dequeue, so the branch on origin is not looked up here — that would be
    // a git subprocess under the branch launch lock.
    let steps = build_push_pipeline_steps(&branch.branch_name, &branch.branch_name, variant);
    let pipeline = PipelineExecution::from_steps(&steps)
        .with_kind(PipelineKind::Push)
        .with_push_force(variant.force)
        .with_push_no_verify(variant.no_verify)
        .with_depends_on_session_id(depends_on.map(str::to_string));
    // No artifact row: a push produces no commit, and a pending-commit stub
    // would render as a failed commit once the push finishes without a new sha.
    // `branch_id` is what keeps this session on the branch queue instead.
    let mut session = store::Session::new_queued(pipeline_prompt(&PipelineKind::Push, variant))
        .with_branch(branch_id);
    if let Some(dependency_id) = depends_on {
        // Sort strictly after the dependency. The two rows are created in the
        // same critical section, so they can share a millisecond, and
        // `created_at` is what orders both the branch queue and the timeline.
        if let Some(dependency) = store
            .get_session(dependency_id)
            .map_err(|e| e.to_string())?
        {
            if session.created_at <= dependency.created_at {
                session.created_at = dependency.created_at + 1;
                session.updated_at = session.created_at;
            }
        }
    }
    if let Some(p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline);
    store.create_session(&session).map_err(|e| e.to_string())?;

    Ok(session.id)
}

pub(crate) async fn has_unpushed_commits_impl(
    store: Arc<Store>,
    branch_id: String,
) -> Result<bool, String> {
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if branch.branch_type == store::BranchType::Remote {
        let workspace_name = branch
            .workspace_name
            .as_deref()
            .ok_or_else(|| format!("Branch has no workspace name: {branch_id}"))?
            .to_string();
        let repo_subpath = crate::branches::resolve_branch_workspace_subpath(&store, &branch)?;
        let local_branch = branch.branch_name.clone();
        let base_branch = branch.base_branch.clone();

        // Run the blocking SSH calls on a background thread so we don't
        // block the Tauri IPC thread and freeze the UI.
        let result = tauri::async_runtime::spawn_blocking(move || {
            let remote_branch = crate::branches::workspace_upstream_branch_name(
                &workspace_name,
                repo_subpath.as_deref(),
                &local_branch,
                &base_branch,
            );
            let remote_ref = format!("origin/{remote_branch}");
            // Remote tracking branch doesn't exist — all commits are unpushed
            if crate::branches::run_workspace_git(
                &workspace_name,
                repo_subpath.as_deref(),
                &["rev-parse", "--verify", &remote_ref],
            )
            .is_err()
            {
                return Ok(true);
            }
            let rev_range = format!("{remote_ref}..HEAD");
            let output = crate::branches::run_workspace_git(
                &workspace_name,
                repo_subpath.as_deref(),
                &["rev-list", &rev_range],
            )
            .map_err(|e| e.to_string())?;
            Ok(!output.trim().is_empty())
        })
        .await
        .map_err(|e| format!("has_unpushed_commits task failed: {e}"))?;
        return result;
    }

    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

    // Run the blocking git subprocesses on a background thread so a slow cold
    // `git` invocation can't block the Tauri IPC thread and freeze the UI,
    // matching the remote path above.
    let path = workdir.path.clone();
    let local_branch = branch.branch_name.clone();
    let base_branch = branch.base_branch.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let worktree = Path::new(&path);
        let remote_branch = git::upstream_branch_name(worktree, &local_branch, &base_branch);
        git::has_unpushed_commits(worktree, &remote_branch).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("has_unpushed_commits task failed: {e}"))?
}

/// Push a branch to its remote by kicking off an agent session, or queue the
/// push behind whatever the branch is already doing.
pub(crate) async fn start_or_queue_push_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    variant: PushVariant,
) -> Result<BranchPipelineResponse, String> {
    // Pre-flight check, then a re-check at insert time — see
    // `start_or_queue_commit_pipeline_for_branch` for why both are needed.
    if let Some(session_id) =
        queue_push_pipeline_if_branch_busy(&store, &branch_id, provider.as_deref(), variant)?
    {
        return Ok(BranchPipelineResponse::queued(session_id));
    }

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let remote_branch = crate::branches::resolve_remote_branch(&store, &ctx.branch).await;
    let steps = build_push_pipeline_steps(&ctx.branch.branch_name, &remote_branch, variant);

    let running = {
        let launch_lock = crate::session_commands::branch_session_launch_lock_for(&branch_id);
        let _guard = launch_lock.lock().unwrap();

        if let Some(session_id) =
            queue_push_pipeline_locked(&store, &branch_id, provider.as_deref(), variant)?
        {
            return Ok(BranchPipelineResponse::queued(session_id));
        }

        insert_running_push_pipeline_session(
            &store,
            &registry,
            &ctx,
            variant,
            &steps,
            provider.as_deref(),
        )?
    };

    let session_id = launch_running_pipeline_session(
        ctx,
        running,
        steps,
        "push",
        provider,
        store,
        &app_handle,
        &registry,
    )?;

    Ok(BranchPipelineResponse::running(session_id))
}
