//! Why each failed branch is in `Error`, for the UI.
//!
//! A setup's error used to reach the frontend only as the rejection of the
//! `start_workspace` call that happened to be waiting on it. A run nobody
//! was joined to — the startup pass, a poll's trigger, a retry — failed into
//! the log, and the branch showed "Something went wrong."
//!
//! So the reason is kept here and announced: a change is emitted as
//! `workspace-setup-error` (`{ branchId, error, userRequested }`, `error:
//! null` when it is cleared), and [`get_workspace_setup_errors`] hands a
//! window that mounts later what it missed. This is how a failure reaches
//! the UI; no client call waits on a setup to be told.
//!
//! `userRequested` says the failed setup was one the user asked for (a new
//! branch, the error view's Retry, a resume), which is what a client raises
//! a toast for. A retry on a timer, the startup pass and a poll's own run
//! fail into the card only, and a reason that has not changed is not
//! announced again.
//!
//! In memory, per process: the branch row keeps only its status. After a
//! restart the reasons are gone until the next attempt fails, which the
//! startup pass's retries ([`super::retry`]) bring about on their own.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use crate::store::{Store, WorkspaceStatus};

/// The event a reason's change is announced with.
pub(crate) const EVENT: &str = "workspace-setup-error";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupErrorEvent<'a> {
    branch_id: &'a str,
    error: Option<&'a str>,
    user_requested: bool,
}

fn reasons() -> &'static Mutex<HashMap<String, String>> {
    static REASONS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    REASONS.get_or_init(Default::default)
}

/// Record why `branch_id` failed (`Some`) or that it no longer has a reason
/// (`None`), and tell every window and web client when that changed
/// something, or when the failure is of a setup the user asked for.
///
/// Recording a failure is also when the reasons of branches that are gone,
/// or no longer in `Error`, are dropped: a deleted branch, or one a poll
/// moved on, would otherwise keep its reason for every window to come.
pub(crate) fn set<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Store,
    branch_id: &str,
    error: Option<&str>,
    user_requested: bool,
) {
    let emit = |branch_id: &str, error: Option<&str>| {
        let event = SetupErrorEvent {
            branch_id,
            error,
            user_requested: user_requested && error.is_some(),
        };
        crate::web_server::emit_to_all(app_handle, EVENT, event);
    };
    let Some(error) = error else {
        if lock().remove(branch_id).is_some() {
            emit(branch_id, None);
        }
        return;
    };
    let previous = lock().insert(branch_id.to_string(), error.to_string());
    if user_requested || previous.as_deref() != Some(error) {
        emit(branch_id, Some(error));
    }
    let recorded: Vec<String> = lock().keys().cloned().collect();
    for stale in recorded.iter().filter(|id| !is_in_error(store, id)) {
        if lock().remove(stale).is_some() {
            emit(stale, None);
        }
    }
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, String>> {
    reasons().lock().unwrap_or_else(|p| p.into_inner())
}

fn is_in_error(store: &Store, branch_id: &str) -> bool {
    matches!(
        store.get_branch(branch_id),
        Ok(Some(branch)) if branch.workspace_status == Some(WorkspaceStatus::Error)
    )
}

/// Every branch's recorded reason.
pub(crate) fn all() -> HashMap<String, String> {
    lock().clone()
}

/// The reasons the branches in `Error` failed for, by branch id, for a
/// client that missed their `workspace-setup-error` events (one mounting, or
/// reloaded). Branches that failed before this process started are absent.
#[tauri::command]
pub fn get_workspace_setup_errors() -> HashMap<String, String> {
    all()
}
