//! Bundled ACP bridge tool resolution and spawn-environment shaping.
//!
//! Staged ships pinned ACP bridge CLIs (`claude-agent-acp`, `codex-acp`) as
//! application resources (see `acp-tools.lock.json` and
//! `scripts/prepare-acp-tools-resource.sh`). This module resolves the staged
//! bin directory at runtime and shapes captured shell-env snapshots so the
//! bundled bridges win over user-installed copies while everything else on
//! the user's PATH (including installed harness CLIs and their auth state)
//! stays discoverable. The Staged-managed install dirs (`managed_acp_tools`:
//! bridge shims, the private npm prefix's bin, the managed Node runtime's
//! bin) are folded into the same shaping, so sessions and doctor share one
//! PATH layout.

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

/// Resolve the directory holding the bundled ACP bridge executables: the dev
/// env override wins, then the Tauri resource dir for packaged apps.
pub fn resolve_bundled_acp_tools_dir(app_handle: &tauri::AppHandle) -> Option<PathBuf> {
    bundled_acp_tools_dir_from_parts(
        crate::managed_acp_tools::dev_tools_override_dir(),
        app_handle
            .path()
            .resolve(ACP_TOOLS_RESOURCE_DIR, BaseDirectory::Resource)
            .ok()
            .as_deref(),
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

fn bundled_acp_tools_dir_from_parts(
    env_override: Option<PathBuf>,
    resource_dir: Option<&Path>,
) -> Option<PathBuf> {
    env_override.or_else(|| resource_dir.map(Path::to_path_buf))
}

/// Shape a captured shell-env snapshot so the bundled ACP bridges — and the
/// Staged-managed npm install locations — win:
///
/// - Prepend the tool search dirs (see [`tool_search_dirs`]) to the
///   snapshot's PATH, keeping the rest of the imported shell PATH intact so
///   user-installed CLIs (and their auth state) remain discoverable.
/// - Pin `GOOSE_SEARCH_PATHS` *after* the shell-env import so same-named
///   values from the user's shell cannot override the bundled tools. The
///   value is a JSON array to match Goose's config env parsing, scoped to
///   the same search dirs plus any explicit pre-existing Goose search dirs —
///   never the whole shell PATH.
///
/// Sessions and doctor checks/fixes both shape their snapshots here, so an
/// agent installed by a doctor fix into the private npm prefix resolves
/// identically at check time and at spawn time.
pub fn apply_bundled_tools_env(vars: &mut Vec<(String, String)>, bundled_dir: &Path) {
    let dirs = tool_search_dirs(bundled_dir);
    prepend_dirs_to_path(vars, &dirs);
    apply_goose_search_paths(vars, &dirs);
}

/// Every dir agent binaries resolve from, in precedence order: the bundled
/// (or `STAGED_ACP_TOOLS_DIR`) bridge dir, then the managed dirs — bridge
/// shims, the private npm prefix's bin, and the managed Node runtime's bin,
/// which also lets the bundled bridge wrappers find `node` without a host
/// install once the managed runtime exists.
fn tool_search_dirs(bundled_dir: &Path) -> Vec<PathBuf> {
    tool_search_dirs_from_parts(
        bundled_dir,
        crate::managed_acp_tools::managed_prepend_dirs(),
    )
}

fn tool_search_dirs_from_parts(bundled_dir: &Path, managed_dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    // The dev override dir appears both as `bundled_dir` and as the first
    // managed prepend when the env var is set; keep the first occurrence.
    let mut dirs = vec![bundled_dir.to_path_buf()];
    for dir in managed_dirs {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
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
        apply_goose_search_paths, bundled_acp_tools_dir_from_parts, prepend_dirs_to_path,
        tool_search_dirs_from_parts,
    };
    use std::path::{Path, PathBuf};

    fn var<'a>(vars: &'a [(String, String)], key: &str) -> Option<&'a str> {
        vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn dirs(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn env_override_wins_over_resource_dir() {
        assert_eq!(
            bundled_acp_tools_dir_from_parts(
                Some(PathBuf::from("/dev/acp/bin")),
                Some(Path::new("/bundle/resources/acp/bin")),
            )
            .as_deref(),
            Some(Path::new("/dev/acp/bin")),
        );
    }

    #[test]
    fn missing_env_override_falls_back_to_resource_dir() {
        assert_eq!(
            bundled_acp_tools_dir_from_parts(None, Some(Path::new("/bundle/resources/acp/bin")))
                .as_deref(),
            Some(Path::new("/bundle/resources/acp/bin")),
        );
    }

    #[test]
    fn missing_inputs_resolve_to_none() {
        assert!(bundled_acp_tools_dir_from_parts(None, None).is_none());
    }

    #[test]
    fn node_runtime_manifest_sits_beside_bin_dir() {
        assert_eq!(
            super::node_runtime_manifest_path(Path::new("/bundle/resources/acp/bin")).as_deref(),
            Some(Path::new("/bundle/resources/acp/node-runtime.json")),
        );
        assert!(super::node_runtime_manifest_path(Path::new("/")).is_none());
    }

    #[test]
    fn tool_search_dirs_start_with_bundled_then_managed() {
        assert_eq!(
            tool_search_dirs_from_parts(
                Path::new("/bundle/acp/bin"),
                dirs(&["/data/packages/bin", "/data/packages/npm-prefix/bin"]),
            ),
            dirs(&[
                "/bundle/acp/bin",
                "/data/packages/bin",
                "/data/packages/npm-prefix/bin",
            ])
        );
    }

    #[test]
    fn tool_search_dirs_dedupe_the_dev_override() {
        // With STAGED_ACP_TOOLS_DIR set, the override dir arrives both as the
        // bundled dir and as the first managed prepend; it must appear once.
        assert_eq!(
            tool_search_dirs_from_parts(
                Path::new("/dev/acp/bin"),
                dirs(&["/dev/acp/bin", "/data/packages/npm-prefix/bin"]),
            ),
            dirs(&["/dev/acp/bin", "/data/packages/npm-prefix/bin"])
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
