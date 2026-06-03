//! Health Check ("Doctor") — backend checks for external dependencies.
//!
//! Each check probes a single external dependency and returns a status
//! (pass / warn / fail) with a human-readable summary and an optional
//! URL the user can visit to install or configure the dependency.

pub mod agents;
pub mod checks;
pub(crate) mod freshness;
pub(crate) mod package_ids;
pub mod resolve;
pub mod types;

pub use types::{CheckStatus, DoctorCheck, DoctorReport, FixType};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agents::{check_single_ai_agent, lookup_fix_command, AI_AGENT_CHECKS};
use checks::{check_clonefile, check_gh, check_gh_auth, check_git, check_git_lfs};
use freshness::{
    fetch_version_info, is_self_updating, load_cache, save_cache, select_installed_probe,
};
use package_ids::{lookup_package_id, LatestSource};
use resolve::resolve_binary;
use types::{AgentVersionInfo, InstallSource, ResolvedBinary};

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
}

/// Run all health checks and return the report. Equivalent to
/// `run_checks_with_options(RunChecksOptions::default())`.
pub async fn run_checks() -> DoctorReport {
    run_checks_with_options(RunChecksOptions::default()).await
}

/// Run all health checks with explicit options. Existing callers that want
/// the cheap, no-network path should keep using [`run_checks`].
pub async fn run_checks_with_options(opts: RunChecksOptions) -> DoctorReport {
    let npm_registry = opts.npm_registry.as_deref();
    let report = collect_base_report(npm_registry).await;

    if opts.check_freshness {
        populate_freshness(report, opts.offline, npm_registry).await
    } else {
        report
    }
}

async fn collect_base_report(npm_registry: Option<&str>) -> DoctorReport {
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
        .map(|&name| tokio::task::spawn_blocking(move || (name, resolve_binary(name))))
        .collect();

    let mut resolved: HashMap<&str, ResolvedBinary> = HashMap::new();
    for handle in handles {
        if let Ok((name, rb)) = handle.await {
            resolved.insert(name, rb);
        }
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
    let git_r2 = r_git.clone();
    let git_lfs_r = r_git_lfs;
    let git_r3 = r_git;

    let c_git = tokio::task::spawn_blocking(move || check_git(&git_r));
    let c_gh = tokio::task::spawn_blocking(move || check_gh(&gh_r));
    let c_gh_auth = tokio::task::spawn_blocking(move || check_gh_auth(&gh_r2));
    let c_git_lfs = tokio::task::spawn_blocking(move || check_git_lfs(&git_r2, &git_lfs_r));
    let c_clonefile = tokio::task::spawn_blocking(move || check_clonefile(&git_r3));

    let npm_registry_owned = npm_registry.map(|s| s.to_string());
    let agent_handles: Vec<_> = AI_AGENT_CHECKS
        .iter()
        .map(|info| {
            let found = any_agent_found;
            let registry = npm_registry_owned.clone();
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
                check_single_ai_agent(info, found, &cmds, main.as_ref(), registry.as_deref())
            })
        })
        .collect();

    let (c_git, c_gh, c_gh_auth, c_git_lfs, c_clonefile) =
        tokio::join!(c_git, c_gh, c_gh_auth, c_git_lfs, c_clonefile);

    let mut checks = vec![
        c_git.unwrap_or_else(|_| empty_check("git", "Git")),
        c_gh.unwrap_or_else(|_| empty_check("gh", "GitHub CLI")),
        c_gh_auth.unwrap_or_else(|_| empty_check("gh-auth", "GitHub Auth")),
        c_git_lfs.unwrap_or_else(|_| empty_check("git-lfs", "Git LFS")),
        c_clonefile.unwrap_or_else(|_| empty_check("git-clonefile", "Copy on Write Git Clones")),
    ];

    for (i, handle) in agent_handles.into_iter().enumerate() {
        let info = &AI_AGENT_CHECKS[i];
        checks.push(
            handle
                .await
                .unwrap_or_else(|_| empty_check(info.id, info.label)),
        );
    }

    DoctorReport { checks }
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
) -> (Option<String>, Option<LatestSource>) {
    source
        .cloned()
        .and_then(|src| lookup_package_id(check_id, src))
        .map(|(pkg, latest)| (Some(pkg.to_string()), Some(latest)))
        .unwrap_or((None, None))
}

/// Fold a freshness [`freshness::VersionInfo`] into a per-binary readout,
/// applying the self-updating suppression rule for `update_available`.
fn apply_freshness(readout: &mut AgentVersionInfo, info: &freshness::VersionInfo) {
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
) -> DoctorReport {
    let cache = Arc::new(Mutex::new(load_cache()));
    let npm_registry = npm_registry.map(|s| s.to_string());

    let mut targets: Vec<FreshnessTarget> = Vec::new();
    for check in &report.checks {
        let is_agent = check.main.is_some() || check.bridge.is_some();
        if is_agent {
            if let (Some(readout), Some(path)) = (&check.main, check.path.as_deref()) {
                let (package_id, latest_source) =
                    resolve_package(&check.id, readout.install_source.as_ref());
                targets.push(FreshnessTarget {
                    id: check.id.clone(),
                    slot: ReadoutSlot::Main,
                    path: PathBuf::from(path),
                    latest_source,
                    package_id,
                    install_source: readout.install_source.clone(),
                });
            }
            if let (Some(readout), Some(path)) = (&check.bridge, check.bridge_path.as_deref()) {
                let (package_id, latest_source) =
                    resolve_package(&check.id, readout.install_source.as_ref());
                targets.push(FreshnessTarget {
                    id: check.id.clone(),
                    slot: ReadoutSlot::Bridge,
                    path: PathBuf::from(path),
                    latest_source,
                    package_id,
                    install_source: readout.install_source.clone(),
                });
            }
        } else {
            // Non-agent check: prefer the bridge path when present (matches the
            // install_source the check carries — see check_single_ai_agent).
            let path_str = check.bridge_path.as_deref().or(check.path.as_deref());
            let Some(path_str) = path_str else { continue };
            let (package_id, latest_source) =
                resolve_package(&check.id, check.install_source.as_ref());
            targets.push(FreshnessTarget {
                id: check.id.clone(),
                slot: ReadoutSlot::Flat,
                path: PathBuf::from(path_str),
                latest_source,
                package_id,
                install_source: check.install_source.clone(),
            });
        }
    }

    let futures = targets.into_iter().map(|t| {
        let cache = cache.clone();
        let npm_registry = npm_registry.clone();
        async move {
            // npm-distributed bridges don't honor `--version`; read their
            // installed version straight from the owning `package.json`.
            let probe = select_installed_probe(t.install_source.as_ref(), t.package_id.as_deref());
            let info = fetch_version_info(
                t.latest_source,
                t.package_id.as_deref(),
                &t.path,
                probe,
                offline,
                npm_registry.as_deref(),
                cache,
            )
            .await;
            ((t.id, t.slot), info)
        }
    });

    let results = futures::future::join_all(futures).await;
    let mut by_target: HashMap<(String, ReadoutSlot), freshness::VersionInfo> = HashMap::new();
    for (key, info) in results {
        by_target.insert(key, info);
    }

    for check in &mut report.checks {
        let is_agent = check.main.is_some() || check.bridge.is_some();
        if is_agent {
            if let (Some(readout), Some(info)) = (
                check.main.as_mut(),
                by_target.remove(&(check.id.clone(), ReadoutSlot::Main)),
            ) {
                apply_freshness(readout, &info);
            }
            if let (Some(readout), Some(info)) = (
                check.bridge.as_mut(),
                by_target.remove(&(check.id.clone(), ReadoutSlot::Bridge)),
            ) {
                apply_freshness(readout, &info);
            }
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
        } else if let Some(info) = by_target.remove(&(check.id.clone(), ReadoutSlot::Flat)) {
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
}

/// Run a fix command for a doctor check, identified by check ID and fix type.
///
/// The actual shell command is looked up from the static check definitions —
/// the caller never sends a raw command string.
pub async fn execute_fix(check_id: String, fix_type: FixType) -> Result<(), String> {
    execute_fix_streaming(check_id, fix_type, |_| {}).await
}

/// Like [`execute_fix`], but routes npm-backed fix commands through an optional
/// caller-supplied registry (see [`RunChecksOptions::npm_registry`]). Callers
/// that displayed a registry-overridden `fix_command` should pass the same
/// `npm_registry` here so the executed command matches what was shown.
pub async fn execute_fix_with_options(
    check_id: String,
    fix_type: FixType,
    npm_registry: Option<&str>,
) -> Result<(), String> {
    execute_fix_streaming_with_options(check_id, fix_type, npm_registry, |_| {}).await
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
    execute_fix_streaming_with_options(check_id, fix_type, None, on_line).await
}

/// Like [`execute_fix_streaming`], but routes npm-backed fix commands through an
/// optional caller-supplied registry before shelling out (see
/// [`RunChecksOptions::npm_registry`]). Non-npm commands are run verbatim.
pub async fn execute_fix_streaming_with_options<F>(
    check_id: String,
    fix_type: FixType,
    npm_registry: Option<&str>,
    on_line: F,
) -> Result<(), String>
where
    F: FnMut(&str) + Send + 'static,
{
    let command = lookup_fix_command(&check_id, &fix_type)
        .ok_or_else(|| format!("Unknown check '{check_id}' or fix type '{fix_type:?}'"))?;
    let command = agents::apply_npm_registry(&command, npm_registry);

    run_command_streaming(command, on_line).await
}

/// Async wrapper that runs `run_command_streaming_blocking` on the blocking pool.
pub(crate) async fn run_command_streaming<F>(command: String, on_line: F) -> Result<(), String>
where
    F: FnMut(&str) + Send + 'static,
{
    tokio::task::spawn_blocking(move || run_command_streaming_blocking(&command, on_line))
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {e}")))
}

enum StreamLine {
    Stdout(String),
    Stderr(String),
}

/// Spawn `command` through a login shell, stream stdout/stderr lines to
/// `on_line`, and return based on the process exit status. Stderr lines are
/// also accumulated so a non-zero exit can surface a useful error message
/// (matching the non-streaming behavior of the previous `execute_command`).
fn run_command_streaming_blocking<F>(command: &str, mut on_line: F) -> Result<(), String>
where
    F: FnMut(&str),
{
    use std::io::{BufRead, BufReader};

    let (shell, args) = if std::path::Path::new("/bin/zsh").exists() {
        ("/bin/zsh", vec!["-l", "-c", command])
    } else {
        ("/bin/bash", vec!["-l", "-c", command])
    };
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let user = std::env::var("USER").unwrap_or_default();

    let mut child = std::process::Command::new(shell)
        .args(&args)
        .env_clear()
        .env("HOME", &home)
        .env("USER", &user)
        .env("TERM", "xterm-256color")
        .current_dir(&home)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run command: {e}"))?;

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

    let mut stderr_accum = String::new();
    for msg in rx.iter() {
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

    let _ = stdout_thread.join();
    let _ = stderr_thread.join();

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for command: {e}"))?;

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

/// Synchronous, non-streaming variant for callers running inside an existing
/// `spawn_blocking` closure (the auth probes in `check_single_ai_agent` use
/// this — they need a sync result, not an `on_line` stream).
pub(crate) fn execute_command_blocking(command: &str) -> Result<(), String> {
    run_command_streaming_blocking(command, |_| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    /// Streaming helper must invoke `on_line` for each output line of a
    /// successful command. Lines from `.zshrc` etc. may also appear; we only
    /// assert that our expected payload showed up.
    #[tokio::test]
    async fn run_command_streaming_emits_each_stdout_line() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = lines.clone();

        let result = run_command_streaming(
            "echo doctor-streaming-marker-hello && echo doctor-streaming-marker-world".to_string(),
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
