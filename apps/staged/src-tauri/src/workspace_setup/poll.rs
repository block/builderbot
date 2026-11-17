//! The workspace status pollers, shared by the Tauri commands and the web
//! server's arms.
//!
//! A poll reconciles a branch row with what Blox reports for its
//! workstation. It owns every transition but one: it never writes `Running`
//! over `Starting`. A running workstation says nothing about whether the
//! branch is checked out in it, so a poll that finds one behind a `Starting`
//! row triggers branch setup (see the parent module) and reports `Starting`
//! until setup has written `Running` itself. Nor does it write `Running`
//! over `Error`: a setup that failed left the row there, and a client still
//! polling on a stale `starting` must be told so, not handed a branch whose
//! checkout never happened.

use std::collections::HashMap;
use std::sync::Arc;

use tauri::AppHandle;

use crate::blox::{self, BloxError};
use crate::branches;
use crate::store::{self, Store, WorkspaceStatus};
use crate::PollWorkspaceResult;

/// Called with a branch id and what [`Store::update_branch_workspace_status`]
/// reported for a status write (see [`branches::workspace_status_written`]).
pub(crate) type StatusWritten<'a> = Box<dyn Fn(&str, bool, &WorkspaceStatus) + 'a>;

/// What a poll does besides writing the row, injected so tests can watch it.
pub(crate) struct PollEffects<'a> {
    pub(crate) status_written: StatusWritten<'a>,
    /// Called for a `Starting` branch whose workstation is running.
    pub(crate) trigger_setup: Box<dyn Fn(&str) + 'a>,
}

/// What Blox said about one workstation.
pub(crate) struct Reported<'a> {
    pub(crate) status: Option<&'a str>,
    pub(crate) workstation_id: Option<u64>,
}

fn result(status: &WorkspaceStatus, workspace: &str) -> PollWorkspaceResult {
    PollWorkspaceResult {
        status: status.as_str().to_string(),
        workstation_id: branches::cached_workstation_id(workspace),
    }
}

/// Whether `branch` is a secondary clone still being set up in a workstation
/// a peer branch already runs on. Such a branch uses `Starting` as its own
/// loading state while its clone, fetch and checkout run, and is held there
/// until that setup marks it `Running`.
///
/// This must apply only to secondary clones. A project's first remote branch
/// has a workspace subpath too, so keying on the subpath alone would skip
/// polling for it altogether.
pub(crate) fn is_secondary_clone_setup(
    store: &Store,
    branch: &store::Branch,
) -> Result<bool, String> {
    if branch.workspace_status != Some(WorkspaceStatus::Starting)
        || branches::resolve_branch_workspace_subpath(store, branch)?.is_none()
    {
        return Ok(false);
    }
    let Some(workspace) = branch.workspace_name.as_deref() else {
        return Ok(false);
    };
    let peers = store
        .list_branches_for_project(&branch.project_id)
        .map_err(|e| e.to_string())?;
    Ok(peers.into_iter().any(|peer| {
        peer.id != branch.id
            && peer.branch_type == store::BranchType::Remote
            && peer.workspace_name.as_deref() == Some(workspace)
            && peer.workspace_status == Some(WorkspaceStatus::Running)
    }))
}

/// Reconcile `branch`'s row with the status Blox reported, and return the
/// status the row now holds.
pub(crate) fn settle_reported(
    store: &Store,
    branch: &store::Branch,
    workspace: &str,
    reported: &Reported<'_>,
    effects: &PollEffects<'_>,
) -> Result<WorkspaceStatus, String> {
    // Cache the numeric workstation ID for proxy URL construction.
    if let Some(ws_id) = reported.workstation_id {
        if let Ok(mut cache) = branches::workstation_id_cache().lock() {
            cache.insert(workspace.to_string(), ws_id);
        }
    }

    let row = branch.workspace_status.as_ref();
    let mapped = branches::map_blox_status_to_workspace_status(reported.status, row);
    if mapped == WorkspaceStatus::Running {
        match row {
            // The workstation is up; the branch may not be checked out in
            // it. Setup finds out, and is the one to say `Running`.
            Some(WorkspaceStatus::Starting) => {
                (effects.trigger_setup)(&branch.id);
                return Ok(WorkspaceStatus::Starting);
            }
            // Setup failed here. Only another setup can clear that.
            Some(WorkspaceStatus::Error) => return Ok(WorkspaceStatus::Error),
            _ => {}
        }
    }

    let previous = row.map(|s| s.as_str()).unwrap_or("none");
    if previous != mapped.as_str() {
        log::debug!(
            "[workspace_poll] branch={} ws={workspace} transition {previous} -> {} (raw={:?})",
            branch.id,
            mapped.as_str(),
            reported.status
        );
    }
    let changed = store
        .update_branch_workspace_status(&branch.id, &mapped)
        .map_err(|e| e.to_string())?;
    (effects.status_written)(&branch.id, changed, &mapped);
    Ok(mapped)
}

/// Settle one branch against the outcome of its `ws info`.
pub(crate) fn settle_one(
    store: &Store,
    branch: &store::Branch,
    workspace: &str,
    info: Result<Reported<'_>, &BloxError>,
    effects: &PollEffects<'_>,
) -> Result<PollWorkspaceResult, String> {
    let reported = match info {
        Ok(reported) => reported,
        Err(BloxError::NotAuthenticated) => {
            // Auth errors are definitive — stop polling and surface the error.
            store
                .update_branch_workspace_status(&branch.id, &WorkspaceStatus::Error)
                .ok();
            return Err("Not authenticated with Blox. Run: sq login".to_string());
        }
        Err(e) => {
            // During initial creation, `blox ws start` may still be running
            // when the first poll fires `blox ws info`. The workspace doesn't
            // exist yet, so the CLI returns "not found". If the DB still says
            // Starting, swallow the error and keep polling.
            if branch.workspace_status == Some(WorkspaceStatus::Starting) {
                log::debug!(
                    "blox ws info failed for '{workspace}' while still Starting, treating as Starting: {e}"
                );
                return Ok(result(&WorkspaceStatus::Starting, workspace));
            }
            // If the workspace was Running but Blox explicitly says "not
            // found" (e.g. deleted externally), transition to Stopped. Other
            // errors (network blips, timeouts, etc.) are transient — keep the
            // Running status so the poller retries.
            if branch.workspace_status == Some(WorkspaceStatus::Running) {
                let is_not_found = matches!(e, BloxError::CommandFailed(msg) if msg.to_lowercase().contains("not found"));
                if is_not_found {
                    log::debug!(
                        "blox ws info returned not-found for '{workspace}' while Running, treating as Stopped: {e}"
                    );
                    store
                        .update_branch_workspace_status(&branch.id, &WorkspaceStatus::Stopped)
                        .ok();
                    return Ok(result(&WorkspaceStatus::Stopped, workspace));
                }
                log::warn!(
                    "blox ws info failed for '{workspace}' while Running, keeping Running status: {e}"
                );
                return Ok(result(&WorkspaceStatus::Running, workspace));
            }
            return Err(e.to_string());
        }
    };

    let status = settle_reported(store, branch, workspace, &reported, effects)?;
    Ok(result(&status, workspace))
}

/// What a batch poll settled.
pub(crate) struct BatchSettled {
    pub(crate) results: HashMap<String, PollWorkspaceResult>,
    /// Workspace name → the branches still `Starting` on it, whose bootstrap
    /// progress is worth fetching.
    pub(crate) starting_workspaces: HashMap<String, Vec<String>>,
}

/// Settle `branch_ids` against one `ws list`. Branches that cannot be
/// resolved (not in the store, no workspace name) are left out.
pub(crate) fn settle_all(
    store: &Store,
    branch_ids: &[String],
    entries: &[blox::WorkspaceListEntry],
    effects: &PollEffects<'_>,
) -> BatchSettled {
    let listed: HashMap<&str, &blox::WorkspaceListEntry> =
        entries.iter().map(|e| (e.name.as_str(), e)).collect();
    let mut settled = BatchSettled {
        results: HashMap::new(),
        starting_workspaces: HashMap::new(),
    };

    for branch_id in branch_ids {
        let Ok(Some(branch)) = store.get_branch(branch_id) else {
            continue;
        };
        let Some(workspace) = branch.workspace_name.as_deref() else {
            continue;
        };

        let held = is_secondary_clone_setup(store, &branch).unwrap_or_else(|e| {
            log::warn!("Failed to check secondary clone setup for branch {branch_id}: {e}");
            false
        });
        if held {
            log::debug!(
                "[workspace_poll] branch={branch_id} ws={workspace} held at Starting for secondary clone setup"
            );
            settled.results.insert(
                branch_id.clone(),
                result(&WorkspaceStatus::Starting, workspace),
            );
            continue;
        }

        let status = match listed.get(workspace) {
            Some(entry) => {
                let reported = Reported {
                    status: entry.status.as_deref(),
                    workstation_id: entry.workstation_id,
                };
                match settle_reported(store, &branch, workspace, &reported, effects) {
                    Ok(status) => status,
                    Err(e) => {
                        log::warn!("[workspace_poll] branch={branch_id} status write failed: {e}");
                        continue;
                    }
                }
            }
            // Not in the list — the same "not found" reading as a single poll.
            None => match branch.workspace_status {
                Some(WorkspaceStatus::Starting) => WorkspaceStatus::Starting,
                Some(WorkspaceStatus::Running) => {
                    log::debug!(
                        "[workspace_poll] branch={branch_id} ws={workspace} not in list while Running, treating as Stopped"
                    );
                    store
                        .update_branch_workspace_status(branch_id, &WorkspaceStatus::Stopped)
                        .ok();
                    WorkspaceStatus::Stopped
                }
                // Stopped, Error, etc. — no change, nothing to report.
                _ => continue,
            },
        };

        // Track workspaces still starting so their bootstrap progress shows.
        if status == WorkspaceStatus::Starting {
            settled
                .starting_workspaces
                .entry(workspace.to_string())
                .or_default()
                .push(branch_id.clone());
        }
        settled
            .results
            .insert(branch_id.clone(), result(&status, workspace));
    }

    settled
}

/// The `poll_workspace_status` command and web arm.
pub(crate) async fn poll_workspace_status(
    store: &Arc<Store>,
    app_handle: &AppHandle,
    branch_id: &str,
) -> Result<PollWorkspaceResult, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    let workspace = branch
        .workspace_name
        .clone()
        .ok_or("Branch is not a remote workspace branch")?;

    if is_secondary_clone_setup(store, &branch)? {
        log::debug!(
            "[workspace_poll] branch={branch_id} ws={workspace} held at Starting for secondary clone setup"
        );
        return Ok(result(&WorkspaceStatus::Starting, &workspace));
    }

    let info = branches::run_blox_blocking({
        let workspace = workspace.clone();
        move || blox::ws_info(&workspace)
    })
    .await;
    let reported = info.as_ref().map(|info| Reported {
        status: info.status.as_deref(),
        workstation_id: info.workstation_id,
    });
    settle_one(
        store,
        &branch,
        &workspace,
        reported,
        &app_effects(store, app_handle),
    )
}

/// The `poll_all_workspace_statuses` command and web arm.
pub(crate) async fn poll_all_workspace_statuses(
    store: &Arc<Store>,
    app_handle: &AppHandle,
    branch_ids: &[String],
) -> Result<HashMap<String, PollWorkspaceResult>, String> {
    // Fetch all workspaces in one CLI call.
    let entries = branches::run_blox_blocking(blox::ws_list)
        .await
        .map_err(|e| {
            if matches!(e, BloxError::NotAuthenticated) {
                "Not authenticated with Blox. Run: sq login".to_string()
            } else {
                e.to_string()
            }
        })?;

    let BatchSettled {
        results,
        starting_workspaces,
    } = settle_all(store, branch_ids, &entries, &app_effects(store, app_handle));

    // For workspaces that are still starting, fetch bootstrap command
    // progress and emit events so the UI can show which step is running. On
    // its own blocking task, so it doesn't hold up the response.
    if !starting_workspaces.is_empty() {
        let app_handle = app_handle.clone();
        tauri::async_runtime::spawn_blocking(move || {
            for (workspace, branch_ids) in &starting_workspaces {
                match blox::ws_commands(workspace) {
                    Ok(commands) => {
                        branches::emit_workspace_setup_progress(&app_handle, branch_ids, &commands);
                    }
                    Err(e) => {
                        log::debug!("[workspace_poll] ws_commands({workspace}) failed: {e}");
                    }
                }
            }
        });
    }

    Ok(results)
}

/// The effects a real poll has: kick the session queue on a `Running` write,
/// and hand a `Starting` branch on a running workstation to setup.
fn app_effects<'a>(store: &'a Arc<Store>, app_handle: &'a AppHandle) -> PollEffects<'a> {
    PollEffects {
        status_written: Box::new(move |branch_id, changed, status| {
            branches::workspace_status_written(app_handle, branch_id, changed, status);
        }),
        trigger_setup: Box::new(move |branch_id| {
            super::trigger_branch_setup(app_handle, store, branch_id);
        }),
    }
}
