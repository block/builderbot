//! The Tauri command wrappers for the branch git actions and PR status.
//!
//! Each wrapper resolves the managed store and hands off to the
//! `*_impl` / `*_for_branch` function in the action's own module, which the
//! web server's `dispatch()` arms call directly.

use std::sync::{Arc, Mutex};

use crate::git;
use crate::session_runner;
use crate::store::{PipelineKind, Store};

use super::commit::start_or_queue_commit_pipeline_for_branch;
use super::context::resolve_branch_repo_and_subpath;
use super::create_pr::start_create_pr_pipeline_for_branch;
use super::deferred::{
    cancel_deferred_push_for_branch, defer_branch_push_for_branch, DeferredPushResponse,
};
use super::pipeline::BranchPipelineResponse;
use super::pr_status::{
    clear_branch_pr_status_impl, refresh_pr_status_impl, refresh_project_pr_statuses,
    PR_REFRESH_CONCURRENCY,
};
use super::pull::pull_or_queue_branch_for_branch;
use super::push::{
    has_unpushed_commits_impl, start_or_queue_push_pipeline_for_branch, PushVariant,
};
use super::revert::revert_commit_for_branch;

fn get_store(store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized — please reset from the startup prompt".into())
}

/// Pull origin's new commits into the branch.
///
/// Returns the queued session id when the pull had to join the branch queue, and
/// `None` when it ran immediately.
#[tauri::command(rename_all = "camelCase")]
pub async fn pull_or_queue_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<Option<String>, String> {
    let store = get_store(&store)?;
    pull_or_queue_branch_for_branch(store, Arc::clone(&registry), app_handle, branch_id).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn create_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    draft: Option<bool>,
) -> Result<String, String> {
    let store = get_store(&store)?;
    start_create_pr_pipeline_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        provider,
        draft,
    )
    .await
}

/// Build the GitHub PR URL for a branch.
///
/// For fork PRs the stored repo may be the fork (head) repo, but PRs always
/// live on the base (upstream) repo. This queries the GitHub API to resolve
/// the canonical URL, falling back to the parent repo when the stored repo
/// is a fork.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_pr_url(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    pr_number: u64,
) -> Result<String, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let (repo_slug, _) = resolve_branch_repo_and_subpath(&store, &project, &branch)?;

    tauri::async_runtime::spawn_blocking(move || git::fetch_pr_url(&repo_slug, pr_number))
        .await
        .map_err(|e| format!("get_pr_url task failed: {e}"))?
        .map_err(|e| e.to_string())
}

/// Update the PR number for a branch.
#[tauri::command(rename_all = "camelCase")]
pub fn update_branch_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    pr_number: Option<u64>,
) -> Result<(), String> {
    get_store(&store)?
        .update_branch_pr_number(&branch_id, pr_number, None)
        .map_err(|e| e.to_string())
}

/// Refresh PR status for a single branch.
#[tauri::command(rename_all = "camelCase")]
pub async fn refresh_pr_status(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    refresh_pr_status_impl(store, app_handle, branch_id).await
}

/// Refresh PR status for all branches in a project.
///
/// Thin command wrapper around [`refresh_project_pr_statuses`]; the same core is
/// also driven on a cadence by the backend PR-poll scheduler. Each command call
/// gets its own bounded pool (the scheduler instead shares one pool across
/// projects).
#[tauri::command(rename_all = "camelCase")]
pub async fn refresh_all_pr_statuses(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    project_id: String,
) -> Result<u32, String> {
    let store = get_store(&store)?;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(PR_REFRESH_CONCURRENCY));
    refresh_project_pr_statuses(&store, &app_handle, &project_id, semaphore).await
}

/// Clear stale PR status fields for a branch (e.g. after a push invalidates them).
///
/// This nulls out checks, mergeable, review-decision, etc. in the DB and emits
/// a `pr-status-cleared` event so the frontend can drop the stale indicators
/// immediately instead of waiting for the next GitHub refresh.
#[tauri::command(rename_all = "camelCase")]
pub fn clear_branch_pr_status(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    clear_branch_pr_status_impl(&store, &app_handle, &branch_id)
}

/// Check if a branch has commits that haven't been pushed to the remote.
#[tauri::command(rename_all = "camelCase")]
pub async fn has_unpushed_commits(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<bool, String> {
    let store = get_store(&store)?;
    has_unpushed_commits_impl(store, branch_id).await
}

/// Push a branch to its remote.
///
/// Queues behind in-flight branch work instead of failing, so the response
/// reports whether the push started or is waiting on the branch queue.
///
/// `no_verify` ("Push without verifying") skips git's `pre-push` hook.
#[tauri::command(rename_all = "camelCase")]
pub async fn push_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    force: Option<bool>,
    no_verify: Option<bool>,
) -> Result<BranchPipelineResponse, String> {
    let store = get_store(&store)?;
    start_or_queue_push_pipeline_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        provider,
        PushVariant::from_request(force, no_verify),
    )
    .await
}

/// "Push after current work" and its variants: arm a push to run once the
/// branch and its project are idle, or push now if they already are.
#[tauri::command(rename_all = "camelCase")]
pub async fn defer_branch_push(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    force: Option<bool>,
    no_verify: Option<bool>,
) -> Result<DeferredPushResponse, String> {
    let store = get_store(&store)?;
    defer_branch_push_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        provider,
        PushVariant::from_request(force, no_verify),
    )
    .await
}

/// Clear a branch's deferred push.
#[tauri::command(rename_all = "camelCase")]
pub fn cancel_deferred_push(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;
    cancel_deferred_push_for_branch(&store, &branch_id)
}

/// Rebase a branch onto its base branch via a pipeline.
///
/// When `target` is `None` or `"base"`, rebases onto `origin/{base_branch}`
/// (the default behaviour used by the base-moved row and the `…` menu).
/// When `target` is `"origin"`, rebases onto `origin/{remote_branch}` so that
/// the local branch incorporates remote-only commits (used by the diverged row).
///
/// Queues behind in-flight branch work instead of failing, so the response
/// reports whether the rebase started or is waiting on the branch queue.
///
/// `then_force_push` ("Rebase and force push") also queues a force push behind
/// the rebase that only runs if the rebase finishes; its id comes back as
/// `follow_up_session_id`.
#[tauri::command(rename_all = "camelCase")]
pub async fn rebase_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    target: Option<String>,
    then_force_push: Option<bool>,
) -> Result<BranchPipelineResponse, String> {
    let store = get_store(&store)?;
    start_or_queue_commit_pipeline_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        PipelineKind::Rebase,
        provider,
        target,
        then_force_push.unwrap_or(false),
    )
    .await
}

/// Squash all commits on a branch into a single commit via a pipeline.
///
/// Fetches the latest base branch, captures the commit history, then uses
/// `git reset --soft $(git merge-base origin/{base} HEAD)` to collapse only the
/// branch's own commits into staged changes.
/// Hands off to AI to write a single conventional-commit message using the
/// original commit history as context.
///
/// Queues behind in-flight branch work instead of failing, so the response
/// reports whether the squash started or is waiting on the branch queue.
#[tauri::command(rename_all = "camelCase")]
pub async fn squash_commits(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
) -> Result<BranchPipelineResponse, String> {
    let store = get_store(&store)?;
    start_or_queue_commit_pipeline_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        PipelineKind::Squash,
        provider,
        None,
        false,
    )
    .await
}

/// Revert a completed commit by creating a new signed-off revert commit.
#[tauri::command(rename_all = "camelCase")]
pub async fn revert_commit(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    commit_sha: String,
    provider: Option<String>,
) -> Result<String, String> {
    let store = get_store(&store)?;
    revert_commit_for_branch(
        store,
        Arc::clone(&registry),
        app_handle,
        branch_id,
        commit_sha,
        provider,
    )
    .await
}
