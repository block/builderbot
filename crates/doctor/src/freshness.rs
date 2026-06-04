//! Version-freshness lookups: installed version (local subprocess) +
//! latest available version (brew/npm/crates.io) with a disk cache.
//!
//! Only runs when `RunChecksOptions::check_freshness` is set — the default
//! `run_checks()` path is unaffected.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::command::{
    run_command_with_timeout, CommandError, CommandTimeout, FRESHNESS_PROBE_TIMEOUT,
};
use crate::package_ids::LatestSource;
use crate::types::InstallSource;

/// One hour: long enough that repeated `run_checks_with_options` calls in the
/// same session don't re-hit registries; short enough that real upgrades
/// surface within a reasonable window.
const CACHE_TTL_SECONDS: i64 = 60 * 60;

/// Result of a single version-freshness probe.
#[derive(Debug, Clone, Default)]
pub(crate) struct VersionInfo {
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub update_available: Option<bool>,
    pub command_timeouts: Vec<CommandTimeout>,
}

#[derive(Debug, Default)]
struct ProbeResult {
    value: Option<String>,
    timeouts: Vec<CommandTimeout>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    value: String,
    fetched_at: i64,
}

/// In-memory mirror of the on-disk cache. Read once at the start of a freshness
/// run, mutated as fetches happen, persisted once at the end.
#[derive(Debug, Default)]
pub(crate) struct FreshnessCache {
    entries: HashMap<String, CacheEntry>,
}

impl FreshnessCache {
    fn cache_key(source: LatestSource, package_id: &str) -> String {
        format!("{:?}:{}", source, package_id)
    }

    fn get_fresh(&self, source: LatestSource, package_id: &str, now: i64) -> Option<String> {
        let key = Self::cache_key(source, package_id);
        let entry = self.entries.get(&key)?;
        if now.saturating_sub(entry.fetched_at) <= CACHE_TTL_SECONDS {
            Some(entry.value.clone())
        } else {
            None
        }
    }

    fn insert(&mut self, source: LatestSource, package_id: &str, value: String, now: i64) {
        let key = Self::cache_key(source, package_id);
        self.entries.insert(
            key,
            CacheEntry {
                value,
                fetched_at: now,
            },
        );
    }
}

/// Resolve the on-disk cache file path. Prefers `dirs::cache_dir()`; the
/// `<cache_dir>/doctor` parent is created lazily on save.
pub(crate) fn cache_file_path() -> Option<PathBuf> {
    let base = dirs::cache_dir()?;
    Some(base.join("doctor").join("freshness.json"))
}

/// Read the on-disk cache. Missing-or-malformed → empty (and we'll overwrite
/// on next save). Errors are logged, never propagated.
pub(crate) fn load_cache() -> FreshnessCache {
    let Some(path) = cache_file_path() else {
        return FreshnessCache::default();
    };
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<HashMap<String, CacheEntry>>(&bytes) {
            Ok(entries) => FreshnessCache { entries },
            Err(e) => {
                eprintln!(
                    "doctor: freshness cache malformed at {}: {e}",
                    path.display()
                );
                FreshnessCache::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FreshnessCache::default(),
        Err(e) => {
            eprintln!("doctor: failed to read freshness cache: {e}");
            FreshnessCache::default()
        }
    }
}

/// Atomically persist the cache: write to `<file>.tmp`, then rename.
pub(crate) fn save_cache(cache: &FreshnessCache) {
    let Some(path) = cache_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!(
                "doctor: failed to create freshness cache dir {}: {e}",
                parent.display(),
            );
            return;
        }
    }
    let json = match serde_json::to_vec_pretty(&cache.entries) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("doctor: failed to serialize freshness cache: {e}");
            return;
        }
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &json) {
        eprintln!("doctor: failed to write freshness cache tmp file: {e}");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        eprintln!("doctor: failed to rename freshness cache tmp -> final: {e}");
    }
}

fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Locate the first semver-shaped token (`X.Y.Z`) on any line of `text`.
///
/// We accept an optional leading `v` and ignore anything after the patch
/// component (pre-release / build metadata is fine to include in the returned
/// string — it's compared as a string only).
pub(crate) fn extract_version(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(v) = find_semver_in_line(line) {
            return Some(v);
        }
    }
    None
}

fn find_semver_in_line(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            // Greedy: extend through digits.dots; require at least two dots.
            let start = i;
            let mut dots = 0;
            while i < bytes.len() {
                let b = bytes[i];
                if b.is_ascii_digit() {
                    i += 1;
                } else if b == b'.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                    dots += 1;
                    i += 1;
                } else {
                    break;
                }
            }
            if dots >= 2 {
                return Some(line[start..i].to_string());
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Lightweight parse of `X.Y.Z[...]` into `(major, minor, patch)`. Returns
/// `None` if any of the first three numeric components is missing.
fn parse_semver_triple(v: &str) -> Option<(u64, u64, u64)> {
    let trimmed = v.trim().trim_start_matches('v');
    let core = trimmed.split(['-', '+', ' ']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch_str = parts.next()?;
    let patch = patch_str
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse::<u64>()
        .ok()?;
    Some((major, minor, patch))
}

/// Compute `update_available` if both sides parse as semver triples; otherwise
/// `None` (so the UI doesn't render a misleading "outdated" badge based on
/// string inequality of unparseable versions).
fn compute_update_available(installed: &Option<String>, latest: &Option<String>) -> Option<bool> {
    let i = installed.as_deref()?;
    let l = latest.as_deref()?;
    let it = parse_semver_triple(i)?;
    let lt = parse_semver_triple(l)?;
    Some(lt > it)
}

/// How to read a target's *installed* version. The mechanism depends on how the
/// binary was installed: a CLI `--version` probe works for native/brew/cargo
/// binaries, but npm-distributed ACP bridges are `node` scripts that don't
/// implement `--version` (e.g. `codex-acp --version` errors, `amp-acp
/// --version` prints nothing), so those are read straight from the package's
/// `package.json` instead — no subprocess, no network.
pub(crate) enum InstalledProbe<'a> {
    /// Run `<binary> <args>` and extract a semver (the default behavior).
    Cli(&'a [&'a str]),
    /// Walk up from the canonicalized binary to the owning
    /// `node_modules/<pkg>/package.json` and read its `version`.
    NpmPackageJson { package_id: Option<&'a str> },
}

/// Pick the installed-version probe for a readout from its install source.
/// `Npm` installs read `package.json`; everything else uses the CLI
/// `--version` probe.
pub(crate) fn select_installed_probe<'a>(
    install_source: Option<&InstallSource>,
    package_id: Option<&'a str>,
) -> InstalledProbe<'a> {
    if matches!(install_source, Some(InstallSource::Npm)) {
        InstalledProbe::NpmPackageJson { package_id }
    } else {
        InstalledProbe::Cli(&["--version"])
    }
}

/// Owned mirror of [`InstalledProbe`] so the probe can cross the
/// `spawn_blocking` boundary (which requires `'static`).
enum OwnedProbe {
    Cli(Vec<String>),
    NpmPackageJson { package_id: Option<String> },
}

impl InstalledProbe<'_> {
    fn to_owned_probe(&self) -> OwnedProbe {
        match self {
            InstalledProbe::Cli(args) => {
                OwnedProbe::Cli(args.iter().map(|s| s.to_string()).collect())
            }
            InstalledProbe::NpmPackageJson { package_id } => OwnedProbe::NpmPackageJson {
                package_id: package_id.map(|s| s.to_string()),
            },
        }
    }
}

/// Read an npm package's installed version from its `package.json`. Pure
/// filesystem: canonicalize the binary (resolving the symlink npm leaves in
/// `bin/`), then walk up a bounded number of levels looking for the first
/// `package.json`. When the package id is known, the file's `name` must match
/// it — otherwise we keep walking, so a dependency's `package.json` nested
/// below the real one is never mistaken for the target.
fn installed_version_from_package_json(
    binary_path: &Path,
    expected_pkg: Option<&str>,
) -> Option<String> {
    let resolved = std::fs::canonicalize(binary_path).ok()?;
    let mut dir = resolved.parent();
    for _ in 0..6 {
        let d = dir?;
        let pj = d.join("package.json");
        if pj.is_file() {
            if let Ok(bytes) = std::fs::read(&pj) {
                if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                    let name_ok = expected_pkg
                        .map(|p| v.get("name").and_then(|n| n.as_str()) == Some(p))
                        .unwrap_or(true);
                    if name_ok {
                        return v
                            .get("version")
                            .and_then(|x| x.as_str())
                            .map(str::to_string);
                    }
                }
            }
        }
        dir = d.parent();
    }
    None
}

/// Run `<binary> <args>` and parse the first semver-shaped token out of the
/// combined stdout/stderr. Errors and missing tokens both yield `None`.
fn installed_version(binary_path: &Path, version_args: &[&str]) -> ProbeResult {
    let mut command = Command::new(binary_path);
    command.args(version_args);
    let display_command = format!("{} {}", binary_path.display(), version_args.join(" "))
        .trim()
        .to_string();
    let output = match run_command_with_timeout(command, display_command, FRESHNESS_PROBE_TIMEOUT) {
        Ok(output) => output,
        Err(CommandError::Timeout { command, timeout }) => {
            return ProbeResult {
                value: None,
                timeouts: vec![CommandTimeout::new("installed version", command, timeout)],
            };
        }
        Err(_) => return ProbeResult::default(),
    };
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    ProbeResult {
        value: extract_version(&combined),
        timeouts: Vec::new(),
    }
}

/// Whether a tool keeps itself up to date and therefore should never raise an
/// "update available" nag. Curl/native installers (Claude native, Cursor,
/// Amp-curl) self-update; those installs are fingerprinted as
/// [`InstallSource::CurlPipe`] (directly or via a per-agent override). Registry
/// installs (brew/npm/cargo) are user-managed and remain actionable, which is
/// why a brew/npm install of the same agent is *not* treated as self-updating.
pub(crate) fn is_self_updating(source: Option<&InstallSource>) -> bool {
    matches!(source, Some(InstallSource::CurlPipe))
}

/// Dispatch a latest-version probe per source.
fn latest_version(
    source: LatestSource,
    package_id: &str,
    npm_registry: Option<&str>,
) -> ProbeResult {
    match source {
        LatestSource::Brew => latest_brew(package_id),
        LatestSource::Npm => latest_npm(package_id, npm_registry),
        LatestSource::CratesIo => ProbeResult {
            value: latest_crates_io(package_id),
            timeouts: Vec::new(),
        },
        LatestSource::GitHubReleases => ProbeResult {
            value: latest_github_releases(package_id),
            timeouts: Vec::new(),
        },
    }
}

fn latest_brew(package_id: &str) -> ProbeResult {
    let mut command = Command::new("brew");
    command.args(["info", "--json=v2", package_id]);
    let display_command = format!("brew info --json=v2 {package_id}");
    let output = match run_command_with_timeout(command, display_command, FRESHNESS_PROBE_TIMEOUT) {
        Ok(output) => output,
        Err(CommandError::Timeout { command, timeout }) => {
            return ProbeResult {
                value: None,
                timeouts: vec![CommandTimeout::new("brew latest version", command, timeout)],
            };
        }
        Err(_) => return ProbeResult::default(),
    };
    if !output.status.success() {
        return ProbeResult::default();
    }
    ProbeResult {
        value: parse_brew_info_v2(&output.stdout, package_id),
        timeouts: Vec::new(),
    }
}

/// Pull `formulae[0].versions.stable` or `casks[0].version` out of a
/// `brew info --json=v2` payload. Public to the crate so unit tests can
/// drive it with a fixture without shelling out.
pub(crate) fn parse_brew_info_v2(bytes: &[u8], _package_id: &str) -> Option<String> {
    let root: Value = serde_json::from_slice(bytes).ok()?;
    if let Some(formulae) = root.get("formulae").and_then(|v| v.as_array()) {
        if let Some(first) = formulae.first() {
            if let Some(v) = first
                .get("versions")
                .and_then(|v| v.get("stable"))
                .and_then(|v| v.as_str())
            {
                return Some(v.to_string());
            }
        }
    }
    if let Some(casks) = root.get("casks").and_then(|v| v.as_array()) {
        if let Some(first) = casks.first() {
            if let Some(v) = first.get("version").and_then(|v| v.as_str()) {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn latest_npm(package_id: &str, npm_registry: Option<&str>) -> ProbeResult {
    let mut cmd = Command::new("npm");
    cmd.args(["view", package_id, "version"]);
    if let Some(registry) = npm_registry {
        cmd.args(["--registry", registry]);
    }
    let display_command = if let Some(registry) = npm_registry {
        format!("npm view {package_id} version --registry {registry}")
    } else {
        format!("npm view {package_id} version")
    };
    let output = match run_command_with_timeout(cmd, display_command, FRESHNESS_PROBE_TIMEOUT) {
        Ok(output) => output,
        Err(CommandError::Timeout { command, timeout }) => {
            return ProbeResult {
                value: None,
                timeouts: vec![CommandTimeout::new("npm latest version", command, timeout)],
            };
        }
        Err(_) => return ProbeResult::default(),
    };
    if !output.status.success() {
        return ProbeResult::default();
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        ProbeResult::default()
    } else {
        ProbeResult {
            value: Some(raw),
            timeouts: Vec::new(),
        }
    }
}

fn latest_crates_io(package_id: &str) -> Option<String> {
    let url = format!("https://crates.io/api/v1/crates/{package_id}");
    let client = reqwest::blocking::Client::builder()
        .user_agent("block-builderbot-doctor/0.1")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client.get(&url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: Value = resp.json().ok()?;
    json.get("crate")
        .and_then(|c| c.get("max_stable_version"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Fetch the latest release tag for a `owner/repo` slug via the GitHub REST API.
///
/// Used for tools published only through GitHub releases (no brew/npm/crates
/// presence), e.g. Cursor's curl install. Degrades gracefully to `None` on any
/// error — no token, rate-limited, network failure, or unparseable payload —
/// and never returns a hard error. A `GITHUB_TOKEN`/`GH_TOKEN` in the
/// environment is used as a bearer credential to relax the unauthenticated
/// rate limit, but is entirely optional.
fn latest_github_releases(repo: &str) -> Option<String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let client = reqwest::blocking::Client::builder()
        // GitHub rejects requests without a User-Agent.
        .user_agent("block-builderbot-doctor/0.1")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let mut req = client
        .get(&url)
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = github_token() {
        req = req.bearer_auth(token);
    }
    let resp = req.send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().ok()?;
    parse_github_release_tag(&bytes)
}

/// Read GitHub's optional release-auth token from the environment. Empty values
/// are treated as absent. Kept separate so the fetcher stays testable.
fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .ok()
        .or_else(|| std::env::var("GH_TOKEN").ok())
        .filter(|s| !s.is_empty())
}

/// Pull `tag_name` out of a GitHub `releases/latest` payload, stripping a
/// leading `v` so it lines up with the semver shapes the rest of the module
/// produces. Crate-public so unit tests can drive it without a network call.
pub(crate) fn parse_github_release_tag(bytes: &[u8]) -> Option<String> {
    let root: Value = serde_json::from_slice(bytes).ok()?;
    root.get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_start_matches('v').to_string())
}

/// Top-level: returns installed (always, when binary present), latest (only
/// if `package_id` is `Some` and we're not offline), and `update_available`
/// (only if both parse as semver triples).
pub(crate) async fn fetch_version_info(
    latest_source: Option<LatestSource>,
    package_id: Option<&str>,
    binary_path: &Path,
    probe: InstalledProbe<'_>,
    offline: bool,
    npm_registry: Option<&str>,
    cache: Arc<Mutex<FreshnessCache>>,
) -> VersionInfo {
    let path = binary_path.to_path_buf();
    let probe = probe.to_owned_probe();
    let pkg = package_id.map(|s| s.to_string());
    let npm_registry = npm_registry.map(|s| s.to_string());

    let result = tokio::task::spawn_blocking(move || {
        let mut command_timeouts = Vec::new();
        let installed = if path.as_os_str().is_empty() {
            None
        } else {
            match &probe {
                OwnedProbe::Cli(args) => {
                    let result = installed_version(
                        &path,
                        &args.iter().map(|s| s.as_str()).collect::<Vec<&str>>(),
                    );
                    command_timeouts.extend(result.timeouts);
                    result.value
                }
                OwnedProbe::NpmPackageJson { package_id } => {
                    installed_version_from_package_json(&path, package_id.as_deref())
                }
            }
        };

        let latest = if offline {
            None
        } else if let (Some(source), Some(pkg)) = (latest_source, pkg) {
            let now = now_epoch_seconds();
            // Cache lookup first.
            let cached = {
                let guard = cache.lock().ok();
                guard.as_ref().and_then(|c| c.get_fresh(source, &pkg, now))
            };
            if let Some(v) = cached {
                Some(v)
            } else {
                let result = latest_version(source, &pkg, npm_registry.as_deref());
                command_timeouts.extend(result.timeouts);
                if let Some(v) = result.value {
                    if let Ok(mut guard) = cache.lock() {
                        guard.insert(source, &pkg, v.clone(), now);
                    }
                    Some(v)
                } else {
                    None
                }
            }
        } else {
            None
        };

        let update_available = compute_update_available(&installed, &latest);
        VersionInfo {
            installed,
            latest,
            update_available,
            command_timeouts,
        }
    })
    .await;

    result.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_version_finds_git_style_string() {
        assert_eq!(
            extract_version("git version 2.39.5 (Apple Git-154)"),
            Some("2.39.5".to_string()),
        );
    }

    #[test]
    fn extract_version_finds_gh_multiline() {
        let s = "gh version 2.62.0 (2024-11-12)\nhttps://github.com/cli/cli/releases/tag/v2.62.0";
        assert_eq!(extract_version(s), Some("2.62.0".to_string()));
    }

    #[test]
    fn extract_version_returns_none_when_no_semver() {
        assert_eq!(extract_version("no version here"), None);
        assert_eq!(extract_version("1.2"), None); // need 3 components
    }

    #[test]
    fn extract_version_handles_leading_garbage() {
        assert_eq!(
            extract_version("Codex CLI 0.21.4-beta"),
            Some("0.21.4".to_string()),
        );
    }

    #[test]
    fn parse_semver_triple_basic() {
        assert_eq!(parse_semver_triple("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver_triple("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver_triple("1.2.3-rc1"), Some((1, 2, 3)));
        assert_eq!(parse_semver_triple("1.2"), None);
    }

    #[test]
    fn compute_update_available_handles_parse_failure() {
        assert_eq!(
            compute_update_available(
                &Some("not-a-version".to_string()),
                &Some("1.0.0".to_string()),
            ),
            None,
        );
        assert_eq!(
            compute_update_available(&Some("1.0.0".to_string()), &Some("1.0.0".to_string())),
            Some(false),
        );
        assert_eq!(
            compute_update_available(&Some("1.0.0".to_string()), &Some("1.0.1".to_string())),
            Some(true),
        );
    }

    #[test]
    fn brew_info_v2_parses_formula_versions_stable() {
        let json = br#"{
            "formulae": [{
                "name": "git",
                "versions": {"stable": "2.50.1", "head": null, "bottle": true}
            }],
            "casks": []
        }"#;
        assert_eq!(parse_brew_info_v2(json, "git").as_deref(), Some("2.50.1"),);
    }

    #[test]
    fn brew_info_v2_parses_cask_version() {
        let json = br#"{
            "formulae": [],
            "casks": [{"token": "codex", "version": "0.21.4"}]
        }"#;
        assert_eq!(parse_brew_info_v2(json, "codex").as_deref(), Some("0.21.4"),);
    }

    #[test]
    fn brew_info_v2_returns_none_on_empty_payload() {
        let json = br#"{"formulae": [], "casks": []}"#;
        assert_eq!(parse_brew_info_v2(json, "ghost"), None);
    }

    #[test]
    fn cache_ttl_treats_old_entries_as_stale() {
        let mut cache = FreshnessCache::default();
        let now = 1_000_000;
        cache.insert(LatestSource::Brew, "git", "2.50.0".to_string(), now);

        // Within TTL.
        assert_eq!(
            cache.get_fresh(LatestSource::Brew, "git", now + 60),
            Some("2.50.0".to_string()),
        );

        // 2 hours later — stale.
        let two_hours_later = now + 2 * 60 * 60;
        assert_eq!(
            cache.get_fresh(LatestSource::Brew, "git", two_hours_later),
            None,
        );
    }

    #[test]
    fn cache_key_uses_source_and_package_id() {
        let k1 = FreshnessCache::cache_key(LatestSource::Brew, "git");
        let k2 = FreshnessCache::cache_key(LatestSource::Npm, "git");
        assert_ne!(k1, k2, "different sources should map to different keys");
    }

    #[test]
    fn github_release_tag_strips_leading_v() {
        let json = br#"{"tag_name": "v1.2.3", "name": "1.2.3"}"#;
        assert_eq!(
            parse_github_release_tag(json).as_deref(),
            Some("1.2.3"),
            "leading v should be stripped",
        );
    }

    #[test]
    fn github_release_tag_without_v_prefix() {
        let json = br#"{"tag_name": "2025.06.01"}"#;
        assert_eq!(
            parse_github_release_tag(json).as_deref(),
            Some("2025.06.01")
        );
    }

    #[test]
    fn github_release_tag_missing_field_is_none() {
        assert_eq!(
            parse_github_release_tag(br#"{"name": "no tag here"}"#),
            None
        );
        assert_eq!(parse_github_release_tag(b"not json"), None);
    }

    #[test]
    fn self_updating_only_for_curl_pipe() {
        assert!(is_self_updating(Some(&InstallSource::CurlPipe)));
        assert!(!is_self_updating(Some(&InstallSource::Npm)));
        assert!(!is_self_updating(Some(&InstallSource::Brew)));
        assert!(!is_self_updating(None));
    }

    /// Fresh per-test scratch dir under the system temp dir, unique by name +
    /// process id (matches the pattern resolve.rs's tests use — no extra dev
    /// dependency).
    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("doctor-freshness-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn package_json_version_read_through_bin_symlink() {
        let root = scratch_dir("pj-symlink");
        let pkg = root.join("lib/node_modules/amp-acp");
        std::fs::create_dir_all(pkg.join("dist")).unwrap();
        let index = pkg.join("dist/index.js");
        std::fs::write(&index, "// node script\n").unwrap();
        std::fs::write(
            pkg.join("package.json"),
            br#"{"name": "amp-acp", "version": "0.4.2"}"#,
        )
        .unwrap();

        // npm leaves a symlink in bin/ pointing at the package's entrypoint.
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let bin = root.join("bin/amp-acp");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&index, &bin).unwrap();

        assert_eq!(
            installed_version_from_package_json(&bin, Some("amp-acp")).as_deref(),
            Some("0.4.2"),
        );
        // No expected name → first package.json found still wins.
        assert_eq!(
            installed_version_from_package_json(&bin, None).as_deref(),
            Some("0.4.2"),
        );
    }

    #[test]
    fn package_json_skips_mismatched_name_and_keeps_walking() {
        let root = scratch_dir("pj-mismatch");
        let pkg = root.join("node_modules/amp-acp");
        // A nested dependency's package.json sits closer to the binary; its
        // name doesn't match, so the walk must continue up to amp-acp's.
        let dep = pkg.join("node_modules/inner-dep");
        std::fs::create_dir_all(&dep).unwrap();
        std::fs::write(
            dep.join("package.json"),
            br#"{"name": "inner-dep", "version": "9.9.9"}"#,
        )
        .unwrap();
        let index = dep.join("index.js");
        std::fs::write(&index, "// dep\n").unwrap();
        std::fs::write(
            pkg.join("package.json"),
            br#"{"name": "amp-acp", "version": "1.0.0"}"#,
        )
        .unwrap();

        assert_eq!(
            installed_version_from_package_json(&index, Some("amp-acp")).as_deref(),
            Some("1.0.0"),
            "should skip inner-dep and find amp-acp",
        );
    }

    #[test]
    fn package_json_resolves_scoped_package() {
        let root = scratch_dir("pj-scoped");
        let pkg = root.join("node_modules/@zed-industries/codex-acp");
        std::fs::create_dir_all(pkg.join("dist")).unwrap();
        let index = pkg.join("dist/index.js");
        std::fs::write(&index, "// node script\n").unwrap();
        std::fs::write(
            pkg.join("package.json"),
            br#"{"name": "@zed-industries/codex-acp", "version": "0.7.1"}"#,
        )
        .unwrap();

        assert_eq!(
            installed_version_from_package_json(&index, Some("@zed-industries/codex-acp"))
                .as_deref(),
            Some("0.7.1"),
        );
    }

    /// Exercises the Claude main CLI's new npm package id end-to-end at the
    /// freshness layer: when claude is npm-installed under nvm, its main
    /// readout walks up to `@anthropic-ai/claude-code`'s `package.json`.
    #[test]
    fn package_json_resolves_claude_main_npm_layout() {
        let root = scratch_dir("pj-claude-main");
        let pkg = root.join("node_modules/@anthropic-ai/claude-code");
        std::fs::create_dir_all(pkg.join("cli")).unwrap();
        let entry = pkg.join("cli/cli.js");
        std::fs::write(&entry, "// node script\n").unwrap();
        std::fs::write(
            pkg.join("package.json"),
            br#"{"name": "@anthropic-ai/claude-code", "version": "2.1.0"}"#,
        )
        .unwrap();

        // npm leaves a `claude` symlink in `bin/`.
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let bin = root.join("bin/claude");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&entry, &bin).unwrap();

        assert_eq!(
            installed_version_from_package_json(&bin, Some("@anthropic-ai/claude-code")).as_deref(),
            Some("2.1.0"),
        );
    }

    #[test]
    fn package_json_missing_returns_none() {
        let root = scratch_dir("pj-missing");
        let bin = root.join("bin/whatever");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(&bin, "x\n").unwrap();
        assert_eq!(
            installed_version_from_package_json(&bin, Some("nope")),
            None
        );
    }

    #[test]
    fn select_probe_npm_reads_package_json() {
        match select_installed_probe(Some(&InstallSource::Npm), Some("amp-acp")) {
            InstalledProbe::NpmPackageJson { package_id } => {
                assert_eq!(package_id, Some("amp-acp"));
            }
            _ => panic!("npm install source should select NpmPackageJson probe"),
        }
    }

    #[test]
    fn select_probe_non_npm_uses_cli_version() {
        for src in [
            Some(&InstallSource::Brew),
            Some(&InstallSource::CurlPipe),
            Some(&InstallSource::Cargo),
            None,
        ] {
            match select_installed_probe(src, Some("amp-acp")) {
                InstalledProbe::Cli(args) => assert_eq!(args, &["--version"]),
                _ => panic!("non-npm source {src:?} should select Cli probe"),
            }
        }
    }
}
