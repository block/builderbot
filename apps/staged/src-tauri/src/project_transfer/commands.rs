//! The transfer commands, over Tauri and (through the `_impl`s) the web
//! transport.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::AppHandle;

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::store::{ProjectLocation, Store};

use super::backups;
use super::host::{LiveHost, TransferHost};
use super::orchestrator::{
    check_direction, run_transfer, ProjectTransferResult, RepoTransferResult, TransferOptions,
    TransferProgress, TransferStopped,
};
use super::preflight::{inspect, ProjectTransferPreflight, TransferDirection, TransferIssue};
use super::{recovery, registry};

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
/// `project-transfer-progress` as it goes. `direction` is the one the
/// caller's preflight showed: a project that has moved since is refused, with
/// nothing done. Resolves once the project has switched (or failed, with
/// everything undone). A transfer that stopped unexpectedly after the switch
/// resolves too, with a warning saying so.
#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
pub async fn transfer_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: AppHandle,
    executor: tauri::State<'_, Arc<ActionExecutor>>,
    act_registry: tauri::State<'_, Arc<ActionRegistry>>,
    project_id: String,
    direction: TransferDirection,
    include_uncommitted: bool,
    include_ignored: bool,
) -> Result<ProjectTransferResult, String> {
    let store = crate::get_store(&store)?;
    transfer_project_impl(
        store,
        app_handle,
        Arc::clone(&executor),
        Arc::clone(&act_registry),
        project_id,
        direction,
        TransferOptions {
            include_uncommitted,
            include_ignored,
        },
    )
    .await
}

pub(crate) async fn transfer_project_impl(
    store: Arc<Store>,
    app_handle: AppHandle,
    executor: Arc<ActionExecutor>,
    act_registry: Arc<ActionRegistry>,
    project_id: String,
    direction: TransferDirection,
    options: TransferOptions,
) -> Result<ProjectTransferResult, String> {
    let branches = store
        .list_branches_for_project(&project_id)
        .map_err(|e| e.to_string())?;
    let branch_ids: Vec<String> = branches.iter().map(|b| b.id.clone()).collect();
    let transfer_id = uuid::Uuid::new_v4().to_string();
    let guard = registry::begin(&project_id, &transfer_id, direction, branch_ids.clone())?;
    // Under the entry, before anything is stopped: only a transfer moves a
    // project, so what this reads holds until the guard drops.
    let project = check_direction(&store, &guard)?;
    guard.cover_workspaces(branches.iter().filter_map(|b| b.workspace_name.as_deref()));
    let ids: Vec<&str> = branch_ids.iter().map(String::as_str).collect();
    crate::actions::commands::stop_actions_for_branches(&executor, &act_registry, &ids);

    let outcome = {
        let store = Arc::clone(&store);
        let app = app_handle.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let emit = |progress: &TransferProgress| {
                crate::web_server::emit_to_all(&app, PROGRESS_EVENT, progress.clone())
            };
            // The guard drops as this returns: the project is free again
            // before anything below runs.
            run_transfer(&store, &LiveHost, &guard, options, &emit)
        })
        .await
        .map_err(|e| format!("The transfer stopped unexpectedly: {e}"))?
    };

    settle(outcome, |result| {
        after_transfer(
            &store,
            &app_handle,
            &executor,
            &act_registry,
            project.location,
            result,
        )
    })
}

/// What a run comes to for the caller: its result, once `after_switch` has
/// run on it, whenever the project moved — including a run that stopped
/// unexpectedly after the switch, whose result says so in a warning.
pub(super) fn settle(
    outcome: Result<ProjectTransferResult, TransferStopped>,
    after_switch: impl FnOnce(&ProjectTransferResult),
) -> Result<ProjectTransferResult, String> {
    let result = match outcome {
        Ok(result) => result,
        Err(TransferStopped::AfterSwitch(result)) => *result,
        Err(TransferStopped::Failed(message)) => return Err(message),
    };
    after_switch(&result);
    Ok(result)
}

/// Ask a running transfer to stop before it switches the project over.
/// Returns whether one was running.
#[tauri::command(rename_all = "camelCase")]
pub fn cancel_project_transfer(project_id: String) -> bool {
    registry::cancel(&project_id)
}

/// Every transfer running in this process, at its latest progress: what a
/// window that loads or reconnects mid-transfer shows until the next
/// `project-transfer-progress` arrives.
#[tauri::command(rename_all = "camelCase")]
pub fn list_transferring_projects() -> Vec<TransferProgress> {
    registry::list()
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
        if runs_prerun(result, repo) {
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

/// Whether `repo`'s prerun actions run after the transfer: it was carried
/// into a new local worktree.
pub(super) fn runs_prerun(result: &ProjectTransferResult, repo: &RepoTransferResult) -> bool {
    result.location == ProjectLocation::Local && repo.carried
}

/// Roll back, or mark for the user, each transfer a Staged that is no
/// longer running left unfinished; see [`recovery`]. Run once at startup,
/// before [`prune_backups`].
pub(crate) fn recover_interrupted_transfers(store: &Store) {
    let recovered = recovery::recover(store, &LiveHost, &recovery::is_running);
    if !recovered.is_empty() {
        log::info!(
            "[project_transfer] dealt with {} interrupted transfers",
            recovered.len()
        );
    }
}

/// Remove transfer backups past their retention. Run once at startup.
pub(crate) fn prune_backups(store: Option<&Store>) {
    let Ok(dir) = LiveHost.transfers_dir() else {
        return;
    };
    let workdirs = store.map(workdir_paths).unwrap_or_default();
    let pruned = backups::prune(
        &dir,
        backups::RETENTION,
        std::time::SystemTime::now(),
        &workdirs,
    );
    if !pruned.removed.is_empty() || !pruned.trimmed.is_empty() {
        log::info!(
            "[project_transfer] pruned {} transfer backups older than {} days, keeping only the \
             ignored files left behind in {} more",
            pruned.removed.len(),
            backups::RETENTION.as_secs() / 86_400,
            pruned.trimmed.len()
        );
    }
}

/// Every workdir path the store has, as it names them: what a worktree git
/// lists by its real path is recorded as, when it is one of them.
fn workdir_paths(store: &Store) -> Vec<PathBuf> {
    let projects = match store.list_projects() {
        Ok(projects) => projects,
        Err(e) => {
            log::warn!("[project_transfer] cannot list projects: {e}");
            return Vec::new();
        }
    };
    projects
        .iter()
        .filter_map(|project| store.list_workdirs_for_project(&project.id).ok())
        .flatten()
        .map(|workdir| PathBuf::from(workdir.path))
        .collect()
}
