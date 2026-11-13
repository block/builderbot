//! The bodies of the commands that delete branches — `delete_branch`,
//! `remove_project_repo` and `delete_project` — shared by the Tauri commands
//! and the web dispatcher.
//!
//! Each takes its state owned, so a web arm can run it through `detached` and
//! a closed tab cannot drop a teardown between its session stop and its row
//! delete. All three tear a branch down in the same order: stop its actions,
//! stop its sessions ([`stop_branch_sessions`]), remove its worktree or
//! workspace, then delete the row, which cascades to everything on it.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::session_queue::teardown::stop_branch_sessions;
use crate::session_runner::SessionRegistry;
use crate::store::{self, Store};
use crate::{branches, git};

#[cfg(test)]
mod tests;

/// How many clones a project delete tears down at once. Few projects span
/// more repos than this, and a teardown is mostly `git worktree remove`
/// deleting files, so more threads would only queue on the disk.
const MAX_PARALLEL_CLONE_CLEANUPS: usize = 4;

/// The services a branch teardown stops work through.
pub(crate) struct TeardownServices<R: tauri::Runtime> {
    pub store: Arc<Store>,
    pub action_executor: Arc<ActionExecutor>,
    pub action_registry: Arc<ActionRegistry>,
    pub session_registry: Arc<SessionRegistry>,
    pub app_handle: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> TeardownServices<R> {
    /// Stop the actions and sessions running on `branches`. The branches are
    /// about to be deleted, so the actions' output is given up as well.
    async fn stop_work(&self, branches: &[store::Branch]) -> Result<(), String> {
        let branch_ids: Vec<&str> = branches.iter().map(|b| b.id.as_str()).collect();
        crate::actions::commands::stop_and_discard_actions_for_branches(
            &self.action_executor,
            &self.action_registry,
            &branch_ids,
        );
        stop_branch_sessions(
            &self.store,
            &self.session_registry,
            &self.app_handle,
            branches,
        )
        .await
    }
}

pub(crate) async fn delete_branch_impl<R: tauri::Runtime>(
    services: TeardownServices<R>,
    branch_id: String,
) -> Result<(), String> {
    crate::project_transfer::ensure_branch_not_transferring(&branch_id)?;
    let store = Arc::clone(&services.store);
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    services.stop_work(std::slice::from_ref(&branch)).await?;

    tauri::async_runtime::spawn_blocking({
        let store = Arc::clone(&store);
        let branch = branch.clone();
        move || branches::cleanup_branch_resources(&store, &branch)
    })
    .await
    .map_err(|e| format!("Failed to clean up branch resources: {e}"))??;

    // Delete the branch record (cascades to commits, notes, reviews, sessions)
    store.delete_branch(&branch_id).map_err(|e| e.to_string())
}

pub(crate) async fn remove_project_repo_impl<R: tauri::Runtime>(
    services: TeardownServices<R>,
    project_id: String,
    project_repo_id: String,
) -> Result<(), String> {
    crate::project_transfer::ensure_project_not_transferring(&project_id)?;
    let store = Arc::clone(&services.store);
    let removed = store
        .get_project_repo(&project_repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project repo not found: {project_repo_id}"))?;
    let branches = store
        .list_branches_for_project(&project_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|b| b.project_repo_id.as_deref() == Some(project_repo_id.as_str()))
        .collect::<Vec<_>>();

    services.stop_work(&branches).await?;

    // Run heavy cleanup (worktree removal, remote workspace deletion) off the
    // main thread so the UI stays responsive for large repos.
    tauri::async_runtime::spawn_blocking({
        let store = Arc::clone(&store);
        let branches = branches.clone();
        move || -> Result<(), String> {
            for branch in &branches {
                branches::cleanup_branch_resources(&store, branch)?;
            }
            Ok(())
        }
    })
    .await
    .map_err(|e| format!("Failed to clean up branch resources: {e}"))??;

    for branch in &branches {
        store.delete_branch(&branch.id).map_err(|e| e.to_string())?;
    }
    store
        .delete_project_repo(&project_repo_id)
        .map_err(|e| e.to_string())?;

    if removed.is_primary {
        let next_primary = store
            .list_project_repos(&project_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .next();
        let project = store
            .get_project(&project_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Project not found: {project_id}"))?;
        if let Some(next) = next_primary {
            store
                .set_primary_project_repo(&project_id, &next.id)
                .map_err(|e| e.to_string())?;
            store
                .update_project(
                    &project_id,
                    &project.name,
                    Some(&next.github_repo),
                    &project.location,
                    next.subpath.as_deref(),
                )
                .map_err(|e| e.to_string())?;
        } else {
            store
                .update_project(&project_id, &project.name, None, &project.location, None)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub(crate) async fn delete_project_impl<R: tauri::Runtime>(
    services: TeardownServices<R>,
    id: String,
) -> Result<(), String> {
    crate::project_transfer::ensure_project_not_transferring(&id)?;
    let store = Arc::clone(&services.store);

    // Clean up branch-backed resources (local worktrees / remote workspaces)
    // before removing DB records via cascade.
    let branches = store
        .list_branches_for_project(&id)
        .map_err(|e| e.to_string())?;

    services.stop_work(&branches).await?;

    // Run heavy cleanup (worktree removal, remote workspace deletion, directory
    // cleanup) off the main thread so the UI stays responsive for large repos.
    tauri::async_runtime::spawn_blocking({
        let store = Arc::clone(&store);
        let id = id.clone();
        move || {
            cleanup_project_branches_best_effort(
                &branches,
                |branch| cleanup_lane(&store, branch),
                |branch| branches::cleanup_branch_resources_best_effort(&store, branch),
                |branch| store.delete_branch(&branch.id).map_err(|e| e.to_string()),
            );

            // Best-effort cleanup for project-scoped local worktree roots.
            // Worktree-level cleanup removes individual directories; this removes any
            // leftover project container folder.
            if let Ok(project_root) = git::project_worktree_root_for(&id) {
                if project_root.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&project_root) {
                        log::warn!(
                            "failed to remove project worktree root '{}': {e}",
                            project_root.display()
                        );
                    }
                }
            }
        }
    })
    .await
    .map_err(|e| format!("Failed to clean up project resources: {e}"))?;

    store.delete_project(&id).map_err(|e| e.to_string())
}

/// The lane [`cleanup_project_branches_best_effort`] runs `branch` in: its
/// repo slug for a local branch, which names the clone its worktree teardown
/// runs git in. Lowercased, as GitHub slugs and the macOS clone paths they
/// name are both case-insensitive. `None` for a remote branch, and for a
/// local one whose repo can't be resolved, as its cleanup then fails before
/// running git.
fn cleanup_lane(store: &Arc<Store>, branch: &store::Branch) -> Option<String> {
    if branch.branch_type != store::BranchType::Local {
        return None;
    }
    let project = store.get_project(&branch.project_id).ok().flatten()?;
    branches::resolve_branch_repo_slug(store, &project, branch)
        .ok()
        .map(|slug| slug.to_ascii_lowercase())
}

/// Tear down every branch of a project being deleted and delete their rows,
/// best effort: a failure is logged and the rest carry on.
///
/// Branches run in lanes keyed by `lane_of`: one per clone, holding its local
/// branches, and one (`None`) for the remote ones. Up to
/// [`MAX_PARALLEL_CLONE_CLEANUPS`] lanes run at once, each running its
/// branches in list order. `worktree remove`, `prune` and `branch -D` in one
/// clone contend on its `.git` locks, and the remote lane deletes rows as it
/// goes so the last branch on a shared workspace is the one that deletes it.
///
/// A panic abandons only the rest of its own lane. It is logged with the
/// lane's key, its thread moves on to the next unclaimed lane, and every
/// other lane still runs. A panic in the row-delete retry or the remote
/// sweep below costs only that branch's step there. Once everything has
/// run, the teardown's first panic is re-raised with its own payload, so the
/// caller still sees a crashed teardown and keeps the project row for a
/// retry.
///
/// This isolates panics in a teardown's git and filesystem work. A panic
/// inside a store call is no better off: it poisons the store's connection
/// mutex, and every later store call in the app panics on it until a
/// restart, as it always has. Here that only means the later lanes' store
/// calls panic too, each caught and logged like the first.
pub(crate) fn cleanup_project_branches_best_effort<L, F, G>(
    branches: &[store::Branch],
    lane_of: L,
    cleanup_branch_resources: F,
    delete_branch_row: G,
) where
    L: Fn(&store::Branch) -> Option<String>,
    F: Fn(&store::Branch) + Sync,
    G: Fn(&store::Branch) -> Result<(), String> + Sync,
{
    // Lanes in first-seen order: each lane's key and its indexes into
    // `branches`.
    let mut lanes: Vec<(Option<String>, Vec<usize>)> = Vec::new();
    let mut lane_ids: HashMap<Option<String>, usize> = HashMap::new();
    for (index, branch) in branches.iter().enumerate() {
        let key = lane_of(branch);
        let lane = *lane_ids.entry(key.clone()).or_insert_with(|| {
            lanes.push((key, Vec::new()));
            lanes.len() - 1
        });
        lanes[lane].1.push(index);
    }

    let failed_branch_deletes = Mutex::new(Vec::new());
    let first_panic: Mutex<Option<Box<dyn Any + Send>>> = Mutex::new(None);
    let next_lane = AtomicUsize::new(0);
    let drain_lanes = || {
        while let Some((key, lane)) = lanes.get(next_lane.fetch_add(1, Ordering::Relaxed)) {
            let step = format!(
                "project cleanup lane '{}'",
                key.as_deref().unwrap_or("(no clone)")
            );
            run_isolated(&first_panic, &step, || {
                for &index in lane {
                    let branch = &branches[index];
                    cleanup_branch_resources(branch);
                    // Delete branch rows as we go so shared-workspace cleanup
                    // can converge on the final owner and remove the
                    // workspace once.
                    if let Err(e) = delete_branch_row(branch) {
                        log::warn!(
                            "failed to delete branch '{}' during project cleanup: {e}",
                            branch.id
                        );
                        failed_branch_deletes
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(index);
                    }
                }
            });
        }
    };
    std::thread::scope(|scope| {
        // This thread drains lanes too, so a one-clone project spawns none.
        for _ in 1..lanes.len().min(MAX_PARALLEL_CLONE_CLEANUPS) {
            let spawned = std::thread::Builder::new()
                .name("project-cleanup".to_string())
                .spawn_scoped(scope, drain_lanes);
            if let Err(e) = spawned {
                // The lanes still drain, on the threads already running.
                log::warn!("failed to spawn a project cleanup thread: {e}");
                break;
            }
        }
        drain_lanes();
    });

    let mut failed_branch_deletes = failed_branch_deletes
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    if !failed_branch_deletes.is_empty() {
        failed_branch_deletes.sort_unstable();

        // One retry can unblock convergence when an early branch-row delete
        // failed.
        for index in failed_branch_deletes {
            let branch = &branches[index];
            run_isolated(
                &first_panic,
                &format!("retry delete for branch '{}'", branch.id),
                || {
                    if let Err(e) = delete_branch_row(branch) {
                        log::warn!(
                            "failed retry delete for branch '{}' during project cleanup: {e}",
                            branch.id
                        );
                    }
                },
            );
        }

        // Re-run remote cleanup once after retries so shared workspace
        // deletion can converge when the first pass observed stale peer rows.
        // This is also the only cleanup the remote branches after a panicked
        // one get, so a branch that panics again here doesn't stop it.
        for branch in branches {
            if branch.branch_type == store::BranchType::Remote {
                run_isolated(
                    &first_panic,
                    &format!("remote sweep of branch '{}'", branch.id),
                    || cleanup_branch_resources(branch),
                );
            }
        }
    }

    if let Some(payload) = first_panic
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
    {
        resume_unwind(payload);
    }
}

/// Run one step of [`cleanup_project_branches_best_effort`], catching a
/// panic in it. The panic is logged with `step`'s description and the
/// teardown carries on. Only the first panic's payload is kept, to re-raise
/// once the teardown has run.
///
/// Unwind-safe enough: a panic abandons only the rest of this step, and what
/// the steps share here is atomic or behind a poison-tolerant lock.
fn run_isolated(first_panic: &Mutex<Option<Box<dyn Any + Send>>>, step: &str, run: impl FnOnce()) {
    if let Err(payload) = catch_unwind(AssertUnwindSafe(run)) {
        log::error!(
            "{step} panicked during project cleanup, skipping the rest of it: {}",
            panic_message(payload.as_ref())
        );
        first_panic
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(payload);
    }
}

/// The message of a `panic!` payload, which is a `&str` or a `String`.
fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("(non-string panic payload)")
}
