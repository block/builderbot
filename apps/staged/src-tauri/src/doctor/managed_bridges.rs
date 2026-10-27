//! Managed bridge readouts — what the Claude/Codex rows say about the copies
//! Staged installs: versions, update/install fixes, and why a managed copy is
//! missing.

use std::path::Path;

use crate::managed_acp_tools::{ManagedTool, ReconcileRecord};

use super::{
    managed_tool_id_for_check, AgentVersionInfo, CheckStatus, DoctorCheck, DoctorReport, FixType,
    InstallSource,
};

#[cfg(test)]
#[path = "tests/managed_bridges.rs"]
mod tests;

// =============================================================================
// Managed bridge readouts — the versions of the ACP bridges Staged installs
// =============================================================================

/// What Staged knows about one managed bridge's versions at report time.
///
/// Gathered outside the doctor crate: the crate labels the managed shims
/// bundled and, on that policy, probes only the vendored agent CLI's version
/// (`main`) and offers no update. Staged updates these packages itself at
/// launch and daily, so its rows additionally show — and can update — the ACP
/// package, as a second readout on the same executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ManagedBridgeVersions {
    tool: ManagedTool,
    /// The ACP package version installed in `~/.staged/packages`, from the
    /// live package's `package.json` (`state.json` as fallback).
    installed: Option<String>,
    /// The newest published version of the *same* package. `None` outside the
    /// freshness pass and whenever the lookup failed — unknown, not current.
    latest: Option<String>,
}

/// The versions of every bridge this build manages, gathered concurrently.
/// Empty when nothing is managed (dev override, `no-managed-acp-tools`,
/// unsupported target), which is also what keeps those builds from ever
/// growing a managed update action. The registry is asked only when
/// `check_freshness` is set.
pub(super) async fn managed_bridge_versions(
    check_freshness: bool,
    env_vars: &[(String, String)],
) -> Vec<ManagedBridgeVersions> {
    let tools = crate::managed_acp_tools::managed_tools();
    if tools.is_empty() {
        return Vec::new();
    }
    let Some(packages_root) = crate::paths::packages_dir() else {
        return Vec::new();
    };
    let handles: Vec<_> = tools
        .into_iter()
        .map(|tool| {
            let env_vars = env_vars.to_vec();
            let packages_root = packages_root.clone();
            tokio::spawn(async move {
                let installed =
                    crate::managed_acp_tools::installed_tool_version(&packages_root, &tool);
                let latest = if check_freshness {
                    crate::managed_acp_tools::latest_published_version(&tool, &env_vars).await
                } else {
                    None
                };
                ManagedBridgeVersions {
                    tool,
                    installed,
                    latest,
                }
            })
        })
        .collect();
    let mut versions = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(version) = handle.await {
            versions.push(version);
        }
    }
    versions
}

/// What Doctor says about the managed installer on a Claude/Codex row whose
/// executable is *not* the managed shim: the last reconcile's outcome from
/// `state.json` — the only record a failed launch install leaves behind — and
/// the npm registry a managed install resolves on this machine. Gathered once
/// per report; nothing here is persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ManagedInstallDiagnostics {
    /// `state.json`'s `lastReconcile`, `None` before the first reconcile has
    /// finished (or with no `state.json` at all).
    last_reconcile: Option<ReconcileRecord>,
    /// The registry label for the row's raw output: the `STAGED_NPM_REGISTRY`
    /// override, what `npm config get registry` answered under the doctor
    /// env snapshot, or "npm default" when neither could be asked. `None`
    /// when the probe was skipped because every managed shim was present
    /// when the report started (see [`any_managed_shim_missing`]).
    npm_registry: Option<String>,
    /// Wall-clock milliseconds the reconcile age is rendered against.
    now_ms: u64,
}

impl ManagedInstallDiagnostics {
    /// The last reconcile's error for `tool_id`, without the `<id>: ` prefix
    /// the reconciler records it under.
    fn reconcile_error_for(&self, tool_id: &str) -> Option<&str> {
        reconcile_error_for(self.last_reconcile.as_ref()?, tool_id)
    }

    /// One raw-output line on the last managed install run.
    fn last_reconcile_line(&self) -> String {
        match &self.last_reconcile {
            None => "last managed install: not run yet (runs at launch and daily)".to_string(),
            Some(record) => format!(
                "last managed install: {} {}",
                if record.ok { "succeeded" } else { "failed" },
                format_age(record.at_ms, self.now_ms)
            ),
        }
    }

    /// One raw-output line naming the npm registry Doctor detected.
    fn npm_registry_line(&self) -> String {
        format!(
            "npm registry: {}",
            self.npm_registry.as_deref().unwrap_or("not probed")
        )
    }
}

/// The reconciler records each failure as `<tool id>: <error>`; the error on
/// its own, for the tool asked about.
fn reconcile_error_for<'a>(record: &'a ReconcileRecord, tool_id: &str) -> Option<&'a str> {
    record
        .errors
        .iter()
        .find_map(|error| error.strip_prefix(tool_id)?.strip_prefix(": "))
}

/// `just now`, `N minutes ago`, `N hours ago`, `N days ago` — enough to tell a
/// stale record from the launch reconcile that just ran.
pub(super) fn format_age(at_ms: u64, now_ms: u64) -> String {
    let seconds = now_ms.saturating_sub(at_ms) / 1000;
    let ago = |count: u64, unit: &str| {
        let plural = if count == 1 { "" } else { "s" };
        format!("{count} {unit}{plural} ago")
    };
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => ago(seconds / 60, "minute"),
        3600..=86_399 => ago(seconds / 3600, "hour"),
        _ => ago(seconds / 86_400, "day"),
    }
}

/// Gather [`ManagedInstallDiagnostics`] for a report from `state.json` and
/// the registry label the report's probes produced (see
/// [`detect_npm_registry_label_if`]).
pub(super) fn managed_install_diagnostics(
    npm_registry: Option<String>,
) -> ManagedInstallDiagnostics {
    ManagedInstallDiagnostics {
        last_reconcile: last_reconcile_record(),
        npm_registry,
        now_ms: crate::managed_acp_tools::now_ms(),
    }
}

/// Whether any managed bridge's shim is absent from `shim_bin_dir`. The shim
/// dir is first on the probe PATH, so this is the only way a Claude/Codex row
/// can resolve to a copy Staged does not manage — and so the only time the
/// npm registry probe, which spawns `npm`, has a row to show its answer on.
/// Decided before the probes run so that probe can join them rather than
/// wait on the report.
pub(super) fn any_managed_shim_missing(shim_bin_dir: &Path) -> bool {
    crate::managed_acp_tools::managed_tools()
        .iter()
        .any(|tool| !shim_bin_dir.join(tool.binary).is_file())
}

/// [`detect_npm_registry_label`] when `probe` is set; `None` otherwise.
pub(super) async fn detect_npm_registry_label_if(
    probe: bool,
    env_vars: &[(String, String)],
) -> Option<String> {
    if probe {
        Some(detect_npm_registry_label(env_vars).await)
    } else {
        None
    }
}

/// `state.json`'s last reconcile record, if any.
pub(super) fn last_reconcile_record() -> Option<ReconcileRecord> {
    let packages_root = crate::paths::packages_dir()?;
    crate::managed_acp_tools::read_state(&packages_root).last_reconcile
}

/// The npm registry a managed install resolves, labeled with where that
/// answer came from — so a row can say which mirror (or the public registry)
/// the failed install actually asked. The underlying `npm config get
/// registry` is bounded to 5 s and cached per env snapshot for a minute, so
/// a Doctor refresh does not spawn npm again.
async fn detect_npm_registry_label(env_vars: &[(String, String)]) -> String {
    if let Some(registry) = crate::managed_acp_tools::npm_registry_override() {
        return format!("{registry} (STAGED_NPM_REGISTRY)");
    }
    match crate::managed_acp_tools::configured_npm_registry(env_vars).await {
        Some(registry) => format!("{registry} (per npm config)"),
        None => "npm default (per npm config; no npm on PATH to ask)".to_string(),
    }
}

/// Layer the managed ACP readout onto every check whose executable is one of
/// the managed shims in `shim_bin_dir` (see [`apply_managed_bridge_readout`]).
/// Every other Claude/Codex row is a copy Staged does not manage: it is
/// flagged as such, with why the managed copy is missing and the managed
/// install as its fix (see [`flag_missing_managed_copy`]), and its update is
/// pointed at the managed installer too (see
/// [`redirect_unmanaged_update_to_managed_install`]) — on a managing build it
/// is the only writer for these two packages.
pub(super) fn apply_managed_bridge_readouts(
    report: &mut DoctorReport,
    versions: &[ManagedBridgeVersions],
    shim_bin_dir: &Path,
    check_freshness: bool,
    diagnostics: &ManagedInstallDiagnostics,
) {
    for check in &mut report.checks {
        let Some(tool_id) = managed_tool_id_for_check(&check.id) else {
            continue;
        };
        if let Some(versions) = versions.iter().find(|v| v.tool.id == tool_id) {
            if !apply_managed_bridge_readout(check, versions, shim_bin_dir, check_freshness) {
                flag_missing_managed_copy(check, versions, shim_bin_dir, diagnostics);
                redirect_unmanaged_update_to_managed_install(check, versions, shim_bin_dir);
            }
        }
    }
}

/// Note on the Claude/Codex rows that this build does not manage bridge
/// installs, and why — the `STAGED_ACP_TOOLS_DIR` dev override, the
/// `no-managed-acp-tools` feature, an unsupported target. A copy resolved from
/// Homebrew (or the override dir) is then by design, and the row must not read
/// like a failed install. Status is kept; only the message and raw output
/// gain the note.
pub(super) fn note_managed_installs_disabled(report: &mut DoctorReport, reason: &str) {
    for check in &mut report.checks {
        if managed_tool_id_for_check(&check.id).is_none() {
            continue;
        }
        let note = format!("managed installs disabled: {reason}");
        check.message = format!("{} ({note})", check.message);
        append_raw_output(check, &note);
    }
}

/// Add a block to a check's raw output, below whatever the crate recorded.
fn append_raw_output(check: &mut DoctorCheck, block: &str) {
    check.raw_output = Some(match check.raw_output.take() {
        Some(existing) if !existing.trim().is_empty() => {
            format!("{}\n\n{block}", existing.trim_end())
        }
        _ => block.to_string(),
    });
}

/// Flag a Claude/Codex row whose executable is a copy Staged does not manage
/// — Homebrew, the user's npm prefix, nvm — and report whether it did.
///
/// On a managing build the managed shim dir is first on the probe PATH, so a
/// copy from anywhere else can only win while the managed copy is *absent*:
/// the launch reconcile has not finished yet, or it failed (no network to the
/// registry, a gateway block page, a 404 from a mirror, a validation failure).
/// Left as the crate built it, the row is a green "Installed" pointing at
/// `/opt/homebrew/bin`, indistinguishable from a healthy setup and with no
/// way to get the managed copy. So:
///
/// - `status` drops to `Warn` (a row already warning or failing keeps its
///   status), and `message` says which copy is in use and, before any
///   reconcile has finished, that the managed install "hasn't completed yet"
///   — a healthy first launch, not a failure; after one, that the managed
///   copy is missing, with — when the last reconcile recorded an error for
///   this tool — the short of that error (see [`short_error_fragment`]).
/// - `raw_output` gains the expected shim path, the last reconcile's outcome
///   and age, the full recorded error with a classified hint on what to
///   change (`install_failure_hint`), and the npm registry Doctor detected.
/// - The row's fix becomes the managed install (`Bridge`, which
///   `run_doctor_fix` routes to `install_managed_tool_logged` for these two
///   checks), described by [`managed_install_description`] — the same text
///   [`redirect_unmanaged_update_to_managed_install`] gives the row's update
///   when a newer release is known, so the two buttons never disagree. The
///   fix is offered whether or not an update is available: the point is to
///   get the managed copy, not to chase a version.
///
/// `path`, `main` and the version fields stay the crate's: the row still shows
/// the copy that is actually on PATH and how old it is.
fn flag_missing_managed_copy(
    check: &mut DoctorCheck,
    versions: &ManagedBridgeVersions,
    shim_bin_dir: &Path,
    diagnostics: &ManagedInstallDiagnostics,
) -> bool {
    let Some(path) = check.path.clone() else {
        return false;
    };
    let tool = &versions.tool;
    if is_managed_shim_path(&path, shim_bin_dir, tool.binary) {
        return false;
    }
    let reconcile_error = diagnostics.reconcile_error_for(tool.id);
    let situation = match (&diagnostics.last_reconcile, reconcile_error) {
        (None, _) => "Staged's managed install hasn't completed yet".to_string(),
        (Some(_), None) => "Staged's managed copy is missing".to_string(),
        (Some(_), Some(error)) => format!(
            "Staged's managed copy is missing: {}",
            short_error_fragment(error)
        ),
    };
    if check.status == CheckStatus::Pass {
        check.status = CheckStatus::Warn;
        check.message = format!("Using {path} — {situation}");
    } else {
        check.message = format!("{}; using {path} — {situation}", check.message);
    }

    let mut detail = vec![
        format!(
            "Staged-managed copy: missing (expected {})",
            shim_bin_dir.join(tool.binary).display()
        ),
        diagnostics.last_reconcile_line(),
    ];
    if let Some(error) = reconcile_error {
        detail.push(format!("{}: {error}", tool.id));
        if let Some(hint) = crate::managed_acp_tools::install_failure_hint(error, tool.package) {
            detail.push(format!("hint: {hint}"));
        }
    }
    detail.push(diagnostics.npm_registry_line());
    append_raw_output(check, &detail.join("\n"));

    let latest = versions.latest.as_deref().or_else(|| {
        check
            .main
            .as_ref()
            .and_then(|main| main.latest_version.as_deref())
    });
    check.fix_type = Some(FixType::Bridge);
    check.fix_command = Some(managed_install_description(tool, latest, &path));
    true
}

/// Most a recorded install error contributes to a row's one-line message:
/// npm's own `npm error code <CODE>` when the error carries one (what
/// `npm_error_summary` leads with), else the error's first
/// [`SHORT_ERROR_CHARS`] characters, ellipsized. The full text, its hint and
/// the registry stay in the raw output for **Copy details**.
fn short_error_fragment(error: &str) -> String {
    const NPM_CODE: &str = "npm error code ";
    if let Some((_, rest)) = error.split_once(NPM_CODE) {
        let code = rest
            .split(|c: char| c == ';' || c.is_whitespace())
            .next()
            .unwrap_or_default();
        if !code.is_empty() {
            return format!("{NPM_CODE}{code}");
        }
    }
    match error.char_indices().nth(SHORT_ERROR_CHARS) {
        Some((cut, _)) => format!("{}…", error[..cut].trim_end()),
        None => error.to_string(),
    }
}

/// How much of a recorded error without an npm code fits on the row.
const SHORT_ERROR_CHARS: usize = 120;

/// Give a managed Claude/Codex row the two readouts its one executable
/// stands for, and report whether it did.
///
/// - `main` is left as the doctor crate built it: the vendored agent CLI's
///   version, probed through the bridge passthrough, with the crate's bundled
///   suppression (no update) intact — the agent is updated as part of the
///   package, never on its own.
/// - `bridge` is added: the ACP package's installed version, and on the
///   freshness pass the latest published version of that same package,
///   `update_available` from comparing the two, and — only when newer — the
///   one actionable update, typed `UpdateBridge`, which `run_doctor_update`
///   routes to the managed installer. The command text describes updating
///   Staged's managed copy; nothing runs it as a shell command.
/// - `bridge_path` is set to the same executable so the row's path gate
///   renders the readout, on both the initial and the freshness report — the
///   frontend's freshness merge copies `bridge` but not `bridge_path`, so a
///   readout that appeared only on the second pass would stay hidden.
/// - The flat version fields mirror the ACP readout, as the crate does for
///   any check with a bridge readout.
///
/// Nothing is touched unless the check's executable is the managed shim for
/// this tool and the crate labeled it bundled: a copy that resolved elsewhere
/// keeps its source-aware readout, is flagged as missing the managed copy
/// (see [`flag_missing_managed_copy`]), and has its update redirected to the
/// managed install (see [`redirect_unmanaged_update_to_managed_install`]). An
/// unknown installed or latest version leaves `update_available` unknown
/// rather than false.
fn apply_managed_bridge_readout(
    check: &mut DoctorCheck,
    versions: &ManagedBridgeVersions,
    shim_bin_dir: &Path,
    check_freshness: bool,
) -> bool {
    let Some(path) = check.path.clone() else {
        return false;
    };
    if !is_managed_shim_path(&path, shim_bin_dir, versions.tool.binary) {
        return false;
    }
    if check.main.as_ref().and_then(|main| main.bundled) != Some(true) {
        return false;
    }

    let update_available = if check_freshness {
        newer_version_available(versions.installed.as_deref(), versions.latest.as_deref())
    } else {
        None
    };
    let update_command = match (&update_available, &versions.latest) {
        (Some(true), Some(latest)) => Some(managed_update_description(&versions.tool, latest)),
        _ => None,
    };
    let bridge = AgentVersionInfo {
        install_source: Some(InstallSource::Bundled),
        bundled: Some(true),
        installed_version: versions.installed.clone(),
        latest_version: versions.latest.clone(),
        update_available,
        // Only the freshness pass says anything about updates, matching the
        // crate; and what it says is "not self-updating": the update is the
        // user's to action here, unlike the crate's bundled default.
        self_updating: check_freshness.then_some(false),
        update_fix_type: update_command.as_ref().map(|_| FixType::UpdateBridge),
        update_command,
    };
    check.installed_version = bridge.installed_version.clone();
    check.latest_version = bridge.latest_version.clone();
    check.update_available = bridge.update_available;
    check.self_updating = bridge.self_updating;
    check.bridge_path = Some(path);
    check.bridge = Some(bridge);
    true
}

/// Point a non-managed copy's update at the managed installer, and report
/// whether it did.
///
/// Doctor can resolve one of these two checks to a copy of the bridge the user
/// installed themselves (Homebrew, their own npm prefix, nvm) — before the
/// launch reconcile has written the managed shim, or for as long as it keeps
/// failing. On a managing build the crate's source-aware `UpdateMain` must not
/// run: `npm install -g <pkg>@latest` under the fix env's `NPM_CONFIG_PREFIX`
/// would install a third copy into `~/.staged/packages/npm-prefix` that
/// nothing owns, and would not upgrade the user's copy the confirmation named.
///
/// So the action becomes the managed install, described as such, and only the
/// action: `path`, `install_source`, both versions and `update_available` stay
/// as the crate reported them, so the row keeps showing the copy that is
/// actually on PATH, where it is, and how old it is. The fix type is set even
/// when the crate derived no command (an `Unknown` source, say) — Staged can
/// install its own copy where the crate had nothing to offer.
///
/// The version the description names is the one the installer will fetch:
/// `versions.latest`, Staged's own `npm view <pkg> version` against the
/// configured registry, gathered alongside the managed readout. The crate's
/// `latest_version` is derived per install source — `brew info` for a Homebrew
/// copy — and can name a release the configured registry does not (yet) carry,
/// or lag one it does; it is only the fallback for when the managed probe
/// failed, so the text at least names the release the row is flagged stale
/// against.
///
/// Only ever fires on the freshness pass, since `update_available` is unset
/// until then, and never on a management-off build, where
/// [`managed_bridge_versions`] returns nothing.
fn redirect_unmanaged_update_to_managed_install(
    check: &mut DoctorCheck,
    versions: &ManagedBridgeVersions,
    shim_bin_dir: &Path,
) -> bool {
    let Some(path) = check.path.clone() else {
        return false;
    };
    if is_managed_shim_path(&path, shim_bin_dir, versions.tool.binary) {
        return false;
    }
    let Some(main) = check.main.as_mut() else {
        return false;
    };
    if main.update_available != Some(true) {
        return false;
    }
    let latest = versions
        .latest
        .as_deref()
        .or(main.latest_version.as_deref());
    main.update_command = Some(managed_install_description(&versions.tool, latest, &path));
    main.update_fix_type = Some(FixType::UpdateMain);
    true
}

/// Whether `path` is the shim Staged writes for `binary` in `shim_bin_dir` —
/// the executable of a managed install, as opposed to a same-named copy that
/// resolved from anywhere else on PATH.
fn is_managed_shim_path(path: &str, shim_bin_dir: &Path, binary: &str) -> bool {
    let path = Path::new(path);
    path.starts_with(shim_bin_dir) && path.file_name().is_some_and(|name| name == binary)
}

/// What the update confirmation shows for a managed bridge. Not a shell
/// command: `run_doctor_update` dispatches this readout to the managed
/// installer, which installs `<pkg>@latest` into `~/.staged/packages` — so
/// the text says that, rather than offering a global `npm install -g` the
/// user might run by hand and end up with a second copy.
fn managed_update_description(tool: &ManagedTool, latest: &str) -> String {
    format!(
        "update Staged's managed {} to {latest} in ~/.staged/packages",
        tool.package
    )
}

/// What the update confirmation shows when the row is a copy Staged does not
/// manage: the managed copy is absent or stale, so the installer *installs*
/// rather than updates, and the text is explicit that `replaced` — the copy on
/// PATH, the one the version beside it was read from — is not touched.
fn managed_install_description(tool: &ManagedTool, latest: Option<&str>, replaced: &str) -> String {
    let version = latest
        .map(|latest| format!(" {latest}"))
        .unwrap_or_default();
    format!(
        "install Staged's managed {}{version} in ~/.staged/packages \
         (the copy at {replaced} is left as is)",
        tool.package
    )
}

/// `Some(true)` when `latest` is a newer release than `installed`, comparing
/// the two versions of one package as semver triples. `None` — unknown, not
/// "current" — when either side is missing or does not parse.
fn newer_version_available(installed: Option<&str>, latest: Option<&str>) -> Option<bool> {
    let installed = parse_version_triple(installed?)?;
    let latest = parse_version_triple(latest?)?;
    Some(latest > installed)
}

/// `X.Y.Z[-pre][+build]`, with an optional leading `v`, as `(X, Y, Z)`.
fn parse_version_triple(version: &str) -> Option<(u64, u64, u64)> {
    let core = version
        .trim()
        .trim_start_matches('v')
        .split(['-', '+', ' '])
        .next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}
