//! Tauri command wrappers for the doctor health-check system.

use std::path::PathBuf;

pub use doctor::types::{AuthStatus, InstallSource};
pub use doctor::{
    AgentVersionInfo, CheckStatus, DoctorCheck, DoctorEnv, DoctorReport, ExecuteFixOptions,
    FixCancelHandle, FixCancellation, FixStdin, FixStdinWriter, FixType, RunChecksOptions,
};

mod deprecated_bridge;
pub mod login;
mod managed_bridges;
mod runtime_checks;

#[cfg(test)]
#[path = "tests/support.rs"]
pub(crate) mod test_support;
#[cfg(test)]
#[path = "tests/commands.rs"]
mod tests;

use deprecated_bridge::{
    deprecated_claude_acp_check, run_deprecated_claude_acp_fix, DEPRECATED_CLAUDE_ACP_CHECK_ID,
};
pub use login::{
    cancel_doctor_login, doctor_login_status, send_doctor_login_code, start_doctor_login,
    DoctorLoginOutput, DoctorLoginStatus, LoginStart,
};
use login::{claim_login, run_login_fix, LoginClaim};
use managed_bridges::{
    any_managed_shim_missing, apply_managed_bridge_readouts, detect_npm_registry_label_if,
    managed_bridge_versions, managed_install_diagnostics, note_managed_installs_disabled,
};
use runtime_checks::{run_node_runtime_check, session_queue_doctor_check, NODE_RUNTIME_CHECK_ID};

/// Environment snapshot for doctor checks and fixes — and for the managed
/// bridge installs themselves (see
/// [`crate::managed_acp_tools::install_managed_tool`]), so the registry npm
/// resolves is the same for a check, its fix, and the reconciler. Shaped
/// through `apply_managed_tools_env` so checks resolve binaries from the same
/// PATH the agent spawn path uses — a bridge Staged manages must never be
/// reported missing (or prompt an install) just because the user has no
/// global copy. The managed npm env is overlaid on top, so checks probe npm
/// state (`npm prefix -g`, version lookups) with the same private-prefix view
/// the fixes install into — a check never contradicts the fix that just ran.
pub(crate) async fn doctor_env_vars() -> Vec<(String, String)> {
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

/// `agent_env` is each agent check's per-agent variables (see
/// [`agent_env_overlays`]), keyed by check id. They ride on the snapshot as
/// per-check overlays, so an agent's own probes see its rows over the shared
/// snapshot and no other check does.
fn run_checks_options(
    check_freshness: bool,
    env_vars: Vec<(String, String)>,
    agent_env: Vec<(String, Vec<(String, String)>)>,
    bundled_dir: Option<PathBuf>,
) -> RunChecksOptions {
    let env = agent_env
        .into_iter()
        .fold(DoctorEnv::new(env_vars), |env, (check_id, vars)| {
            env.with_check_overlay(check_id, vars)
        });
    RunChecksOptions {
        check_freshness,
        offline: false,
        // npm-backed checks and fixes pass a `--registry` only when the user
        // set `STAGED_NPM_REGISTRY`; otherwise npm resolves its own config
        // (see the registry policy note in `managed_acp_tools`).
        npm_registry: crate::managed_acp_tools::npm_registry_override(),
        env: Some(env),
        // Doctor labels binaries resolved from this dir as bundled (install
        // source + readout flag) and suppresses registry update fixes for
        // them — the startup reconciler floats the managed shims to @latest,
        // so Staged owns their updates.
        bundled_tools_dir: bundled_dir,
    }
}

/// The agent (provider) id behind an `ai-agent-<id>` doctor check — the key
/// its environment variables are stored under in preferences — or `None` for
/// any other check. Mirrors `check.id.replace('ai-agent-', '')` in
/// `DoctorSettingsPanel.svelte`, which mounts the variables editor under the
/// row by it.
pub(super) fn provider_id_for_check(check_id: &str) -> Option<&str> {
    check_id.strip_prefix("ai-agent-")
}

/// The enabled environment variables configured for the agent an
/// `ai-agent-*` check fronts (Settings → Doctor → Environment variables);
/// empty for every other check. Doctor's probes of that agent and its login
/// fix run with these over the shared snapshot — as a `DoctorEnv` overlay,
/// which the login shell re-exports after its startup files — as a session
/// launch of the agent does (see `agent::env_vars`), so the row can't report
/// "not authenticated" against a key the session right next to it launches
/// with.
pub(super) fn agent_env_for_check(check_id: &str) -> Vec<(String, String)> {
    provider_id_for_check(check_id)
        .map(crate::agent::env_vars::read_agent_env)
        .unwrap_or_default()
}

/// Every agent check's variables for [`run_checks_options`], keyed by check
/// id; agents with no enabled variables are left out. `preferences.json` is
/// read and parsed once and every overlay derived from that value, so the
/// overlays describe one consistent snapshot of the file rather than one
/// read per agent that a UI write could land between.
fn agent_env_overlays() -> Vec<(String, Vec<(String, String)>)> {
    crate::agent::env_vars::read_preferences()
        .map(|preferences| agent_env_overlays_from(&preferences))
        .unwrap_or_default()
}

/// [`agent_env_overlays`] for an already parsed `preferences.json`.
fn agent_env_overlays_from(
    preferences: &serde_json::Value,
) -> Vec<(String, Vec<(String, String)>)> {
    doctor::agents::AI_AGENT_CHECKS
        .iter()
        .filter_map(|info| {
            let provider_id = provider_id_for_check(info.id)?;
            let vars = crate::agent::env_vars::agent_env_from_preferences(preferences, provider_id);
            (!vars.is_empty()).then(|| (info.id.to_string(), vars))
        })
        .collect()
}

fn execute_fix_options(
    command_override: Option<String>,
    env_vars: Vec<(String, String)>,
) -> ExecuteFixOptions {
    execute_fix_options_through(
        command_override,
        crate::managed_acp_tools::npm_registry_override(),
        env_vars,
    )
}

/// [`execute_fix_options`] with the registry chosen by the caller — the
/// gateway-blocked retry passes the compiled-in fallback here.
fn execute_fix_options_through(
    command_override: Option<String>,
    npm_registry: Option<String>,
    env_vars: Vec<(String, String)>,
) -> ExecuteFixOptions {
    // Everything else stays at doctor's defaults: the fixes that reach this
    // builder are the non-interactive ones (installs and updates), so nothing
    // here feeds a prompt and the child keeps inheriting stdin rather than
    // getting a piped one; the standard fix timeout is far above any install
    // this runs. Interactive logins do *not* come through here — every
    // `FixType::Auth` run goes through [`run_login_fix`], which pipes stdin so
    // the code the CLI asks for can actually be delivered. Spelled with
    // `..Default::default()` so a new doctor option doesn't break this
    // workspace-excluded crate, which `cargo check` under `crates/` never
    // compiles but `staged-ci.yml` does.
    ExecuteFixOptions {
        command_override,
        npm_registry,
        ..Default::default()
    }
    .with_env_snapshot(env_vars)
}

/// Run a doctor fix through the crate's executor with the registry precedence
/// the managed installer applies: npm's own config (or the explicit
/// `STAGED_NPM_REGISTRY`) first, and — only for an npm-backed command that
/// failed as a gateway block of `registry.npmjs.org`, with no override set
/// and a fallback compiled in — one retry with `--registry <fallback>`. The
/// executor surfaces the failed command's stderr as its error string, which
/// is exactly what the classifier reads; when the retry fails too, the
/// surfaced error keeps the original diagnosis in front of the retry's.
async fn execute_fix_with_gateway_fallback(
    check_id: String,
    fix_type: FixType,
    command_override: Option<String>,
    npm_backed: bool,
    env_vars: Vec<(String, String)>,
) -> Result<(), String> {
    let first = doctor::execute_fix_with_env_options(
        check_id.clone(),
        fix_type.clone(),
        execute_fix_options(command_override.clone(), env_vars.clone()),
    )
    .await;
    let Err(error) = first else {
        return Ok(());
    };
    if !npm_backed {
        return Err(error);
    }
    let Some(fallback) = crate::managed_acp_tools::gateway_fallback_registry(
        crate::managed_acp_tools::npm_registry_override().as_deref(),
        crate::managed_acp_tools::fallback_npm_registry(),
        &error,
    ) else {
        return Err(error);
    };
    log::info!(
        "[doctor fix {check_id}] npm could not reach registry.npmjs.org (network gateway block page); retrying through {fallback}"
    );
    doctor::execute_fix_with_env_options(
        check_id,
        fix_type,
        execute_fix_options_through(command_override, Some(fallback.to_string()), env_vars),
    )
    .await
    .map_err(|retry_error| {
        crate::managed_acp_tools::gateway_retry_failed_message(fallback, &retry_error)
    })
}

/// Run all health checks and return the report.
///
/// This is the cheap, no-network path: it resolves binaries and reports
/// install/auth status but does not probe registries for version freshness.
/// The frontend calls this first for an instant paint, then follows up with
/// [`run_doctor_freshness`] to fill in version/update information.
///
/// Takes the app handle rather than `State` borrows so the command keeps its
/// plain `DoctorReport` return: the session-queue check reads store and
/// registry state synchronously before the first await.
#[tauri::command]
pub async fn run_doctor(app_handle: tauri::AppHandle) -> DoctorReport {
    let queue = session_queue_doctor_check(&app_handle);
    run_doctor_report(false, queue).await
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
    let queue = session_queue_doctor_check(&app_handle);
    run_doctor_report(true, queue).await
}

/// Run the doctor crate's checks plus Staged-local ones over one shared env
/// snapshot: the managed Node.js runtime check, the session-queue check the
/// caller already took (`queue_check`; both passes carry it, since the
/// frontend merges the freshness report into the first by check id), and the
/// managed-bridge version readouts layered onto the Claude/Codex rows (see
/// [`apply_managed_bridge_readouts`]). Bundled readouts are labeled by the
/// doctor crate itself via `RunChecksOptions::bundled_tools_dir`.
///
/// The managed versions are gathered concurrently with the crate's run: the
/// installed version is a local `package.json` read on both passes, and the
/// registry lookup for the latest version runs only when `check_freshness`
/// is set, so the first paint never waits on the network. The npm registry
/// probe for a missing-managed-copy row (see [`managed_install_diagnostics`])
/// joins them too, decided from the shim dir up front, so it never adds to
/// the report's wall-clock after the crate's checks are in.
async fn run_doctor_report(
    check_freshness: bool,
    queue_check: Option<DoctorCheck>,
) -> DoctorReport {
    let env_vars = doctor_env_vars().await;
    let shim_bin_dir = crate::managed_acp_tools::managed_shim_bin_dir();
    let probe_registry = shim_bin_dir
        .as_deref()
        .is_some_and(any_managed_shim_missing);
    let (mut report, node_runtime, managed_versions, npm_registry) = tokio::join!(
        doctor::run_checks_with_options(run_checks_options(
            check_freshness,
            env_vars.clone(),
            agent_env_overlays(),
            crate::acp_tools::primary_tools_dir(),
        )),
        run_node_runtime_check(),
        managed_bridge_versions(check_freshness, &env_vars),
        detect_npm_registry_label_if(probe_registry, &env_vars),
    );
    match shim_bin_dir {
        Some(shim_bin_dir) => {
            let diagnostics = managed_install_diagnostics(npm_registry);
            apply_managed_bridge_readouts(
                &mut report,
                &managed_versions,
                &shim_bin_dir,
                check_freshness,
                &diagnostics,
            );
        }
        None => {
            if let Some(reason) = crate::managed_acp_tools::managed_tools_disabled_reason() {
                note_managed_installs_disabled(&mut report, &reason);
            }
        }
    }
    if let Some(check) = node_runtime {
        report.checks.push(check);
    }
    if let Some(check) = deprecated_claude_acp_check(&report, &env_vars).await {
        report.checks.push(check);
    }
    report.checks.extend(queue_check);
    report
}

/// Reserve the login slot for `check_id` under a fresh run id, returning what
/// the run needs — or the id of the login already running for the check. The
/// entry parked in [`ACTIVE_LOGINS`] is also the "a login is running" flag:
/// doctor's `FixStdin` is single-use, so a second concurrent login for one
/// check is refused here rather than spawning a CLI nothing can type into.

#[tauri::command]
pub async fn run_doctor_fix(
    app_handle: tauri::AppHandle,
    check_id: String,
    fix_type: FixType,
) -> Result<(), String> {
    if check_id == NODE_RUNTIME_CHECK_ID {
        return ensure_managed_node_runtime_for_fix().await;
    }
    // An auth fix is the one interactive fix: it prints a verification URL and
    // then blocks reading a code from stdin. Route it through the same piped,
    // streamed path `start_doctor_login` uses so both entry points behave the
    // same. On inherited stdin — `/dev/null` in the GUI — this printed its URL
    // to a log nobody reads and could only ever finish through the CLI's own
    // browser callback, otherwise dying at the fix timeout with no way to enter
    // the code (block/berd#99).
    if matches!(fix_type, FixType::Auth) {
        // This entry point awaits the fix to its end, so there is no run to
        // hand an "already running" answer to — the caller asked for a fix and
        // there is one it can't have.
        let login = match claim_login(&check_id)? {
            LoginClaim::Claimed(login) => login,
            LoginClaim::AlreadyRunning { .. } => {
                return Err(format!("A login is already running for {check_id}"));
            }
        };
        return run_login_fix(app_handle, check_id, login).await;
    }
    if matches!(fix_type, FixType::Command | FixType::Bridge) {
        if let Some(tool_id) = managed_tool_for_check(&check_id) {
            return install_managed_tool_logged(tool_id, &check_id).await;
        }
    }
    let env_vars = doctor_env_vars().await;
    // The deprecated-bridge fix carries its own command rather than a static
    // check definition, so the npm-backed gate below can't see it — ensure the
    // managed runtime here, since the fix shells out to npm.
    if check_id == DEPRECATED_CLAUDE_ACP_CHECK_ID {
        ensure_managed_node_runtime_for_fix().await?;
        return run_deprecated_claude_acp_fix(fix_type, env_vars).await;
    }
    let npm_backed = doctor::agents::lookup_fix_command(&check_id, &fix_type)
        .as_deref()
        .is_some_and(crate::managed_acp_tools::is_npm_backed_command);
    if npm_backed {
        ensure_managed_node_runtime_for_fix().await?;
    }
    execute_fix_with_gateway_fallback(check_id, fix_type, None, npm_backed, env_vars).await
}

/// The install id of the ACP bridge a doctor check fronts, for the two checks
/// whose bridge Staged can manage — whether or not this build does.
fn managed_tool_id_for_check(check_id: &str) -> Option<&'static str> {
    match check_id {
        "ai-agent-claude" => Some("claude-acp"),
        "ai-agent-codex" => Some("codex-acp"),
        _ => None,
    }
}

/// The managed ACP bridge behind a doctor check id, when this build manages
/// it. `None` routes the check to the doctor crate's regular fix commands —
/// which is also the correct fallback whenever bridge management is off (dev
/// override, `no-managed-acp-tools`, unsupported target).
fn managed_tool_for_check(check_id: &str) -> Option<&'static str> {
    crate::managed_acp_tools::managed_tool(managed_tool_id_for_check(check_id)?).map(|tool| tool.id)
}

/// The managed bridge a doctor *update* targets: on a build that manages these
/// two packages, the managed installer is the only writer for them, so *every*
/// update of the Claude and Codex checks goes to it — `UpdateBridge` from a
/// managed row's ACP readout, and `UpdateMain` from a copy that resolved
/// elsewhere (a user install found on PATH before the first reconcile lands,
/// or while it keeps failing). Letting that `UpdateMain` take the
/// command-validated path instead would run `npm install -g <pkg>@latest`
/// under the fix env's `NPM_CONFIG_PREFIX`, landing a third, unowned copy in
/// `~/.staged/packages/npm-prefix` that nothing prunes, updates or runs — and
/// leaving the user's copy stale anyway.
///
/// There is still exactly one install per row, because the two update fix
/// types never both carry an action on the same check:
/// [`apply_managed_bridge_readout`] gives the managed row its `bridge` action
/// while `main` stays bundled and update-free, and an unmanaged row has only
/// the `main` action that
/// [`redirect_unmanaged_update_to_managed_install`] rewrites — the crate emits
/// no `bridge` readout for the single-binary Claude and Codex checks.
///
/// Non-update fix types, and every check when management is off (dev override,
/// `no-managed-acp-tools`, unsupported target), fall through to the
/// command-validated path.
fn managed_update_target(check_id: &str, fix_type: &FixType) -> Option<&'static str> {
    if !matches!(fix_type, FixType::UpdateMain | FixType::UpdateBridge) {
        return None;
    }
    managed_tool_for_check(check_id)
}

/// Install (or float-upgrade) a managed bridge for a doctor fix/update.
/// Progress goes to the log — doctor fixes have no streamed-output channel,
/// only a button spinner.
async fn install_managed_tool_logged(tool_id: &str, check_id: &str) -> Result<(), String> {
    let log_prefix = format!("[doctor fix {check_id}]");
    let env_vars = doctor_env_vars().await;
    crate::managed_acp_tools::install_managed_tool(tool_id, &env_vars, &|line| {
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
    // On a build that manages these two bridges, every update of their checks
    // is the floating installer itself: `<pkg>@latest` staged, verified and
    // swapped into `~/.staged/packages` on the managed runtime. That covers
    // both the managed row's ACP readout (see `apply_managed_bridge_readout`)
    // and a copy that resolved elsewhere, whose source-aware `UpdateMain` is
    // rewritten into a managed-install description by
    // `redirect_unmanaged_update_to_managed_install` — its own `npm install -g`
    // would write into the private prefix and orphan a copy there. No shell
    // command runs, so the frontend-supplied `command` — the description the
    // confirmation showed — needs no validation; what is validated is the
    // target: a check whose bridge this build manages, and an update fix type.
    if let Some(tool_id) = managed_update_target(&check_id, &fix_type) {
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
    let npm_backed = crate::managed_acp_tools::is_npm_backed_command(&expected);
    if npm_backed {
        ensure_managed_node_runtime_for_fix().await?;
    }
    // Run the backend-derived `expected`, not the frontend-supplied `command`.
    // They are equal past the guard above, but executing `expected` makes the
    // command that runs provably the one the backend derived — no dependence on
    // the equality check surviving future edits.
    execute_fix_with_gateway_fallback(check_id, fix_type, Some(expected), npm_backed, env_vars)
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
    // Same overlays as the report the UI rendered, so the version probe that
    // derives the command runs the agent the way the report's did.
    let report = doctor::run_checks_with_options(run_checks_options(
        true,
        env_vars,
        agent_env_overlays(),
        bundled_dir,
    ))
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
