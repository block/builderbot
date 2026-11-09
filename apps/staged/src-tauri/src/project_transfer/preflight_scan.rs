//! A preflight that streams: the dialog starts a scan, gets the preflight as
//! it stands after each step over [`PREFLIGHT_EVENT`], and can stop it. The
//! one-shot [`super::commands::get_project_transfer_preflight`] stays for
//! callers that want to wait.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tauri::AppHandle;

use crate::store::Store;

use super::host::LiveHost;
use super::preflight::{
    inspect_after_resuming, Inspection, PreflightObserver, ProjectTransferPreflight,
};
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
    let cancel = register_scan(&scan_id);
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
        run_scan(
            &scan_id,
            &cancel,
            |observer| {
                inspect_after_resuming(
                    &store,
                    &LiveHost,
                    &project_id,
                    &Default::default(),
                    observer,
                )
            },
            &emit,
        );
    });
    Ok(())
}

/// Register `scan_id` as running, with the flag that cancels it.
fn register_scan(scan_id: &str) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    scans()
        .lock()
        .unwrap()
        .insert(scan_id.to_string(), cancel.clone());
    cancel
}

/// Run the registered scan `scan_id`: `inspect` with an observer that
/// `emit`s each partial (the preflight, not done, no error) and stops at
/// `cancel`, then `emit` the end — done, with the final preflight, or with
/// the error (cancelled, failed, or panicked). The scan's registration goes
/// however `inspect` ends, a panic included: the dialog is told the scan is
/// over, never left on its last partial.
fn run_scan(
    scan_id: &str,
    cancel: &AtomicBool,
    inspect: impl FnOnce(&PreflightObserver<'_>) -> Result<Inspection, String>,
    emit: &dyn Fn(Option<&ProjectTransferPreflight>, bool, Option<String>),
) {
    let on_partial = |preflight: &ProjectTransferPreflight| {
        if !preflight.scanning {
            // The last step is the final result: one event says both.
            return;
        }
        emit(Some(preflight), false, None);
    };
    let cancelled = || cancel.load(Ordering::SeqCst);
    let observer = PreflightObserver {
        on_partial: &on_partial,
        cancelled: &cancelled,
    };
    let result = catch_unwind(AssertUnwindSafe(|| inspect(&observer)));
    scans().lock().unwrap().remove(scan_id);
    match result {
        Ok(Ok(inspection)) => emit(Some(&inspection.preflight), true, None),
        Ok(Err(e)) if cancel.load(Ordering::SeqCst) => {
            emit(None, true, Some(TransferError::Cancelled.to_string()));
            log::info!("[project_transfer] preflight scan {scan_id} cancelled: {e}");
        }
        Ok(Err(e)) => emit(None, true, Some(e)),
        Err(panic) => {
            let what = panic
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("a panic");
            let message = format!("The scan stopped unexpectedly: {what}");
            log::error!("[project_transfer] preflight scan {scan_id}: {message}");
            emit(None, true, Some(message));
        }
    }
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

#[cfg(test)]
mod tests {
    use super::super::preflight::TransferDirection;
    use super::*;
    use crate::store::ProjectLocation;

    fn partial(project_id: &str) -> ProjectTransferPreflight {
        ProjectTransferPreflight {
            project_id: project_id.to_string(),
            from: ProjectLocation::Local,
            to: ProjectLocation::Remote,
            direction: TransferDirection::ToRemote,
            workspace_name: None,
            repos: Vec::new(),
            blockers: Vec::new(),
            warnings: Vec::new(),
            can_transfer: true,
            has_uncommitted_changes: false,
            scanning: true,
        }
    }

    /// What one scan emitted: `(had a preflight, done, error)` per event.
    type Emitted = Vec<(bool, bool, Option<String>)>;

    fn run(
        scan_id: &str,
        inspect: impl FnOnce(&PreflightObserver<'_>) -> Result<Inspection, String>,
    ) -> Emitted {
        let cancel = register_scan(scan_id);
        assert!(scans().lock().unwrap().contains_key(scan_id));
        let emitted: Mutex<Emitted> = Mutex::new(Vec::new());
        let emit = |p: Option<&ProjectTransferPreflight>, done: bool, error: Option<String>| {
            emitted.lock().unwrap().push((p.is_some(), done, error));
        };
        run_scan(scan_id, &cancel, inspect, &emit);
        emitted.into_inner().unwrap()
    }

    /// An inspection that panics — on a poisoned lock, a bug — still ends
    /// the scan: its registration goes and the dialog is told, with the
    /// panic's message, rather than left on the last partial with Transfer
    /// enabled.
    #[test]
    fn a_scan_whose_inspection_panics_ends_with_an_error() {
        let scan_id = format!("panic-{}", uuid::Uuid::new_v4());
        let project_id = "project";
        let emitted = run(&scan_id, |observer| -> Result<Inspection, String> {
            (observer.on_partial)(&partial(project_id));
            panic!("a scan per unit")
        });
        assert_eq!(
            emitted,
            vec![
                (true, false, None),
                (
                    false,
                    true,
                    Some("The scan stopped unexpectedly: a scan per unit".to_string())
                ),
            ]
        );
        assert!(
            !cancel_project_transfer_preflight_impl(&scan_id),
            "the scan is no longer registered"
        );
    }

    /// The ordinary ends, through the same path: a failure is reported as
    /// such, a cancelled scan as cancelled, and either drops the
    /// registration.
    #[test]
    fn a_scan_that_fails_or_is_cancelled_ends_the_same_way() {
        let scan_id = format!("fail-{}", uuid::Uuid::new_v4());
        let emitted = run(&scan_id, |_| Err("no such project".to_string()));
        assert_eq!(
            emitted,
            vec![(false, true, Some("no such project".to_string()))]
        );
        assert!(!cancel_project_transfer_preflight_impl(&scan_id));

        let scan_id = format!("cancel-{}", uuid::Uuid::new_v4());
        let emitted = run(&scan_id, |observer| {
            (observer.on_partial)(&partial("project"));
            assert!(cancel_project_transfer_preflight_impl(&scan_id));
            assert!((observer.cancelled)());
            Err(TransferError::Cancelled.to_string())
        });
        assert_eq!(
            emitted,
            vec![
                (true, false, None),
                (false, true, Some(TransferError::Cancelled.to_string())),
            ]
        );
        assert!(!cancel_project_transfer_preflight_impl(&scan_id));
    }
}
