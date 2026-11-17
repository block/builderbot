//! Why each failed branch is in `Error`, for the UI.
//!
//! A setup's error used to reach the frontend only as the rejection of the
//! `start_workspace` call that happened to be waiting on it. A run nobody
//! was joined to — the startup pass, a poll's trigger, a retry — failed into
//! the log, and the branch showed "Something went wrong."
//!
//! So the reason is kept here and announced: every change is emitted as
//! `workspace-setup-error` (`{ branchId, error }`, `error: null` when it is
//! cleared), and [`get_workspace_setup_errors`] hands a window that mounts
//! later what it missed.
//!
//! In memory, per process: the branch row keeps only its status. After a
//! restart the reasons are gone until the next attempt fails, which the
//! startup pass's retries ([`super::retry`]) bring about on their own.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

/// The event a reason's change is announced with.
pub(crate) const EVENT: &str = "workspace-setup-error";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupErrorEvent<'a> {
    branch_id: &'a str,
    error: Option<&'a str>,
}

fn reasons() -> &'static Mutex<HashMap<String, String>> {
    static REASONS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    REASONS.get_or_init(Default::default)
}

/// Record why `branch_id` failed (`Some`) or that it no longer has a reason
/// (`None`), and tell every window and web client. Clearing a branch that
/// has no reason announces nothing.
pub(crate) fn set<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    branch_id: &str,
    error: Option<&str>,
) {
    let changed = {
        let mut reasons = reasons().lock().unwrap_or_else(|p| p.into_inner());
        match error {
            Some(error) => {
                reasons.insert(branch_id.to_string(), error.to_string());
                true
            }
            None => reasons.remove(branch_id).is_some(),
        }
    };
    if changed {
        crate::web_server::emit_to_all(app_handle, EVENT, SetupErrorEvent { branch_id, error });
    }
}

/// Every branch's recorded reason.
pub(crate) fn all() -> HashMap<String, String> {
    reasons().lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// The reasons the branches in `Error` failed for, by branch id, for a
/// client that missed their `workspace-setup-error` events (one mounting, or
/// reloaded). Branches that failed before this process started are absent.
#[tauri::command]
pub fn get_workspace_setup_errors() -> HashMap<String, String> {
    all()
}
