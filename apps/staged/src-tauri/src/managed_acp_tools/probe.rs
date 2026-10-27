//! Registry probes behind Doctor's readouts: the latest published version of
//! a managed bridge (`npm view`) and the registry npm would resolve with no
//! `--registry` (`npm config get registry`), each cached.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::install::installed_version;
use super::registry::{
    fallback_npm_registry, gateway_fallback_registry, npm_registry_args, npm_registry_override,
    registry_label,
};
use super::{read_state, tool_install_dir, ManagedTool};

#[cfg(test)]
#[path = "tests/probe.rs"]
mod tests;

// =============================================================================
// Doctor readouts — installed and latest versions of a managed bridge
// =============================================================================

/// The ACP package version installed in the live managed prefix for `tool`,
/// read from the package's own `package.json` — the tree the shim actually
/// execs. Falls back to the version `state.json` recorded at install time when
/// that tree is unreadable; state is supporting information, not the
/// authority, since it can be missing (a state write that failed after the
/// swap) or stale (a tree swapped in by another Staged instance).
pub fn installed_tool_version(packages_root: &Path, tool: &ManagedTool) -> Option<String> {
    installed_version(&tool_install_dir(packages_root, tool.id), tool.package).or_else(|| {
        read_state(packages_root)
            .tools
            .get(tool.id)
            .map(|pin| pin.version.clone())
            .filter(|version| !version.is_empty())
    })
}

/// How long a registry answer is reused before the registry is asked again.
/// Mirrors the doctor crate's freshness cache: a Doctor re-run within the
/// hour costs no network, and a real release still shows up within one.
const LATEST_VERSION_CACHE_TTL: Duration = Duration::from_secs(60 * 60);

/// A registry lookup that hangs must not hold the freshness pass open — the
/// same bound the doctor crate puts on its own `npm view` probes.
const LATEST_VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// What, beyond the package name, decides which registry `npm view` answers
/// from: the explicit override, the env snapshot (its `npm`, `HOME`, and any
/// `NPM_CONFIG_*`/`npm_config_*` exports), and the identity of the user npmrc
/// npm reads under it. Keying the cache on all of it means an edited
/// `~/.npmrc` or a changed `STAGED_NPM_REGISTRY` starts a fresh lookup instead
/// of Doctor showing one registry's version for an hour while the installer
/// fetches `@latest` from another.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RegistryFingerprint {
    override_registry: Option<String>,
    env: EnvSnapshot,
    /// The user npmrc's path, size and mtime; `None` when there is none.
    user_npmrc: Option<(PathBuf, u64, Option<std::time::SystemTime>)>,
}

impl RegistryFingerprint {
    fn current(env_vars: &[(String, String)]) -> Self {
        Self {
            override_registry: npm_registry_override(),
            env: env_vars.to_vec(),
            user_npmrc: user_npmrc_path(env_vars).and_then(|path| {
                let meta = std::fs::metadata(&path).ok()?;
                Some((path, meta.len(), meta.modified().ok()))
            }),
        }
    }
}

/// The user-level npmrc npm reads under `env_vars`: `NPM_CONFIG_USERCONFIG`
/// (either spelling), else `$HOME/.npmrc`.
fn user_npmrc_path(env_vars: &[(String, String)]) -> Option<PathBuf> {
    snapshot_var(env_vars, "NPM_CONFIG_USERCONFIG")
        .or_else(|| snapshot_var(env_vars, "npm_config_userconfig"))
        .map(PathBuf::from)
        .or_else(|| snapshot_var(env_vars, "HOME").map(|home| Path::new(home).join(".npmrc")))
}

/// One latest-version cache entry: a package under one registry configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LatestVersionKey {
    package: String,
    registry: RegistryFingerprint,
}

impl LatestVersionKey {
    fn new(package: &str, env_vars: &[(String, String)]) -> Self {
        Self {
            package: package.to_string(),
            registry: RegistryFingerprint::current(env_vars),
        }
    }
}

/// Latest published versions by package and registry configuration, with
/// the time each was fetched. Only answers are cached: a failed lookup is
/// retried on the next pass rather than pinned as "unknown" for an hour.
#[derive(Debug, Default)]
struct LatestVersionCache {
    entries: HashMap<LatestVersionKey, (String, Instant)>,
}

impl LatestVersionCache {
    fn get_fresh(&self, key: &LatestVersionKey, now: Instant) -> Option<String> {
        let (version, fetched_at) = self.entries.get(key)?;
        (now.saturating_duration_since(*fetched_at) <= LATEST_VERSION_CACHE_TTL)
            .then(|| version.clone())
    }

    /// Records an answer, dropping entries the TTL has already expired — a
    /// superseded npmrc or override would otherwise linger as a dead key.
    fn insert(&mut self, key: LatestVersionKey, version: String, now: Instant) {
        self.entries.retain(|_, (_, fetched_at)| {
            now.saturating_duration_since(*fetched_at) <= LATEST_VERSION_CACHE_TTL
        });
        self.entries.insert(key, (version, now));
    }
}

fn latest_version_cache() -> &'static Mutex<LatestVersionCache> {
    static CACHE: OnceLock<Mutex<LatestVersionCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(LatestVersionCache::default()))
}

/// The newest published version of `tool`'s npm package on the configured
/// registry, or `None` when it cannot be determined — offline, registry
/// unreachable, no `npm` to run, or the probe timed out. The caller must keep
/// `None` as *unknown*, never read it as "up to date".
///
/// Runs `npm view <pkg> version` under the same env snapshot doctor's own
/// probes and the managed install itself run under (see
/// [`install_managed_tool`]), so the managed runtime's `npm` is preferred and
/// the lookup resolves the registry the install will — `~/.npmrc` and any
/// `NPM_CONFIG_REGISTRY` the user's shell rc exports alike — with the same
/// override → npm config → gateway-blocked fallback precedence. Bounded by
/// [`LATEST_VERSION_PROBE_TIMEOUT`] and cached for [`LATEST_VERSION_CACHE_TTL`]
/// per registry configuration ([`RegistryFingerprint`]): an edited `~/.npmrc`
/// or a changed override is asked afresh, not served the old registry's answer.
pub async fn latest_published_version(
    tool: &ManagedTool,
    env_vars: &[(String, String)],
) -> Option<String> {
    let now = Instant::now();
    let key = LatestVersionKey::new(tool.package, env_vars);
    let cached = latest_version_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_fresh(&key, now);
    if cached.is_some() {
        return cached;
    }
    let npm = find_on_env_path(env_vars, "npm")?;
    let version = probe_latest_version(
        &npm,
        tool.package,
        npm_registry_override().as_deref(),
        fallback_npm_registry(),
        env_vars,
        LATEST_VERSION_PROBE_TIMEOUT,
    )
    .await?;
    latest_version_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, version.clone(), now);
    Some(version)
}

/// One uncached latest-version lookup with the registry precedence applied:
/// `override_registry` if given, else npm's own config, and — only when that
/// fails as a gateway block of `registry.npmjs.org` and no override was
/// given — one retry through `fallback_registry` within whatever is left of
/// `timeout`.
async fn probe_latest_version(
    npm_path: &Path,
    package: &str,
    override_registry: Option<&str>,
    fallback_registry: Option<&str>,
    env_vars: &[(String, String)],
    timeout: Duration,
) -> Option<String> {
    let started = Instant::now();
    let failure =
        match npm_view_version(npm_path, package, override_registry, env_vars, timeout).await {
            Ok(version) => return Some(version),
            Err(failure) => failure,
        };
    let NpmViewFailure::Exit { stderr } = failure else {
        return None;
    };
    let fallback = gateway_fallback_registry(override_registry, fallback_registry, &stderr)?;
    let remaining = timeout
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())?;
    log::info!(
        "npm view {package} version: registry.npmjs.org is gateway-blocked; retrying through {fallback}"
    );
    npm_view_version(npm_path, package, Some(fallback), env_vars, remaining)
        .await
        .ok()
}

/// The first `<dir>/<name>` that is a file along the snapshot's `PATH` —
/// resolved by hand because `Command::new("npm")` would search the *parent*
/// process's PATH, not the snapshot's, and the snapshot is what puts the
/// managed runtime's bin dir first.
pub(crate) fn find_on_env_path(env_vars: &[(String, String)], name: &str) -> Option<PathBuf> {
    std::env::split_paths(snapshot_var(env_vars, "PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The last value of `key` in an env snapshot — the one a `Command` applying
/// the snapshot in order would end up with.
pub(super) fn snapshot_var<'a>(env_vars: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env_vars
        .iter()
        .rev()
        .find(|(existing, _)| existing == key)
        .map(|(_, value)| value.as_str())
}

/// Why an `npm view` probe produced no version.
#[derive(Debug)]
enum NpmViewFailure {
    /// npm ran and exited non-zero; its stderr is kept for classification.
    Exit { stderr: String },
    /// npm could not be started, timed out, or printed something that is
    /// not a version.
    Other,
}

/// `npm view <package> version [--registry <url>]` with `npm` at `npm_path`,
/// under exactly the `env_vars` snapshot, killed at `timeout`. `Ok` only for
/// a clean exit whose stdout is a version-shaped token.
///
/// npm's own retry/timeout defaults (`fetch-retries=2`, backoff up to 60 s)
/// would let a dead registry host run ~70 s; a probe is not worth that, so
/// retries are off and npm's fetch timeout is pinned to the probe budget.
/// Installs keep npm's defaults.
async fn npm_view_version(
    npm_path: &Path,
    package: &str,
    registry: Option<&str>,
    env_vars: &[(String, String)],
    timeout: Duration,
) -> Result<String, NpmViewFailure> {
    let mut command = tokio::process::Command::new(npm_path);
    command.args(["view", package, "version", "--fetch-retries=0"]);
    command.arg(format!("--fetch-timeout={}", timeout.as_millis()));
    if let Some(registry) = registry {
        command.args(npm_registry_args(package, registry));
    }
    command.env_clear();
    for (key, value) in env_vars {
        command.env(key, value);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            log::debug!("npm view {package} version failed to run: {error}");
            return Err(NpmViewFailure::Other);
        }
        Err(_) => {
            log::debug!(
                "npm view {package} version timed out after {}s (registry: {})",
                timeout.as_secs(),
                registry_label(registry)
            );
            return Err(NpmViewFailure::Other);
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        log::debug!(
            "npm view {package} version exited with {} (registry: {}): {stderr}",
            output.status,
            registry_label(registry)
        );
        return Err(NpmViewFailure::Exit { stderr });
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    version
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_digit())
        .then_some(version)
        .ok_or(NpmViewFailure::Other)
}

/// How long Doctor waits on `npm config get registry`.
const NPM_CONFIG_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a `npm config get registry` answer is reused for the same env
/// snapshot. Doctor asks on every report while a managed bridge is missing —
/// two passes per open, plus refreshes — and each ask spawns npm; one spawn a
/// minute keeps that off the report's critical path without hiding a
/// `~/.npmrc` edit for long. Unlike [`LatestVersionCache`] a `None` answer
/// is cached too: it is what a hung or absent npm costs, and re-asking every
/// pass is exactly the stall this avoids.
const NPM_REGISTRY_CACHE_TTL: Duration = Duration::from_secs(60);

/// The env snapshot a registry answer was asked under: the cache key, since
/// the answer depends on the snapshot's `npm`, `~/.npmrc` and exports.
type EnvSnapshot = Vec<(String, String)>;

/// `npm config get registry` answers by the env snapshot they were asked
/// under, with the time each was fetched.
#[derive(Debug, Default)]
struct NpmRegistryCache {
    entries: HashMap<EnvSnapshot, (Option<String>, Instant)>,
}

impl NpmRegistryCache {
    /// `Some(answer)` when a fresh answer (itself possibly `None`) is cached.
    fn get_fresh(&self, env_vars: &[(String, String)], now: Instant) -> Option<Option<String>> {
        let (registry, fetched_at) = self.entries.get(env_vars)?;
        (now.saturating_duration_since(*fetched_at) <= NPM_REGISTRY_CACHE_TTL)
            .then(|| registry.clone())
    }

    fn insert(&mut self, env_vars: &[(String, String)], registry: Option<String>, now: Instant) {
        self.entries.insert(env_vars.to_vec(), (registry, now));
    }
}

fn npm_registry_cache() -> &'static Mutex<NpmRegistryCache> {
    static CACHE: OnceLock<Mutex<NpmRegistryCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(NpmRegistryCache::default()))
}

/// The registry npm would use under the `env_vars` snapshot with no
/// `--registry` — `npm config get registry` asked of the snapshot's own
/// `npm` (the managed runtime's, when installed), under exactly that env, so
/// the answer is the one a managed install resolves. `None` when the
/// snapshot has no `npm`, the probe fails or times out, or the answer is not
/// a URL; Doctor then says "npm default" rather than guessing. Cached per
/// snapshot for [`NPM_REGISTRY_CACHE_TTL`].
pub(crate) async fn configured_npm_registry(env_vars: &[(String, String)]) -> Option<String> {
    let now = Instant::now();
    let cached = npm_registry_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_fresh(env_vars, now);
    if let Some(registry) = cached {
        return registry;
    }
    let registry = probe_configured_npm_registry(env_vars).await;
    npm_registry_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(env_vars, registry.clone(), now);
    registry
}

/// One uncached `npm config get registry` (see [`configured_npm_registry`]).
async fn probe_configured_npm_registry(env_vars: &[(String, String)]) -> Option<String> {
    let npm_path = find_on_env_path(env_vars, "npm")?;
    let mut command = tokio::process::Command::new(npm_path);
    command.args(["config", "get", "registry"]);
    command.env_clear();
    for (key, value) in env_vars {
        command.env(key, value);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(NPM_CONFIG_PROBE_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let registry = String::from_utf8_lossy(&output.stdout).trim().to_string();
    registry.starts_with("http").then_some(registry)
}
