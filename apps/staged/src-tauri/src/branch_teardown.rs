//! The bodies of the commands that delete branches — `delete_branch`,
//! `remove_project_repo` and `delete_project` — shared by the Tauri commands
//! and the web dispatcher.
//!
//! Each takes its state owned, so a web arm can run it through `detached` and
//! a closed tab cannot drop a teardown between its session stop and its row
//! delete. All three tear a branch down in the same order: stop its actions,
//! stop its sessions ([`stop_branch_sessions`]), remove its worktree or
//! workspace, then delete the row, which cascades to everything on it.

use std::sync::Arc;

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::session_queue::teardown::stop_branch_sessions;
use crate::session_runner::SessionRegistry;
use crate::store::{self, Store};
use crate::{branches, git};

/// The services a branch teardown stops work through.
pub(crate) struct TeardownServices<R: tauri::Runtime> {
    pub store: Arc<Store>,
    pub action_executor: Arc<ActionExecutor>,
    pub action_registry: Arc<ActionRegistry>,
    pub session_registry: Arc<SessionRegistry>,
    pub app_handle: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> TeardownServices<R> {
    /// Stop the actions and sessions running on `branches`.
    async fn stop_work(&self, branches: &[store::Branch]) -> Result<(), String> {
        let branch_ids: Vec<&str> = branches.iter().map(|b| b.id.as_str()).collect();
        crate::actions::commands::stop_actions_for_branches(
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
            crate::cleanup_project_branches_best_effort(
                &branches,
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
