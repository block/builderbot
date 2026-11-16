//! Background setup for a repo that was just attached to a project.

use std::sync::Arc;

use tauri::Manager;

use crate::branches;
use crate::store::Store;

/// Background worktree + prerun-actions setup for a repo that was just
/// attached to a project — fire and forget. Shared by project creation, the
/// `add_project_repo` command and the note move that adds a repo on the way,
/// through [`super::create_project_and_setup`] and
/// [`super::add_project_repo_and_setup`].
///
/// What it does depends on the repo's branch:
///
/// - **Local**: creates the git worktree, then runs the repo's prerun
///   actions.
/// - **Remote, `provisions_workspace`**: nothing. No workspace is running to
///   clone into; the frontend's workspace lifecycle runs `start_workspace`
///   for the `Starting` branch, which starts the workspace from this repo.
/// - **Remote, joining a running workspace**: clones the repo into it.
///
/// `provisions_workspace` is whether the repo's branch provisions the
/// project's Blox workspace: [`super::add_repo::AddedProjectRepo`]'s field
/// for an added repo, and always the case for the remote branch a project is
/// created with. `log_tag` names the entry point in the log lines.
pub(super) fn spawn_project_repo_setup(
    app_handle: tauri::AppHandle,
    store: Arc<Store>,
    project_id: String,
    repo_id: String,
    provisions_workspace: bool,
    log_tag: &'static str,
) {
    log::info!("[{log_tag}] spawning background setup for repo {repo_id} in project {project_id}");
    tauri::async_runtime::spawn(async move {
        crate::web_server::emit_to_all(&app_handle, "project-setup-progress", project_id.clone());

        let branch = match store.list_branches_for_project(&project_id) {
            Ok(branches) => branches
                .into_iter()
                .find(|b| b.project_repo_id.as_deref() == Some(repo_id.as_str())),
            Err(e) => {
                log::warn!("[{log_tag}] failed to list branches: {e}");
                return;
            }
        };
        let branch = match branch {
            Some(b) => b,
            None => {
                log::warn!("[{log_tag}] no branch found for repo {repo_id}");
                return;
            }
        };

        if branch.workspace_name.is_none() {
            // Local branch: set up git worktree and run prerun actions.
            let branch_id = branch.id.clone();
            let store_clone = Arc::clone(&store);
            let app_handle_clone = app_handle.clone();
            let worktree_result = tauri::async_runtime::spawn_blocking(move || {
                branches::setup_worktree_sync(&store_clone, &branch_id, &app_handle_clone)
            })
            .await;

            match worktree_result {
                Ok(Ok(path)) => {
                    log::info!("[{log_tag}] worktree ready at {path}");
                    crate::web_server::emit_to_all(
                        &app_handle,
                        "project-setup-progress",
                        project_id.clone(),
                    );
                }
                Ok(Err(e)) => {
                    log::warn!("[{log_tag}] worktree setup failed: {e}");
                    return;
                }
                Err(e) => {
                    log::warn!("[{log_tag}] worktree task panicked: {e}");
                    return;
                }
            }

            let executor = app_handle.state::<Arc<crate::actions::ActionExecutor>>();
            let act_registry = app_handle.state::<Arc<crate::actions::ActionRegistry>>();
            if let branches::PrerunOutcome::Ran(_) = branches::claim_and_run_prerun_actions(
                &store,
                &app_handle,
                &branch.id,
                &executor,
                &act_registry,
                None,
                log_tag,
            )
            .await
            {
                crate::web_server::emit_to_all(
                    &app_handle,
                    "project-setup-progress",
                    project_id.clone(),
                );
            }
        } else if provisions_workspace {
            // Nothing is running to clone into yet. As with a project created
            // with a repo, the workspace lifecycle runs `start_workspace` for
            // the `Starting` branch, which starts the workspace from this repo.
            log::info!(
                "[{log_tag}] branch '{}' provisions workspace '{}'; leaving the clone to start_workspace",
                branch.branch_name,
                branch.workspace_name.as_deref().unwrap_or_default()
            );
        } else {
            // Remote branch: clone the repo into the running workspace,
            // fetch the base branch, and create the feature branch.
            match branches::setup_remote_repo_clone(&store, &app_handle, &branch.id).await {
                Ok(()) => {
                    log::info!(
                        "[{log_tag}] remote repo cloned for branch '{}'",
                        branch.branch_name
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[{log_tag}] remote repo clone failed for branch '{}': {e}",
                        branch.branch_name
                    );
                    crate::web_server::emit_to_all(
                        &app_handle,
                        "project-setup-progress",
                        project_id,
                    );
                    return;
                }
            }
            crate::web_server::emit_to_all(&app_handle, "project-setup-progress", project_id);
        }
    });
}
