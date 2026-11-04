//! Rebase and squash: the pipelines that end in a commit.
//!
//! Both create a pending-commit stub alongside their session, so the timeline
//! shows the commit that is coming. A rebase can target the branch's base or
//! the branch's own remote ("rebase onto origin", for a diverged branch), and
//! "Rebase and force push" queues a force push gated on the rebase succeeding
//! in the same critical section as the rebase row.

use std::sync::Arc;

use crate::git;
use crate::session_commands::{fail_claimed_queued_start, QueuedSessionClaim, QueuedSessionStart};
use crate::session_runner;
use crate::store::{self, FailureStrategy, PipelineExecution, PipelineKind, PipelineStep, Store};

use super::context::{base_branch_name, resolve_branch_pipeline_context, BranchPipelineContext};
use super::git_commands::git_fetch_with_fallback;
use super::pipeline::{
    branch_has_work_in_flight, find_queued_pipeline, launch_running_pipeline_session,
    pipeline_prompt, BranchPipelineResponse, RunningPipelineSession,
};
use super::push::{queue_force_push_after_locked, PushVariant};

/// Determine the remote ref to rebase onto based on the `target` parameter.
///
/// - `None` or `Some("base")` → base branch name (e.g. `main`)
/// - `Some("origin")` → `remote_branch`, the branch on origin the checkout
///   pushes to (the PR head rather than the local name when the two differ)
pub(super) fn rebase_ref_for_target(
    branch: &store::Branch,
    remote_branch: &str,
    target: Option<&str>,
) -> String {
    match target {
        Some("origin") => remote_branch.to_string(),
        _ => base_branch_name(branch).to_string(),
    }
}

/// The rebase target to persist on the pipeline, or `None` when the pipeline
/// targets the branch's configured base.
///
/// Only "rebase onto origin" needs a persisted target; a base rebase re-derives
/// it from the branch on dequeue. Keeping this in one place means the queued and
/// running paths agree, which the same-kind dedupe check relies on.
fn persisted_rebase_target(target: Option<&str>, rebase_ref: &str) -> Option<String> {
    matches!(target, Some("origin")).then(|| rebase_ref.to_string())
}

/// Build the steps for a rebase or squash pipeline.
///
/// `base_branch` is the branch's configured upstream base (e.g. `main`). For
/// squash this is also the bound the reset must not cross. For rebase it is
/// purely informational in the AI handoff prompts so the agent can sanity-check
/// the requested target.
///
/// `rebase_target` is the remote ref the rebase will actually run against. It
/// equals `base_branch` for the default "rebase onto base" action and equals
/// the branch's own remote branch for "rebase onto origin" (used when local
/// has diverged from `origin/{branch}`). Only the rebase variant consults this
/// value; squash always operates against the base branch.
///
/// Every rebase variant runs [`git::rebase_preserving_merge_edits_command`]
/// rather than a bare `git rebase`, so merge commits that carry hand edits are
/// linearised into ordinary commits instead of being silently dropped.
///
/// Errors for [`PipelineKind::Push`] and [`PipelineKind::Pull`], which produce no
/// commit and belong to the git pipeline path — the only caller that reads the
/// kind from the database checks it first, so this guards against a misrouted
/// queued session rather than a reachable input.
pub(super) fn build_commit_pipeline_steps(
    kind: &PipelineKind,
    base_branch: &str,
    rebase_target: &str,
) -> Result<Vec<PipelineStep>, String> {
    let steps = match kind {
        PipelineKind::Rebase => {
            let target_note = if base_branch == rebase_target {
                String::new()
            } else {
                format!(
                    " The branch's configured base is `origin/{base_branch}`; the requested target `origin/{rebase_target}` is different. If `origin/{rebase_target}` is itself behind `origin/{base_branch}` by a non-trivial amount, surface that to the user before continuing — the rebase target may be wrong."
                )
            };
            let (fetch_label, rebase_label) = if base_branch == rebase_target {
                ("Fetch latest base".to_string(), "Rebase onto base".to_string())
            } else {
                (
                    format!("Fetch origin/{rebase_target}"),
                    format!("Rebase onto origin/{rebase_target}"),
                )
            };
            vec![
                PipelineStep::Command {
                    label: fetch_label,
                    command: git_fetch_with_fallback(rebase_target),
                    on_failure: FailureStrategy::HandoffToAi {
                        prompt_template: format!(
                            "The fetch failed. Diagnose and fix the issue, then rebase this branch onto `origin/{rebase_target}` with DCO signoffs. Resolve conflicts if present and continue the rebase. Do not push the branch.{target_note}\n\n{{step_outputs}}"
                        ),
                    },
                },
                PipelineStep::Command {
                    label: rebase_label,
                    command: git::rebase_preserving_merge_edits_command(&format!(
                        "origin/{rebase_target}"
                    )),
                    on_failure: FailureStrategy::HandoffToAi {
                        prompt_template: format!(
                            "The rebase onto `origin/{rebase_target}` failed. Inspect the output and recover from the actual failure. If the rebase is stopped on a conflict, resolve it, stage the resolution, and run `git rebase --continue` (the rebase already applies DCO signoffs) until it finishes. Never restart the rebase and never run a plain `git rebase`: commits titled \"{hand_edit_prefix}<sha>\" carry content that existed only inside that merge commit and is in no other commit, so a fresh rebase would lose it. Resolve conflicts in those commits so the content is kept, and never drop or skip them. `git rebase --abort` is the user's decision, not yours; if you cannot resolve a conflict, leave the rebase in progress and report why. If the output says the rebase was refused before it started (for example because git is too old), do not attempt it another way; report the reason to the user. Do not push the branch.{target_note}\n\n{{step_outputs}}",
                            hand_edit_prefix = git::HAND_EDIT_COMMIT_SUBJECT_PREFIX,
                        ),
                    },
                },
            ]
        }
        PipelineKind::Squash => vec![
            // Fetch first so origin/{base} is up-to-date before computing the
            // merge-base for the destructive soft reset. Without this, a stale
            // remote-tracking ref could cause the reset to target the wrong commit.
            PipelineStep::Command {
                label: "Fetch latest base".to_string(),
                command: git_fetch_with_fallback(base_branch),
                on_failure: FailureStrategy::HandoffToAi {
                    prompt_template: format!(
                        "The fetch failed. Diagnose and fix the issue, then squash only this branch's commits manually. Do not squash beyond the merge-base with `origin/{base_branch}`. Create one signed-off conventional commit from the staged changes and do not push the branch.\n\n{{step_outputs}}"
                    ),
                },
            },
            PipelineStep::Command {
                label: "View commit history".to_string(),
                command: format!("git log --oneline origin/{base_branch}..HEAD"),
                on_failure: FailureStrategy::Continue,
            },
            PipelineStep::Command {
                label: "Soft reset to merge-base".to_string(),
                command: format!(
                    r#"merge_base=$(git merge-base origin/{base_branch} HEAD) && git reset --soft "$merge_base""#
                ),
                on_failure: FailureStrategy::HandoffToAi {
                    prompt_template: format!(
                        "The soft reset failed. Diagnose and fix the issue, then squash only this branch's commits manually. Do not squash beyond the merge-base with `origin/{base_branch}`. Create one signed-off conventional commit from the staged changes and do not push the branch.\n\n{{step_outputs}}"
                    ),
                },
            },
            PipelineStep::AiHandoff {
                label: "Write squashed commit message".to_string(),
                prompt_template: r#"<action>
Create a single conventional-commit from the staged changes.

The branch's commits have been soft-reset to the merge-base and are staged. Now create one commit:
- The commit message MUST use conventional commit style (e.g., "feat: add user authentication", "fix: resolve null pointer in parser")
- Choose the most appropriate conventional commit type (feat, fix, refactor, docs, style, test, chore, perf, ci, build) based on the actual changes
- Use the user's global git identity for author and committer
- Create the commit with DCO signoff (`git commit --signoff`)
- Use the original commit history (from step 2) as context to write a meaningful message
- Do NOT push the branch

Here is the context from the prior steps:

{step_outputs}
</action>"#
                    .to_string(),
            },
        ],
        PipelineKind::Push | PipelineKind::Pull => {
            return Err(format!("{kind:?} is not a commit pipeline"));
        }
    };

    Ok(steps)
}

/// Insert the session and pending-commit rows for a rebase/squash that runs now.
///
/// Callers must hold the branch launch lock: these rows are what make the branch
/// look busy to a concurrent launch, so the busy check and this insert have to be
/// atomic. Synchronous for the same reason — the lock must not be held across an
/// await.
pub(super) fn insert_running_commit_pipeline_session(
    store: &Arc<Store>,
    registry: &Arc<session_runner::SessionRegistry>,
    ctx: &BranchPipelineContext,
    kind: PipelineKind,
    steps: &[PipelineStep],
    rebase_target: Option<String>,
    provider: Option<&str>,
) -> Result<RunningPipelineSession, String> {
    let prompt = pipeline_prompt(&kind, PushVariant::default());
    let mut pipeline = PipelineExecution::from_steps(steps).with_kind(kind);
    if let Some(target) = rebase_target {
        pipeline = pipeline.with_rebase_target(target);
    }

    let mut session =
        store::Session::new_running(prompt, &ctx.working_dir).with_branch(&ctx.branch.id);
    if let Some(p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline.clone());
    let liveness = registry.hold(&session.id);
    store.create_session(&session).map_err(|e| e.to_string())?;

    let commit = store::Commit::new_pending(&ctx.branch.id).with_session(&session.id);
    store.create_commit(&commit).map_err(|e| e.to_string())?;

    Ok(RunningPipelineSession {
        session_id: session.id,
        pipeline,
        prompt: prompt.to_string(),
        pre_head_sha: None,
        _liveness: liveness,
    })
}

/// Queue a rebase/squash pipeline when the branch has work in flight.
///
/// Returns the queued session id — either a freshly created one, or an existing
/// queued pipeline that already covers this request — and `None` when the branch
/// is idle so the caller should start the pipeline immediately.
///
/// The busy check, the dedupe scan, and the insert all run under the branch
/// launch lock, so two rapid clicks (or a click racing a session start) cannot
/// both observe an idle branch or both miss the same queued pipeline. This stays
/// synchronous on purpose: the lock must not be held across an await.
///
/// With `then_force_push`, the force push that follows the rebase is queued in
/// the same critical section (see [`with_follow_up_force_push_locked`]).
pub(super) fn queue_commit_pipeline_if_branch_busy(
    store: &Arc<Store>,
    branch_id: &str,
    kind: &PipelineKind,
    provider: Option<&str>,
    target: Option<&str>,
    then_force_push: bool,
) -> Result<Option<BranchPipelineResponse>, String> {
    let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
    let _guard = launch_lock.lock().unwrap();
    queue_commit_pipeline_locked(store, branch_id, kind, provider, target)?
        .map(|session_id| {
            with_follow_up_force_push_locked(
                store,
                branch_id,
                provider,
                BranchPipelineResponse::queued(session_id),
                then_force_push,
            )
        })
        .transpose()
}

/// Queue the force push of "Rebase and force push" behind `response`'s rebase,
/// gated on that rebase succeeding, and report it on the response.
///
/// Callers must hold the branch launch lock, the same hold that put the rebase
/// row in place. That is what guarantees the push row exists before the rebase
/// can finish: the drain that runs on the rebase's terminal state is the only
/// thing that releases (or skips) the push, so a push inserted after it ran
/// would sit in the queue with nothing left to drain it.
pub(super) fn with_follow_up_force_push_locked(
    store: &Arc<Store>,
    branch_id: &str,
    provider: Option<&str>,
    mut response: BranchPipelineResponse,
    then_force_push: bool,
) -> Result<BranchPipelineResponse, String> {
    if then_force_push {
        response.follow_up_session_id = Some(queue_force_push_after_locked(
            store,
            branch_id,
            provider,
            &response.session_id,
        )?);
    }
    Ok(response)
}

/// [`with_follow_up_force_push_locked`] for a rebase row already inserted as
/// `running`, with the failure path repaired: if the push cannot be queued,
/// the rebase is marked errored with the store error rather than left
/// `running`. Left as it was, the row would have no runner and no cancel
/// token, and would keep the branch busy until the startup sweep.
///
/// Failing the rebase is simpler than inserting the push first. The push row
/// is stamped from the rebase row — its id as the dependency and a
/// `created_at` strictly after it — so the inserts cannot be swapped without
/// restructuring the push path, and a push left behind by a failed rebase
/// insert would be a queued row gated on a session that never existed, which
/// keeps the branch busy with nothing to drain it. This runs under the same
/// launch lock and before the row is announced, so no client has seen the
/// rebase running when it is failed.
///
/// The failure goes through [`session_runner::finish_failed_before_run`], so
/// the `error` status event is emitted the way every other pre-run failure
/// emits it. The requesting client only toasts the returned error; without the
/// event, the pending commit stub the rebase insert created would show as
/// failed on every client only after the next unrelated timeline refresh.
/// Nothing is drained on this path: the branch was idle when the rebase row
/// went in, and the lock held since then kept it that way.
pub(super) fn with_follow_up_force_push_or_fail_rebase_locked<R: tauri::Runtime>(
    store: &Arc<Store>,
    app_handle: &tauri::AppHandle<R>,
    branch_id: &str,
    project_id: &str,
    provider: Option<&str>,
    rebase_session_id: &str,
    then_force_push: bool,
) -> Result<BranchPipelineResponse, String> {
    let result = with_follow_up_force_push_locked(
        store,
        branch_id,
        provider,
        BranchPipelineResponse::running(rebase_session_id.to_string()),
        then_force_push,
    );
    if let Err(reason) = &result {
        // The row was inserted under this lock and never announced, so no
        // Stop can have taken it first; the transition is expected to win,
        // and there is no queue to drain whichever way it goes.
        let _ = session_runner::finish_failed_before_run(
            rebase_session_id,
            Some(branch_id.to_string()),
            Some(project_id.to_string()),
            store,
            app_handle,
            reason,
        );
    }
    result
}

/// The body of [`queue_commit_pipeline_if_branch_busy`], for the run-now path,
/// which re-checks while already holding the branch launch lock.
pub(super) fn queue_commit_pipeline_locked(
    store: &Arc<Store>,
    branch_id: &str,
    kind: &PipelineKind,
    provider: Option<&str>,
    target: Option<&str>,
) -> Result<Option<String>, String> {
    crate::project_transfer::ensure_branch_not_transferring(branch_id)?;
    if !branch_has_work_in_flight(store, branch_id)? {
        return Ok(None);
    }

    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    let base_branch = base_branch_name(&branch);
    // A queued "rebase onto origin" marks its target with the branch's own
    // local name. The branch on origin is resolved — workspace-aware, and
    // without a git subprocess under this lock — when the row is dequeued,
    // the same way a queued push or pull is rebuilt.
    let rebase_ref = rebase_ref_for_target(&branch, &branch.branch_name, target);
    let rebase_target = persisted_rebase_target(target, &rebase_ref);

    if let Some(existing) = find_queued_pipeline(store, branch_id, |pipeline| {
        pipeline.kind.as_ref() == Some(kind)
            && pipeline.rebase_target.as_deref() == rebase_target.as_deref()
    })? {
        return Ok(Some(existing));
    }

    let steps = build_commit_pipeline_steps(kind, base_branch, &rebase_ref)?;
    let mut pipeline = PipelineExecution::from_steps(&steps).with_kind(kind.clone());
    if let Some(target) = rebase_target {
        pipeline = pipeline.with_rebase_target(target);
    }
    let mut session = store::Session::new_queued(pipeline_prompt(kind, PushVariant::default()))
        .with_branch(branch_id);
    if let Some(p) = provider {
        session = session.with_provider(p);
    }
    session.pipeline = Some(pipeline);
    store.create_session(&session).map_err(|e| e.to_string())?;

    let commit = store::Commit::new_pending(branch_id).with_session(&session.id);
    store.create_commit(&commit).map_err(|e| e.to_string())?;

    Ok(Some(session.id))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_or_queue_commit_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    kind: PipelineKind,
    provider: Option<String>,
    target: Option<String>,
    then_force_push: bool,
) -> Result<BranchPipelineResponse, String> {
    if then_force_push && kind != PipelineKind::Rebase {
        return Err(format!("{kind:?} cannot be followed by a force push"));
    }

    // Pre-flight check: a branch that is already busy queues without resolving a
    // pipeline context, which for a remote branch can depend on a running
    // workspace the queued work will only need later.
    if let Some(response) = queue_commit_pipeline_if_branch_busy(
        &store,
        &branch_id,
        &kind,
        provider.as_deref(),
        target.as_deref(),
        then_force_push,
    )? {
        return Ok(response);
    }

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let base_branch = base_branch_name(&ctx.branch).to_string();
    let remote_branch = crate::branches::resolve_remote_branch(&store, &ctx.branch).await;
    let rebase_ref = rebase_ref_for_target(&ctx.branch, &remote_branch, target.as_deref());
    let steps = build_commit_pipeline_steps(&kind, &base_branch, &rebase_ref)?;
    let rebase_target = persisted_rebase_target(target.as_deref(), &rebase_ref);

    // Re-check and insert under the lock, mirroring
    // `session_commands::start_or_queue_branch_session_for_store`: the pre-flight
    // check released the lock to resolve the context above, so without a second
    // look two near-simultaneous actions could both have seen an idle branch and
    // both start running — exactly what git-pipeline exclusivity exists to stop.
    let (running, response) = {
        let launch_lock = crate::session_commands::branch_session_launch_lock_for(&branch_id);
        let _guard = launch_lock.lock().unwrap();

        if let Some(session_id) = queue_commit_pipeline_locked(
            &store,
            &branch_id,
            &kind,
            provider.as_deref(),
            target.as_deref(),
        )? {
            return with_follow_up_force_push_locked(
                &store,
                &branch_id,
                provider.as_deref(),
                BranchPipelineResponse::queued(session_id),
                then_force_push,
            );
        }

        let running = insert_running_commit_pipeline_session(
            &store,
            &registry,
            &ctx,
            kind,
            &steps,
            rebase_target,
            provider.as_deref(),
        )?;
        // Still under the lock and before the runner has the rebase, so the
        // push is queued before the rebase can possibly finish.
        let response = with_follow_up_force_push_or_fail_rebase_locked(
            &store,
            &app_handle,
            &branch_id,
            &ctx.branch.project_id,
            provider.as_deref(),
            &running.session_id,
            then_force_push,
        )?;
        (running, response)
    };

    launch_running_pipeline_session(
        ctx,
        running,
        steps,
        "commit",
        provider,
        store,
        &app_handle,
        &registry,
    )?;

    Ok(response)
}

/// Start a queued rebase or squash that the branch queue (or a forced start)
/// has decided may run.
///
/// A persisted `rebase_target` only says the rebase is "onto origin"; the
/// branch on origin is resolved here, not replayed from the queued row, so a
/// Blox branch (whose queue path never asks its workspace) and a branch
/// renamed while it waited both rebase onto the branch they actually push to.
///
/// `claim` is what actually takes the row: it re-checks the branch under the
/// launch lock and flips the row to `running` in one step, so the decision the
/// caller made before its awaits cannot be overtaken by another start.
pub(crate) async fn start_queued_commit_pipeline_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    session: store::Session,
    provider: Option<String>,
    claim: QueuedSessionClaim,
) -> Result<QueuedSessionStart, String> {
    let kind = session
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.kind.clone())
        .ok_or_else(|| format!("Queued session {} has no pipeline kind", session.id))?;
    let target = session
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.rebase_target.as_deref())
        .map(|_| "origin");

    let ctx = resolve_branch_pipeline_context(&store, &branch_id)?;
    let base_branch = base_branch_name(&ctx.branch).to_string();
    let remote_branch = if target.is_some() {
        crate::branches::resolve_remote_branch(&store, &ctx.branch).await
    } else {
        ctx.branch.branch_name.clone()
    };
    let rebase_ref = rebase_ref_for_target(&ctx.branch, &remote_branch, target);
    let rebase_target = persisted_rebase_target(target, &rebase_ref);
    let steps = build_commit_pipeline_steps(&kind, &base_branch, &rebase_ref)?;
    let prompt = pipeline_prompt(&kind, PushVariant::default());
    let mut pipeline = PipelineExecution::from_steps(&steps).with_kind(kind);
    if let Some(target) = rebase_target {
        pipeline = pipeline.with_rebase_target(target);
    }
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

    // The row is ours and `running` from here: a failure before the handoff
    // ends it itself (see `fail_claimed_queued_start`).
    let prepared = (|| -> Result<(), String> {
        store
            .mark_session_artifact_started(&session.id)
            .map_err(|e| e.to_string())?;

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
        "commit",
    );

    session_runner::start_pipeline_session(
        session_runner::PipelineConfig {
            session_id: session.id.clone(),
            prompt: prompt.to_string(),
            steps,
            pipeline,
            working_dir: ctx.working_dir,
            pre_head_sha: None,
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
