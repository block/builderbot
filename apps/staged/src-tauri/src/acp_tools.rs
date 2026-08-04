//! ACP bridge tool resolution and spawn-environment shaping.
//!
//! The claude/codex ACP bridges resolve from the `STAGED_ACP_TOOLS_DIR` dev
//! override or the Staged-managed bridge shims the startup reconciler
//! installs (`managed_acp_tools`). This module shapes captured shell-env
//! snapshots so those dirs — together with the other Staged-managed install
//! dirs (the private npm prefix's bin, the managed Node runtime's bin) — win
//! over user-installed copies while everything else on the user's PATH
//! (including installed harness CLIs and their auth state) stays
//! discoverable. Sessions and doctor both shape their snapshots here, so an
//! agent installed by a doctor fix resolves identically at check time and at
//! spawn time.

use std::path::PathBuf;

/// Goose reads extra binary search dirs from this env var as a JSON array.
const GOOSE_SEARCH_PATHS_ENV: &str = "GOOSE_SEARCH_PATHS";

/// The highest-precedence bridge dir: the `STAGED_ACP_TOOLS_DIR` dev
/// override, else the managed shim dir (when this build manages bridges).
/// Registered with `acp_client::set_bundled_tools_dir` and labeled `Bundled`
/// by doctor — Staged owns updates for whatever resolves here, so the user
/// is never nagged to update it manually.
pub fn primary_tools_dir() -> Option<PathBuf> {
    // The shim dir is already `None` while the dev override is active, so
    // the override wins whenever both could apply.
    crate::managed_acp_tools::dev_tools_override_dir()
        .or_else(crate::managed_acp_tools::managed_shim_bin_dir)
}

/// Shape a captured shell-env snapshot so the Staged-managed install dirs
/// ([`crate::managed_acp_tools::managed_prepend_dirs`]) win:
///
/// - Prepend them to the snapshot's PATH, keeping the rest of the imported
///   shell PATH intact so user-installed CLIs (and their auth state) remain
///   discoverable.
/// - Pin `GOOSE_SEARCH_PATHS` *after* the shell-env import so same-named
///   values from the user's shell cannot override the managed tools. The
///   value is a JSON array to match Goose's config env parsing, scoped to
///   the same search dirs plus any explicit pre-existing Goose search dirs —
///   never the whole shell PATH.
pub fn apply_managed_tools_env(vars: &mut Vec<(String, String)>) {
    apply_tools_env_with_dirs(vars, &crate::managed_acp_tools::managed_prepend_dirs());
}

fn apply_tools_env_with_dirs(vars: &mut Vec<(String, String)>, dirs: &[PathBuf]) {
    if dirs.is_empty() {
        return;
    }
    prepend_dirs_to_path(vars, dirs);
    apply_goose_search_paths(vars, dirs);
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
    use super::{apply_goose_search_paths, apply_tools_env_with_dirs, prepend_dirs_to_path};
    use std::path::PathBuf;

    fn var<'a>(vars: &'a [(String, String)], key: &str) -> Option<&'a str> {
        vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn dirs(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn no_managed_dirs_leaves_the_snapshot_untouched() {
        // A build with nothing to manage (no override, no managed dirs) must
        // not touch PATH or invent a GOOSE_SEARCH_PATHS.
        let mut vars = vec![("PATH".to_string(), "/shell/bin".to_string())];
        apply_tools_env_with_dirs(&mut vars, &[]);
        assert_eq!(vars, vec![("PATH".to_string(), "/shell/bin".to_string())]);
    }

    #[test]
    fn managed_dirs_shape_path_and_goose_search_paths_together() {
        let mut vars = vec![("PATH".to_string(), "/shell/bin".to_string())];

        apply_tools_env_with_dirs(
            &mut vars,
            &dirs(&["/data/packages/bin", "/data/packages/npm-prefix/bin"]),
        );

        let path = var(&vars, "PATH").expect("PATH should be set");
        let paths: Vec<_> = std::env::split_paths(path).collect();
        assert_eq!(
            paths,
            dirs(&[
                "/data/packages/bin",
                "/data/packages/npm-prefix/bin",
                "/shell/bin",
            ])
        );
        let goose = var(&vars, "GOOSE_SEARCH_PATHS").expect("GOOSE_SEARCH_PATHS should be set");
        let goose_paths: Vec<String> = serde_json::from_str(goose).expect("valid JSON array");
        assert_eq!(
            goose_paths,
            vec!["/data/packages/bin", "/data/packages/npm-prefix/bin"]
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
