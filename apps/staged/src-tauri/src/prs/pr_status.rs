//! GitHub pull-request status for a branch: fetching it (one branch, or a
//! project's branches through a bounded pool), caching it on the branch row,
//! announcing it to every client, and clearing it when a push makes it stale.

use serde::Serialize;
use std::sync::Arc;

use crate::git;
use crate::store::{self, Store};

use super::context::resolve_branch_repo_and_subpath;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrStatusEvent {
    pub(crate) branch_id: String,
    pub(crate) pr_state: String,
    pub(crate) pr_checks_status: String,
    pub(crate) pr_review_decision: Option<String>,
    pub(crate) pr_mergeable: bool,
    pub(crate) pr_draft: bool,
    pub(crate) pr_head_sha: Option<String>,
    pub(crate) pr_fetched_at: i64,
    pub(crate) failed_checks: Vec<git::FailedCheck>,
}

/// Core implementation of the single-branch PR status refresh, shared by the
/// `refresh_pr_status` command, the web-mode `dispatch()` arm, and the
/// session-completion side effects that run after a PR session finishes.
pub(crate) async fn refresh_pr_status_impl(
    store: Arc<Store>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<(), String> {
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    let pr_number = branch
        .pr_number
        .ok_or_else(|| "Branch does not have an associated PR".to_string())?;
    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;
    let (github_repo, _) = resolve_branch_repo_and_subpath(&store, &project, &branch)?;

    // Run the blocking gh CLI call on a background thread so we don't
    // block the Tauri IPC thread and starve other commands.
    let pr_status = {
        let github_repo = github_repo.clone();
        tauri::async_runtime::spawn_blocking(move || {
            git::fetch_pr_status_for_repo(&github_repo, pr_number)
        })
        .await
        .map_err(|e| format!("refresh_pr_status task failed: {e}"))?
    };
    let pr_status = match pr_status {
        Ok(status) => status,
        Err(e) => {
            log::error!(
                "refresh_pr_status failed for branch_id={}, pr_number={}: {}",
                branch_id,
                pr_number,
                e
            );
            return Err(e.to_string());
        }
    };
    let mergeable = pr_status.mergeable == "MERGEABLE";
    let pr_fetched_at = store::now_timestamp();

    let written = store
        .update_branch_pr_status(
            &branch_id,
            Some(pr_number),
            Some(pr_status.state.clone()),
            Some(pr_status.checks_summary.state.clone()),
            pr_status.review_decision.clone(),
            Some(mergeable),
            Some(pr_status.is_draft),
            None,
            None,
            pr_status.head_sha.clone(),
            pr_status.title.clone(),
        )
        .map_err(|e| e.to_string())?;
    if !written {
        // Relinked or unlinked during the fetch: this status is another PR's.
        log::debug!("refresh_pr_status: branch_id={branch_id} no longer links PR #{pr_number}");
        return Ok(());
    }

    crate::web_server::emit_to_all(
        &app_handle,
        "pr-status-changed",
        PrStatusEvent {
            branch_id: branch_id.clone(),
            pr_state: pr_status.state,
            pr_checks_status: pr_status.checks_summary.state,
            pr_review_decision: pr_status.review_decision,
            pr_mergeable: mergeable,
            pr_draft: pr_status.is_draft,
            pr_head_sha: pr_status.head_sha,
            pr_fetched_at,
            failed_checks: pr_status.failed_checks,
        },
    );

    Ok(())
}

/// Max concurrent PR-status fetches inside a single project refresh. Each fetch
/// spawns a `gh` subprocess + GitHub round-trip, so we cap the fan-out to avoid
/// a subprocess thundering herd while still resolving a project's PRs in ~1
/// round-trip's wall-clock instead of N (fully serial). The backend PR-poll
/// scheduler reuses this cap for a single pool shared across all the projects
/// it refreshes (see `pr_poll_scheduler`).
pub(crate) const PR_REFRESH_CONCURRENCY: usize = 6;

/// Core implementation shared by the `refresh_all_pr_statuses` command and the
/// backend PR-poll scheduler.
///
/// Fans the per-branch fetches out across the bounded `semaphore` pool instead
/// of awaiting them one at a time. Repo resolution is a cheap local DB read so
/// it stays on this task; only the network fetch + DB write + per-branch
/// `pr-status-changed` emit move into the spawned tasks, gated by the semaphore.
/// A final `pr-statuses-refreshed` event is emitted and the number of branches
/// refreshed is returned.
///
/// The semaphore is passed in (rather than created per call) so the scheduler
/// can share a single pool across every project it refreshes — a tick that
/// finds many projects due still caps total concurrent `gh` subprocesses.
pub(crate) async fn refresh_project_pr_statuses(
    store: &Arc<Store>,
    app_handle: &tauri::AppHandle,
    project_id: &str,
    semaphore: Arc<tokio::sync::Semaphore>,
) -> Result<u32, String> {
    let project = store
        .get_project(project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let branches = store
        .list_branches_for_project(project_id)
        .map_err(|e| e.to_string())?;
    let branches_with_prs: Vec<_> = branches
        .into_iter()
        .filter(|b| b.pr_number.is_some())
        .collect();

    let mut tasks = Vec::new();

    for branch in branches_with_prs {
        let pr_number = branch.pr_number.unwrap();
        let github_repo = match resolve_branch_repo_and_subpath(store, &project, &branch) {
            Ok((repo, _)) => repo,
            Err(e) => {
                log::warn!(
                    "Failed to resolve repo for branch {} (PR #{}): {}",
                    branch.id,
                    pr_number,
                    e
                );
                continue;
            }
        };

        let store = Arc::clone(store);
        let app_handle = app_handle.clone();
        let semaphore = Arc::clone(&semaphore);
        let branch_id = branch.id.clone();

        tasks.push(tauri::async_runtime::spawn(async move {
            // Hold a permit for the whole fetch so no more than
            // PR_REFRESH_CONCURRENCY `gh` round-trips are in flight at once.
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|e| format!("refresh_project_pr_statuses semaphore closed: {e}"))?;

            let pr_result = tauri::async_runtime::spawn_blocking(move || {
                git::fetch_pr_status_for_repo(&github_repo, pr_number)
            })
            .await
            .map_err(|e| format!("refresh_project_pr_statuses task failed: {e}"))?;

            match pr_result {
                Ok(pr_status) => {
                    let mergeable = pr_status.mergeable == "MERGEABLE";
                    let pr_fetched_at = store::now_timestamp();

                    match store.update_branch_pr_status(
                        &branch_id,
                        Some(pr_number),
                        Some(pr_status.state.clone()),
                        Some(pr_status.checks_summary.state.clone()),
                        pr_status.review_decision.clone(),
                        Some(mergeable),
                        Some(pr_status.is_draft),
                        None,
                        None,
                        pr_status.head_sha.clone(),
                        pr_status.title.clone(),
                    ) {
                        Ok(true) => {}
                        // Relinked or unlinked during the fetch: this status
                        // is another PR's, so don't emit it either. Nothing
                        // failed — the relink already wrote newer state.
                        Ok(false) => return Ok(BranchRefresh::Skipped),
                        Err(e) => {
                            log::warn!(
                                "Failed to update PR status for branch {}: {}",
                                branch_id,
                                e
                            );
                            return Ok(BranchRefresh::Failed);
                        }
                    }

                    crate::web_server::emit_to_all(
                        &app_handle,
                        "pr-status-changed",
                        PrStatusEvent {
                            branch_id: branch_id.clone(),
                            pr_state: pr_status.state,
                            pr_checks_status: pr_status.checks_summary.state,
                            pr_review_decision: pr_status.review_decision,
                            pr_mergeable: mergeable,
                            pr_draft: pr_status.is_draft,
                            pr_head_sha: pr_status.head_sha,
                            pr_fetched_at,
                            failed_checks: pr_status.failed_checks,
                        },
                    );

                    Ok::<BranchRefresh, String>(BranchRefresh::Refreshed)
                }
                Err(e) => {
                    log::warn!(
                        "Failed to fetch PR status for branch {} (PR #{}): {}",
                        branch_id,
                        pr_number,
                        e
                    );
                    Ok(BranchRefresh::Failed)
                }
            }
        }));
    }

    let refreshed_count = collect_branch_refresh_results(tasks).await?;

    crate::web_server::emit_to_all(app_handle, "pr-statuses-refreshed", project_id);

    Ok(refreshed_count)
}

/// How one branch's PR status refresh in [`refresh_project_pr_statuses`]
/// ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BranchRefresh {
    /// Fetched, written and emitted.
    Refreshed,
    /// Fetched, but the branch was relinked or unlinked during the fetch, so
    /// the status belonged to a PR it no longer links and wasn't written.
    /// Neither refreshed nor failed, as in the web `refresh_all_pr_statuses`
    /// arm: a project whose only PR branch was relinked mid-poll hasn't had a
    /// failed poll.
    Skipped,
    /// The fetch or the write failed.
    Failed,
}

pub(super) async fn collect_branch_refresh_results<T, E>(tasks: Vec<T>) -> Result<u32, String>
where
    T: std::future::Future<Output = Result<Result<BranchRefresh, String>, E>>,
    E: std::fmt::Display,
{
    let mut refreshed_count = 0u32;
    let mut failed_branch_count = 0u32;
    let mut task_errors = Vec::new();

    for task in tasks {
        match task.await {
            Ok(Ok(BranchRefresh::Refreshed)) => refreshed_count += 1,
            Ok(Ok(BranchRefresh::Skipped)) => {}
            Ok(Ok(BranchRefresh::Failed)) => failed_branch_count += 1,
            Ok(Err(e)) => {
                log::warn!("PR status refresh task failed: {e}");
                task_errors.push(e);
            }
            Err(e) => {
                let message = format!("PR status refresh task join failed: {e}");
                log::warn!("{message}");
                task_errors.push(message);
            }
        }
    }

    if refreshed_count == 0 && (failed_branch_count > 0 || !task_errors.is_empty()) {
        let mut errors = Vec::new();
        if failed_branch_count > 0 {
            errors.push(format!("{failed_branch_count} branch refreshes failed"));
        }
        errors.extend(task_errors);

        return Err(format!(
            "all PR status refresh tasks failed: {}",
            errors.join("; ")
        ));
    }

    Ok(refreshed_count)
}

/// Core implementation shared by the `clear_branch_pr_status` command, the
/// web-mode `dispatch()` arm, and the session-completion side effects that
/// run after a push session finishes.
pub(crate) fn clear_branch_pr_status_impl(
    store: &Store,
    app_handle: &tauri::AppHandle,
    branch_id: &str,
) -> Result<(), String> {
    store
        .update_branch_pr_status(
            branch_id, None, None, None, None, None, None, None, None, None, None,
        )
        .map_err(|e| e.to_string())?;

    crate::web_server::emit_to_all(app_handle, "pr-status-cleared", branch_id);

    Ok(())
}
