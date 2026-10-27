//! Staged's own doctor rows: the managed Node.js runtime and the session
//! queue.

use std::path::Path;
use std::sync::{Arc, Mutex};

use tauri::Manager;

use crate::managed_acp_tools::ReconcileRecord;
use crate::session_queue::sweep::{self, QueueHealth};
use crate::session_runner::SessionRegistry;
use crate::store::Store;

use super::managed_bridges::{format_age, last_reconcile_record};
use super::{CheckStatus, DoctorCheck, FixType};

#[cfg(test)]
#[path = "tests/runtime_checks.rs"]
mod tests;

// =============================================================================
// Managed Node.js runtime check
// =============================================================================

pub(super) const NODE_RUNTIME_CHECK_ID: &str = "node-runtime";
const NODE_RUNTIME_CHECK_LABEL: &str = "Node.js Runtime";
const SESSION_QUEUE_CHECK_ID: &str = "session-queue";
const SESSION_QUEUE_CHECK_LABEL: &str = "Session Queue";

/// The session-queue readout: what the stranded-session sweep sees right now
/// and how often it has had to act. `None` until the store is open — the
/// needs-reset flow runs the Doctor with no store managed.
pub(super) fn session_queue_doctor_check<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Option<DoctorCheck> {
    let store = crate::get_store(&app_handle.try_state::<Mutex<Option<Arc<Store>>>>()?).ok()?;
    let registry = app_handle.try_state::<Arc<SessionRegistry>>()?;
    match sweep::queue_health(&store, &registry) {
        Ok(health) => Some(build_session_queue_check(&health)),
        Err(e) => {
            log::warn!("[doctor] session-queue check failed to read the store: {e}");
            None
        }
    }
}

/// `Warn` while any row would be swept right now; `Pass` otherwise, naming
/// the lifetime sweep count when it is non-zero so a recurrence stays
/// visible after the sweep has already cleaned up.
fn build_session_queue_check(health: &QueueHealth) -> DoctorCheck {
    let stranded = health.stranded.len();
    let (status, message) = if stranded > 0 {
        (
            CheckStatus::Warn,
            format!(
                "{} {} running with nothing in this process owning {}; the stranded-session sweep ends {} within a minute",
                stranded,
                if stranded == 1 { "session is" } else { "sessions are" },
                if stranded == 1 { "it" } else { "them" },
                if stranded == 1 { "it" } else { "them" },
            ),
        )
    } else if health.swept_total > 0 {
        (
            CheckStatus::Pass,
            format!(
                "No stranded sessions; the sweep has ended {} so far",
                if health.swept_total == 1 {
                    "1 session".to_string()
                } else {
                    format!("{} sessions", health.swept_total)
                }
            ),
        )
    } else {
        (CheckStatus::Pass, "No stranded sessions".to_string())
    };

    let mut detail = vec![
        "checked: running session rows owned by this process".to_string(),
        format!("owned running rows: {}", health.owned_running),
        format!(
            "of which owned by a registered runner or a liveness hold: {}",
            health.live
        ),
        format!("stranded now: {stranded}"),
    ];
    detail.extend(health.stranded.iter().map(|row| {
        format!(
            "- session {} (branch {})",
            row.session_id,
            row.branch_id.as_deref().unwrap_or("none"),
        )
    }));
    detail.push(format!("ended by the sweep so far: {}", health.swept_total));

    DoctorCheck {
        id: SESSION_QUEUE_CHECK_ID.to_string(),
        label: SESSION_QUEUE_CHECK_LABEL.to_string(),
        status,
        message,
        fix_url: None,
        fix_command: None,
        fix_type: None,
        path: None,
        bridge_path: None,
        raw_output: Some(detail.join("\n")),
        auth_status: None,
        login_command: None,
        installed_version: None,
        latest_version: None,
        update_available: None,
        install_source: None,
        self_updating: None,
        main: None,
        bridge: None,
    }
}

/// Disk states of the Staged-managed Node.js runtime the check reports on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ManagedNodeRuntimeState {
    /// The pinned version is installed and answers the readiness probe.
    Ready,
    /// The pinned install dir exists but the probe fails — a crashed install
    /// or damaged tree that a reinstall repairs.
    Broken,
    /// The pinned version is not on disk (fresh profile, or a pin bump left
    /// only a superseded version behind).
    Missing,
}

/// Report the state of the Staged-managed Node.js runtime that npm-installed
/// agent tools run on, with a native fix that (re)installs the pinned
/// version (`run_doctor_fix` routes this check id to
/// `ensure_managed_node_runtime`). Silent when there is nothing to report:
/// an unsupported target, or a runtime that was never installed and no
/// Staged-installed npm tools that would need it.
pub(super) async fn run_node_runtime_check() -> Option<DoctorCheck> {
    let node_root = crate::managed_node::managed_node_root()?;
    let install_dir = crate::managed_node::pinned_install_dir(&node_root)?;
    let state = if crate::managed_node::pinned_runtime_ready(&node_root).await {
        ManagedNodeRuntimeState::Ready
    } else if install_dir.exists() {
        ManagedNodeRuntimeState::Broken
    } else {
        ManagedNodeRuntimeState::Missing
    };
    // Both install families depend on the runtime: the private-prefix npm
    // tools (copilot, amp-acp) and the managed bridge shims, whose embedded
    // node paths break silently without it.
    let mut npm_tools: Vec<String> = [
        crate::managed_acp_tools::npm_prefix_bin_dir(),
        crate::managed_acp_tools::managed_shim_bin_dir(),
    ]
    .into_iter()
    .flatten()
    .flat_map(|dir| installed_npm_tool_names(&dir))
    .collect();
    npm_tools.sort();
    npm_tools.dedup();
    build_node_runtime_check(
        state,
        &install_dir,
        &npm_tools,
        last_reconcile_record().as_ref(),
    )
}

/// Names of the bin shims npm wrote into the Staged-private prefix — the
/// tools that need the managed runtime to run at all.
fn installed_npm_tool_names(bin_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(bin_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect()
}

/// Build the runtime row. `last_reconcile` is `state.json`'s record of the
/// last managed install run: a runtime that is `Missing` after a reconcile
/// that *failed* is not "never needed", it is the install that could not
/// happen — every managed bridge install starts by ensuring the runtime — so
/// that case warns with the recorded error even when nothing is installed
/// yet, instead of staying silent. With no record (first launch still
/// reconciling, or a profile that never ran one) a missing runtime is
/// reported only when installed tools need it, as before.
fn build_node_runtime_check(
    state: ManagedNodeRuntimeState,
    install_dir: &Path,
    npm_tools: &[String],
    last_reconcile: Option<&ReconcileRecord>,
) -> Option<DoctorCheck> {
    let version = &crate::managed_node::node_runtime_lock().version;
    let failed_reconcile = last_reconcile.filter(|record| !record.ok);
    let (status, message) = match (&state, failed_reconcile) {
        (ManagedNodeRuntimeState::Ready, _) => (
            CheckStatus::Pass,
            format!("Staged-managed Node.js {version} is installed"),
        ),
        (ManagedNodeRuntimeState::Broken, _) => (
            CheckStatus::Warn,
            format!("Staged-managed Node.js {version} is damaged; run the fix to reinstall it"),
        ),
        (ManagedNodeRuntimeState::Missing, Some(record)) => (
            CheckStatus::Warn,
            format!(
                "Staged-managed Node.js {version} could not be installed: {}",
                reconcile_headline(record)
            ),
        ),
        (ManagedNodeRuntimeState::Missing, None) if npm_tools.is_empty() => return None,
        (ManagedNodeRuntimeState::Missing, None) => (
            CheckStatus::Warn,
            format!(
                "Staged-managed Node.js {version} is not installed; Staged-installed agent tools require it"
            ),
        ),
    };

    let state_label = match state {
        ManagedNodeRuntimeState::Ready => "ready",
        ManagedNodeRuntimeState::Broken => "broken",
        ManagedNodeRuntimeState::Missing => "missing",
    };
    let mut detail = vec![
        "checked: Staged-managed Node.js runtime".to_string(),
        format!("pinned version: {version}"),
        format!("install dir: {}", install_dir.display()),
        format!("state: {state_label}"),
    ];
    if npm_tools.is_empty() {
        detail.push("Staged-installed npm tools: none".to_string());
    } else {
        detail.push("Staged-installed npm tools:".to_string());
        detail.extend(npm_tools.iter().map(|name| format!("- {name}")));
    }
    if let Some(record) = failed_reconcile {
        detail.push(format!(
            "last managed install: failed {} with {} error(s):",
            format_age(record.at_ms, crate::managed_acp_tools::now_ms()),
            record.errors.len()
        ));
        detail.extend(record.errors.iter().map(|error| format!("- {error}")));
        // Only a runtime download error is this row's to advise on: a bridge's
        // npm 404 as the headline must not read as "no registry carries
        // Node.js".
        let headline = reconcile_headline(record);
        if let Some(hint) = headline
            .starts_with(NODE_DOWNLOAD_ERROR_PREFIX)
            .then(|| crate::managed_acp_tools::install_failure_hint(&headline, "Node.js"))
            .flatten()
        {
            detail.push(format!("hint: {hint}"));
        }
    }

    let node_path = (state == ManagedNodeRuntimeState::Ready)
        .then(|| install_dir.join("bin").join("node").display().to_string());
    // Native fix: `run_doctor_fix` routes this check id to
    // `ensure_managed_node_runtime`. The command string is what the fix
    // confirmation dialog displays, not a shell command.
    let fix = (status != CheckStatus::Pass).then(|| {
        (
            FixType::Command,
            format!("download and install Node.js {version} into ~/.staged/packages"),
        )
    });
    Some(node_runtime_doctor_check(
        status,
        message,
        node_path,
        Some(detail.join("\n")),
        fix,
    ))
}

/// How `ManagedNodeError`'s download failures (network, HTTP status) render,
/// so the runtime row can tell its own download failure from a bridge's.
const NODE_DOWNLOAD_ERROR_PREFIX: &str = "Node.js runtime download failed";

/// The one error a failed reconcile is summarised by on the runtime row: the
/// first that names the Node.js runtime download — the failure every bridge
/// shares when the runtime could not be fetched — else the first recorded,
/// in both cases without the reconciler's `<tool id>: ` prefix.
fn reconcile_headline(record: &ReconcileRecord) -> String {
    record
        .errors
        .iter()
        .find(|error| error.contains("Node.js runtime"))
        .or_else(|| record.errors.first())
        .map(|error| strip_tool_id_prefix(error).to_string())
        .unwrap_or_else(|| "no error recorded".to_string())
}

/// `<tool id>: <error>` → `<error>`, for the ids this build knows; anything
/// else is returned whole.
fn strip_tool_id_prefix(error: &str) -> &str {
    error
        .split_once(": ")
        .filter(|(id, _)| {
            crate::managed_acp_tools::MANAGED_TOOLS
                .iter()
                .any(|tool| tool.id == *id)
        })
        .map(|(_, rest)| rest)
        .unwrap_or(error)
}

pub(super) fn node_runtime_doctor_check(
    status: CheckStatus,
    message: String,
    path: Option<String>,
    raw_output: Option<String>,
    fix: Option<(FixType, String)>,
) -> DoctorCheck {
    let (fix_type, fix_command) = fix.map(|(t, c)| (Some(t), Some(c))).unwrap_or((None, None));
    DoctorCheck {
        id: NODE_RUNTIME_CHECK_ID.to_string(),
        label: NODE_RUNTIME_CHECK_LABEL.to_string(),
        status,
        message,
        fix_url: None,
        fix_command,
        fix_type,
        path,
        bridge_path: None,
        raw_output,
        auth_status: None,
        login_command: None,
        installed_version: None,
        latest_version: None,
        update_available: None,
        install_source: None,
        self_updating: None,
        main: None,
        bridge: None,
    }
}
