//! ACP bridge tool resolution and spawn-environment shaping.
//!
//! The claude/codex ACP bridges resolve, in precedence order, from the
//! `STAGED_ACP_TOOLS_DIR` dev override, the Staged-managed bridge shims the
//! startup reconciler installs (`managed_acp_tools`), and — until the step-4
//! bundle flip — the pinned bridges Staged still ships as application
//! resources (see `acp-tools.lock.json` and
//! `scripts/prepare-acp-tools-resource.sh`). This module resolves those
//! directories at runtime and shapes captured shell-env snapshots so they
//! win over user-installed copies while everything else on the user's PATH
//! (including installed harness CLIs and their auth state) stays
//! discoverable. The other Staged-managed install dirs (the private npm
//! prefix's bin, the managed Node runtime's bin) are folded into the same
//! shaping, so sessions and doctor share one PATH layout.

use std::path::{Path, PathBuf};

use tauri::path::BaseDirectory;
use tauri::Manager;

/// Dev-mode override exported by `just dev`, pointing at the freshly staged
/// `src-tauri/resources/acp/bin` in the working tree. The env var also
/// disables managed bridge installs (see `managed_acp_tools`).
pub use crate::managed_acp_tools::ACP_TOOLS_DIR_ENV;

/// Bundled resource path, relative to the Tauri resource dir (mirrors the
/// `resources/acp` entry in `tauri.conf.json`).
const ACP_TOOLS_RESOURCE_DIR: &str = "resources/acp/bin";
/// Node runtime manifest staged by `scripts/prepare-acp-tools-resource.sh`
/// next to the bundled bin dir.
const NODE_RUNTIME_MANIFEST_FILE: &str = "node-runtime.json";
/// Goose reads extra binary search dirs from this env var as a JSON array.
const GOOSE_SEARCH_PATHS_ENV: &str = "GOOSE_SEARCH_PATHS";

/// The resolved ACP bridge directories for this build and environment.
#[derive(Clone, Debug, Default)]
pub struct AcpToolsDirs {
    /// The highest-precedence bridge dir: the `STAGED_ACP_TOOLS_DIR` dev
    /// override, else the managed shim dir (when this build manages
    /// bridges), else the bundled resource dir. Registered with
    /// `acp_client::set_bundled_tools_dir` and labeled `Bundled` by doctor —
    /// Staged owns updates for whatever resolves here, so the user is never
    /// nagged to update it manually.
    pub primary: Option<PathBuf>,
    /// The bundled resource dir when it is not already the primary: it stays
    /// on the spawned-env search path as the last-resort fallback until the
    /// step-4 bundle flip, so the bundled bridges keep working (for doctor
    /// checks and agent subprocesses) on profiles the reconciler has not
    /// populated yet.
    pub resource_fallback: Option<PathBuf>,
}

/// Resolve the ACP bridge directories: dev env override → managed shim dir →
/// bundled resource dir, with the resource dir kept as a trailing search-dir
/// fallback while it is not the primary.
pub fn resolve_acp_tools_dirs(app_handle: &tauri::AppHandle) -> AcpToolsDirs {
    acp_tools_dirs_from_parts(
        crate::managed_acp_tools::dev_tools_override_dir(),
        crate::managed_acp_tools::managed_shim_bin_dir(),
        app_handle
            .path()
            .resolve(ACP_TOOLS_RESOURCE_DIR, BaseDirectory::Resource)
            .ok(),
    )
}

/// Path of the Node runtime manifest staged by
/// `scripts/prepare-acp-tools-resource.sh`: it lives next to the tools bin
/// dir (`acp/node-runtime.json` beside `acp/bin`), so it resolves for both
/// the bundled resource dir and a `STAGED_ACP_TOOLS_DIR` dev override.
pub fn node_runtime_manifest_path(bin_dir: &Path) -> Option<PathBuf> {
    bin_dir
        .parent()
        .map(|dir| dir.join(NODE_RUNTIME_MANIFEST_FILE))
}

fn acp_tools_dirs_from_parts(
    env_override: Option<PathBuf>,
    managed_shim_dir: Option<PathBuf>,
    resource_dir: Option<PathBuf>,
) -> AcpToolsDirs {
    let primary = env_override
        .or(managed_shim_dir)
        .or_else(|| resource_dir.clone());
    let resource_fallback = resource_dir.filter(|dir| primary.as_deref() != Some(dir));
    AcpToolsDirs {
        primary,
        resource_fallback,
    }
}

/// Shape a captured shell-env snapshot so the managed/bundled ACP bridges —
/// and the Staged-managed npm install locations — win:
///
/// - Prepend the tool search dirs (see [`tool_search_dirs`]) to the
///   snapshot's PATH, keeping the rest of the imported shell PATH intact so
///   user-installed CLIs (and their auth state) remain discoverable.
/// - Pin `GOOSE_SEARCH_PATHS` *after* the shell-env import so same-named
///   values from the user's shell cannot override the managed tools. The
///   value is a JSON array to match Goose's config env parsing, scoped to
///   the same search dirs plus any explicit pre-existing Goose search dirs —
///   never the whole shell PATH.
///
/// Sessions and doctor checks/fixes both shape their snapshots here, so an
/// agent installed by a doctor fix into the private npm prefix resolves
/// identically at check time and at spawn time.
pub fn apply_bundled_tools_env(vars: &mut Vec<(String, String)>, dirs: &AcpToolsDirs) {
    let search_dirs = tool_search_dirs(dirs);
    if search_dirs.is_empty() {
        return;
    }
    prepend_dirs_to_path(vars, &search_dirs);
    apply_goose_search_paths(vars, &search_dirs);
}

/// Every dir agent binaries resolve from, in precedence order: the primary
/// bridge dir (dev override or managed shims), then the remaining managed
/// dirs — the private npm prefix's bin and the managed Node runtime's bin,
/// which also lets the bundled bridge wrappers find `node` without a host
/// install once the managed runtime exists — and the bundled resource dir
/// last, as the fallback for bridges the reconciler has not installed yet.
fn tool_search_dirs(dirs: &AcpToolsDirs) -> Vec<PathBuf> {
    tool_search_dirs_from_parts(dirs, crate::managed_acp_tools::managed_prepend_dirs())
}

fn tool_search_dirs_from_parts(dirs: &AcpToolsDirs, managed_dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    // The primary dir also appears as the first managed prepend (the dev
    // override or the shim dir); keep the first occurrence of each dir.
    let mut search_dirs: Vec<PathBuf> = dirs.primary.clone().into_iter().collect();
    for dir in managed_dirs
        .into_iter()
        .chain(dirs.resource_fallback.clone())
    {
        if !search_dirs.contains(&dir) {
            search_dirs.push(dir);
        }
    }
    search_dirs
}

fn prepend_dirs_to_path(vars: &mut Vec<(String, String)>, dirs: &[PathBuf]) {
    match vars.iter_mut().find(|(key, _)| key == "PATH") {
        Some((_, value)) => {
            let mut paths = dirs.to_vec();
            paths.extend(std::env::split_paths(value).filter(|path| !dirs.contains(path)));
            *value = crate::shell_env::join_paths_best_effort(paths);
        }
        None => vars.push((
            "PATH".to_string(),
            crate::shell_env::join_paths_best_effort(dirs.to_vec()),
        )),
    }
}

fn apply_goose_search_paths(vars: &mut Vec<(String, String)>, dirs: &[PathBuf]) {
    let mut search_paths: Vec<String> = dirs
        .iter()
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect();
    if let Some((_, existing)) = vars.iter().find(|(key, _)| key == GOOSE_SEARCH_PATHS_ENV) {
        match parse_goose_search_paths(existing) {
            Ok(paths) => search_paths.extend(paths),
            Err(error) => log::warn!("Ignoring invalid {GOOSE_SEARCH_PATHS_ENV}: {error}"),
        }
    }

    let value = serde_json::to_string(&search_paths)
        .expect("serializing Goose search path strings should not fail");
    match vars
        .iter_mut()
        .find(|(key, _)| key == GOOSE_SEARCH_PATHS_ENV)
    {
        Some((_, existing)) => *existing = value,
        None => vars.push((GOOSE_SEARCH_PATHS_ENV.to_string(), value)),
    }
}

fn parse_goose_search_paths(value: &str) -> Result<Vec<String>, serde_json::Error> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(value)
}

#[cfg(test)]
mod tests {
    use super::{
        acp_tools_dirs_from_parts, apply_goose_search_paths, prepend_dirs_to_path,
        tool_search_dirs_from_parts, AcpToolsDirs,
    };
    use std::path::{Path, PathBuf};

    fn var<'a>(vars: &'a [(String, String)], key: &str) -> Option<&'a str> {
        vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn dirs(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn env_override_wins_over_shim_and_resource_dirs() {
        let resolved = acp_tools_dirs_from_parts(
            Some(PathBuf::from("/dev/acp/bin")),
            Some(PathBuf::from("/data/packages/bin")),
            Some(PathBuf::from("/bundle/resources/acp/bin")),
        );
        assert_eq!(resolved.primary.as_deref(), Some(Path::new("/dev/acp/bin")));
        assert_eq!(
            resolved.resource_fallback.as_deref(),
            Some(Path::new("/bundle/resources/acp/bin")),
        );
    }

    #[test]
    fn managed_shim_dir_wins_over_resource_dir() {
        let resolved = acp_tools_dirs_from_parts(
            None,
            Some(PathBuf::from("/data/packages/bin")),
            Some(PathBuf::from("/bundle/resources/acp/bin")),
        );
        assert_eq!(
            resolved.primary.as_deref(),
            Some(Path::new("/data/packages/bin")),
        );
        assert_eq!(
            resolved.resource_fallback.as_deref(),
            Some(Path::new("/bundle/resources/acp/bin")),
        );
    }

    #[test]
    fn resource_dir_as_primary_is_not_repeated_as_fallback() {
        // No override and no managed shims (e.g. no-managed-acp-tools
        // builds): the resource dir is the primary and must not double as
        // the fallback.
        let resolved =
            acp_tools_dirs_from_parts(None, None, Some(PathBuf::from("/bundle/resources/acp/bin")));
        assert_eq!(
            resolved.primary.as_deref(),
            Some(Path::new("/bundle/resources/acp/bin")),
        );
        assert!(resolved.resource_fallback.is_none());
    }

    #[test]
    fn missing_inputs_resolve_to_none() {
        let resolved = acp_tools_dirs_from_parts(None, None, None);
        assert!(resolved.primary.is_none());
        assert!(resolved.resource_fallback.is_none());
    }

    #[test]
    fn node_runtime_manifest_sits_beside_bin_dir() {
        assert_eq!(
            super::node_runtime_manifest_path(Path::new("/bundle/resources/acp/bin")).as_deref(),
            Some(Path::new("/bundle/resources/acp/node-runtime.json")),
        );
        assert!(super::node_runtime_manifest_path(Path::new("/")).is_none());
    }

    fn tools_dirs(primary: &str, resource_fallback: Option<&str>) -> AcpToolsDirs {
        AcpToolsDirs {
            primary: Some(PathBuf::from(primary)),
            resource_fallback: resource_fallback.map(PathBuf::from),
        }
    }

    #[test]
    fn tool_search_dirs_order_shims_then_managed_then_resource_fallback() {
        assert_eq!(
            tool_search_dirs_from_parts(
                &tools_dirs("/data/packages/bin", Some("/bundle/acp/bin")),
                dirs(&["/data/packages/bin", "/data/packages/npm-prefix/bin"]),
            ),
            dirs(&[
                "/data/packages/bin",
                "/data/packages/npm-prefix/bin",
                "/bundle/acp/bin",
            ])
        );
    }

    #[test]
    fn tool_search_dirs_dedupe_the_dev_override() {
        // With STAGED_ACP_TOOLS_DIR set, the override dir arrives both as the
        // primary and as the first managed prepend; it must appear once.
        assert_eq!(
            tool_search_dirs_from_parts(
                &tools_dirs("/dev/acp/bin", Some("/bundle/acp/bin")),
                dirs(&["/dev/acp/bin", "/data/packages/npm-prefix/bin"]),
            ),
            dirs(&[
                "/dev/acp/bin",
                "/data/packages/npm-prefix/bin",
                "/bundle/acp/bin",
            ])
        );
    }

    #[test]
    fn tool_search_dirs_handle_a_resource_only_resolution() {
        // Bundled-resource primary (nothing managed): no duplicate, managed
        // prefix dirs still searched.
        assert_eq!(
            tool_search_dirs_from_parts(
                &tools_dirs("/bundle/acp/bin", None),
                dirs(&["/data/packages/npm-prefix/bin"]),
            ),
            dirs(&["/bundle/acp/bin", "/data/packages/npm-prefix/bin"])
        );
    }

    #[test]
    fn tool_dirs_are_prepended_before_shell_path() {
        let mut vars = vec![
            ("PATH".to_string(), "/shell/bin:/user/bin".to_string()),
            ("LANG".to_string(), "en_US.UTF-8".to_string()),
        ];

        prepend_dirs_to_path(&mut vars, &dirs(&["/acp/bin", "/packages/npm-prefix/bin"]));

        let path = var(&vars, "PATH").expect("PATH should be set");
        let paths: Vec<_> = std::env::split_paths(path).collect();
        assert_eq!(
            paths,
            dirs(&[
                "/acp/bin",
                "/packages/npm-prefix/bin",
                "/shell/bin",
                "/user/bin",
            ])
        );
        // Non-PATH variables are untouched.
        assert_eq!(var(&vars, "LANG"), Some("en_US.UTF-8"));
    }

    #[test]
    fn tool_dirs_are_not_duplicated_in_path() {
        let mut vars = vec![("PATH".to_string(), "/shell/bin:/acp/bin".to_string())];

        prepend_dirs_to_path(&mut vars, &dirs(&["/acp/bin"]));

        let path = var(&vars, "PATH").expect("PATH should be set");
        let paths: Vec<_> = std::env::split_paths(path).collect();
        assert_eq!(paths, dirs(&["/acp/bin", "/shell/bin"]));
    }

    #[test]
    #[cfg(unix)]
    fn unjoinable_tool_dir_keeps_shell_path_instead_of_emptying_it() {
        // A dir embedding the separator (legal in macOS paths) can't be joined
        // into PATH; it must be dropped, not erase every shell search path.
        let mut vars = vec![("PATH".to_string(), "/shell/bin:/user/bin".to_string())];

        prepend_dirs_to_path(&mut vars, &dirs(&["/weird:dir/bin"]));

        let path = var(&vars, "PATH").expect("PATH should be set");
        let paths: Vec<_> = std::env::split_paths(path).collect();
        assert!(paths.contains(&PathBuf::from("/shell/bin")));
        assert!(paths.contains(&PathBuf::from("/user/bin")));
        assert!(!paths.iter().any(|p| p.to_string_lossy().contains("weird")));
    }

    #[test]
    fn missing_path_gets_tool_dirs_only() {
        let mut vars = Vec::new();

        prepend_dirs_to_path(&mut vars, &dirs(&["/acp/bin"]));

        assert_eq!(var(&vars, "PATH"), Some("/acp/bin"));
    }

    #[test]
    fn goose_search_paths_is_set_as_json_array() {
        let mut vars = vec![("PATH".to_string(), "/shell/bin".to_string())];

        apply_goose_search_paths(&mut vars, &dirs(&["/acp/bin", "/packages/npm-prefix/bin"]));

        let value = var(&vars, "GOOSE_SEARCH_PATHS").expect("GOOSE_SEARCH_PATHS should be set");
        let paths: Vec<String> = serde_json::from_str(value).expect("valid JSON array");
        assert_eq!(paths, vec!["/acp/bin", "/packages/npm-prefix/bin"]);
    }

    #[test]
    fn goose_search_paths_keeps_existing_goose_dirs_without_shell_path() {
        let mut vars = vec![
            ("PATH".to_string(), "/shell/bin:/user/bin".to_string()),
            (
                "GOOSE_SEARCH_PATHS".to_string(),
                "[\"/custom/goose/bin\"]".to_string(),
            ),
        ];

        apply_goose_search_paths(&mut vars, &dirs(&["/acp/bin"]));

        let value = var(&vars, "GOOSE_SEARCH_PATHS").expect("GOOSE_SEARCH_PATHS should be set");
        let paths: Vec<String> = serde_json::from_str(value).expect("valid JSON array");
        assert_eq!(paths, vec!["/acp/bin", "/custom/goose/bin"]);
        assert!(!paths.iter().any(|path| path == "/shell/bin"));
        assert!(!paths.iter().any(|path| path == "/user/bin"));
    }

    #[test]
    fn invalid_existing_goose_search_paths_is_replaced() {
        let mut vars = vec![(
            "GOOSE_SEARCH_PATHS".to_string(),
            "/not/a/json/array".to_string(),
        )];

        apply_goose_search_paths(&mut vars, &dirs(&["/acp/bin"]));

        let value = var(&vars, "GOOSE_SEARCH_PATHS").expect("GOOSE_SEARCH_PATHS should be set");
        let paths: Vec<String> = serde_json::from_str(value).expect("valid JSON array");
        assert_eq!(paths, vec!["/acp/bin"]);
    }
}
