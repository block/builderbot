//! Tauri commands behind the consent dialog and the Settings "Connected
//! apps" panel. Desktop only: `web_server::dispatch` refuses them, so a
//! browser client can neither answer a consent prompt nor revoke a grant,
//! and they stay out of `ALLOWED_COMMANDS` so no stagedctl caller can
//! approve itself.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, State};

use super::consent::{self, ControlGrantSummary};
use super::enrol::ControlAuthorizationRequest;
use super::ControlGrants;
use crate::store::{now_timestamp, Store};

/// The connection requests awaiting an answer, oldest first. The frontend
/// takes this snapshot once its event listener is live, so a request that
/// arrived before the window loaded is still offered.
#[tauri::command]
pub fn list_pending_control_authorizations(
    grants: State<'_, ControlGrants>,
) -> Vec<ControlAuthorizationRequest> {
    grants
        .enrolment()
        .pending(now_timestamp())
        .iter()
        .map(|pending| pending.summary())
        .collect()
}

/// Answer connection request `request_id`. Approving writes the durable
/// grant and mints this run's bearer; denying tells the waiting CLI
/// `authorization_denied`. Dismissing the dialog denies.
#[tauri::command]
pub fn answer_control_authorization(
    grants: State<'_, ControlGrants>,
    request_id: String,
    approve: bool,
) -> Result<(), String> {
    if approve {
        consent::approve(&grants, &request_id)
    } else {
        consent::deny(&grants, &request_id)
    }
}

/// Every approved client that has not been revoked, newest first.
#[tauri::command]
pub fn list_control_grants(
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    grants: State<'_, ControlGrants>,
) -> Result<Vec<ControlGrantSummary>, String> {
    let store = store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized".to_string())?;
    consent::grant_summaries(&store, &grants)
}

/// Revoke grant `grant_id`: its live bearers are dropped at once and its
/// next reconnect reads `authorization_revoked`.
#[tauri::command]
pub fn revoke_control_grant(
    app_handle: AppHandle,
    grants: State<'_, ControlGrants>,
    grant_id: String,
) -> Result<(), String> {
    consent::revoke_grant(&app_handle, &grants, &grant_id).map(|_| ())
}
