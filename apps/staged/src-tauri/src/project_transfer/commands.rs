//! The transfer commands, over Tauri and (through the `_impl`s) the web
//! transport.

use std::sync::{Arc, Mutex};

use tauri::AppHandle;

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::store::{ProjectLocation, Store};

use super::backups;
use super::host::{LiveHost, TransferHost};
use super::orchestrator::{run_transfer, ProjectTransferResult, TransferProgress};
use super::preflight::{inspect, ProjectTransferPreflight, TransferIssue};
use super::registry;

/// The event every [`TransferProgress`] goes out as.
pub const PROGRESS_EVENT: &str = "project-transfer-progress";

/// The preflight, plus whether a transfer is already running.
pub(super) fn preflight_for(
    store: &Store,
    host: &dyn TransferHost,
    project_id: &str,
) -> Result<ProjectTransferPreflight, String> {
    let mut preflight = inspect(store, host, project_id)?.preflight;
    if registry::is_project_transferring(project_id) {
        preflight.add_blocker(TransferIssue::new(
            "already_transferring",
            "This project is already being transferred.",
        ));
    }
    Ok(preflight)
}

/// What moving the project to its other location would carry, and what
/// stops it. Read-only; asks a running source workspace with one `ws exec`.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_project_transfer_preflight(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
) -> Result<ProjectTransferPreflight, String> {
    let store = crate::get_store(&store)?;
    get_project_transfer_preflight_impl(store, project_id).await
}

pub(crate) async fn get_project_transfer_preflight_impl(
    store: Arc<Store>,
    project_id: String,
) -> Result<ProjectTransferPreflight, String> {
    tauri::async_runtime::spawn_blocking(move || preflight_for(&store, &LiveHost, &project_id))
        .await
        .map_err(|e| e.to_string())?
}

/// Move the project to its other location, emitting
/// `project-transfer-progress` as it goes. Resolves once the project has
/// switched (or failed, with everything undone).
#[tauri::command(rename_all = "camelCase")]
pub async fn transfer_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: AppHandle,
    executor: tauri::State<'_, Arc<ActionExecutor>>,
    act_registry: tauri::State<'_, Arc<ActionRegistry>>,
    project_id: String,
    include_uncommitted: bool,
) -> Result<ProjectTransferResult, String> {
    let store = crate::get_store(&store)?;
    transfer_project_impl(
        store,
        app_handle,
        Arc::clone(&executor),
        Arc::clone(&act_registry),
        project_id,
        include_uncommitted,
    )
    .await
}

pub(crate) async fn transfer_project_impl(
    store: Arc<Store>,
    app_handle: AppHandle,
    executor: Arc<ActionExecutor>,
    act_registry: Arc<ActionRegistry>,
    project_id: String,
    include_uncommitted: bool,
) -> Result<ProjectTransferResult, String> {
    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let branch_ids: Vec<String> = store
        .list_branches_for_project(&project_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|b| b.id)
        .collect();
    let transfer_id = uuid::Uuid::new_v4().to_string();
    let guard = registry::begin(&project_id, &transfer_id, branch_ids.clone())?;
    let ids: Vec<&str> = branch_ids.iter().map(String::as_str).collect();
    crate::actions::commands::stop_actions_for_branches(&executor, &act_registry, &ids);

    let result = {
        let store = Arc::clone(&store);
        let app = app_handle.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let emit = |progress: &TransferProgress| {
                crate::web_server::emit_to_all(&app, PROGRESS_EVENT, progress.clone())
            };
            // The guard drops as this returns: the project is free again
            // before anything below runs.
            run_transfer(&store, &LiveHost, &guard, include_uncommitted, &emit)
        })
        .await
        .map_err(|e| format!("The transfer stopped unexpectedly: {e}"))??
    };

    after_transfer(
        &store,
        &app_handle,
        &executor,
        &act_registry,
        project.location,
        &result,
    );
    Ok(result)
}

/// Ask a running transfer to stop before it switches the project over.
/// Returns whether one was running.
#[tauri::command(rename_all = "camelCase")]
pub fn cancel_project_transfer(project_id: String) -> bool {
    registry::cancel(&project_id)
}

/// Drop the source's diff caches, run prerun actions in the new local
/// worktrees, and kick every branch's queue.
fn after_transfer(
    store: &Arc<Store>,
    app_handle: &AppHandle,
    executor: &Arc<ActionExecutor>,
    act_registry: &Arc<ActionRegistry>,
    source: ProjectLocation,
    result: &ProjectTransferResult,
) {
    for repo in &result.repos {
        if let Err(e) =
            crate::diff_cache::delete_branch_cache(source, &result.project_id, &repo.branch_id)
        {
            log::warn!(
                "[project_transfer] cannot clear the diff cache of {}: {e}",
                repo.branch_id
            );
        }
        if result.location == ProjectLocation::Local && repo.carried {
            let store = Arc::clone(store);
            let app = app_handle.clone();
            let executor = Arc::clone(executor);
            let act_registry = Arc::clone(act_registry);
            let branch_id = repo.branch_id.clone();
            let project_id = result.project_id.clone();
            tauri::async_runtime::spawn(async move {
                if let crate::branches::PrerunOutcome::Ran(_) =
                    crate::branches::claim_and_run_prerun_actions(
                        &store,
                        &app,
                        &branch_id,
                        &executor,
                        &act_registry,
                        None,
                        "transfer_project",
                    )
                    .await
                {
                    crate::web_server::emit_to_all(&app, "project-setup-progress", project_id);
                }
            });
        }
        crate::session_queue::kick(
            app_handle,
            &repo.branch_id,
            crate::session_queue::KickSource::ProjectTransferred,
        );
    }
}

/// Remove transfer backups past their retention. Run once at startup.
pub(crate) fn prune_backups() {
    let Ok(dir) = LiveHost.transfers_dir() else {
        return;
    };
    let removed = backups::prune(&dir, backups::RETENTION, std::time::SystemTime::now());
    if !removed.is_empty() {
        log::info!(
            "[project_transfer] pruned {} transfer backups older than {} days",
            removed.len(),
            backups::RETENTION.as_secs() / 86_400
        );
    }
}
