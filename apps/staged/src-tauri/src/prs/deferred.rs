//! Deferred push ("Push after current work").
//!
//! A push armed on the branch row (`branches.deferred_push`, see
//! [`store::DeferredPush`]) that fires once the branch and its project are
//! idle. It is a flag rather than a queued push row because a queued row sits
//! in the branch FIFO — it would block work queued behind it, and the queue is
//! blind to project sessions. The fired push is an ordinary push pipeline, so
//! the per-variant dedupe, the prompts and the `push-completed` handling all
//! apply.
//!
//! The drain tail of `session_queue` calls [`fire_deferred_push_if_idle`] when
//! nothing queued could start; arming and cancelling are commands.

use serde::Serialize;
use std::future::Future;
use std::sync::Arc;

use crate::session_runner;
use crate::store::{self, PipelineKind, StepStatus, Store};

use super::pipeline::BranchPipelineResponse;
use super::push::{start_or_queue_push_pipeline_for_branch, PushVariant};

impl PushVariant {
    /// The variant a deferred push was armed with.
    pub(super) fn of_deferred(deferred: &store::DeferredPush) -> Self {
        Self {
            force: deferred.force,
            no_verify: deferred.no_verify,
        }
    }
}

/// What `defer_branch_push` did: armed the flag, or — the branch and project
/// were already idle when it ran — pushed right away, in which case `push` is
/// the push's start-or-queue response.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeferredPushResponse {
    pub armed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push: Option<BranchPipelineResponse>,
}

/// The sessions a deferred push is waiting on: the branch's sessions
/// `running`, `disconnected` or `queued` (commit, note, review, pipeline,
/// background hold or Blox turn alike), and the project's own sessions in any
/// of those states.
///
/// "Fully idle" rather than "the sessions running at arm time" on purpose: a
/// project session can enqueue branch work through the project MCP right
/// before it ends, and the branch check catches that. The ids are kept so the
/// drain can say what it is waiting on.
#[derive(Debug, Default, PartialEq, Eq)]
struct DeferredPushBlockers {
    branch_sessions: Vec<String>,
    project_sessions: Vec<String>,
}

impl DeferredPushBlockers {
    fn is_empty(&self) -> bool {
        self.branch_sessions.is_empty() && self.project_sessions.is_empty()
    }
}

fn deferred_push_blockers(
    store: &Arc<Store>,
    branch: &store::Branch,
) -> Result<DeferredPushBlockers, String> {
    let busy = store
        .get_busy_sessions_for_branch(&branch.id)
        .map_err(|e| e.to_string())?;
    let queued = store
        .get_queued_sessions_for_branch(&branch.id)
        .map_err(|e| e.to_string())?;
    Ok(DeferredPushBlockers {
        branch_sessions: busy
            .into_iter()
            .chain(queued)
            .map(|session| session.id)
            .collect(),
        project_sessions: store
            .project_session_ids_in_flight(&branch.project_id)
            .map_err(|e| e.to_string())?,
    })
}

/// Whether the branch and its project have nothing in flight (see
/// [`deferred_push_blockers`]).
fn branch_and_project_idle(store: &Arc<Store>, branch: &store::Branch) -> Result<bool, String> {
    Ok(deferred_push_blockers(store, branch)?.is_empty())
}

/// The push session that already did a deferred push's job, if one has: a
/// push of `variant` that completed successfully on the branch at or after
/// `requested_at` (ms, when the deferred push was armed) *and* is the last
/// thing that ended on the branch since.
///
/// The case: a push queued behind the current work *and* "Push after current
/// work" armed on top of it. The queued push runs first and the deferred one
/// would otherwise fire a second, redundant push the moment the branch goes
/// idle. "Successfully" is the pipeline's own verdict — completed without the
/// agent, or every step succeeded or was skipped — since a rejection the agent
/// recovered from may have pushed something else.
///
/// Both halves of the test use clocks that fail toward firing:
///
/// - *When the push finished* is the pipeline's own
///   [`PipelineExecution::finished_at`], not the row's `updated_at`, which
///   `project_transfer` and `branch_move` bump for every session on a branch
///   long after it ended. An old push whose row was rewritten that way does
///   not count, since its steps still carry their original timestamps.
/// - *Nothing ended after it* reads the other sessions' `updated_at` through
///   [`Store::branch_session_ids_ended_since`], which only ever moves later
///   and counts a tie as "after". A session queued behind the push that
///   commits and completes once the push is done is exactly what the
///   deferred push exists to carry, so a push with anything terminal after it
///   satisfies nothing. The cost of the pessimism is a redundant push, which
///   git answers with "Everything up-to-date".
///
/// Of several qualifying pushes the latest is judged: anything ending after it
/// ended after the earlier ones too.
fn push_satisfying_deferred(
    store: &Arc<Store>,
    branch_id: &str,
    variant: PushVariant,
    requested_at: i64,
) -> Result<Option<String>, String> {
    // `updated_at >= requested_at` is a pre-filter that cannot exclude a push
    // which finished since the arm (its row was written at completion, after
    // its last step); the pipeline's own timestamp decides below.
    let completed = store
        .get_completed_sessions_for_branch_since(branch_id, requested_at)
        .map_err(|e| e.to_string())?;
    let latest = completed
        .into_iter()
        .filter_map(|session| {
            let pipeline = session.pipeline.as_ref()?;
            let is_successful_push_of_variant = pipeline.kind.as_ref() == Some(&PipelineKind::Push)
                && PushVariant::of(pipeline) == variant
                && (pipeline.completed_without_ai
                    || pipeline.steps.iter().all(|step| {
                        matches!(step.status, StepStatus::Succeeded | StepStatus::Skipped)
                    }));
            if !is_successful_push_of_variant {
                return None;
            }
            let finished_at = pipeline.finished_at()?;
            (finished_at >= requested_at).then_some((finished_at, session.id))
        })
        .max_by_key(|(finished_at, _)| *finished_at);
    let Some((finished_at, push_session_id)) = latest else {
        return Ok(None);
    };
    let ended_after = store
        .branch_session_ids_ended_since(branch_id, finished_at, &push_session_id)
        .map_err(|e| e.to_string())?;
    if !ended_after.is_empty() {
        log::info!(
            "[session_queue] deferred push on branch {branch_id}: push session {push_session_id} \
             (force: {}, no_verify: {}) completed since it was armed, but sessions {ended_after:?} \
             ended after it, so it does not stand in",
            variant.force,
            variant.no_verify
        );
        return Ok(None);
    }
    Ok(Some(push_session_id))
}

/// Arm `variant` to push once the branch and its project are idle — or, when
/// they already are, push now through `push`.
///
/// The check-and-set runs under the branch launch lock, so a session start
/// racing it either lands before the check (the push is armed) or after (the
/// push starts, and its own re-check queues it behind the new session). Pushing
/// immediately closes the gap where work finishes between the menu rendering
/// and the click, which would otherwise leave an armed flag nothing kicks.
///
/// Arming again overwrites the variant. `push` is injected so tests can drive
/// the decision without a runner.
pub(crate) async fn defer_branch_push_with<F, Fut>(
    store: &Arc<Store>,
    branch_id: &str,
    variant: PushVariant,
    push: F,
) -> Result<DeferredPushResponse, String>
where
    F: FnOnce(PushVariant) -> Fut,
    Fut: Future<Output = Result<BranchPipelineResponse, String>>,
{
    let armed = {
        let launch_lock = crate::session_commands::branch_session_launch_lock_for(branch_id);
        let _guard = launch_lock.lock().unwrap();
        let branch = store
            .get_branch(branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
        if branch_and_project_idle(store, &branch)? {
            false
        } else {
            store
                .set_branch_deferred_push(
                    branch_id,
                    Some(store::DeferredPush {
                        force: variant.force,
                        no_verify: variant.no_verify,
                        requested_at: store::now_timestamp(),
                    }),
                )
                .map_err(|e| e.to_string())?;
            true
        }
    };
    if armed {
        return Ok(DeferredPushResponse {
            armed: true,
            push: None,
        });
    }
    let response = push(variant).await?;
    Ok(DeferredPushResponse {
        armed: false,
        push: Some(response),
    })
}

/// [`defer_branch_push_with`] whose immediate push is the ordinary
/// start-or-queue push. `provider` only matters for that immediate push; a
/// push fired later by the drain runs with the backend default, like a queued
/// row's does.
pub(crate) async fn defer_branch_push_for_branch(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    variant: PushVariant,
) -> Result<DeferredPushResponse, String> {
    let push_store = Arc::clone(&store);
    let push_branch_id = branch_id.clone();
    defer_branch_push_with(&store, &branch_id, variant, move |variant| {
        start_or_queue_push_pipeline_for_branch(
            push_store,
            registry,
            app_handle,
            push_branch_id,
            provider,
            variant,
        )
    })
    .await
}

/// Clear the branch's deferred push. A no-op when nothing is armed.
pub(crate) fn cancel_deferred_push_for_branch(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<(), String> {
    store
        .set_branch_deferred_push(branch_id, None)
        .map_err(|e| e.to_string())
}

/// What the drain tail finds when it looks at a branch's deferred push.
#[derive(Debug, PartialEq, Eq)]
enum DeferredPushCheck {
    /// Nothing armed, or no such branch.
    NotArmed,
    /// Armed, but the branch or project still has work in flight.
    Blocked(DeferredPushBlockers),
    /// Armed and idle, but a push of the same variant already completed
    /// since it was armed and nothing on the branch ended after it (the named
    /// session; see [`push_satisfying_deferred`]), so there is nothing to
    /// fire.
    Satisfied(store::DeferredPush, String),
    /// Armed, idle, and no such push: fire it.
    Ready(store::DeferredPush),
}

fn check_deferred_push(store: &Arc<Store>, branch_id: &str) -> Result<DeferredPushCheck, String> {
    let Some(branch) = store.get_branch(branch_id).map_err(|e| e.to_string())? else {
        return Ok(DeferredPushCheck::NotArmed);
    };
    let Some(deferred) = branch.deferred_push else {
        return Ok(DeferredPushCheck::NotArmed);
    };
    let blockers = deferred_push_blockers(store, &branch)?;
    if !blockers.is_empty() {
        return Ok(DeferredPushCheck::Blocked(blockers));
    }
    if let Some(session_id) = push_satisfying_deferred(
        store,
        branch_id,
        PushVariant::of_deferred(&deferred),
        deferred.requested_at,
    )? {
        return Ok(DeferredPushCheck::Satisfied(deferred, session_id));
    }
    Ok(DeferredPushCheck::Ready(deferred))
}

/// Fire the branch's deferred push through `fire` if one is armed, the
/// branch and project are idle, and no push of its variant has completed since
/// it was armed as the last thing to end on the branch (that clears it
/// instead). Returns whether a push was started; while blocked, the sessions
/// holding it are logged.
///
/// The flag is cleared *before* the push starts, so the push's own terminal
/// state (which kicks the branch again) cannot find it still armed and fire
/// twice; a failure to start re-arms it — unless the branch is gone or was
/// re-armed meanwhile — so the next kick retries. `fire` is injected so tests
/// can observe the variant without a runner.
pub(crate) async fn fire_deferred_push_with<F, Fut>(
    store: &Arc<Store>,
    branch_id: &str,
    fire: F,
) -> Result<bool, String>
where
    F: FnOnce(PushVariant) -> Fut,
    Fut: Future<Output = Result<BranchPipelineResponse, String>>,
{
    let deferred = match check_deferred_push(store, branch_id)? {
        DeferredPushCheck::NotArmed => return Ok(false),
        DeferredPushCheck::Blocked(blockers) => {
            log::info!(
                "[session_queue] deferred push on branch {branch_id} waits on branch sessions {:?} \
                 and project sessions {:?}",
                blockers.branch_sessions,
                blockers.project_sessions
            );
            return Ok(false);
        }
        DeferredPushCheck::Satisfied(deferred, session_id) => {
            store
                .set_branch_deferred_push(branch_id, None)
                .map_err(|e| e.to_string())?;
            log::info!(
                "[session_queue] deferred push on branch {branch_id} cleared: push session \
                 {session_id} (force: {}, no_verify: {}) completed since it was armed and \
                 nothing ended after it",
                deferred.force,
                deferred.no_verify
            );
            return Ok(false);
        }
        DeferredPushCheck::Ready(deferred) => deferred,
    };
    store
        .set_branch_deferred_push(branch_id, None)
        .map_err(|e| e.to_string())?;
    let variant = PushVariant::of_deferred(&deferred);
    match fire(variant).await {
        Ok(response) => {
            log::info!(
                "[session_queue] deferred push on branch {branch_id} fired as session {} ({:?}, force: {}, no_verify: {})",
                response.session_id,
                response.session_status,
                variant.force,
                variant.no_verify
            );
            Ok(true)
        }
        Err(e) => {
            let still_clear = store
                .get_branch(branch_id)
                .map_err(|e| e.to_string())?
                .is_some_and(|branch| branch.deferred_push.is_none());
            if still_clear {
                store
                    .set_branch_deferred_push(branch_id, Some(deferred))
                    .map_err(|e| e.to_string())?;
            }
            Err(e)
        }
    }
}

/// [`fire_deferred_push_with`] whose push is the ordinary start-or-queue
/// push. Called from the tail of `drain_branch_queue`, on the kicker's own
/// task, when nothing queued could start; `provider` is the backend default,
/// as for a queued row.
pub(crate) async fn fire_deferred_push_if_idle(
    store: Arc<Store>,
    registry: Arc<session_runner::SessionRegistry>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<bool, String> {
    let fire_store = Arc::clone(&store);
    let fire_branch_id = branch_id.clone();
    fire_deferred_push_with(&store, &branch_id, move |variant| {
        start_or_queue_push_pipeline_for_branch(
            fire_store,
            registry,
            app_handle,
            fire_branch_id,
            None,
            variant,
        )
    })
    .await
}
