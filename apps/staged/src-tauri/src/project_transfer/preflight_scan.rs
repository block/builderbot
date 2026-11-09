//! A preflight that streams: the dialog starts a scan, gets the preflight as
//! it stands after each step over [`PREFLIGHT_EVENT`], and can stop it. The
//! one-shot [`super::commands::get_project_transfer_preflight`] stays for
//! callers that want to wait.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tauri::AppHandle;

use crate::store::Store;

use super::host::LiveHost;
use super::preflight::{inspect_after_resuming, PreflightObserver, ProjectTransferPreflight};
use super::registry;
use super::TransferError;

/// The event every scan step goes out as.
pub const PREFLIGHT_EVENT: &str = "project-transfer-preflight";

/// One step of a scan. `preflight` is what is known so far (with the rows
/// still being read marked `scanning`); `done` says the scan is over, with
/// `preflight` final, or `cancelled`, or failed with `error`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightScanEvent {
    pub project_id: String,
    pub scan_id: String,
    pub preflight: Option<ProjectTransferPreflight>,
    pub done: bool,
    pub cancelled: bool,
    pub error: Option<String>,
}

fn scans() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static SCANS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    SCANS.get_or_init(Default::default)
}

/// Scan `project_id` for a transfer, emitting [`PREFLIGHT_EVENT`] as it goes
/// under the caller's `scan_id` — the caller's, so it can listen before the
/// first step is out. Returns once the scan is registered; the scan runs on.
pub(crate) async fn start_project_transfer_preflight_impl(
    store: Arc<Store>,
    app_handle: AppHandle,
    project_id: String,
    scan_id: String,
) -> Result<(), String> {
    if scan_id.is_empty() {
        return Err("A scan needs an id.".to_string());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    scans()
        .lock()
        .unwrap()
        .insert(scan_id.clone(), cancel.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let emit =
            |preflight: Option<&ProjectTransferPreflight>, done: bool, error: Option<String>| {
                let mut preflight = preflight.cloned();
                if let Some(p) = preflight.as_mut() {
                    add_registry_blocker(p);
                }
                crate::web_server::emit_to_all(
                    &app_handle,
                    PREFLIGHT_EVENT,
                    PreflightScanEvent {
                        project_id: project_id.clone(),
                        scan_id: scan_id.clone(),
                        preflight,
                        done,
                        cancelled: done && cancel.load(Ordering::SeqCst),
                        error,
                    },
                );
            };
        let on_partial = |preflight: &ProjectTransferPreflight| {
            if !preflight.scanning {
                // The last step is the final result: one event says both.
                return;
            }
            emit(Some(preflight), false, None);
        };
        let cancelled_flag = cancel.clone();
        let cancelled = move || cancelled_flag.load(Ordering::SeqCst);
        let observer = PreflightObserver {
            on_partial: &on_partial,
            cancelled: &cancelled,
        };
        let result = inspect_after_resuming(
            &store,
            &LiveHost,
            &project_id,
            &Default::default(),
            &observer,
        );
        scans().lock().unwrap().remove(&scan_id);
        match result {
            Ok(inspection) => emit(Some(&inspection.preflight), true, None),
            Err(e) if cancel.load(Ordering::SeqCst) => {
                emit(None, true, Some(TransferError::Cancelled.to_string()));
                log::info!("[project_transfer] preflight scan {scan_id} cancelled: {e}");
            }
            Err(e) => emit(None, true, Some(e)),
        }
    });
    Ok(())
}

/// Stop the scan `scan_id`: the runs already going finish, no more start,
/// and the scan ends with `cancelled`. `false` when no such scan is running.
pub(crate) fn cancel_project_transfer_preflight_impl(scan_id: &str) -> bool {
    match scans().lock().unwrap().get(scan_id) {
        Some(cancel) => {
            cancel.store(true, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

fn add_registry_blocker(preflight: &mut ProjectTransferPreflight) {
    if registry::is_project_transferring(&preflight.project_id) {
        preflight.add_blocker(super::preflight::TransferIssue::new(
            "already_transferring",
            "This project is already being transferred.",
        ));
    }
}

/// See [`start_project_transfer_preflight_impl`].
#[tauri::command(rename_all = "camelCase")]
pub async fn start_project_transfer_preflight(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: AppHandle,
    project_id: String,
    scan_id: String,
) -> Result<(), String> {
    let store = crate::get_store(&store)?;
    start_project_transfer_preflight_impl(store, app_handle, project_id, scan_id).await
}

/// See [`cancel_project_transfer_preflight_impl`].
#[tauri::command(rename_all = "camelCase")]
pub fn cancel_project_transfer_preflight(scan_id: String) -> bool {
    cancel_project_transfer_preflight_impl(&scan_id)
}
