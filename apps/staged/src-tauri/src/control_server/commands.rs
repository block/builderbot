//! Tauri commands behind the consent dialog and the Settings "Connected
//! apps" panel. Desktop only: `web_server::dispatch` refuses them, so a
//! browser client can neither answer a consent prompt nor revoke a grant,
//! and they stay out of `ALLOWED_COMMANDS` so no stagedctl caller can
//! approve itself.

use tauri::{AppHandle, Manager, State};

use super::consent::{self, ControlGrantSummary};
use super::enrol::ControlAuthorizationRequest;
use super::ControlGrants;
use crate::store::now_timestamp;

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

/// Answer connection request `request_id`. Approving records the answer in
/// memory only: the durable grant is written when the waiting CLI collects
/// it (`consent::mint_collected_approval`), off this thread. Denying tells
/// the CLI `authorization_denied`. Dismissing the dialog denies.
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
///
/// A store read per grant, run on the blocking pool so a wait on the store
/// lock does not hold the main thread. The store and the grants are looked
/// up from the handle inside the closure.
#[tauri::command]
pub async fn list_control_grants(
    app_handle: AppHandle,
) -> Result<Vec<ControlGrantSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = consent::store_of(&app_handle)?;
        let grants = app_handle.state::<ControlGrants>();
        consent::grant_summaries(&store, &grants)
    })
    .await
    .map_err(|e| format!("list_control_grants task failed: {e}"))?
}

/// Revoke grant `grant_id`: its live bearers are dropped at once and its
/// next reconnect reads `authorization_revoked`.
///
/// Stamps the row in the store, so it runs on the blocking pool like
/// [`list_control_grants`].
#[tauri::command]
pub async fn revoke_control_grant(app_handle: AppHandle, grant_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let grants = app_handle.state::<ControlGrants>();
        consent::revoke_grant(&app_handle, &grants, &grant_id).map(|_| ())
    })
    .await
    .map_err(|e| format!("revoke_control_grant task failed: {e}"))?
}
