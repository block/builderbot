//! Health Check ("Doctor") — backend checks for external dependencies.
//!
//! Each check probes a single external dependency and returns a status
//! (pass / warn / fail) with a human-readable summary and an optional
//! URL the user can visit to install or configure the dependency.

pub mod agents;
pub mod checks;
mod command;
mod environment;
pub(crate) mod freshness;
pub(crate) mod package_ids;
pub mod resolve;
mod timeout_check;
pub mod types;

pub use environment::DoctorEnv;
pub use types::{AgentVersionInfo, CheckStatus, DoctorCheck, DoctorReport, FixType};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agents::{
    bundled_version_probe_args, check_single_ai_agent, derive_update_command, lookup_fix_command,
    AI_AGENT_CHECKS,
};
use checks::{check_gh, check_gh_auth, check_git, check_git_lfs};
use command::{
    format_duration, run_command_with_timeout, CommandError, CommandTimeout, DEFAULT_PROBE_TIMEOUT,
};
use freshness::{
    fetch_version_info, is_self_updating, load_cache, save_cache, select_installed_probe,
    FetchVersionInfoOptions, InstalledProbe,
};
use package_ids::{lookup_package_id, LatestSource, Role};
use resolve::resolve_binary_with_diagnostics;
use types::{InstallSource, ResolvedBinary};

const SNAPSHOT_PATH_ENV: &str = "__BUILDERBOT_DOCTOR_SNAPSHOT_PATH";

/// Fallback check returned when a spawn_blocking task panics.
fn empty_check(id: &str, label: &str) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        label: label.to_string(),
        status: CheckStatus::Fail,
        message: "Check failed to run".to_string(),
        fix_url: None,
        fix_command: None,
        fix_type: None,
        path: None,
        bridge_path: None,
        raw_output: None,
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

/// Options controlling optional, slower passes layered on top of the core
/// check set. Defaults preserve the original [`run_checks`] behavior — no
/// network, no extra subprocess fan-out.
#[derive(Debug, Clone, Default)]
pub struct RunChecksOptions {
    /// If true, populate `installed_version`/`latest_version`/`update_available`
    /// on each check by probing the binary and the relevant registry.
    pub check_freshness: bool,
    /// If true (in combination with `check_freshness`), skip the remote
    /// registry lookups — only the local installed-version probe runs.
    pub offline: bool,
    /// Optional npm registry override. When `Some`, every npm-backed
    /// install/probe command (`npm install -g …`, `npm view …`) is routed
    /// through this registry via `--registry=<url>`. The URL is always
    /// caller-supplied; the crate bakes in no registry of its own. `None`
    /// (the default) reproduces the original commands exactly.
    pub npm_registry: Option<String>,
    /// Optional caller-provided environment snapshot. When set, doctor clears
    /// each child command's environment and applies these variables so binary
    /// resolution, checks, freshness probes, and fixes all see the same shell
    /// environment. `None` preserves the previous per-call-site behavior.
    pub env: Option<DoctorEnv>,
    /// Directory holding agent binaries bundled inside the embedding app
    /// (e.g. Staged's `resources/acp/bin`). Binaries that resolve from inside
    /// this dir are labeled [`InstallSource::Bundled`] and their readouts get
    /// `bundled: Some(true)` — versions are pinned by the app's lock and ship
    /// with app updates, so no registry install/update fix is offered. The
    /// caller remains responsible for putting the dir on the probe PATH (via
    /// `env`); this option only affects labeling.
    pub bundled_tools_dir: Option<PathBuf>,
}

impl RunChecksOptions {
    pub fn with_env_snapshot(mut self, vars: Vec<(String, String)>) -> Self {
        self.env = Some(DoctorEnv::new(vars));
        self
    }
}

/// Run all health checks and return the report. Equivalent to
/// `run_checks_with_options(RunChecksOptions::default())`.
pub async fn run_checks() -> DoctorReport {
    run_checks_with_options(RunChecksOptions::default()).await
}

/// Run all health checks with explicit options. Existing callers that want
/// the cheap, no-network path should keep using [`run_checks`].
pub async fn run_checks_with_options(opts: RunChecksOptions) -> DoctorReport {
    let RunChecksOptions {
        check_freshness,
        offline,
        npm_registry,
        env,
        bundled_tools_dir,
    } = opts;
    let env = env.map(Arc::new);
    let npm_registry = npm_registry.as_deref();
    let report = collect_base_report(npm_registry, env.clone(), bundled_tools_dir.as_deref()).await;

    if check_freshness {
        populate_freshness(report, offline, npm_registry, env).await
    } else {
        report
    }
}

async fn collect_base_report(
    npm_registry: Option<&str>,
    env: Option<Arc<DoctorEnv>>,
    bundled_tools_dir: Option<&Path>,
) -> DoctorReport {
    let mut binary_names: Vec<&'static str> = vec!["git", "gh", "git-lfs"];
    for info in AI_AGENT_CHECKS {
        for cmd in info.commands {
            if !binary_names.contains(cmd) {
                binary_names.push(cmd);
            }
        }
        if let Some(main) = info.main_command {
            if !binary_names.contains(&main) {
                binary_names.push(main);
            }
        }
    }

    let handles: Vec<_> = binary_names
        .iter()
        .map(|&name| {
            let env = env.clone();
            tokio::task::spawn_blocking(move || {
                let (resolved, timeouts) = resolve_binary_with_diagnostics(name, env.as_deref());
                (name, resolved, timeouts)
            })
        })
        .collect();

    let mut resolved: HashMap<&str, ResolvedBinary> = HashMap::new();
    let mut resolution_timeouts = Vec::new();
    for handle in handles {
        if let Ok((name, rb, timeouts)) = handle.await {
            resolved.insert(name, rb);
            resolution_timeouts.extend(timeouts);
        }
    }

    if let Some(dir) = bundled_tools_dir {
        apply_bundled_install_source(&mut resolved, dir);
    }

    let fallback = ResolvedBinary {
        path: None,
        search_output: "resolution task panicked".to_string(),
        install_source: None,
    };
    let r_git = resolved
        .get("git")
        .cloned()
        .unwrap_or_else(|| fallback.clone());
    let r_gh = resolved
        .get("gh")
        .cloned()
        .unwrap_or_else(|| fallback.clone());
    let r_git_lfs = resolved
        .get("git-lfs")
        .cloned()
        .unwrap_or_else(|| fallback.clone());

    let any_agent_found = AI_AGENT_CHECKS.iter().any(|info| {
        info.commands
            .iter()
            .any(|cmd| resolved.get(cmd).is_some_and(|rb| rb.path.is_some()))
    });

    let git_r = r_git.clone();
    let gh_r = r_gh.clone();
    let gh_r2 = r_gh.clone();
    let git_r2 = r_git;
    let git_lfs_r = r_git_lfs;

    let c_git_env = env.clone();
    let c_git = tokio::task::spawn_blocking(move || check_git(&git_r, c_git_env.as_deref()));
    let c_gh_env = env.clone();
    let c_gh = tokio::task::spawn_blocking(move || check_gh(&gh_r, c_gh_env.as_deref()));
    let c_gh_auth_env = env.clone();
    let c_gh_auth =
        tokio::task::spawn_blocking(move || check_gh_auth(&gh_r2, c_gh_auth_env.as_deref()));
    let c_git_lfs_env = env.clone();
    let c_git_lfs = tokio::task::spawn_blocking(move || {
        check_git_lfs(&git_r2, &git_lfs_r, c_git_lfs_env.as_deref())
    });

    let npm_registry_owned = npm_registry.map(|s| s.to_string());
    let agent_handles: Vec<_> = AI_AGENT_CHECKS
        .iter()
        .map(|info| {
            let found = any_agent_found;
            let registry = npm_registry_owned.clone();
            let env = env.clone();
            let cmds: Vec<ResolvedBinary> = info
                .commands
                .iter()
                .map(|cmd| {
                    resolved
                        .get(cmd)
                        .cloned()
                        .unwrap_or_else(|| fallback.clone())
                })
                .collect();
            let main = info.main_command.and_then(|cmd| resolved.get(cmd).cloned());
            tokio::task::spawn_blocking(move || {
                check_single_ai_agent(
                    info,
                    found,
                    &cmds,
                    main.as_ref(),
                    registry.as_deref(),
                    env.as_deref(),
                )
            })
        })
        .collect();

    let (c_git, c_gh, c_gh_auth, c_git_lfs) = tokio::join!(c_git, c_gh, c_gh_auth, c_git_lfs);

    let mut checks = vec![
        c_git.unwrap_or_else(|_| empty_check("git", "Git")),
        c_gh.unwrap_or_else(|_| empty_check("gh", "GitHub CLI")),
        c_gh_auth.unwrap_or_else(|_| empty_check("gh-auth", "GitHub Auth")),
        c_git_lfs.unwrap_or_else(|_| empty_check("git-lfs", "Git LFS")),
    ];

    for (i, handle) in agent_handles.into_iter().enumerate() {
        let info = &AI_AGENT_CHECKS[i];
        checks.push(
            handle
                .await
                .unwrap_or_else(|_| empty_check(info.id, info.label)),
        );
    }

    checks.extend(timeout_diagnostic_checks(resolution_timeouts));

    DoctorReport { checks }
}

/// Re-label binaries that resolved from inside the embedding app's bundled
/// tools dir as [`InstallSource::Bundled`]. Runs before the checks consume the
/// resolution results, so the label flows into readouts (which also stamp
/// `bundled: Some(true)`), the flat `install_source`, and the freshness pass —
/// where `Bundled` has no registry entry and therefore never yields an update
/// nag or a registry fix command.
fn apply_bundled_install_source(resolved: &mut HashMap<&str, ResolvedBinary>, dir: &Path) {
    for rb in resolved.values_mut() {
        if rb.path.as_deref().is_some_and(|p| p.starts_with(dir)) {
            rb.install_source = Some(InstallSource::Bundled);
        }
    }
}

fn timeout_diagnostic_checks(timeouts: Vec<CommandTimeout>) -> Vec<DoctorCheck> {
    let mut seen_timeouts = HashSet::new();
    let mut used_ids = HashSet::new();
    let mut checks = Vec::new();
    for timeout in timeouts {
        let fingerprint = (
            timeout.label.clone(),
            timeout.command.clone(),
            timeout.timeout,
        );
        if !seen_timeouts.insert(fingerprint) {
            continue;
        }
        let id = unique_timeout_diagnostic_id(&timeout, &mut used_ids);
        checks.push(timeout_diagnostic_check(timeout, id));
    }
    checks
}

fn unique_timeout_diagnostic_id(
    timeout: &CommandTimeout,
    used_ids: &mut HashSet<String>,
) -> String {
    let slug_source = format!("{} {}", timeout.label, timeout.command);
    let base = format!("subprocess-timeout-{}", slug(&slug_source));
    if used_ids.insert(base.clone()) {
        return base;
    }

    let suffixed = format!("{base}-{}", stable_timeout_suffix(timeout));
    if used_ids.insert(suffixed.clone()) {
        return suffixed;
    }

    let mut index = 2;
    loop {
        let candidate = format!("{suffixed}-{index}");
        if used_ids.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

fn stable_timeout_suffix(timeout: &CommandTimeout) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;

    fn update(mut hash: u64, bytes: &[u8]) -> u64 {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
    }

    let mut hash = FNV_OFFSET;
    hash = update(hash, timeout.label.as_bytes());
    hash = update(hash, &[0]);
    hash = update(hash, timeout.command.as_bytes());
    hash = update(hash, &[0]);
    hash = update(hash, timeout.timeout.as_nanos().to_string().as_bytes());
    format!("{hash:016x}")
}

fn timeout_diagnostic_check(timeout: CommandTimeout, id: String) -> DoctorCheck {
    DoctorCheck {
        id,
        label: timeout.label.clone(),
        status: CheckStatus::Warn,
        message: timeout.message(),
        fix_url: None,
        fix_command: None,
        fix_type: None,
        path: None,
        bridge_path: None,
        raw_output: Some(format!(
            "# Check: {}\n{}",
            timeout.label,
            timeout.raw_output()
        )),
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

fn slug(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Which binary behind a check a freshness probe targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ReadoutSlot {
    /// The agent's own CLI (or, for agents without a separate bridge, the
    /// single resolved binary). Maps to `DoctorCheck.path` / `DoctorCheck.main`.
    Main,
    /// The agent's ACP bridge. Maps to `DoctorCheck.bridge_path` /
    /// `DoctorCheck.bridge`.
    Bridge,
    /// A non-agent check (git, gh, …) with no main/bridge split. Maps directly
    /// to the flat version fields on `DoctorCheck`.
    Flat,
}

/// Resolve the `(package_id, latest_source)` pair for a check + install source.
/// `None`/`None` when the source is missing or the check has no matching entry.
fn resolve_package(
    check_id: &str,
    source: Option<&InstallSource>,
    role: Role,
) -> (Option<String>, Option<LatestSource>) {
    source
        .cloned()
        .and_then(|src| lookup_package_id(check_id, src, role))
        .map(|(pkg, latest)| (Some(pkg.to_string()), Some(latest)))
        .unwrap_or((None, None))
}

/// Fold a freshness [`freshness::VersionInfo`] into a per-binary readout,
/// applying the self-updating suppression rule for `update_available`. When an
/// update is actionable (and the slot is `Main`/`Bridge`), derive the
/// source-aware `update_command` + `update_fix_type` from the readout's install
/// source and the supplied package id. The flat (non-agent) slot never gets an
/// update command — non-agent updates are out of scope.
fn apply_freshness(
    readout: &mut AgentVersionInfo,
    info: &freshness::VersionInfo,
    slot: ReadoutSlot,
    package_id: Option<&str>,
) {
    readout.installed_version = info.installed.clone();
    readout.latest_version = info.latest.clone();
    // Self-updating tools (curl/native installers) manage their own freshness:
    // report installed/latest for display, but never raise an "update available"
    // nag — the update isn't the user's to action.
    let self_updating = is_self_updating(readout.install_source.as_ref());
    readout.self_updating = Some(self_updating);
    readout.update_available = if self_updating {
        None
    } else {
        info.update_available
    };

    let actionable = readout.update_available == Some(true) && !self_updating;
    let slot_fix_type = match slot {
        ReadoutSlot::Main => Some(FixType::UpdateMain),
        ReadoutSlot::Bridge => Some(FixType::UpdateBridge),
        ReadoutSlot::Flat => None,
    };
    if let (true, Some(fix_type)) = (actionable, slot_fix_type) {
        if let Some(cmd) = derive_update_command(readout.install_source.as_ref(), package_id) {
            readout.update_command = Some(cmd);
            readout.update_fix_type = Some(fix_type);
        }
    }
}

fn apply_freshness_timeouts(check: &mut DoctorCheck, timeouts: &[CommandTimeout]) {
    if timeouts.is_empty() {
        return;
    }

    if check.status == CheckStatus::Pass {
        check.status = CheckStatus::Warn;
    }

    let first = &timeouts[0];
    let summary = if timeouts.len() == 1 {
        format!(
            "freshness timed out after {} running {}",
            format_duration(first.timeout),
            first.command
        )
    } else {
        format!(
            "{} freshness probes timed out, first after {} running {}",
            timeouts.len(),
            format_duration(first.timeout),
            first.command
        )
    };
    check.message = format!("{}; {summary}", check.message);

    let mut raw = check.raw_output.take().unwrap_or_default();
    if !raw.is_empty() {
        raw.push('\n');
    }
    raw.push_str("# Freshness subprocess timeouts:");
    for timeout in timeouts {
        raw.push('\n');
        raw.push_str(&timeout.raw_output());
    }
    check.raw_output = Some(raw);
}

/// Post-hoc pass: for every check that has a usable binary path and a known
/// package id, run the installed/latest version probes in parallel and update
/// the corresponding fields on the report. The on-disk cache is read once at
/// the start and written once at the end.
///
/// Agent checks front up to two independent binaries — the agent CLI (`main`)
/// and its ACP bridge (`bridge`) — and each is probed and reported separately.
/// The flat version fields are kept in sync for backward compatibility: they
/// mirror the bridge readout when a bridge exists, otherwise the main readout
/// (the same headline the pre-split pass produced). Non-agent checks keep
/// writing straight to the flat fields.
async fn populate_freshness(
    mut report: DoctorReport,
    offline: bool,
    npm_registry: Option<&str>,
    env: Option<Arc<DoctorEnv>>,
) -> DoctorReport {
    let cache = Arc::new(Mutex::new(load_cache()));
    let npm_registry = npm_registry.map(|s| s.to_string());

    let mut targets: Vec<FreshnessTarget> = Vec::new();
    for check in &report.checks {
        let is_agent = check.main.is_some() || check.bridge.is_some();
        if is_agent {
            if let (Some(readout), Some(path)) = (&check.main, check.path.as_deref()) {
                let path = PathBuf::from(path);
                let (package_id, latest_source) =
                    resolve_package(&check.id, readout.install_source.as_ref(), Role::Main);
                targets.push(FreshnessTarget {
                    id: check.id.clone(),
                    slot: ReadoutSlot::Main,
                    path,
                    latest_source,
                    package_id,
                    install_source: readout.install_source.clone(),
                    // Bundled bridges probe the vendored harness CLI's version
                    // through the bridge passthrough (e.g. Claude Code 2.1.x),
                    // not the bridge package's own pinned version.
                    version_args: bundled_version_probe_args(
                        &check.id,
                        readout.install_source.as_ref(),
                    ),
                });
            }
            if let (Some(readout), Some(path)) = (&check.bridge, check.bridge_path.as_deref()) {
                let path = PathBuf::from(path);
                let (package_id, latest_source) =
                    resolve_package(&check.id, readout.install_source.as_ref(), Role::Bridge);
                targets.push(FreshnessTarget {
                    id: check.id.clone(),
                    slot: ReadoutSlot::Bridge,
                    path,
                    latest_source,
                    package_id,
                    install_source: readout.install_source.clone(),
                    version_args: None,
                });
            }
        } else {
            // Non-agent check: prefer the bridge path when present (matches the
            // install_source the check carries — see check_single_ai_agent).
            let path_str = check.bridge_path.as_deref().or(check.path.as_deref());
            let Some(path_str) = path_str else { continue };
            let path = PathBuf::from(path_str);
            let (package_id, latest_source) =
                resolve_package(&check.id, check.install_source.as_ref(), Role::Any);
            targets.push(FreshnessTarget {
                id: check.id.clone(),
                slot: ReadoutSlot::Flat,
                path,
                latest_source,
                package_id,
                install_source: check.install_source.clone(),
                version_args: None,
            });
        }
    }

    let futures = targets.into_iter().map(|t| {
        let cache = cache.clone();
        let npm_registry = npm_registry.clone();
        let env = env.clone();
        async move {
            // Bundled bridges probe through their explicit passthrough args.
            // Otherwise: npm-distributed bridges don't honor `--version`, so
            // their installed version is read straight from the owning
            // `package.json`; everything else runs `<binary> --version`.
            let probe = match t.version_args {
                Some(args) => InstalledProbe::Cli(args),
                None => select_installed_probe(t.install_source.as_ref(), t.package_id.as_deref()),
            };
            let info = fetch_version_info(
                t.latest_source,
                t.package_id.as_deref(),
                &t.path,
                probe,
                FetchVersionInfoOptions {
                    offline,
                    npm_registry: npm_registry.as_deref(),
                    env: env.as_deref(),
                    cache,
                },
            )
            .await;
            ((t.id, t.slot), (info, t.package_id))
        }
    });

    let results = futures::future::join_all(futures).await;
    let mut by_target: HashMap<(String, ReadoutSlot), (freshness::VersionInfo, Option<String>)> =
        HashMap::new();
    for (key, payload) in results {
        by_target.insert(key, payload);
    }

    for check in &mut report.checks {
        let is_agent = check.main.is_some() || check.bridge.is_some();
        if is_agent {
            let mut freshness_timeouts = Vec::new();
            if let Some((info, pkg)) = by_target.remove(&(check.id.clone(), ReadoutSlot::Main)) {
                if let Some(readout) = check.main.as_mut() {
                    apply_freshness(readout, &info, ReadoutSlot::Main, pkg.as_deref());
                }
                freshness_timeouts.extend(info.command_timeouts);
            }
            if let Some((info, pkg)) = by_target.remove(&(check.id.clone(), ReadoutSlot::Bridge)) {
                if let Some(readout) = check.bridge.as_mut() {
                    apply_freshness(readout, &info, ReadoutSlot::Bridge, pkg.as_deref());
                }
                freshness_timeouts.extend(info.command_timeouts);
            }
            apply_freshness_timeouts(check, &freshness_timeouts);
            // Mirror the headline readout (bridge if present, else main) into the
            // flat fields for backward-compatible consumers.
            let headline = check.bridge.as_ref().or(check.main.as_ref()).map(|r| {
                (
                    r.installed_version.clone(),
                    r.latest_version.clone(),
                    r.update_available,
                    r.self_updating,
                )
            });
            if let Some((installed, latest, update_available, self_updating)) = headline {
                check.installed_version = installed;
                check.latest_version = latest;
                check.update_available = update_available;
                check.self_updating = self_updating;
            }
        } else if let Some((info, _pkg)) = by_target.remove(&(check.id.clone(), ReadoutSlot::Flat))
        {
            apply_freshness_timeouts(check, &info.command_timeouts);
            check.installed_version = info.installed;
            check.latest_version = info.latest;
            let self_updating = is_self_updating(check.install_source.as_ref());
            check.self_updating = Some(self_updating);
            check.update_available = if self_updating {
                None
            } else {
                info.update_available
            };
        }
    }

    if let Ok(guard) = cache.lock() {
        save_cache(&guard);
    }

    report
}

struct FreshnessTarget {
    id: String,
    slot: ReadoutSlot,
    path: PathBuf,
    latest_source: Option<LatestSource>,
    package_id: Option<String>,
    install_source: Option<InstallSource>,
    /// Explicit installed-version probe args (bridge passthrough for bundled
    /// installs). `None` selects the source-derived default probe.
    version_args: Option<&'static [&'static str]>,
}

/// Opt-in piped stdin for a fix subprocess. Create with [`FixStdin::pipe`];
/// keep the [`FixStdinWriter`], put the `FixStdin` in
/// [`ExecuteFixOptions::stdin`].
///
/// Single-use: the first execution claims the pipe, and any later execution
/// handed the same `FixStdin` — or a clone of it, including one carried along by
/// a cloned [`ExecuteFixOptions`] — fails with an error instead of spawning.
/// Retrying a fix needs a fresh pipe.
#[derive(Debug, Clone)]
pub struct FixStdin {
    state: Arc<Mutex<FixStdinState>>,
}

/// The pipe's whole life cycle: `Buffered` until the fix spawns, `Live` while it
/// runs, then `Closed` — terminal, and reached when the fix ends, when the last
/// writer drops, or when a write finds the read end gone. Holding the child's
/// stdin handle here rather than in a thread of its own is what lets
/// [`FixStdinWriter::send_line`] write through and report the real outcome.
#[derive(Debug)]
enum FixStdinState {
    /// Before the fix spawns: lines the host queued, replayed at spawn.
    /// `claimed` marks the execution that reserved this pipe, so a second one
    /// is rejected before it spawns. `eof` records that every writer dropped
    /// pre-spawn, so the replay is followed immediately by closing the pipe.
    Buffered {
        lines: Vec<String>,
        eof: bool,
        claimed: bool,
    },
    /// Fix running: writes go straight into the child's stdin.
    Live(std::process::ChildStdin),
    /// Fix finished, every writer gone, or a write hit a dead pipe.
    Closed,
}

/// Rejection for an execution handed a `FixStdin` another one already claimed.
const FIX_STDIN_REUSED: &str = "FixStdin already consumed by a previous fix execution; \
     create a fresh pipe with FixStdin::pipe() for each run";

/// Rejection for a line the pipe cannot deliver because it is closed.
const FIX_STDIN_CLOSED: &str = "Fix is no longer accepting input";

/// Locking the pipe state recovers from poisoning instead of propagating it: no
/// invariant spans the lock (the state is a plain enum, and the only work done
/// under it is a `Vec` push or a pipe write), while treating a poisoned lock as
/// a failure would cost `send_line` its delivery guarantee and leak the child's
/// stdin handle for the lifetime of the writer.
fn lock_fix_stdin_state(state: &Mutex<FixStdinState>) -> std::sync::MutexGuard<'_, FixStdinState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl FixStdinState {
    /// Queue or write `line` — with the trailing newline the caller doesn't
    /// supply — according to the current state. A failed write latches `Closed`
    /// so later sends fail without re-discovering the dead pipe.
    fn send_line(&mut self, line: String) -> Result<(), String> {
        match self {
            FixStdinState::Buffered { lines, .. } => {
                lines.push(line);
                Ok(())
            }
            FixStdinState::Live(pipe) => {
                use std::io::Write;
                match pipe
                    .write_all(format!("{line}\n").as_bytes())
                    .and_then(|()| pipe.flush())
                {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        *self = FixStdinState::Closed;
                        Err(format!("{FIX_STDIN_CLOSED}: {e}"))
                    }
                }
            }
            FixStdinState::Closed => Err(FIX_STDIN_CLOSED.to_string()),
        }
    }
}

impl FixStdin {
    /// Create a connected pair: a cloneable writer for the caller to keep and
    /// the `FixStdin` to place in [`ExecuteFixOptions::stdin`]. Lines sent
    /// before the fix subprocess spawns are queued and replayed once it does;
    /// dropping every writer clone closes the child's stdin (EOF).
    ///
    /// Dropping the writers is the only way to say "no more input", and a fix
    /// that reads *to EOF* rather than a fixed number of lines will not exit
    /// until that happens — a host that leaves its input UI open pins the fix
    /// until [`ExecuteFixOptions::timeout`] fires. Nothing else is at stake in
    /// dropping them: the child's stdin handle lives with the fix and is
    /// reclaimed when it ends, held writer or not.
    pub fn pipe() -> (FixStdinWriter, FixStdin) {
        let state = Arc::new(Mutex::new(FixStdinState::Buffered {
            lines: Vec::new(),
            eof: false,
            claimed: false,
        }));
        (
            FixStdinWriter {
                inner: Arc::new(FixStdinWriterInner {
                    state: state.clone(),
                }),
            },
            FixStdin { state },
        )
    }

    /// Reserve this pipe for a child about to be spawned. First caller wins;
    /// `Err` on every later call (a clone already fed an execution), which the
    /// caller surfaces instead of spawning a fix whose stdin is already dead.
    fn claim(&self) -> Result<(), String> {
        match &mut *lock_fix_stdin_state(&self.state) {
            FixStdinState::Buffered { claimed, .. } if !*claimed => {
                *claimed = true;
                Ok(())
            }
            _ => Err(FIX_STDIN_REUSED.to_string()),
        }
    }

    /// Hand the spawned child's stdin to the pipe, replay whatever the host
    /// queued before the spawn, and go live.
    ///
    /// Only ever reached after a successful [`FixStdin::claim`], which is what
    /// guarantees the state is still `Buffered`; any other state means another
    /// execution owns the pipe, and dropping the handle — an immediate EOF for
    /// this child — is the only safe reading of that. A replay write that fails
    /// is not the fix's failure (a command is free to exit successfully without
    /// reading its stdin), so it only latches `Closed`; the host hears about it
    /// from its next `send_line`.
    fn attach(&self, child_stdin: std::process::ChildStdin) {
        let mut state = lock_fix_stdin_state(&self.state);
        let FixStdinState::Buffered { lines, eof, .. } = &mut *state else {
            return;
        };
        let queued = std::mem::take(lines);
        let eof = *eof;
        *state = FixStdinState::Live(child_stdin);
        for line in queued {
            if state.send_line(line).is_err() {
                break;
            }
        }
        if eof {
            // Every writer was dropped before the spawn, so the queued lines
            // above are all the input there will ever be and closing now is the
            // EOF the fix is waiting for.
            *state = FixStdinState::Closed;
        }
    }

    /// The fix is over: close the pipe so every later send fails immediately.
    /// A write hitting `EPIPE` cannot be the signal on its own — a backgrounded
    /// grandchild that inherited the child's stdin keeps the read end open, and
    /// writes into it go on succeeding long after the fix is gone.
    fn close(&self) {
        *lock_fix_stdin_state(&self.state) = FixStdinState::Closed;
    }
}

/// Cloneable handle for feeding lines to a fix subprocess's stdin. Dropping
/// every clone closes the fix's stdin (EOF).
#[derive(Debug, Clone)]
pub struct FixStdinWriter {
    inner: Arc<FixStdinWriterInner>,
}

/// Shared by every [`FixStdinWriter`] clone so EOF is delivered exactly when
/// the last one drops, which is what keeps the writer `Clone`.
#[derive(Debug)]
struct FixStdinWriterInner {
    state: Arc<Mutex<FixStdinState>>,
}

impl Drop for FixStdinWriterInner {
    fn drop(&mut self) {
        match &mut *lock_fix_stdin_state(&self.state) {
            // Pre-spawn the queued lines still have to reach the child first, so
            // record the EOF for `attach` to deliver after the replay.
            FixStdinState::Buffered { eof, .. } => *eof = true,
            // Otherwise dropping the state's `ChildStdin` *is* the EOF.
            state => *state = FixStdinState::Closed,
        }
    }
}

impl FixStdinWriter {
    /// Write one line to the fix's stdin; a trailing `\n` is appended and the
    /// pipe is flushed.
    ///
    /// `Ok` means the bytes were handed to the child's stdin pipe — not that the
    /// fix read them, since a fix can exit with bytes still buffered. `Err`
    /// means the line was *not* delivered: the fix has finished, its stdin is
    /// closed, or this pipe was never attached to a spawned fix.
    ///
    /// Lines sent before the fix spawns are queued and replayed at spawn, so
    /// they return `Ok` before any pipe exists; if the fix never spawns they are
    /// dropped.
    ///
    /// Completion is signalled by the fix's own `Result`, never by `send_line`.
    /// May block if the fix isn't reading and the pipe buffer fills, so a host
    /// sending anything bulkier than a pasted code should call this off its
    /// async runtime.
    pub fn send_line(&self, line: impl Into<String>) -> Result<(), String> {
        lock_fix_stdin_state(&self.inner.state).send_line(line.into())
    }
}

/// Wall-clock bound on a single fix execution.
///
/// Fixes are install/auth/update actions, so the bound has to clear a
/// cold-cache `npm install -g` behind a corporate proxy and a human doing SSO
/// in a browser — orders of magnitude above the probe timeouts in
/// [`crate::command`]. This is an enum rather than `Option<Duration>` because
/// `None` reads as both "use the default" and "no timeout"; here every literal
/// has to say which it means, and `Unbounded` stays reachable for a caller
/// that genuinely wants the old forever-wait.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FixTimeout {
    /// [`DEFAULT_FIX_TIMEOUT`].
    #[default]
    Standard,
    /// A caller-chosen bound.
    After(Duration),
    /// No bound at all: the fix runs until it exits on its own.
    Unbounded,
}

impl FixTimeout {
    /// The wall-clock bound, or `None` for [`FixTimeout::Unbounded`].
    fn duration(self) -> Option<Duration> {
        match self {
            FixTimeout::Standard => Some(DEFAULT_FIX_TIMEOUT),
            FixTimeout::After(duration) => Some(duration),
            FixTimeout::Unbounded => None,
        }
    }
}

/// Deadline applied by [`FixTimeout::Standard`]. Deliberately generous: it
/// exists to stop a wedged fix from pinning a blocking worker and a process
/// tree for the lifetime of the host, not to police slow-but-honest installs
/// or a leisurely browser login.
pub const DEFAULT_FIX_TIMEOUT: Duration = Duration::from_secs(600);

/// Options for executing a doctor fix command.
#[derive(Debug, Clone, Default)]
pub struct ExecuteFixOptions {
    /// Exact command to run instead of looking up a static check fix.
    pub command_override: Option<String>,
    /// Optional npm registry override for npm-backed fix/update commands.
    pub npm_registry: Option<String>,
    /// Optional caller-provided environment snapshot for the fix subprocess.
    pub env: Option<DoctorEnv>,
    /// Opt-in piped stdin for the fix subprocess (see [`FixStdin::pipe`]).
    /// `None` keeps the child inheriting the host process's stdin, so
    /// terminal hosts can still run interactive fixes directly.
    ///
    /// A `FixStdin` feeds exactly one execution, so a cached options struct
    /// must have this field refreshed (or be rebuilt) before a fix is retried;
    /// reusing it fails the run.
    pub stdin: Option<FixStdin>,
    /// Wall-clock bound on the fix. Defaults to [`FixTimeout::Standard`].
    pub timeout: FixTimeout,
}

impl ExecuteFixOptions {
    pub fn with_env_snapshot(mut self, vars: Vec<(String, String)>) -> Self {
        self.env = Some(DoctorEnv::new(vars));
        self
    }

    /// Attach an opt-in stdin pipe (see [`FixStdin::pipe`]). The `FixStdin`
    /// feeds exactly one execution: call this again with a fresh pipe for
    /// every retry rather than reusing a built options struct.
    pub fn with_stdin(mut self, stdin: FixStdin) -> Self {
        self.stdin = Some(stdin);
        self
    }

    /// Override the wall-clock bound on the fix (see [`FixTimeout`]).
    pub fn with_timeout(mut self, timeout: FixTimeout) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Run a fix command for a doctor check, identified by check ID and fix type.
///
/// The actual shell command is looked up from the static check definitions —
/// the caller never sends a raw command string.
pub async fn execute_fix(check_id: String, fix_type: FixType) -> Result<(), String> {
    execute_fix_streaming(check_id, fix_type, |_| {}).await
}

/// Like [`execute_fix`], but routes npm-backed fix commands through an optional
/// caller-supplied registry (see [`RunChecksOptions::npm_registry`]) and
/// optionally accepts a `command_override` — the exact shell command to run,
/// bypassing the static [`lookup_fix_command`] lookup. The override is the only
/// way to dispatch the per-readout `FixType::UpdateMain` / `UpdateBridge`
/// commands, which aren't in the static table.
///
/// When `command_override` is `Some`, `fix_type` is informational only; the
/// override is always used. When `None`, the command is looked up exactly as
/// before. `apply_npm_registry` runs over the final command string regardless
/// of where it came from.
pub async fn execute_fix_with_options(
    check_id: String,
    fix_type: FixType,
    command_override: Option<String>,
    npm_registry: Option<&str>,
) -> Result<(), String> {
    execute_fix_with_env_options(
        check_id,
        fix_type,
        ExecuteFixOptions {
            command_override,
            npm_registry: npm_registry.map(str::to_string),
            ..Default::default()
        },
    )
    .await
}

/// Like [`execute_fix_with_options`], but accepts the complete fix execution
/// options, including a caller-provided environment snapshot.
pub async fn execute_fix_with_env_options(
    check_id: String,
    fix_type: FixType,
    opts: ExecuteFixOptions,
) -> Result<(), String> {
    execute_fix_streaming_with_env_options(check_id, fix_type, opts, |_| {}).await
}

/// Run a fix command and stream its output line-by-line to `on_line`.
///
/// `on_line` is invoked once per output line, with the trailing newline
/// stripped. Both stdout and stderr lines are delivered through the same
/// callback; ordering across the two streams is best-effort (each stream is
/// in order internally, but interleaving between them depends on scheduling).
///
/// The callback runs on the tokio runtime's blocking pool — do not block in it
/// for unbounded periods, but emitting Tauri events / writing to a channel is
/// fine.
pub async fn execute_fix_streaming<F>(
    check_id: String,
    fix_type: FixType,
    on_line: F,
) -> Result<(), String>
where
    F: FnMut(&str) + Send + 'static,
{
    execute_fix_streaming_with_options(check_id, fix_type, None, None, on_line).await
}

/// Like [`execute_fix_streaming`], but accepts a `command_override` to run an
/// exact shell command (skipping [`lookup_fix_command`]) and routes npm-backed
/// commands through an optional caller-supplied registry. See
/// [`execute_fix_with_options`] for the semantics of `command_override`.
pub async fn execute_fix_streaming_with_options<F>(
    check_id: String,
    fix_type: FixType,
    command_override: Option<String>,
    npm_registry: Option<&str>,
    on_line: F,
) -> Result<(), String>
where
    F: FnMut(&str) + Send + 'static,
{
    execute_fix_streaming_with_env_options(
        check_id,
        fix_type,
        ExecuteFixOptions {
            command_override,
            npm_registry: npm_registry.map(str::to_string),
            ..Default::default()
        },
        on_line,
    )
    .await
}

/// Like [`execute_fix_streaming_with_options`], but accepts the complete fix
/// execution options, including a caller-provided environment snapshot.
pub async fn execute_fix_streaming_with_env_options<F>(
    check_id: String,
    fix_type: FixType,
    opts: ExecuteFixOptions,
    mut on_line: F,
) -> Result<(), String>
where
    F: FnMut(&str) + Send + 'static,
{
    let command = match opts.command_override {
        Some(cmd) => cmd,
        None => lookup_fix_command(&check_id, &fix_type)
            .ok_or_else(|| format!("Unknown check '{check_id}' or fix type '{fix_type:?}'"))?,
    };
    let command = agents::apply_npm_registry(&command, opts.npm_registry.as_deref());

    // Echo the resolved command as a preamble line so downstream callers (e.g.
    // goose-internal's `run_fix`, which `info!`s every callback line and emits
    // it via the `agent-setup:output` Tauri event) record *what* ran, not just
    // its output. Matches the `$ <command>` phrasing the auth probe already
    // writes into `raw_output`.
    on_line(&format!("$ {command}"));

    // Fixes are intentionally not routed through the bounded probe runner:
    // these are user-triggered install/auth/update actions and can reasonably
    // be interactive or long-running, so they get the far more generous
    // `FixTimeout` bound instead of a probe timeout.
    run_command_streaming(command, opts.env, opts.stdin, opts.timeout, on_line).await
}

/// Async wrapper that runs `run_command_streaming_blocking` on the blocking pool.
pub(crate) async fn run_command_streaming<F>(
    command: String,
    env: Option<DoctorEnv>,
    stdin: Option<FixStdin>,
    timeout: FixTimeout,
    on_line: F,
) -> Result<(), String>
where
    F: FnMut(&str) + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        run_command_streaming_blocking(&command, env.as_ref(), stdin, timeout, on_line)
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {e}")))
}

enum StreamLine {
    Stdout(String),
    Stderr(String),
}

/// Build the login-shell `Command` used by every doctor exec path. Without a
/// caller env snapshot, `path_prefix` keeps the legacy behavior of prepending
/// resolved binary dirs plus conservative fallbacks. With a snapshot, doctor
/// preserves the snapshot environment and appends missing prefix dirs to its
/// `PATH` so auth probes can still find resolved binaries. Because login shell
/// startup can rewrite `PATH` after process spawn, snapshot callers also carry
/// the merged path in an internal env var and restore it inside the `-c`
/// payload immediately before running the requested command.
fn build_shell_command(
    command: &str,
    path_prefix: &[PathBuf],
    env: Option<&DoctorEnv>,
) -> std::process::Command {
    let shell = if std::path::Path::new("/bin/zsh").exists() {
        "/bin/zsh"
    } else {
        "/bin/bash"
    };
    let home = env
        .and_then(|e| e.get("HOME").map(str::to_string))
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| "/".to_string());
    let user = env
        .and_then(|e| e.get("USER").map(str::to_string))
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_default();

    let mut shell_command = command.to_string();
    let mut cmd = std::process::Command::new(shell);
    cmd.env_clear();

    if let Some(env) = env {
        for (key, value) in &env.vars {
            cmd.env(key, value);
        }
        if env.get("HOME").is_none() {
            cmd.env("HOME", &home);
        }
        if env.get("USER").is_none() {
            cmd.env("USER", &user);
        }
        cmd.env("TERM", "xterm-256color").current_dir(&home);
        if let Some(path) = merged_snapshot_path(env.get("PATH"), path_prefix) {
            cmd.env("PATH", &path);
            cmd.env(SNAPSHOT_PATH_ENV, path);
            shell_command = command_with_snapshot_path_restore(command);
        }
    } else {
        cmd.env("HOME", &home)
            .env("USER", &user)
            .env("TERM", "xterm-256color")
            .current_dir(&home);
        if !path_prefix.is_empty() {
            let path = legacy_prefixed_path(path_prefix);
            cmd.env("PATH", path);
        }
    }

    cmd.arg("-l").arg("-c").arg(shell_command);
    cmd
}

fn command_with_snapshot_path_restore(command: &str) -> String {
    let name = SNAPSHOT_PATH_ENV;
    format!(
        "if [ \"${{{name}+x}}\" = x ]; then PATH=\"${{{name}}}\"; export PATH; unset {name}; fi\n{command}",
    )
}

fn legacy_prefixed_path(path_prefix: &[PathBuf]) -> String {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut path = String::new();
    for p in path_prefix {
        let s = p.to_string_lossy().to_string();
        if !seen.insert(s.clone()) {
            continue;
        }
        if !path.is_empty() {
            path.push(':');
        }
        path.push_str(&s);
    }
    // Conservative fallback ~ what login zsh on macOS sees before
    // /etc/zprofile augments it. Keeps the rest of the resolved-binary
    // dir's command graph reachable (e.g. node, npm) without depending on
    // the parent process's PATH.
    path.push_str(":/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin");
    path
}

fn merged_snapshot_path(snapshot_path: Option<&str>, path_prefix: &[PathBuf]) -> Option<String> {
    if snapshot_path.is_none() && path_prefix.is_empty() {
        return None;
    }

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut path = String::new();
    if let Some(snapshot_path) = snapshot_path {
        for entry in std::env::split_paths(snapshot_path) {
            let s = entry.to_string_lossy().to_string();
            if s.is_empty() || !seen.insert(s.clone()) {
                continue;
            }
            if !path.is_empty() {
                path.push(':');
            }
            path.push_str(&s);
        }
    }
    for p in path_prefix {
        let s = p.to_string_lossy().to_string();
        if s.is_empty() || !seen.insert(s.clone()) {
            continue;
        }
        if !path.is_empty() {
            path.push(':');
        }
        path.push_str(&s);
    }
    Some(path)
}

/// Detailed outcome of running a command. Lets the caller distinguish
/// `command not found` (exit 127 from the shell, or a spawn failure on the
/// shell itself) from a genuine non-zero exit — important for the auth probe,
/// where the former means "we can't tell" and the latter means "not signed in".
#[derive(Debug)]
pub(crate) enum ExecOutcome {
    Ok,
    /// The shell itself couldn't be spawned (or `wait()` failed). Rare; means
    /// we have no signal at all from the inner command.
    Spawn(std::io::Error),
    /// The command ran longer than the bounded doctor probe timeout.
    Timeout {
        command: String,
        timeout: std::time::Duration,
    },
    /// The shell ran, the inner command exited non-zero. Code is `Some(127)`
    /// when the shell reports "command not found".
    Exit {
        code: Option<i32>,
        stderr: String,
    },
}

/// Run `command` through a login shell, merging the caller-supplied
/// `path_prefix` into the child `PATH`. Returns the detailed exec outcome. No
/// streaming — used by the auth probe which wants a single sync result.
pub(crate) fn execute_command_with_path_prefix_with_env(
    command: &str,
    path_prefix: &[PathBuf],
    env: Option<&DoctorEnv>,
) -> ExecOutcome {
    let cmd = build_shell_command(command, path_prefix, env);
    let output = match run_command_with_timeout(cmd, command, DEFAULT_PROBE_TIMEOUT) {
        Ok(output) => output,
        Err(CommandError::Spawn { source, .. } | CommandError::Wait { source, .. }) => {
            return ExecOutcome::Spawn(source)
        }
        Err(CommandError::Timeout { command, timeout }) => {
            return ExecOutcome::Timeout { command, timeout }
        }
    };
    if output.status.success() {
        ExecOutcome::Ok
    } else {
        ExecOutcome::Exit {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }
}

/// Closes the fix's stdin pipe when `run_command_streaming_blocking` leaves its
/// body — normal return, error return, timeout, spawn failure, or a panic in
/// `on_line`. Every path has to close it: a host that still holds a
/// [`FixStdinWriter`] would otherwise keep getting `Ok` from `send_line` for a
/// fix that is already over, and the child's stdin handle would live as long as
/// that writer.
struct FixStdinCloser<'a>(&'a FixStdin);

impl Drop for FixStdinCloser<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Spawn `command` through a login shell, stream stdout/stderr lines to
/// `on_line`, and return based on the process exit status. Bounded by
/// `timeout`, which is generous rather than tight: fix commands are
/// user-triggered install/auth/update actions and may prompt or run package
/// managers. Stderr lines are also accumulated so a non-zero exit can surface a
/// useful error message (matching the non-streaming behavior of the previous
/// `execute_command`).
fn run_command_streaming_blocking<F>(
    command: &str,
    env: Option<&DoctorEnv>,
    stdin: Option<FixStdin>,
    timeout: FixTimeout,
    mut on_line: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc::RecvTimeoutError;

    use wait_timeout::ChildExt;

    fn consume<F: FnMut(&str)>(msg: StreamLine, on_line: &mut F, stderr_accum: &mut String) {
        match msg {
            StreamLine::Stdout(s) => {
                on_line(&s);
            }
            StreamLine::Stderr(s) => {
                on_line(&s);
                if !stderr_accum.is_empty() {
                    stderr_accum.push('\n');
                }
                stderr_accum.push_str(&s);
            }
        }
    }

    // Claim the pipe before anything is launched: a `FixStdin` another execution
    // already consumed can never deliver a line, so the child would block
    // forever on a pipe nobody writes — the exact hang this option exists to
    // fix. Always a caller bug, so surface it at the call site rather than
    // spawning a doomed subprocess.
    if let Some(fix_stdin) = &stdin {
        fix_stdin.claim()?;
    }

    let mut shell_command = build_shell_command(command, &[], env);
    shell_command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Opt-in only: without a `FixStdin` the child keeps inheriting the host
    // process's stdin, so interactive fixes in terminal hosts are untouched.
    if stdin.is_some() {
        shell_command.stdin(std::process::Stdio::piped());
        // Own the whole tree so a timeout can kill more than the login shell:
        // `kill(-pid)` only reaches an `npm install` under `zsh -lc` if the
        // shell leads its own group. Gated on piped stdin because a child in
        // its own group that reads the controlling terminal gets SIGTTIN and
        // stops — impossible here precisely because doctor owns its stdin, but
        // a real regression for a terminal host on the inherited-stdin path.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            shell_command.process_group(0);
        }
    }
    command::configure_command(&mut shell_command);

    // Declared ahead of the spawn so a spawn failure closes the pipe too: the
    // claim above is already spent, so the host must not keep getting `Ok` for a
    // fix that never started.
    let _stdin_closer = stdin.as_ref().map(FixStdinCloser);

    let mut child = shell_command
        .spawn()
        .map_err(|e| format!("Failed to run command: {e}"))?;

    let child_stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let (tx, rx) = std::sync::mpsc::channel::<StreamLine>();
    let tx_err = tx.clone();

    let stdout_thread = std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(StreamLine::Stdout(line)).is_err() {
                break;
            }
        }
    });
    let stderr_thread = std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            if tx_err.send(StreamLine::Stderr(line)).is_err() {
                break;
            }
        }
    });

    // Deliberately after the readers are running: the replay of pre-spawn lines
    // writes inline on this thread, so a queue larger than the pipe buffer would
    // deadlock against a child whose output nobody is draining yet.
    if let (Some(fix_stdin), Some(child_stdin)) = (&stdin, child_stdin) {
        fix_stdin.attach(child_stdin);
    }

    let limit = timeout.duration();
    let deadline = limit.map(|limit| Instant::now() + limit);
    let mut stderr_accum = String::new();
    let mut expired = false;

    loop {
        let msg = match deadline {
            Some(deadline) => {
                match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(msg) => msg,
                    Err(RecvTimeoutError::Timeout) => {
                        expired = true;
                        break;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            // `recv_timeout(Duration::MAX)` overflows instantly, so the
            // unbounded case keeps the plain blocking receive.
            None => match rx.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            },
        };
        consume(msg, &mut on_line, &mut stderr_accum);
    }

    let status = if expired {
        None
    } else {
        // Both pipes hit EOF, so the readers are already done and joining is
        // immediate. The process can still outlive its pipes, though, so the
        // reap is bounded by the same deadline.
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        match deadline {
            Some(deadline) => child
                .wait_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|e| format!("Failed to wait for command: {e}"))?,
            None => Some(
                child
                    .wait()
                    .map_err(|e| format!("Failed to wait for command: {e}"))?,
            ),
        }
    };

    let Some(status) = status else {
        let limit = limit.expect("a deadline only exists when the fix is bounded");
        // Anything the readers already queued is real output the user should
        // see before the notice explaining why it stopped.
        while let Ok(msg) = rx.try_recv() {
            consume(msg, &mut on_line, &mut stderr_accum);
        }
        on_line(&format!(
            "doctor: fix timed out after {} — terminating",
            format_duration(limit)
        ));
        command::kill_child_process_group_or_child(&mut child);
        let _ = child.wait();
        // The reader threads are deliberately not joined: a descendant that
        // escaped the process group can hold the inherited stdout open long
        // after the fix is dead, and waiting on that is the hang this timeout
        // exists to end. Dropping `rx` retires them at their next send.
        return Err(format!(
            "Fix timed out after {} without finishing: {command}",
            format_duration(limit)
        ));
    };

    if status.success() {
        Ok(())
    } else {
        let trimmed = stderr_accum.trim().to_string();
        Err(if trimmed.is_empty() {
            format!("Command failed with exit code {status}")
        } else {
            trimmed
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn timeout(label: &str, command: &str) -> CommandTimeout {
        CommandTimeout::new(label, command, Duration::from_secs(15))
    }

    fn unique_tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "doctor-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_executable(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).unwrap();
        }
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    fn write_login_path_rewrite_profiles(home: &Path, path: &Path) {
        let profile = format!(
            "export PATH={}\n",
            shell_quote(&format!("{}:/usr/bin:/bin", path.to_string_lossy())),
        );
        std::fs::write(home.join(".zprofile"), &profile).unwrap();
        std::fs::write(home.join(".bash_profile"), profile).unwrap();
    }

    #[test]
    fn timeout_diagnostic_ids_are_collision_safe() {
        let checks = timeout_diagnostic_checks(vec![
            timeout("A B", "c"),
            timeout("A", "B C"),
            timeout("A B", "c"),
        ]);

        assert_eq!(checks.len(), 2, "exact duplicate timeout should be deduped");
        assert_eq!(checks[0].id, "subprocess-timeout-a-b-c");
        assert!(
            checks[1].id.starts_with("subprocess-timeout-a-b-c-"),
            "slug collision should get deterministic suffix: {:?}",
            checks.iter().map(|c| &c.id).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn apply_freshness_timeouts_appends_one_combined_block() {
        let mut check = DoctorCheck {
            status: CheckStatus::Pass,
            message: "Installed".into(),
            raw_output: Some("base raw".into()),
            ..empty_check("ai-agent-test", "Test Agent")
        };
        let timeouts = [
            timeout("installed version", "agent --version"),
            timeout("npm latest version", "npm view agent-acp version"),
        ];

        apply_freshness_timeouts(&mut check, &timeouts);

        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.message.contains("2 freshness probes timed out"));
        let raw = check.raw_output.unwrap();
        assert_eq!(raw.matches("# Freshness subprocess timeouts:").count(), 1);
        assert!(raw.contains("$ agent --version"));
        assert!(raw.contains("$ npm view agent-acp version"));
    }

    /// Streaming helper must invoke `on_line` for each output line of a
    /// successful command. Lines from `.zshrc` etc. may also appear; we only
    /// assert that our expected payload showed up.
    #[tokio::test]
    async fn run_command_streaming_emits_each_stdout_line() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = lines.clone();

        let result = run_command_streaming(
            "echo doctor-streaming-marker-hello && echo doctor-streaming-marker-world".to_string(),
            None,
            None,
            FixTimeout::Standard,
            move |line| {
                lines_clone.lock().unwrap().push(line.to_string());
            },
        )
        .await;

        assert!(result.is_ok(), "streaming command failed: {result:?}");
        let captured = lines.lock().unwrap().clone();
        assert!(
            captured
                .iter()
                .any(|l| l == "doctor-streaming-marker-hello"),
            "did not see 'hello' marker; captured: {captured:?}",
        );
        assert!(
            captured
                .iter()
                .any(|l| l == "doctor-streaming-marker-world"),
            "did not see 'world' marker; captured: {captured:?}",
        );
    }

    /// A line sent through the `FixStdin` pipe must reach the child's stdin
    /// and dropping the last writer must deliver EOF: `cat` echoes the line
    /// and exits 0 only when its stdin closes. Sending before the child
    /// spawns also exercises the pre-spawn buffering guarantee.
    #[tokio::test]
    async fn run_command_streaming_piped_stdin_round_trips_through_cat() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = lines.clone();
        let (writer, stdin) = FixStdin::pipe();

        writer.send_line("doctor-stdin-marker-echo").unwrap();
        drop(writer);

        let result = run_command_streaming(
            "cat".to_string(),
            None,
            Some(stdin),
            FixTimeout::Standard,
            move |line| {
                lines_clone.lock().unwrap().push(line.to_string());
            },
        )
        .await;

        assert!(result.is_ok(), "cat should exit 0 on EOF; got {result:?}");
        let captured = lines.lock().unwrap().clone();
        assert!(
            captured.iter().any(|l| l == "doctor-stdin-marker-echo"),
            "cat should echo the line written to its piped stdin; captured: {captured:?}",
        );
    }

    /// The paste-an-auth-code shape: the command prompts by blocking on a line
    /// read, and the caller feeds the answer through the writer while the fix is
    /// running. Sending from inside `on_line` — on the fix's own thread, in
    /// response to the prompt the fix printed — pins the send to a moment when
    /// the pipe is provably live, so the `Ok` asserted here is the delivery
    /// guarantee and not the pre-spawn queueing one.
    #[tokio::test]
    async fn run_command_streaming_piped_stdin_feeds_prompt_style_read() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = lines.clone();
        let live_send: Arc<Mutex<Option<Result<(), String>>>> = Arc::new(Mutex::new(None));
        let live_send_clone = live_send.clone();
        let (writer, stdin) = FixStdin::pipe();

        let result = run_command_streaming(
            "echo doctor-stdin-prompt; read -r line && echo \"got-$line\"".to_string(),
            None,
            Some(stdin),
            FixTimeout::Standard,
            move |line| {
                lines_clone.lock().unwrap().push(line.to_string());
                if line == "doctor-stdin-prompt" {
                    *live_send_clone.lock().unwrap() =
                        Some(writer.send_line("doctor-stdin-auth-code"));
                }
            },
        )
        .await;

        assert!(result.is_ok(), "read/echo should exit 0; got {result:?}");
        let captured = lines.lock().unwrap().clone();
        let sent = live_send
            .lock()
            .unwrap()
            .take()
            .expect("the fix's prompt line should have reached on_line");
        assert!(
            sent.is_ok(),
            "a send while the fix is live should report delivery; got {sent:?}",
        );
        assert!(
            captured.iter().any(|l| l == "got-doctor-stdin-auth-code"),
            "prompt-style read should see the sent line; captured: {captured:?}",
        );
    }

    /// A writer held across the fix's completion must not hang the run, and the
    /// *first* send after it must fail: the runner closes the pipe as it returns,
    /// so `Ok` never means "queued for a fix that is already over". That is the
    /// berd#99 shape — the login subprocess dies, the user pastes the auth code
    /// a beat later — and a host keying off `Ok` would otherwise wait forever
    /// with nothing in the log to explain it.
    #[tokio::test]
    async fn run_command_streaming_piped_stdin_rejects_sends_once_the_fix_finishes() {
        let (writer, stdin) = FixStdin::pipe();

        let result = run_command_streaming(
            "echo doctor-stdin-done".to_string(),
            None,
            Some(stdin),
            FixTimeout::Standard,
            |_| {},
        )
        .await;

        assert!(result.is_ok(), "echo fix should complete; got {result:?}");
        let err = writer
            .send_line("late-line")
            .expect_err("the first send after the fix finished should fail");
        assert!(
            err.contains("no longer accepting input"),
            "error should say the input is closed; got {err:?}",
        );
    }

    /// `EPIPE` alone can't carry "the fix is over": a backgrounded grandchild
    /// inherits the child's stdin and keeps the read end open, so a write into a
    /// finished fix's pipe still succeeds. Only the runner's explicit close on
    /// the way out makes this send fail. The grandchild's stdout is redirected so
    /// it doesn't also hold the reader threads open — this test is about stdin.
    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_streaming_piped_stdin_rejects_sends_when_a_grandchild_holds_the_pipe() {
        let (writer, stdin) = FixStdin::pipe();

        let result = run_command_streaming(
            "sleep 2 >/dev/null 2>&1 & echo doctor-stdin-done".to_string(),
            None,
            Some(stdin),
            FixTimeout::Standard,
            |_| {},
        )
        .await;

        assert!(result.is_ok(), "echo fix should complete; got {result:?}");
        assert!(
            writer.send_line("late-line").is_err(),
            "a grandchild holding the read end must not make a dead fix look writable",
        );
    }

    /// Reusing a `FixStdin` (or a clone) for a second execution must fail
    /// loudly rather than hand the child an immediately-EOF'd stdin — the
    /// receiver lives with the first run, so a second could only hang. The
    /// second run must also never spawn: nothing reaches `on_line`.
    #[tokio::test]
    async fn run_command_streaming_piped_stdin_errors_when_reused() {
        let (writer, stdin) = FixStdin::pipe();
        let reused = stdin.clone();
        writer.send_line("doctor-stdin-reuse-first").unwrap();
        drop(writer);

        let first = run_command_streaming(
            "cat".to_string(),
            None,
            Some(stdin),
            FixTimeout::Standard,
            |_| {},
        )
        .await;
        assert!(first.is_ok(), "first run should succeed; got {first:?}");

        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = lines.clone();
        let second = run_command_streaming(
            "echo doctor-stdin-reuse-second".to_string(),
            None,
            Some(reused),
            FixTimeout::Standard,
            move |line| lines_clone.lock().unwrap().push(line.to_string()),
        )
        .await;

        let err = second.expect_err("reusing a consumed FixStdin should fail");
        let captured = lines.lock().unwrap().clone();
        assert!(
            err.contains("already consumed"),
            "error should name the reuse; got {err:?}",
        );
        assert!(
            captured.is_empty(),
            "second run must not spawn; captured: {captured:?}",
        );
    }

    /// The default bound must stay at fix scale, not probe scale. A fix is an
    /// `npm install -g` behind a corporate proxy or a human doing SSO in a
    /// browser; retuning this toward `DEFAULT_PROBE_TIMEOUT` would kill honest
    /// work mid-flight.
    #[test]
    fn default_fix_timeout_stays_at_fix_scale() {
        assert_eq!(DEFAULT_FIX_TIMEOUT, Duration::from_secs(600));
        assert_eq!(ExecuteFixOptions::default().timeout, FixTimeout::Standard);
        assert_eq!(FixTimeout::Standard.duration(), Some(DEFAULT_FIX_TIMEOUT));
        assert_eq!(FixTimeout::Unbounded.duration(), None);
        assert!(
            DEFAULT_FIX_TIMEOUT >= DEFAULT_PROBE_TIMEOUT * 30,
            "fix timeout must stay far above probe scale",
        );
    }

    /// A fix that never finishes must return on its deadline instead of
    /// pinning the blocking worker forever — the whole point of the bound.
    #[tokio::test]
    async fn run_command_streaming_returns_when_the_fix_outlives_its_timeout() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = lines.clone();
        let started = Instant::now();

        let result = run_command_streaming(
            "sleep 60".to_string(),
            None,
            None,
            FixTimeout::After(Duration::from_millis(100)),
            move |line| lines_clone.lock().unwrap().push(line.to_string()),
        )
        .await;

        let err = result.expect_err("a fix past its deadline should fail");
        assert!(
            err.contains("timed out") && err.contains("sleep 60"),
            "error should name the timeout and the command; got {err:?}",
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timeout path waited for the fix instead of its deadline",
        );
        let captured = lines.lock().unwrap().clone();
        assert!(
            captured
                .iter()
                .any(|l| l.starts_with("doctor: fix timed out")),
            "callers should see a notice line explaining the stop; captured: {captured:?}",
        );
    }

    /// With piped stdin the shell leads its own process group, so the timeout
    /// kill must take the whole tree — not just the login shell, leaving a
    /// backgrounded installer running.
    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_streaming_timeout_kills_the_whole_process_tree() {
        let tmp = unique_tmp_dir("fix-timeout-tree");
        let marker = tmp.join("grandchild-ran");
        let (_writer, stdin) = FixStdin::pipe();

        let result = run_command_streaming(
            format!("(sleep 2; touch {}) & sleep 60", marker.display()),
            None,
            Some(stdin),
            FixTimeout::After(Duration::from_millis(300)),
            |_| {},
        )
        .await;

        assert!(result.is_err(), "timed-out fix should fail; got {result:?}");
        // Past when the backgrounded grandchild would have written its marker
        // had it survived the group kill.
        tokio::time::sleep(Duration::from_secs(3)).await;
        let survived = marker.exists();
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(
            !survived,
            "backgrounded grandchild outlived the timeout kill",
        );
    }

    /// A descendant that escaped the process group keeps the inherited
    /// stdout/stderr open, so the reader threads never see EOF. The timeout
    /// path must not join them — it must return on the deadline regardless
    /// (the streaming twin of `command_runner_returns_when_escaped_descendant_
    /// keeps_pipes_open`).
    #[cfg(unix)]
    #[tokio::test]
    async fn run_command_streaming_timeout_returns_when_escaped_descendant_keeps_pipes_open() {
        let started = Instant::now();

        let result = run_command_streaming(
            "perl -MPOSIX=setsid -e 'setsid(); sleep 5' & wait".to_string(),
            None,
            None,
            FixTimeout::After(Duration::from_millis(250)),
            |_| {},
        )
        .await;

        let err = result.expect_err("a fix past its deadline should fail");
        assert!(
            err.contains("timed out"),
            "error should name the timeout; got {err:?}",
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timeout path waited for the escaped descendant to close the pipes",
        );
    }

    /// `execute_fix(|_| {})` and `execute_fix_streaming(.., |_| {})` must
    /// produce identical results for the same fix lookup — `execute_fix` is
    /// supposed to be a thin delegate.
    #[tokio::test]
    async fn execute_fix_delegates_to_streaming() {
        let direct = execute_fix("ai-agent-nonexistent".to_string(), FixType::Auth).await;
        let streamed =
            execute_fix_streaming("ai-agent-nonexistent".to_string(), FixType::Auth, |_| {}).await;
        assert_eq!(direct, streamed);
    }

    /// A command not found by the shell must surface as `Exit { code: 127, .. }`
    /// — the auth probe maps that to `AuthStatus::Unknown` instead of
    /// `NotAuthenticated`. Using an unambiguously-nonexistent command name so
    /// the shell hits its "command not found" path regardless of rc files.
    #[tokio::test]
    async fn exec_command_not_found_reports_exit_127() {
        let outcome = tokio::task::spawn_blocking(|| {
            execute_command_with_path_prefix_with_env("doctor-nonexistent-xyz-12345", &[], None)
        })
        .await
        .unwrap();
        match outcome {
            ExecOutcome::Exit { code, .. } => assert_eq!(
                code,
                Some(127),
                "expected exit 127 for command-not-found; got {code:?}",
            ),
            other => panic!("expected Exit(127); got {other:?}"),
        }
    }

    /// PATH prefix lets the spawned shell find a command that's only in the
    /// supplied dir — without it, the same command would exit 127. This is the
    /// PATH-shadowing fix in miniature.
    #[tokio::test]
    async fn exec_command_with_path_prefix_finds_command_in_prefix_dir() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = std::env::temp_dir().join(format!(
            "doctor-pathprefix-{}-{}",
            std::process::id(),
            // Coarse uniqueness — enough for this test.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        // Distinct, unguessable name so no real PATH entry could shadow it.
        let script_name = "doctor-pathprefix-probe-abcdef";
        let script = tmp.join(script_name);
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let prefix = vec![tmp.clone()];
        let outcome = tokio::task::spawn_blocking(move || {
            execute_command_with_path_prefix_with_env(script_name, &prefix, None)
        })
        .await
        .unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
        match outcome {
            ExecOutcome::Ok => {}
            other => {
                panic!("expected Ok with the script reachable via path prefix; got {other:?}",)
            }
        }
    }

    #[tokio::test]
    async fn exec_command_with_path_prefix_merges_env_snapshot() {
        let tmp = unique_tmp_dir("auth-env-merge");
        let script_name = "doctor-auth-env-probe";
        let script = tmp.join(script_name);
        write_executable(
            &script,
            "#!/bin/sh\n\
             test \"$DOCTOR_AUTH_MARKER\" = yes\n",
        );
        let env = DoctorEnv::new(vec![
            ("DOCTOR_AUTH_MARKER".to_string(), "yes".to_string()),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("HOME".to_string(), tmp.to_string_lossy().to_string()),
            ("USER".to_string(), "doctor-test".to_string()),
        ]);
        let prefix = vec![tmp.clone()];

        let outcome = tokio::task::spawn_blocking(move || {
            execute_command_with_path_prefix_with_env(script_name, &prefix, Some(&env))
        })
        .await
        .unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
        match outcome {
            ExecOutcome::Ok => {}
            other => panic!("expected Ok with prefix PATH and env marker merged; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn exec_command_with_env_snapshot_restores_path_after_login_shell_rewrite() {
        let tmp = unique_tmp_dir("auth-env-path-login-rewrite");
        let snapshot_bin = tmp.join("nvm/bin");
        let login_rewrite_bin = tmp.join("homebrew/bin");
        let script_name = "doctor-auth-path-probe";
        write_executable(&snapshot_bin.join(script_name), "#!/bin/sh\nexit 0\n");
        write_executable(&login_rewrite_bin.join(script_name), "#!/bin/sh\nexit 42\n");
        write_login_path_rewrite_profiles(&tmp, &login_rewrite_bin);

        let env = DoctorEnv::new(vec![
            (
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin", snapshot_bin.to_string_lossy()),
            ),
            ("HOME".to_string(), tmp.to_string_lossy().to_string()),
            ("USER".to_string(), "doctor-test".to_string()),
            ("ZDOTDIR".to_string(), tmp.to_string_lossy().to_string()),
        ]);

        let outcome = tokio::task::spawn_blocking(move || {
            execute_command_with_path_prefix_with_env(script_name, &[], Some(&env))
        })
        .await
        .unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
        match outcome {
            ExecOutcome::Ok => {}
            other => {
                panic!("expected snapshot PATH binary to beat login profile rewrite; got {other:?}")
            }
        }
    }

    /// `apply_freshness` derives a source-aware update command for a Main slot
    /// when the npm-installed agent has an actionable update.
    #[tokio::test]
    async fn apply_freshness_npm_main_emits_update_main_command() {
        let mut readout = AgentVersionInfo {
            install_source: Some(InstallSource::Npm),
            ..AgentVersionInfo::default()
        };
        let info = freshness::VersionInfo {
            installed: Some("0.1.0".into()),
            latest: Some("0.2.0".into()),
            update_available: Some(true),
            command_timeouts: Vec::new(),
        };
        apply_freshness(
            &mut readout,
            &info,
            ReadoutSlot::Main,
            Some("@agentclientprotocol/claude-agent-acp"),
        );
        assert_eq!(
            readout.update_command.as_deref(),
            Some("npm install -g @agentclientprotocol/claude-agent-acp@latest"),
        );
        assert_eq!(readout.update_fix_type, Some(FixType::UpdateMain));
    }

    /// A brew-installed main CLI updates via `brew upgrade <pkg>`.
    #[tokio::test]
    async fn apply_freshness_brew_main_emits_update_main_command() {
        let mut readout = AgentVersionInfo {
            install_source: Some(InstallSource::Brew),
            ..AgentVersionInfo::default()
        };
        let info = freshness::VersionInfo {
            installed: Some("0.1.0".into()),
            latest: Some("0.2.0".into()),
            update_available: Some(true),
            command_timeouts: Vec::new(),
        };
        apply_freshness(&mut readout, &info, ReadoutSlot::Main, Some("ampcode"));
        assert_eq!(
            readout.update_command.as_deref(),
            Some("brew upgrade ampcode"),
        );
        assert_eq!(readout.update_fix_type, Some(FixType::UpdateMain));
    }

    /// Bundled readouts report the probed version but never an update nag or
    /// command — the binary is pinned by the embedding app's lock and updates
    /// ship with the app.
    #[tokio::test]
    async fn apply_freshness_bundled_suppresses_update() {
        let mut readout = AgentVersionInfo {
            bundled: Some(true),
            install_source: Some(InstallSource::Bundled),
            ..AgentVersionInfo::default()
        };
        let info = freshness::VersionInfo {
            installed: Some("2.1.205".into()),
            latest: Some("2.2.0".into()),
            update_available: Some(true),
            command_timeouts: Vec::new(),
        };
        apply_freshness(&mut readout, &info, ReadoutSlot::Main, None);
        assert_eq!(readout.installed_version.as_deref(), Some("2.1.205"));
        assert!(readout.update_available.is_none());
        assert!(readout.update_command.is_none());
        assert!(readout.update_fix_type.is_none());
    }

    /// Binaries resolved from inside the bundled tools dir are re-labeled
    /// `Bundled`; everything else keeps its detected source.
    #[test]
    fn apply_bundled_install_source_relabels_only_bundled_paths() {
        let mut resolved: HashMap<&str, ResolvedBinary> = HashMap::new();
        resolved.insert(
            "codex-acp",
            ResolvedBinary {
                path: Some(PathBuf::from("/bundle/resources/acp/bin/codex-acp")),
                search_output: String::new(),
                install_source: Some(InstallSource::Unknown),
            },
        );
        resolved.insert(
            "pi-acp",
            ResolvedBinary {
                path: Some(PathBuf::from("/Users/me/.npm-global/bin/pi-acp")),
                search_output: String::new(),
                install_source: Some(InstallSource::Npm),
            },
        );
        resolved.insert(
            "goose",
            ResolvedBinary {
                path: None,
                search_output: String::new(),
                install_source: None,
            },
        );

        apply_bundled_install_source(&mut resolved, Path::new("/bundle/resources/acp/bin"));

        assert_eq!(
            resolved["codex-acp"].install_source,
            Some(InstallSource::Bundled),
        );
        assert_eq!(resolved["pi-acp"].install_source, Some(InstallSource::Npm));
        assert_eq!(resolved["goose"].install_source, None);
    }

    /// Bridge slot with a brew install upgrades via `brew upgrade <pkg>` and
    /// is tagged `UpdateBridge`.
    #[tokio::test]
    async fn apply_freshness_brew_bridge_emits_update_bridge_command() {
        let mut readout = AgentVersionInfo {
            install_source: Some(InstallSource::Brew),
            ..AgentVersionInfo::default()
        };
        let info = freshness::VersionInfo {
            installed: Some("0.1.0".into()),
            latest: Some("0.2.0".into()),
            update_available: Some(true),
            command_timeouts: Vec::new(),
        };
        apply_freshness(&mut readout, &info, ReadoutSlot::Bridge, Some("amp"));
        assert_eq!(readout.update_command.as_deref(), Some("brew upgrade amp"),);
        assert_eq!(readout.update_fix_type, Some(FixType::UpdateBridge));
    }

    /// Self-updating (CurlPipe) readouts never get an update command, even when
    /// upstream reports a newer version — `is_self_updating` suppresses both
    /// `update_available` and the derived update command.
    #[tokio::test]
    async fn apply_freshness_curl_pipe_never_emits_update_command() {
        let mut readout = AgentVersionInfo {
            install_source: Some(InstallSource::CurlPipe),
            ..AgentVersionInfo::default()
        };
        let info = freshness::VersionInfo {
            installed: Some("1.0.0".into()),
            latest: Some("2.0.0".into()),
            update_available: Some(true),
            command_timeouts: Vec::new(),
        };
        apply_freshness(
            &mut readout,
            &info,
            ReadoutSlot::Main,
            Some("getcursor/cursor"),
        );
        assert!(readout.update_command.is_none());
        assert!(readout.update_fix_type.is_none());
        assert_eq!(readout.self_updating, Some(true));
        assert!(
            readout.update_available.is_none(),
            "self-updating readout suppresses update_available too",
        );
    }

    /// No update available -> no update command, even on a registry source.
    #[tokio::test]
    async fn apply_freshness_no_update_available_emits_no_command() {
        let mut readout = AgentVersionInfo {
            install_source: Some(InstallSource::Npm),
            ..AgentVersionInfo::default()
        };
        let info = freshness::VersionInfo {
            installed: Some("0.2.0".into()),
            latest: Some("0.2.0".into()),
            update_available: Some(false),
            command_timeouts: Vec::new(),
        };
        apply_freshness(&mut readout, &info, ReadoutSlot::Main, Some("amp-acp"));
        assert!(readout.update_command.is_none());
        assert!(readout.update_fix_type.is_none());
    }

    /// `command_override` makes the executor run the exact string supplied,
    /// bypassing `lookup_fix_command`. We test by overriding for the
    /// `UpdateMain` variant — which has no static lookup — so a successful
    /// run can only be the override path.
    #[tokio::test]
    async fn execute_fix_with_options_runs_command_override() {
        let result = execute_fix_with_options(
            "ai-agent-claude".to_string(),
            FixType::UpdateMain,
            Some("true".to_string()),
            None,
        )
        .await;
        assert!(
            result.is_ok(),
            "override should execute `true` and exit 0; got {result:?}",
        );
    }

    /// The streaming executor must emit a `$ <resolved-command>` preamble line
    /// through the callback *before* any subprocess output, so downstream
    /// callers record what ran. Exercises the `command_override` branch
    /// (the codepath `UpdateMain`/`UpdateBridge` clicks take, since they have
    /// no static recipe).
    #[tokio::test]
    async fn execute_fix_streaming_emits_command_preamble_first() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = lines.clone();

        let result = execute_fix_streaming_with_options(
            "ai-agent-claude".to_string(),
            FixType::UpdateMain,
            Some("echo hello".to_string()),
            None,
            move |line| {
                lines_clone.lock().unwrap().push(line.to_string());
            },
        )
        .await;

        assert!(result.is_ok(), "override should execute; got {result:?}");
        let captured = lines.lock().unwrap().clone();
        assert_eq!(
            captured.first().map(String::as_str),
            Some("$ echo hello"),
            "first callback line must be the command preamble; captured: {captured:?}",
        );
        assert!(
            captured.iter().any(|l| l == "hello"),
            "subprocess output line should follow the preamble; captured: {captured:?}",
        );
    }

    #[tokio::test]
    async fn execute_fix_streaming_with_env_options_uses_snapshot_path() {
        let tmp = unique_tmp_dir("fix-env-path");
        let script_name = "doctor-fix-env-probe";
        let script = tmp.join(script_name);
        write_executable(
            &script,
            "#!/bin/sh\n\
             test \"$DOCTOR_FIX_MARKER\" = yes || exit 42\n\
             echo fix-env-ok\n",
        );
        let mut path = tmp.to_string_lossy().to_string();
        if let Ok(existing) = std::env::var("PATH") {
            path.push(':');
            path.push_str(&existing);
        }
        let env = DoctorEnv::new(vec![
            ("DOCTOR_FIX_MARKER".to_string(), "yes".to_string()),
            ("PATH".to_string(), path),
            ("HOME".to_string(), tmp.to_string_lossy().to_string()),
            ("USER".to_string(), "doctor-test".to_string()),
            ("ZDOTDIR".to_string(), tmp.to_string_lossy().to_string()),
        ]);
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = lines.clone();

        let result = execute_fix_streaming_with_env_options(
            "ai-agent-claude".to_string(),
            FixType::UpdateMain,
            ExecuteFixOptions {
                command_override: Some(script_name.to_string()),
                env: Some(env),
                ..Default::default()
            },
            move |line| {
                lines_clone.lock().unwrap().push(line.to_string());
            },
        )
        .await;

        let captured = lines.lock().unwrap().clone();
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_ok(), "snapshot PATH command failed: {result:?}");
        assert!(
            captured.iter().any(|line| line == "fix-env-ok"),
            "expected command output from snapshot PATH script; captured: {captured:?}",
        );
    }

    #[tokio::test]
    async fn execute_fix_streaming_update_restores_snapshot_path_after_login_shell_rewrite() {
        let tmp = unique_tmp_dir("fix-update-env-path-login-rewrite");
        let snapshot_bin = tmp.join("nvm/bin");
        let login_rewrite_bin = tmp.join("homebrew/bin");
        write_executable(
            &snapshot_bin.join("npm"),
            "#!/bin/sh\n\
             echo \"snapshot-npm $*\"\n",
        );
        write_executable(
            &login_rewrite_bin.join("npm"),
            "#!/bin/sh\n\
             echo \"homebrew-npm $*\"\n\
             exit 42\n",
        );
        write_login_path_rewrite_profiles(&tmp, &login_rewrite_bin);

        let env = DoctorEnv::new(vec![
            (
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin", snapshot_bin.to_string_lossy()),
            ),
            ("HOME".to_string(), tmp.to_string_lossy().to_string()),
            ("USER".to_string(), "doctor-test".to_string()),
            ("ZDOTDIR".to_string(), tmp.to_string_lossy().to_string()),
        ]);
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = lines.clone();
        let command = "npm install -g @agentclientprotocol/claude-agent-acp@latest";

        let result = execute_fix_streaming_with_env_options(
            "ai-agent-claude".to_string(),
            FixType::UpdateMain,
            ExecuteFixOptions {
                command_override: Some(command.to_string()),
                env: Some(env),
                ..Default::default()
            },
            move |line| {
                lines_clone.lock().unwrap().push(line.to_string());
            },
        )
        .await;

        let captured = lines.lock().unwrap().clone();
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_ok(), "snapshot npm update failed: {result:?}");
        assert!(
            captured.iter().any(|line| line
                == "snapshot-npm install -g @agentclientprotocol/claude-agent-acp@latest"),
            "expected update to run through snapshot npm; captured: {captured:?}",
        );
        assert!(
            captured
                .iter()
                .all(|line| !line.starts_with("homebrew-npm")),
            "login profile PATH rewrite should not select homebrew npm; captured: {captured:?}",
        );
    }

    /// Without a `command_override`, `UpdateMain` has no static recipe and
    /// must surface as the standard "Unknown check / fix type" error.
    #[tokio::test]
    async fn execute_fix_with_options_update_without_override_errors() {
        let result = execute_fix_with_options(
            "ai-agent-claude".to_string(),
            FixType::UpdateMain,
            None,
            None,
        )
        .await;
        assert!(result.is_err());
    }

    /// Default `run_checks()` must not populate any of the version-freshness
    /// fields — guards against accidentally flipping the default to on
    /// (which would slow down every staged Tauri app launch).
    #[tokio::test]
    async fn run_checks_default_leaves_freshness_fields_empty() {
        let report = run_checks().await;
        for check in &report.checks {
            assert!(
                check.installed_version.is_none(),
                "check {} unexpectedly populated installed_version = {:?}",
                check.id,
                check.installed_version,
            );
            assert!(
                check.latest_version.is_none(),
                "check {} unexpectedly populated latest_version = {:?}",
                check.id,
                check.latest_version,
            );
            assert!(
                check.update_available.is_none(),
                "check {} unexpectedly populated update_available = {:?}",
                check.id,
                check.update_available,
            );
            // The split main/bridge readouts may carry an install source on the
            // cheap path, but never version fields — those are freshness-only.
            for (slot, readout) in [("main", &check.main), ("bridge", &check.bridge)] {
                if let Some(r) = readout {
                    assert!(
                        r.installed_version.is_none()
                            && r.latest_version.is_none()
                            && r.update_available.is_none()
                            && r.self_updating.is_none(),
                        "check {} {slot} readout unexpectedly populated version fields: {r:?}",
                        check.id,
                    );
                }
            }
        }
    }
}
