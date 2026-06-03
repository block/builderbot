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
    fn cache_key(source: InstallSource, package_id: &str) -> String {
        format!("{:?}:{}", source, package_id)
    }

    fn get_fresh(&self, source: InstallSource, package_id: &str, now: i64) -> Option<String> {
        let key = Self::cache_key(source, package_id);
        let entry = self.entries.get(&key)?;
        if now.saturating_sub(entry.fetched_at) <= CACHE_TTL_SECONDS {
            Some(entry.value.clone())
        } else {
            None
        }
    }

    fn insert(&mut self, source: InstallSource, package_id: &str, value: String, now: i64) {
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

/// Run `<binary> <args>` and parse the first semver-shaped token out of the
/// combined stdout/stderr. Errors and missing tokens both yield `None`.
fn installed_version(binary_path: &Path, version_args: &[&str]) -> Option<String> {
    let output = Command::new(binary_path).args(version_args).output().ok()?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    extract_version(&combined)
}

/// Dispatch a latest-version probe per source.
fn latest_version(
    source: InstallSource,
    package_id: &str,
    npm_registry: Option<&str>,
) -> Option<String> {
    match source {
        InstallSource::Brew => latest_brew(package_id),
        InstallSource::Npm => latest_npm(package_id, npm_registry),
        InstallSource::Cargo => latest_crates_io(package_id),
        InstallSource::CurlPipe
        | InstallSource::System
        | InstallSource::Unknown
        | InstallSource::Mise
        | InstallSource::Asdf => None,
    }
}

fn latest_brew(package_id: &str) -> Option<String> {
    let output = Command::new("brew")
        .args(["info", "--json=v2", package_id])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_brew_info_v2(&output.stdout, package_id)
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

fn latest_npm(package_id: &str, npm_registry: Option<&str>) -> Option<String> {
    let mut cmd = Command::new("npm");
    cmd.args(["view", package_id, "version"]);
    if let Some(registry) = npm_registry {
        cmd.args(["--registry", registry]);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        None
    } else {
        Some(raw)
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

/// Top-level: returns installed (always, when binary present), latest (only
/// if `package_id` is `Some` and we're not offline), and `update_available`
/// (only if both parse as semver triples).
pub(crate) async fn fetch_version_info(
    install_source: Option<InstallSource>,
    package_id: Option<&str>,
    binary_path: &Path,
    version_args: &[&str],
    offline: bool,
    npm_registry: Option<&str>,
    cache: Arc<Mutex<FreshnessCache>>,
) -> VersionInfo {
    let path = binary_path.to_path_buf();
    let args: Vec<String> = version_args.iter().map(|s| s.to_string()).collect();
    let pkg = package_id.map(|s| s.to_string());
    let npm_registry = npm_registry.map(|s| s.to_string());

    let result = tokio::task::spawn_blocking(move || {
        let installed = if path.as_os_str().is_empty() {
            None
        } else {
            installed_version(
                &path,
                &args.iter().map(|s| s.as_str()).collect::<Vec<&str>>(),
            )
        };

        let latest = if offline {
            None
        } else if let (Some(source), Some(pkg)) = (install_source, pkg) {
            let now = now_epoch_seconds();
            // Cache lookup first.
            let cached = {
                let guard = cache.lock().ok();
                guard
                    .as_ref()
                    .and_then(|c| c.get_fresh(source.clone(), &pkg, now))
            };
            if let Some(v) = cached {
                Some(v)
            } else if let Some(v) = latest_version(source.clone(), &pkg, npm_registry.as_deref()) {
                if let Ok(mut guard) = cache.lock() {
                    guard.insert(source, &pkg, v.clone(), now);
                }
                Some(v)
            } else {
                None
            }
        } else {
            None
        };

        let update_available = compute_update_available(&installed, &latest);
        VersionInfo {
            installed,
            latest,
            update_available,
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
        cache.insert(InstallSource::Brew, "git", "2.50.0".to_string(), now);

        // Within TTL.
        assert_eq!(
            cache.get_fresh(InstallSource::Brew, "git", now + 60),
            Some("2.50.0".to_string()),
        );

        // 2 hours later — stale.
        let two_hours_later = now + 2 * 60 * 60;
        assert_eq!(
            cache.get_fresh(InstallSource::Brew, "git", two_hours_later),
            None,
        );
    }

    #[test]
    fn cache_key_uses_source_and_package_id() {
        let k1 = FreshnessCache::cache_key(InstallSource::Brew, "git");
        let k2 = FreshnessCache::cache_key(InstallSource::Npm, "git");
        assert_ne!(k1, k2, "different sources should map to different keys");
    }
}
