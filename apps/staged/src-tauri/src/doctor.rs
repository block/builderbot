//! Tauri command wrappers for the doctor health-check system.

pub use doctor::types::{AuthStatus, InstallSource};
pub use doctor::{
    AgentVersionInfo, CheckStatus, DoctorCheck, DoctorReport, ExecuteFixOptions, FixType,
    RunChecksOptions,
};

/// Environment snapshot for doctor checks and fixes. Shaped through
/// `apply_bundled_tools_env` so checks resolve binaries from the same PATH
/// the agent spawn path uses — a bridge Staged bundles must never be
/// reported missing (or prompt an install) just because the user has no
/// global copy.
async fn doctor_env_vars(app_handle: &tauri::AppHandle) -> Vec<(String, String)> {
    let mut env_vars = crate::shell_env::home_env_vars_with_extended_path(
        crate::session_runner::shell_env_cache().as_ref(),
    )
    .await;
    if let Some(dir) = crate::acp_tools::resolve_bundled_acp_tools_dir(app_handle) {
        crate::acp_tools::apply_bundled_tools_env(&mut env_vars, &dir);
    }
    env_vars
}

fn run_checks_options(check_freshness: bool, env_vars: Vec<(String, String)>) -> RunChecksOptions {
    RunChecksOptions {
        check_freshness,
        offline: false,
        // Use the default public registries — Staged installs these agents
        // from public npm/brew/crates.io, not an internal mirror.
        npm_registry: None,
        env: None,
    }
    .with_env_snapshot(env_vars)
}

fn execute_fix_options(
    command_override: Option<String>,
    env_vars: Vec<(String, String)>,
) -> ExecuteFixOptions {
    ExecuteFixOptions {
        command_override,
        npm_registry: None,
        env: None,
    }
    .with_env_snapshot(env_vars)
}

/// Run all health checks and return the report.
///
/// This is the cheap, no-network path: it resolves binaries and reports
/// install/auth status but does not probe registries for version freshness.
/// The frontend calls this first for an instant paint, then follows up with
/// [`run_doctor_freshness`] to fill in version/update information.
#[tauri::command]
pub async fn run_doctor(app_handle: tauri::AppHandle) -> DoctorReport {
    let env_vars = doctor_env_vars(&app_handle).await;
    doctor::run_checks_with_options(run_checks_options(false, env_vars)).await
}

/// Run all health checks with version freshness enabled.
///
/// This is the slower second pass: it probes each readout's installed version
/// and looks up the latest version from the relevant registry (npm, brew,
/// crates.io, GitHub releases), populating `installedVersion`, `latestVersion`,
/// `updateAvailable`, and the source-aware `updateCommand`/`updateFixType` on
/// each readout. Hits the network, so it must never block first paint.
#[tauri::command]
pub async fn run_doctor_freshness(app_handle: tauri::AppHandle) -> DoctorReport {
    let env_vars = doctor_env_vars(&app_handle).await;
    doctor::run_checks_with_options(run_checks_options(true, env_vars)).await
}

/// Run a fix for a doctor check, identified by check ID and fix type.
///
/// The actual shell command is looked up from the static check definitions —
/// the caller never sends a raw command string.
#[tauri::command]
pub async fn run_doctor_fix(
    app_handle: tauri::AppHandle,
    check_id: String,
    fix_type: FixType,
) -> Result<(), String> {
    let env_vars = doctor_env_vars(&app_handle).await;
    doctor::execute_fix_with_env_options(check_id, fix_type, execute_fix_options(None, env_vars))
        .await
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
    app_handle: tauri::AppHandle,
    check_id: String,
    fix_type: FixType,
    command: String,
) -> Result<(), String> {
    let env_vars = doctor_env_vars(&app_handle).await;
    let expected = expected_update_command(&check_id, &fix_type, env_vars.clone()).await?;
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
    doctor::execute_fix_with_env_options(
        check_id,
        fix_type,
        execute_fix_options(Some(expected), env_vars),
    )
    .await
}

/// Re-run freshness and return the authoritative update command for the given
/// check + slot, or an error if no actionable update is derivable.
async fn expected_update_command(
    check_id: &str,
    fix_type: &FixType,
    env_vars: Vec<(String, String)>,
) -> Result<String, String> {
    let report = doctor::run_checks_with_options(run_checks_options(true, env_vars)).await;

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
