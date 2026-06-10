//! Tauri command wrappers for the doctor health-check system.

pub use doctor::types::{AuthStatus, InstallSource};
pub use doctor::{
    AgentVersionInfo, CheckStatus, DoctorCheck, DoctorReport, FixType, RunChecksOptions,
};

/// Run all health checks and return the report.
///
/// This is the cheap, no-network path: it resolves binaries and reports
/// install/auth status but does not probe registries for version freshness.
/// The frontend calls this first for an instant paint, then follows up with
/// [`run_doctor_freshness`] to fill in version/update information.
#[tauri::command]
pub async fn run_doctor() -> DoctorReport {
    doctor::run_checks().await
}

/// Run all health checks with version freshness enabled.
///
/// This is the slower second pass: it probes each readout's installed version
/// and looks up the latest version from the relevant registry (npm, brew,
/// crates.io, GitHub releases), populating `installedVersion`, `latestVersion`,
/// `updateAvailable`, and the source-aware `updateCommand`/`updateFixType` on
/// each readout. Hits the network, so it must never block first paint.
#[tauri::command]
pub async fn run_doctor_freshness() -> DoctorReport {
    doctor::run_checks_with_options(RunChecksOptions {
        check_freshness: true,
        offline: false,
        // Use the default public registries — Staged installs these agents
        // from public npm/brew/crates.io, not an internal mirror.
        npm_registry: None,
    })
    .await
}

/// Run a fix for a doctor check, identified by check ID and fix type.
///
/// The actual shell command is looked up from the static check definitions —
/// the caller never sends a raw command string.
#[tauri::command]
pub async fn run_doctor_fix(check_id: String, fix_type: FixType) -> Result<(), String> {
    doctor::execute_fix(check_id, fix_type).await
}

/// Run a source-aware update for a single readout (main CLI or ACP bridge).
///
/// Unlike [`run_doctor_fix`], update commands (`UpdateMain`/`UpdateBridge`) are
/// derived per-readout at freshness time rather than living in the static check
/// table, so the executor needs the command passed in as an override.
///
/// **Trust boundary:** we do not execute the frontend-supplied `command`
/// blindly. We re-run freshness, re-derive the expected `updateCommand` for
/// `(check_id, fix_type)` backend-side, and only proceed if the two match. This
/// keeps `run_doctor_update` from becoming an arbitrary-shell-exec hole — the
/// `command` argument is effectively a confirmation of what the UI displayed,
/// validated against the authoritative backend derivation.
#[tauri::command]
pub async fn run_doctor_update(
    check_id: String,
    fix_type: FixType,
    command: String,
) -> Result<(), String> {
    let expected = expected_update_command(&check_id, &fix_type).await?;
    if expected != command {
        return Err(format!(
            "Update command mismatch for {check_id}: refusing to run a command \
             that does not match the backend-derived update command."
        ));
    }
    // Run the backend-derived `expected`, not the frontend-supplied `command`.
    // They are equal past the guard above, but executing `expected` makes the
    // command that runs provably the one the backend derived — no dependence on
    // the equality check surviving future edits.
    doctor::execute_fix_with_options(check_id, fix_type, Some(expected), None).await
}

/// Re-run freshness and return the authoritative update command for the given
/// check + slot, or an error if no actionable update is derivable.
async fn expected_update_command(check_id: &str, fix_type: &FixType) -> Result<String, String> {
    let report = doctor::run_checks_with_options(RunChecksOptions {
        check_freshness: true,
        offline: false,
        npm_registry: None,
    })
    .await;

    let check = report
        .checks
        .iter()
        .find(|c| c.id == check_id)
        .ok_or_else(|| format!("No such check: {check_id}"))?;

    // The fix type selects which readout's update applies.
    let readout = match fix_type {
        FixType::UpdateMain => check.main.as_ref(),
        FixType::UpdateBridge => check.bridge.as_ref(),
        _ => return Err(format!("{fix_type:?} is not an update fix type")),
    };

    readout
        .and_then(|r| r.update_command.clone())
        .ok_or_else(|| format!("No actionable update available for {check_id}"))
}
