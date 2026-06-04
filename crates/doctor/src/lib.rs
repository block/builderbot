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

pub use types::{AgentVersionInfo, CheckStatus, DoctorCheck, DoctorReport, FixType};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agents::{check_single_ai_agent, derive_update_command, lookup_fix_command, AI_AGENT_CHECKS};
use checks::{check_clonefile, check_gh, check_gh_auth, check_git, check_git_lfs};
use freshness::{
    fetch_version_info, is_self_updating, load_cache, save_cache, select_installed_probe,
};
use package_ids::{lookup_package_id, LatestSource, Role};
use resolve::resolve_binary;
use types::{InstallSource, ResolvedBinary};

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
                    resolve_package(&check.id, readout.install_source.as_ref(), Role::Main);
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
                    resolve_package(&check.id, readout.install_source.as_ref(), Role::Bridge);
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
                resolve_package(&check.id, check.install_source.as_ref(), Role::Any);
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
            if let (Some(readout), Some((info, pkg))) = (
                check.main.as_mut(),
                by_target.remove(&(check.id.clone(), ReadoutSlot::Main)),
            ) {
                apply_freshness(readout, &info, ReadoutSlot::Main, pkg.as_deref());
            }
            if let (Some(readout), Some((info, pkg))) = (
                check.bridge.as_mut(),
                by_target.remove(&(check.id.clone(), ReadoutSlot::Bridge)),
            ) {
                apply_freshness(readout, &info, ReadoutSlot::Bridge, pkg.as_deref());
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
        } else if let Some((info, _pkg)) = by_target.remove(&(check.id.clone(), ReadoutSlot::Flat))
        {
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
    execute_fix_streaming_with_options(check_id, fix_type, command_override, npm_registry, |_| {})
        .await
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
    mut on_line: F,
) -> Result<(), String>
where
    F: FnMut(&str) + Send + 'static,
{
    let command = match command_override {
        Some(cmd) => cmd,
        None => lookup_fix_command(&check_id, &fix_type)
            .ok_or_else(|| format!("Unknown check '{check_id}' or fix type '{fix_type:?}'"))?,
    };
    let command = agents::apply_npm_registry(&command, npm_registry);

    // Echo the resolved command as a preamble line so downstream callers (e.g.
    // goose-internal's `run_fix`, which `info!`s every callback line and emits
    // it via the `agent-setup:output` Tauri event) record *what* ran, not just
    // its output. Matches the `$ <command>` phrasing the auth probe already
    // writes into `raw_output`.
    on_line(&format!("$ {command}"));

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

/// Build the login-shell `Command` used by every doctor exec path. Optionally
/// prepends `path_prefix` directories to the child `PATH` so a binary that's
/// only findable in the resolved location (e.g. an nvm bin dir) is visible to
/// the spawned shell even when the parent process was launched with a
/// restricted `PATH` (the macOS Finder/launchd case). When `path_prefix` is
/// empty the env layout is byte-identical to the previous implementation, so
/// existing call sites are unaffected.
fn build_shell_command(command: &str, path_prefix: &[PathBuf]) -> std::process::Command {
    let (shell, args) = if std::path::Path::new("/bin/zsh").exists() {
        ("/bin/zsh", vec!["-l", "-c", command])
    } else {
        ("/bin/bash", vec!["-l", "-c", command])
    };
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let user = std::env::var("USER").unwrap_or_default();

    let mut cmd = std::process::Command::new(shell);
    cmd.args(&args)
        .env_clear()
        .env("HOME", &home)
        .env("USER", &user)
        .env("TERM", "xterm-256color")
        .current_dir(&home);
    if !path_prefix.is_empty() {
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
        cmd.env("PATH", path);
    }
    cmd
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
    /// The shell ran, the inner command exited non-zero. Code is `Some(127)`
    /// when the shell reports "command not found".
    Exit {
        code: Option<i32>,
        stderr: String,
    },
}

/// Run `command` through a login shell, with the caller-supplied `path_prefix`
/// prepended to the child `PATH`. Returns the detailed exec outcome. No
/// streaming — used by the auth probe which wants a single sync result.
pub(crate) fn execute_command_with_path_prefix(
    command: &str,
    path_prefix: &[PathBuf],
) -> ExecOutcome {
    let mut cmd = build_shell_command(command, path_prefix);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return ExecOutcome::Spawn(e),
    };
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return ExecOutcome::Spawn(e),
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

/// Spawn `command` through a login shell, stream stdout/stderr lines to
/// `on_line`, and return based on the process exit status. Stderr lines are
/// also accumulated so a non-zero exit can surface a useful error message
/// (matching the non-streaming behavior of the previous `execute_command`).
fn run_command_streaming_blocking<F>(command: &str, mut on_line: F) -> Result<(), String>
where
    F: FnMut(&str),
{
    use std::io::{BufRead, BufReader};

    let mut child = build_shell_command(command, &[])
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

    /// A command not found by the shell must surface as `Exit { code: 127, .. }`
    /// — the auth probe maps that to `AuthStatus::Unknown` instead of
    /// `NotAuthenticated`. Using an unambiguously-nonexistent command name so
    /// the shell hits its "command not found" path regardless of rc files.
    #[tokio::test]
    async fn exec_command_not_found_reports_exit_127() {
        let outcome = tokio::task::spawn_blocking(|| {
            execute_command_with_path_prefix("doctor-nonexistent-xyz-12345", &[])
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
            execute_command_with_path_prefix(script_name, &prefix)
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
        };
        apply_freshness(
            &mut readout,
            &info,
            ReadoutSlot::Main,
            Some("@anthropic-ai/claude-code"),
        );
        assert_eq!(
            readout.update_command.as_deref(),
            Some("npm install -g @anthropic-ai/claude-code@latest"),
        );
        assert_eq!(readout.update_fix_type, Some(FixType::UpdateMain));
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
