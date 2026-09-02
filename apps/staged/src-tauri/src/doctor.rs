//! Tauri command wrappers for the doctor health-check system.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

pub use doctor::types::{AuthStatus, InstallSource};
pub use doctor::{
    AgentVersionInfo, CheckStatus, DoctorCheck, DoctorReport, ExecuteFixOptions, FixStdin,
    FixStdinWriter, FixType, RunChecksOptions,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorLoginOutput {
    pub check_id: String,
    pub line: Option<String>,
    pub done: bool,
    pub error: Option<String>,
}

/// Writers for login fixes currently owned by the UI. This is intentionally
/// only a lifetime map for active subprocesses, not a cache of authentication
/// state; doctor remains the source of truth for whether login is available.
static ACTIVE_LOGINS: OnceLock<Mutex<HashMap<String, FixStdinWriter>>> = OnceLock::new();

fn active_logins() -> &'static Mutex<HashMap<String, FixStdinWriter>> {
    ACTIVE_LOGINS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Environment snapshot for doctor checks and fixes. Shaped through
/// `apply_managed_tools_env` so checks resolve binaries from the same PATH
/// the agent spawn path uses — a bridge Staged manages must never be
/// reported missing (or prompt an install) just because the user has no
/// global copy. The managed npm env is overlaid on top, so checks probe npm
/// state (`npm prefix -g`, version lookups) with the same private-prefix view
/// the fixes install into — a check never contradicts the fix that just ran.
async fn doctor_env_vars() -> Vec<(String, String)> {
    let mut env_vars = crate::shell_env::home_env_vars_with_extended_path(
        crate::session_runner::shell_env_cache().as_ref(),
    )
    .await;
    crate::acp_tools::apply_managed_tools_env(&mut env_vars);
    crate::managed_acp_tools::apply_managed_npm_env(
        &mut env_vars,
        &crate::managed_acp_tools::managed_npm_env(),
    );
    env_vars
}

fn run_checks_options(
    check_freshness: bool,
    env_vars: Vec<(String, String)>,
    bundled_dir: Option<PathBuf>,
) -> RunChecksOptions {
    RunChecksOptions {
        check_freshness,
        offline: false,
        // npm-backed checks and fixes route through Block's Artifactory
        // square-npm proxy (registry.npmjs.org is blocked on managed
        // devices); `no-block-npm-registry` builds fall back to npm's
        // default public registry.
        npm_registry: crate::managed_acp_tools::npm_registry().map(str::to_string),
        env: None,
        // Doctor labels binaries resolved from this dir as bundled (install
        // source + readout flag) and suppresses registry update fixes for
        // them — the startup reconciler floats the managed shims to @latest,
        // so Staged owns their updates.
        bundled_tools_dir: bundled_dir,
    }
    .with_env_snapshot(env_vars)
}

fn execute_fix_options(
    command_override: Option<String>,
    env_vars: Vec<(String, String)>,
) -> ExecuteFixOptions {
    // Everything else stays at doctor's defaults: Staged's fixes are
    // non-interactive, so nothing here feeds a prompt and the child keeps
    // inheriting stdin rather than getting a piped one; the standard fix
    // timeout is far above any install or login this runs. Spelled with
    // `..Default::default()` so a new doctor option doesn't break this
    // workspace-excluded crate, which `cargo check` under `crates/` never
    // compiles but `staged-ci.yml` does.
    ExecuteFixOptions {
        command_override,
        npm_registry: crate::managed_acp_tools::npm_registry().map(str::to_string),
        ..Default::default()
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
pub async fn run_doctor() -> DoctorReport {
    run_doctor_report(false).await
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
    run_doctor_report(true).await
}

/// Run the doctor crate's checks plus Staged-local ones (currently the
/// managed Node.js runtime check) over one shared env snapshot. Bundled
/// readouts are labeled by the doctor crate itself via
/// `RunChecksOptions::bundled_tools_dir`.
async fn run_doctor_report(check_freshness: bool) -> DoctorReport {
    let env_vars = doctor_env_vars().await;
    let (mut report, node_runtime) = tokio::join!(
        doctor::run_checks_with_options(run_checks_options(
            check_freshness,
            env_vars.clone(),
            crate::acp_tools::primary_tools_dir(),
        )),
        run_node_runtime_check(),
    );
    if let Some(check) = node_runtime {
        report.checks.push(check);
    }
    report
}

/// Start an interactive login fix and stream its output to the frontend.
#[tauri::command]
pub async fn start_doctor_login(
    app_handle: tauri::AppHandle,
    check_id: String,
) -> Result<(), String> {
    doctor::agents::lookup_fix_command(&check_id, &FixType::Auth)
        .ok_or_else(|| format!("No login fix available for {check_id}"))?;
    let env_vars = doctor_env_vars().await;
    let (writer, stdin) = FixStdin::pipe();
    {
        let mut logins = active_logins().lock().unwrap_or_else(|e| e.into_inner());
        if logins.contains_key(&check_id) {
            return Err(format!("A login is already running for {check_id}"));
        }
        logins.insert(check_id.clone(), writer);
    }

    let event_check_id = check_id.clone();
    let event_app = app_handle.clone();
    tokio::spawn(async move {
        let result = doctor::execute_fix_streaming_with_env_options(
            check_id.clone(),
            FixType::Auth,
            ExecuteFixOptions::default()
                .with_env_snapshot(env_vars)
                .with_stdin(stdin),
            move |line| {
                crate::web_server::emit_to_all(
                    &event_app,
                    "doctor-login-output",
                    DoctorLoginOutput {
                        check_id: event_check_id.clone(),
                        line: Some(line.to_string()),
                        done: false,
                        error: None,
                    },
                );
            },
        )
        .await;

        active_logins()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&check_id);
        crate::web_server::emit_to_all(
            &app_handle,
            "doctor-login-output",
            DoctorLoginOutput {
                check_id,
                line: None,
                done: true,
                error: result.err(),
            },
        );
    });
    Ok(())
}

#[tauri::command]
pub fn send_doctor_login_code(check_id: String, code: String) -> Result<(), String> {
    let writer = active_logins()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&check_id)
        .cloned()
        .ok_or_else(|| format!("No active login for {check_id}"))?;
    writer.send_line(code)
}

#[tauri::command]
pub async fn run_doctor_fix(check_id: String, fix_type: FixType) -> Result<(), String> {
    if check_id == NODE_RUNTIME_CHECK_ID {
        return ensure_managed_node_runtime_for_fix().await;
    }
    if matches!(fix_type, FixType::Command | FixType::Bridge) {
        if let Some(tool_id) = managed_tool_for_check(&check_id) {
            return install_managed_tool_logged(tool_id, &check_id).await;
        }
    }
    let env_vars = doctor_env_vars().await;
    if doctor::agents::lookup_fix_command(&check_id, &fix_type)
        .as_deref()
        .is_some_and(crate::managed_acp_tools::is_npm_backed_command)
    {
        ensure_managed_node_runtime_for_fix().await?;
    }
    doctor::execute_fix_with_env_options(check_id, fix_type, execute_fix_options(None, env_vars))
        .await
}

/// The managed ACP bridge behind a doctor check id, when this build manages
/// it. `None` routes the check to the doctor crate's regular fix commands —
/// which is also the correct fallback whenever bridge management is off (dev
/// override, `no-managed-acp-tools`, unsupported target).
fn managed_tool_for_check(check_id: &str) -> Option<&'static str> {
    let tool_id = match check_id {
        "ai-agent-claude" => "claude-acp",
        "ai-agent-codex" => "codex-acp",
        _ => return None,
    };
    crate::managed_acp_tools::managed_tool(tool_id).map(|tool| tool.id)
}

/// Install (or float-upgrade) a managed bridge for a doctor fix/update.
/// Progress goes to the log — doctor fixes have no streamed-output channel,
/// only a button spinner.
async fn install_managed_tool_logged(tool_id: &str, check_id: &str) -> Result<(), String> {
    let log_prefix = format!("[doctor fix {check_id}]");
    crate::managed_acp_tools::install_managed_tool(tool_id, &|line| {
        log::info!("{log_prefix} {line}");
    })
    .await
    .map_err(|error| error.to_string())
}

/// Install (or repair) the managed Node.js runtime ahead of a fix that needs
/// it. Progress goes to the log — doctor fixes have no streamed-output
/// channel, only a button spinner.
async fn ensure_managed_node_runtime_for_fix() -> Result<(), String> {
    crate::managed_node::ensure_managed_node_runtime()
        .await
        .map_err(|error| error.to_string())
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
    // Updates for the managed ACP bridges are the floating installer itself
    // (`<pkg>@latest` onto the managed runtime) — no shell command runs, so
    // the frontend-supplied command needs no validation here. Readouts
    // resolved from the managed shim dir derive no update command at all
    // (they are labeled bundled), so this arm only fires for a bridge copy
    // that resolved elsewhere (e.g. a user install found on PATH before the
    // first reconcile lands) — and the managed install is the correct
    // upgrade for that state too.
    if let Some(tool_id) = managed_tool_for_check(&check_id) {
        return install_managed_tool_logged(tool_id, &check_id).await;
    }
    let env_vars = doctor_env_vars().await;
    let expected = expected_update_command(
        &check_id,
        &fix_type,
        env_vars.clone(),
        crate::acp_tools::primary_tools_dir(),
    )
    .await?;
    if expected != command {
        return Err(format!(
            "Update command mismatch for {check_id}: refusing to run a command \
             that does not match the backend-derived update command."
        ));
    }
    // npm-backed updates run the managed npm into the private prefix, so the
    // managed runtime must exist before the command does.
    if crate::managed_acp_tools::is_npm_backed_command(&expected) {
        ensure_managed_node_runtime_for_fix().await?;
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
/// check + slot, or an error if no actionable update is derivable. Passes the
/// bundled dir through so bundled readouts derive no update command here,
/// exactly as in the report the UI rendered.
async fn expected_update_command(
    check_id: &str,
    fix_type: &FixType,
    env_vars: Vec<(String, String)>,
    bundled_dir: Option<PathBuf>,
) -> Result<String, String> {
    let report =
        doctor::run_checks_with_options(run_checks_options(true, env_vars, bundled_dir)).await;

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

// =============================================================================
// Managed Node.js runtime check
// =============================================================================

const NODE_RUNTIME_CHECK_ID: &str = "node-runtime";
const NODE_RUNTIME_CHECK_LABEL: &str = "Node.js Runtime";

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
async fn run_node_runtime_check() -> Option<DoctorCheck> {
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
    build_node_runtime_check(state, &install_dir, &npm_tools)
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

fn build_node_runtime_check(
    state: ManagedNodeRuntimeState,
    install_dir: &Path,
    npm_tools: &[String],
) -> Option<DoctorCheck> {
    let version = &crate::managed_node::node_runtime_lock().version;
    let (status, message) = match state {
        ManagedNodeRuntimeState::Ready => (
            CheckStatus::Pass,
            format!("Staged-managed Node.js {version} is installed"),
        ),
        ManagedNodeRuntimeState::Broken => (
            CheckStatus::Warn,
            format!("Staged-managed Node.js {version} is damaged; run the fix to reinstall it"),
        ),
        ManagedNodeRuntimeState::Missing if npm_tools.is_empty() => return None,
        ManagedNodeRuntimeState::Missing => (
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

fn node_runtime_doctor_check(
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
        installed_version: None,
        latest_version: None,
        update_available: None,
        install_source: None,
        self_updating: None,
        main: None,
        bridge: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned_version() -> String {
        crate::managed_node::node_runtime_lock().version.clone()
    }

    #[test]
    fn ready_runtime_passes_without_a_fix() {
        let check = build_node_runtime_check(
            ManagedNodeRuntimeState::Ready,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &["copilot".to_string()],
        )
        .expect("ready runtime is reported");

        assert_eq!(check.status, CheckStatus::Pass);
        assert_eq!(
            check.message,
            format!("Staged-managed Node.js {} is installed", pinned_version())
        );
        assert_eq!(
            check.path.as_deref(),
            Some("/data/packages/node/v9.9.9/plat/bin/node")
        );
        assert!(check.fix_type.is_none());
        assert!(check.fix_command.is_none());
        assert!(check.fix_url.is_none());
        let output = check.raw_output.as_deref().expect("raw output");
        assert!(output.contains("state: ready"));
        assert!(output.contains("- copilot"));
    }

    #[test]
    fn damaged_runtime_warns_with_a_native_reinstall_fix() {
        let check = build_node_runtime_check(
            ManagedNodeRuntimeState::Broken,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &[],
        )
        .expect("damaged runtime is reported");

        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.message.contains("is damaged"));
        assert!(check.path.is_none());
        assert_eq!(check.fix_type, Some(FixType::Command));
        let fix_command = check.fix_command.as_deref().expect("fix command");
        assert!(fix_command.contains(&pinned_version()));
        assert!(fix_command.contains("~/.staged/packages"));
        let output = check.raw_output.as_deref().expect("raw output");
        assert!(output.contains("state: broken"));
        assert!(output.contains("Staged-installed npm tools: none"));
    }

    #[test]
    fn missing_runtime_warns_only_when_installed_tools_need_it() {
        // Tools installed into the private prefix need the runtime: warn with
        // the reinstall fix.
        let check = build_node_runtime_check(
            ManagedNodeRuntimeState::Missing,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &["amp-acp".to_string(), "copilot".to_string()],
        )
        .expect("needed-but-missing runtime is reported");
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.message.contains("is not installed"));
        assert_eq!(check.fix_type, Some(FixType::Command));
        let output = check.raw_output.as_deref().expect("raw output");
        assert!(output.contains("- amp-acp"));
        assert!(output.contains("- copilot"));

        // Nothing installed that needs it: stay silent.
        assert!(build_node_runtime_check(
            ManagedNodeRuntimeState::Missing,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &[],
        )
        .is_none());
    }

    #[test]
    fn installed_npm_tool_names_lists_visible_entries_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("copilot"), "").unwrap();
        std::fs::write(dir.path().join(".copilot.tmp"), "").unwrap();

        let names = installed_npm_tool_names(dir.path());
        assert_eq!(names, vec!["copilot".to_string()]);

        // An absent dir reads as no tools, not an error.
        assert!(installed_npm_tool_names(&dir.path().join("absent")).is_empty());
    }

    /// Bundled-readout labeling lives in the doctor crate; Staged's job is
    /// only to hand the managed shim dir into the run options.
    #[test]
    fn run_checks_options_carries_bundled_tools_dir() {
        let dir = PathBuf::from("/data/packages/bin");
        let opts = run_checks_options(false, Vec::new(), Some(dir.clone()));
        assert_eq!(opts.bundled_tools_dir, Some(dir));

        let opts = run_checks_options(false, Vec::new(), None);
        assert!(opts.bundled_tools_dir.is_none());
    }

    /// Fixes and updates for the two bridge checks route to the managed
    /// installer exactly when this build manages bridges; every other check
    /// keeps the doctor crate's regular fix commands.
    #[test]
    fn managed_bridge_checks_route_to_the_managed_installer() {
        let managed = crate::managed_acp_tools::managed_tools_enabled();
        assert_eq!(
            managed_tool_for_check("ai-agent-claude"),
            managed.then_some("claude-acp")
        );
        assert_eq!(
            managed_tool_for_check("ai-agent-codex"),
            managed.then_some("codex-acp")
        );
        assert_eq!(managed_tool_for_check("ai-agent-copilot"), None);
        assert_eq!(managed_tool_for_check("ai-agent-amp"), None);
        assert_eq!(managed_tool_for_check(NODE_RUNTIME_CHECK_ID), None);
    }

    /// Checks and fixes must agree on the registry: both option builders take
    /// it from the same `managed_acp_tools::npm_registry()` gate.
    #[test]
    fn doctor_options_route_npm_through_the_managed_registry() {
        let expected = crate::managed_acp_tools::npm_registry().map(str::to_string);
        assert_eq!(
            run_checks_options(false, Vec::new(), None).npm_registry,
            expected
        );
        assert_eq!(execute_fix_options(None, Vec::new()).npm_registry, expected);
        if !cfg!(feature = "no-block-npm-registry") {
            assert!(expected.is_some());
        }
    }
}
