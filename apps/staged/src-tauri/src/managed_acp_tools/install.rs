//! `install_managed_tool` — the floating npm install of one managed bridge:
//! stage, validate, swap into place, write the shim, record the version.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::io::AsyncBufReadExt;

use crate::managed_node;

use super::probe::snapshot_var;
use super::registry::{
    fallback_npm_registry, gateway_fallback_registry, gateway_retry_failed_message,
    npm_registry_args, npm_registry_override, registry_label,
};
use super::{
    managed_tool, read_state, shim_bin_dir, staging_install_dir, tool_install_dir, validation,
    write_state, InstalledToolPin, ManagedTool,
};

#[cfg(test)]
#[path = "tests/install.rs"]
mod tests;

// =============================================================================
// install_managed_tool — floating npm install + shim + state
// =============================================================================

/// Floating bridge installs download ~70-95 MB of packages through the
/// registry; a hung npm must not wedge the install mutex forever.
const NPM_INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug)]
pub enum ManagedToolError {
    DataDir(String),
    /// This id is not managed on this build/target; callers route before
    /// installing, so surfacing one means the managed set changed under a
    /// running operation.
    NotManaged(String),
    Node(managed_node::ManagedNodeError),
    NpmInstall(String),
    /// npm exited cleanly but the staged bridge failed install validation.
    Incomplete(String),
    Io(String),
}

impl std::fmt::Display for ManagedToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DataDir(message) => {
                write!(
                    f,
                    "failed to resolve the managed ACP tools directory: {message}"
                )
            }
            Self::NotManaged(message) => {
                write!(f, "not a Staged-managed ACP bridge: {message}")
            }
            Self::Node(error) => error.fmt(f),
            Self::NpmInstall(message) => write!(f, "npm install failed: {message}"),
            Self::Incomplete(message) => {
                write!(f, "installed ACP bridge is incomplete: {message}")
            }
            Self::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ManagedToolError {}

/// Receives every progress/output line of an install. Doctor fixes and the
/// startup reconciler feed these to the log with their own prefixes; there is
/// no streamed-output UI channel for installs.
pub type InstallLineFn<'a> = dyn Fn(&str) + Send + Sync + 'a;

pub(super) fn tool_install_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Install (or upgrade) one managed bridge to the latest published version:
/// ensure the managed Node runtime, run the floating `npm install
/// <pkg>@latest` into a scratch prefix, swap the verified tree into the live
/// prefix, write the absolute-path shim, and record the installed version in
/// `state.json`. Safe to call concurrently — doctor fixes and the startup
/// reconciler serialize on one process-wide install mutex, and mutations of
/// the shared packages tree additionally hold the cross-process flock (other
/// Staged instances reconcile the same `~/.staged/packages`). A failed
/// install — including one npm aborts mid-reify, or an upstream `@latest`
/// that drops the entrypoint — leaves any previously installed version fully
/// in place, since the live tree is only ever replaced by an atomic swap of a
/// verified staging tree; the Node runtime its shim execs is likewise kept,
/// as superseded runtimes are pruned only after a fully-successful reconcile.
/// So an offline or partial launch never removes a working bridge.
///
/// `env_vars` is the sanitized login-shell snapshot doctor's checks and the
/// freshness probe run under ([`crate::doctor::doctor_env_vars`]); npm runs
/// under exactly that, so the registry config it resolves (`~/.npmrc`,
/// `NPM_CONFIG_REGISTRY` from the shell rc) is the one the probe saw.
pub async fn install_managed_tool(
    id: &str,
    env_vars: &[(String, String)],
    on_line: &InstallLineFn<'_>,
) -> Result<(), ManagedToolError> {
    let tool = managed_tool(id).ok_or_else(|| {
        ManagedToolError::NotManaged(format!("'{id}' is not a Staged-managed ACP bridge"))
    })?;
    let packages_root = crate::paths::packages_dir()
        .ok_or_else(|| ManagedToolError::DataDir("home directory is unavailable".to_string()))?;
    let node_root = managed_node::managed_node_root()
        .ok_or_else(|| ManagedToolError::DataDir("home directory is unavailable".to_string()))?;
    let node_install_dir = managed_node::pinned_install_dir(&node_root).ok_or_else(|| {
        ManagedToolError::NotManaged("no managed Node.js runtime pin for this target".to_string())
    })?;

    let _guard = tool_install_lock().lock().await;
    managed_node::ensure_managed_node_runtime()
        .await
        .map_err(ManagedToolError::Node)?;
    // The cross-process lock is taken only after the runtime ensure has
    // released the same flock — nesting the two would deadlock (see
    // `lock_packages_dir`). In the unlocked window another process can at
    // most install or prune, and both leave the pinned runtime in place.
    let _packages_lock = managed_node::lock_packages_dir(&packages_root)
        .await
        .map_err(ManagedToolError::Node)?;
    install_npm_tool(
        &packages_root,
        &node_install_dir,
        &managed_node::node_runtime_lock().version,
        &tool,
        env_vars,
        npm_registry_override().as_deref(),
        fallback_npm_registry(),
        validation::VALIDATION_TIMEOUT,
        on_line,
    )
    .await
}

/// The install body, path-parameterized so tests drive it with a fixture
/// `npm`. Caller holds the install mutex + packages flock and has ensured
/// the runtime. npm runs under the `env_vars` snapshot; `registry` is the
/// explicit override (or `None` for npm's own config) and `fallback_registry`
/// the build-time fallback tried once if the first attempt fails as
/// gateway-blocked (see the registry policy note).
#[allow(clippy::too_many_arguments)]
pub(super) async fn install_npm_tool(
    packages_root: &Path,
    node_install_dir: &Path,
    node_version: &str,
    tool: &ManagedTool,
    env_vars: &[(String, String)],
    registry: Option<&str>,
    fallback_registry: Option<&str>,
    validation_timeout: Duration,
    on_line: &InstallLineFn<'_>,
) -> Result<(), ManagedToolError> {
    let install_dir = tool_install_dir(packages_root, tool.id);
    // Stage the floating install into a scratch prefix and swap it into the
    // live tree only after entrypoint and runtime validation pass. npm reifies in
    // place, so installing straight into `install_dir` would let a failure
    // mid-reify — or an upstream `@latest` that drops the entrypoint — replace
    // the live tree the (version-independent) shim already points at, breaking
    // the previously working bridge. Staging keeps the old tree untouched
    // until a verified new one is ready to swap in atomically.
    let staging_dir = staging_install_dir(packages_root, tool.id);
    reset_dir(&staging_dir)
        .map_err(|error| ManagedToolError::Io(format!("prepare staging dir: {error}")))?;

    on_line(&format!(
        "Installing {}@latest into ~/.staged/packages",
        tool.package
    ));
    if let Err(failure) = run_floating_npm_install(
        packages_root,
        node_install_dir,
        &staging_dir,
        tool,
        env_vars,
        registry,
        on_line,
    )
    .await
    {
        let Some(fallback) =
            gateway_fallback_registry(registry, fallback_registry, &failure.stderr)
        else {
            let _ = std::fs::remove_dir_all(&staging_dir);
            return Err(failure.error);
        };
        on_line(&format!(
            "npm could not reach registry.npmjs.org (network gateway block page); retrying through {fallback}"
        ));
        // A fresh scratch prefix: whatever the blocked attempt reified must
        // not seed the retry.
        if let Err(error) = reset_dir(&staging_dir) {
            let _ = std::fs::remove_dir_all(&staging_dir);
            return Err(ManagedToolError::Io(format!(
                "prepare staging dir for registry retry: {error}"
            )));
        }
        if let Err(failure) = run_floating_npm_install(
            packages_root,
            node_install_dir,
            &staging_dir,
            tool,
            env_vars,
            Some(fallback),
            on_line,
        )
        .await
        {
            let _ = std::fs::remove_dir_all(&staging_dir);
            // The retry's own error is an `NpmInstall` too; its bare message
            // goes inside so the prefix is not doubled.
            let retry_error = match &failure.error {
                ManagedToolError::NpmInstall(message) => message.clone(),
                other => other.to_string(),
            };
            return Err(ManagedToolError::NpmInstall(gateway_retry_failed_message(
                fallback,
                &retry_error,
            )));
        }
    }

    let staged_entrypoint = npm_entrypoint(&staging_dir, tool.package);
    if !staged_entrypoint.is_file() {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(ManagedToolError::Incomplete(format!(
            "{}: bridge entrypoint {} is missing after install",
            tool.package,
            staged_entrypoint.display()
        )));
    }
    on_line(&format!("Validating {} before promotion", tool.package));
    if let Err(error) =
        validation::validate_staged_tool(node_install_dir, &staging_dir, tool, validation_timeout)
            .await
    {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(error);
    }
    let version = installed_version(&staging_dir, tool.package).unwrap_or_default();

    // Atomically replace the live tree with the verified staging tree, keeping
    // the previous tree aside as `.old` to roll back to if the rename fails.
    swap_into_place(&staging_dir, &install_dir)
        .map_err(|error| ManagedToolError::Io(format!("install staged bridge: {error}")))?;

    let entrypoint = npm_entrypoint(&install_dir, tool.package);
    write_shim(
        &shim_bin_dir(packages_root),
        tool.binary,
        &shim_contents(&node_binary(node_install_dir), &entrypoint),
    )
    .map_err(|error| ManagedToolError::Io(format!("write bridge shim: {error}")))?;

    let mut state = read_state(packages_root);
    state.tools.insert(
        tool.id.to_string(),
        InstalledToolPin {
            binary: tool.binary.to_string(),
            version: version.clone(),
            node_version: node_version.to_string(),
        },
    );
    write_state(packages_root, &state)
        .map_err(|error| ManagedToolError::Io(format!("write state.json: {error}")))?;
    on_line(&format!(
        "{}@{} is ready",
        tool.package,
        if version.is_empty() {
            "latest"
        } else {
            version.as_str()
        }
    ));
    Ok(())
}

/// Remove `dir` if it exists, then recreate it empty — a clean scratch prefix
/// for a fresh floating install (a stale staging tree from a crashed run must
/// not seed the next one).
fn reset_dir(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)
}

/// Atomically replace `final_dir` with `staging_dir`: stage any previous tree
/// aside as `<final_dir>.old`, rename the verified staging tree into place,
/// and roll the previous tree back if that rename fails. Mirrors
/// `managed_node`'s runtime swap; both dirs are siblings under `tools/`, so
/// each rename is a single same-filesystem operation.
fn swap_into_place(staging_dir: &Path, final_dir: &Path) -> std::io::Result<()> {
    if let Some(parent) = final_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let old_dir = final_dir.with_extension("old");
    if old_dir.exists() {
        std::fs::remove_dir_all(&old_dir)?;
    }
    if final_dir.exists() {
        std::fs::rename(final_dir, &old_dir)?;
    }
    if let Err(error) = std::fs::rename(staging_dir, final_dir) {
        if old_dir.exists() {
            let _ = std::fs::rename(&old_dir, final_dir);
        }
        let _ = std::fs::remove_dir_all(staging_dir);
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(&old_dir);
    Ok(())
}

/// `<install-dir>/node_modules/<package>/dist/index.js` — the bridge
/// entrypoint convention both managed bridges follow.
pub(super) fn npm_entrypoint(install_dir: &Path, package: &str) -> PathBuf {
    package_dir(install_dir, package)
        .join("dist")
        .join("index.js")
}

pub(super) fn package_dir(install_dir: &Path, package: &str) -> PathBuf {
    package
        .split('/')
        .fold(install_dir.join("node_modules"), |dir, part| dir.join(part))
}

pub(super) fn node_binary(node_install_dir: &Path) -> PathBuf {
    node_install_dir.join("bin").join("node")
}

/// The version npm resolved for the just-installed package, from its
/// `package.json`. Best-effort: the state record is informational, so an
/// unreadable version does not fail the install.
pub(super) fn installed_version(install_dir: &Path, package: &str) -> Option<String> {
    let json =
        std::fs::read_to_string(package_dir(install_dir, package).join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&json).ok()?;
    value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// A failed floating install: the error to surface plus what npm wrote to
/// stderr, kept so the caller can classify the failure for a registry retry.
struct NpmInstallFailure {
    error: ManagedToolError,
    stderr: String,
}

/// How much of npm's stderr is kept for the retry classifier. npm writes its
/// `npm error code …` block *last*, after any deprecation and peer warnings,
/// so the capture keeps the newest lines (see [`push_captured_line`]) — a
/// warning-heavy install must not push the error block out of view.
const NPM_STDERR_CAPTURE_LIMIT: usize = 64 * 1024;

/// Append `line` to the capture, dropping whole lines from the *front* once
/// it outgrows [`NPM_STDERR_CAPTURE_LIMIT`] (a single oversized line keeps
/// its tail).
fn push_captured_line(capture: &mut String, line: &str) {
    capture.push_str(line);
    capture.push('\n');
    while capture.len() > NPM_STDERR_CAPTURE_LIMIT {
        match capture.find('\n') {
            Some(end) if end + 1 < capture.len() => {
                capture.drain(..=end);
            }
            _ => {
                let mut cut = capture.len() - NPM_STDERR_CAPTURE_LIMIT;
                while !capture.is_char_boundary(cut) {
                    cut += 1;
                }
                capture.drain(..cut);
            }
        }
    }
}

/// How many `npm error` detail lines after the `npm error code` line are
/// kept in the recorded error, and how long each may be.
const NPM_ERROR_SUMMARY_LINES: usize = 3;
const NPM_ERROR_SUMMARY_LINE_LIMIT: usize = 240;

/// npm's own account of why it failed, from its captured stderr: the last
/// `npm error code <CODE>` line and the first few `npm error` detail lines
/// after it (minus `errno`, which repeats the code, and the log-file pointer).
/// Appended to the recorded install error so `state.json` and Doctor can say
/// *why* — the exit status alone says nothing about a block page, a 404, or
/// an unreachable mirror. `None` when npm printed no error block.
fn npm_error_summary(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr.lines().map(str::trim_end).collect();
    let code_index = lines.iter().rposition(|line| {
        line.starts_with("npm error code ") || line.starts_with("npm ERR! code ")
    })?;
    let mut summary = vec![clip_line(lines[code_index])];
    let details = lines[code_index + 1..]
        .iter()
        .filter(|line| line.starts_with("npm error") || line.starts_with("npm ERR!"))
        .map(|line| {
            line.trim_start_matches("npm error")
                .trim_start_matches("npm ERR!")
                .trim()
        })
        .filter(|detail| {
            !detail.is_empty()
                && !detail.starts_with("errno ")
                && !detail.starts_with("A complete log")
        })
        .take(NPM_ERROR_SUMMARY_LINES)
        .map(clip_line);
    summary.extend(details);
    Some(summary.join("; "))
}

fn clip_line(line: &str) -> String {
    if line.len() <= NPM_ERROR_SUMMARY_LINE_LIMIT {
        return line.to_string();
    }
    let mut cut = NPM_ERROR_SUMMARY_LINE_LIMIT;
    while !line.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &line[..cut])
}

#[allow(clippy::too_many_arguments)]
async fn run_floating_npm_install(
    packages_root: &Path,
    node_install_dir: &Path,
    install_dir: &Path,
    tool: &ManagedTool,
    env_vars: &[(String, String)],
    registry: Option<&str>,
    on_line: &InstallLineFn<'_>,
) -> Result<(), NpmInstallFailure> {
    let mut stderr_capture = String::new();
    run_floating_npm_install_capturing(
        packages_root,
        node_install_dir,
        install_dir,
        tool,
        env_vars,
        registry,
        on_line,
        &mut stderr_capture,
    )
    .await
    .map_err(|error| NpmInstallFailure {
        error,
        stderr: stderr_capture,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_floating_npm_install_capturing(
    packages_root: &Path,
    node_install_dir: &Path,
    install_dir: &Path,
    tool: &ManagedTool,
    env_vars: &[(String, String)],
    registry: Option<&str>,
    on_line: &InstallLineFn<'_>,
    stderr_capture: &mut String,
) -> Result<(), ManagedToolError> {
    let node_bin_dir = node_install_dir.join("bin");
    let mut command = tokio::process::Command::new(node_bin_dir.join("npm"));
    command
        .arg("install")
        .arg("--prefix")
        .arg(install_dir)
        .args([
            "--omit=dev",
            "--include=optional",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
        ]);
    if let Some(registry) = registry {
        command.args(npm_registry_args(tool.package, registry));
    }
    // `@latest` floats to the newest published version; npm on the managed
    // runtime resolves the platform-native optional dependency for the
    // running machine on its own, so no `--os`/`--cpu` pinning is needed.
    command.arg(format!("{}@latest", tool.package));

    // Exactly the snapshot, not the Tauri process env: launched from Finder
    // the process carries none of the user's shell rc, so an
    // `NPM_CONFIG_REGISTRY`/`npm_config_registry` exported there would steer
    // the freshness probe and doctor's fixes but not this install. The
    // snapshot is what all of those run under.
    command.env_clear();
    for (key, value) in env_vars {
        command.env(key, value);
    }
    // npm's own `#!/usr/bin/env node` shebang must resolve the managed node:
    // its bin dir goes first on the snapshot's PATH (the process PATH when
    // the snapshot carries none).
    let inherited_path = snapshot_var(env_vars, "PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let mut paths = vec![node_bin_dir.clone()];
    paths.extend(std::env::split_paths(&inherited_path));
    if let Ok(path_value) = std::env::join_paths(paths) {
        command.env("PATH", path_value);
    }
    // Share the private prefix's download cache; `--prefix` on the command
    // line outranks any inherited prefix config.
    let cache = packages_root.join("npm-prefix").join("cache");
    command.env("NPM_CONFIG_CACHE", &cache);
    command.env("npm_config_cache", &cache);
    command
        .current_dir(install_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|error| ManagedToolError::NpmInstall(format!("spawn managed npm: {error}")))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let forward_out = async {
        if let Some(stream) = stdout {
            let mut lines = tokio::io::BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                on_line(&line);
            }
        }
    };
    let forward_err = async {
        if let Some(stream) = stderr {
            let mut lines = tokio::io::BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                on_line(&line);
                push_captured_line(stderr_capture, &line);
            }
        }
    };
    let label = registry_label(registry);
    let wait = async {
        match tokio::time::timeout(NPM_INSTALL_TIMEOUT, child.wait()).await {
            Ok(result) => result
                .map_err(|error| ManagedToolError::NpmInstall(format!("wait on npm: {error}"))),
            Err(_) => {
                let _ = child.kill().await;
                Err(ManagedToolError::NpmInstall(format!(
                    "timed out after {} seconds (registry: {label})",
                    NPM_INSTALL_TIMEOUT.as_secs()
                )))
            }
        }
    };
    let (status, (), ()) = tokio::join!(wait, forward_out, forward_err);
    let status = status?;
    if status.success() {
        Ok(())
    } else {
        let summary = npm_error_summary(stderr_capture)
            .map(|summary| format!(": {summary}"))
            .unwrap_or_default();
        Err(ManagedToolError::NpmInstall(format!(
            "npm install exited with {status} (registry: {label}){summary}"
        )))
    }
}

/// Shim body for a managed bridge. Both paths are absolute, so the shim runs
/// with no `node` on PATH at all.
pub(super) fn shim_contents(node: &Path, entrypoint: &Path) -> String {
    format!(
        "#!/bin/sh\n# Written by Staged's managed ACP tools installer; do not edit.\nexec {} {} \"$@\"\n",
        sh_quote(node),
        sh_quote(entrypoint)
    )
}

fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

pub(super) fn write_shim(bin_dir: &Path, binary: &str, contents: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(bin_dir)?;
    let path = bin_dir.join(binary);
    let temp = bin_dir.join(format!(".{binary}.tmp"));
    std::fs::write(&temp, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&temp, &path)
}
