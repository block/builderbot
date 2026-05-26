//! Apply staged-managed `.git/config` flags to clones.
//!
//! Sets `core.fsmonitor=true` (git ≥ 2.36) and `core.untrackedcache=true` so
//! `git status` on big monorepos like cash-server skips the per-file `stat()`
//! walk and the untracked-file enumeration. Respects users who explicitly set
//! these flags to anything else — `set_if_unset` only writes when the key is
//! absent.
//!
//! Two surfaces: [`apply_to_clone`] for local clones, [`apply_to_blox_clone`]
//! for remote (blox) clones via `ws_exec_async`. Both are idempotent so they
//! double as in-place migration for pre-existing clones.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use super::cli::GitError;

const FSMONITOR_MIN_MAJOR: u32 = 2;
const FSMONITOR_MIN_MINOR: u32 = 36;

/// Local git version. Probed once per process.
fn local_git_version() -> Option<(u32, u32)> {
    static VERSION: OnceLock<Option<(u32, u32)>> = OnceLock::new();
    *VERSION.get_or_init(|| {
        let output = Command::new("git").arg("--version").output().ok()?;
        if !output.status.success() {
            return None;
        }
        parse_git_version(&String::from_utf8_lossy(&output.stdout))
    })
}

/// Parse `git version 2.54.0\n` -> `Some((2, 54))`. Tolerates trailing fields
/// like `git version 2.39.5 (Apple Git-154)`.
fn parse_git_version(output: &str) -> Option<(u32, u32)> {
    let token = output.split_whitespace().nth(2)?;
    let mut parts = token.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some((major, minor))
}

fn version_supports_fsmonitor(v: (u32, u32)) -> bool {
    v.0 > FSMONITOR_MIN_MAJOR || (v.0 == FSMONITOR_MIN_MAJOR && v.1 >= FSMONITOR_MIN_MINOR)
}

fn local_supports_fsmonitor() -> bool {
    local_git_version()
        .map(version_supports_fsmonitor)
        .unwrap_or(false)
}

/// Apply staged-managed git config to a local clone. Idempotent.
///
/// Does not fail the calling clone flow on individual config errors — these
/// flags are an optimization, not a correctness requirement. Logs and
/// returns `Ok(())`.
pub fn apply_to_clone(clone_path: &Path) {
    if local_supports_fsmonitor() {
        set_if_unset_local(clone_path, "core.fsmonitor", "true");
    }
    set_if_unset_local(clone_path, "core.untrackedcache", "true");
}

fn set_if_unset_local(clone_path: &Path, key: &str, value: &str) {
    match super::cli::run(clone_path, &["config", "--local", "--get", key]) {
        Ok(_) => {
            // User (or a previous run) already set this key. Leave it alone.
        }
        Err(GitError::CommandFailed(_)) => {
            // `git config --get` exits non-zero when the key is unset.
            if let Err(e) = super::cli::run(clone_path, &["config", "--local", key, value]) {
                log::warn!(
                    "[config_apply] failed to set {key}={value} in {}: {e}",
                    clone_path.display()
                );
            }
        }
        Err(e) => {
            log::warn!(
                "[config_apply] could not check {key} in {}: {e}",
                clone_path.display()
            );
        }
    }
}

/// Apply staged-managed git config to a clone inside a blox workspace.
///
/// `repo_path` is the absolute path on the workstation (e.g.
/// `/home/bloxer/cash-server`). Cached version probe is per-workspace.
pub async fn apply_to_blox_clone(ws_name: &str, repo_path: &str) {
    if blox_supports_fsmonitor(ws_name).await {
        set_if_unset_blox(ws_name, repo_path, "core.fsmonitor", "true").await;
    }
    set_if_unset_blox(ws_name, repo_path, "core.untrackedcache", "true").await;
}

async fn set_if_unset_blox(ws_name: &str, repo_path: &str, key: &str, value: &str) {
    match crate::branches::ws_exec_async(
        ws_name,
        &["git", "-C", repo_path, "config", "--local", "--get", key],
    )
    .await
    {
        Ok(_) => {}
        Err(crate::blox::BloxError::CommandFailed(_)) => {
            if let Err(e) = crate::branches::ws_exec_async(
                ws_name,
                &["git", "-C", repo_path, "config", "--local", key, value],
            )
            .await
            {
                log::warn!(
                    "[config_apply] failed to set {key}={value} in {ws_name}:{repo_path}: {e}"
                );
            }
        }
        Err(e) => {
            log::warn!("[config_apply] could not check {key} in {ws_name}:{repo_path}: {e}");
        }
    }
}

async fn blox_supports_fsmonitor(ws_name: &str) -> bool {
    use std::collections::HashMap;
    use tokio::sync::Mutex;
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let guard = cache.lock().await;
        if let Some(v) = guard.get(ws_name) {
            return *v;
        }
    }

    let supported = match crate::branches::ws_exec_async(ws_name, &["git", "--version"]).await {
        Ok(out) => parse_git_version(&out)
            .map(version_supports_fsmonitor)
            .unwrap_or(false),
        Err(e) => {
            log::warn!("[config_apply] could not probe git version on {ws_name}: {e}");
            false
        }
    };
    cache.lock().await.insert(ws_name.to_string(), supported);
    supported
}

/// Migration: apply staged-managed config to every existing local clone under
/// `~/.staged/repos/<owner>/<repo>/`. Worktrees share the parent clone's
/// `.git/config` so we only touch the top-level clones.
pub fn migrate_existing_clones() -> Result<(), String> {
    let Some(repos_dir) = crate::paths::repos_dir() else {
        return Ok(());
    };
    if !repos_dir.exists() {
        return Ok(());
    }

    for clone in iter_clones(&repos_dir) {
        apply_to_clone(&clone);
    }
    Ok(())
}

fn iter_clones(repos_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(owners) = std::fs::read_dir(repos_dir) else {
        return out;
    };
    for owner in owners.flatten() {
        let owner_path = owner.path();
        if !owner_path.is_dir() {
            continue;
        }
        let Ok(repos) = std::fs::read_dir(&owner_path) else {
            continue;
        };
        for repo in repos.flatten() {
            let repo_path = repo.path();
            if repo_path.join(".git").is_dir() {
                out.push(repo_path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_version_string() {
        assert_eq!(parse_git_version("git version 2.54.0\n"), Some((2, 54)));
    }

    #[test]
    fn parses_apple_git_suffix() {
        assert_eq!(
            parse_git_version("git version 2.39.5 (Apple Git-154)\n"),
            Some((2, 39))
        );
    }

    #[test]
    fn parses_two_component_version() {
        assert_eq!(parse_git_version("git version 2.36"), Some((2, 36)));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_git_version(""), None);
        assert_eq!(parse_git_version("not a version"), None);
    }

    #[test]
    fn fsmonitor_supported_at_threshold() {
        assert!(version_supports_fsmonitor((2, 36)));
        assert!(version_supports_fsmonitor((2, 50)));
        assert!(version_supports_fsmonitor((3, 0)));
    }

    #[test]
    fn fsmonitor_unsupported_below_threshold() {
        assert!(!version_supports_fsmonitor((2, 35)));
        assert!(!version_supports_fsmonitor((1, 9)));
    }

    #[test]
    fn set_if_unset_local_is_idempotent() {
        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("a.txt", "hello\n");
        repo.commit("init");

        set_if_unset_local(repo.path(), "core.untrackedcache", "true");
        let v1 = super::super::cli::run(
            repo.path(),
            &["config", "--local", "--get", "core.untrackedcache"],
        )
        .expect("first set should leave key readable");
        assert_eq!(v1.trim(), "true");

        // Explicit value preserved — set_if_unset must not overwrite.
        super::super::cli::run(
            repo.path(),
            &["config", "--local", "core.untrackedcache", "false"],
        )
        .expect("override should succeed");
        set_if_unset_local(repo.path(), "core.untrackedcache", "true");
        let v2 = super::super::cli::run(
            repo.path(),
            &["config", "--local", "--get", "core.untrackedcache"],
        )
        .expect("after no-op set, key still readable");
        assert_eq!(
            v2.trim(),
            "false",
            "set_if_unset must not clobber explicit value"
        );
    }
}
