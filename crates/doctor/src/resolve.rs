//! Binary resolution and command output formatting helpers.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::types::{InstallSource, ResolvedBinary};

/// Resolve a binary by trying login shell path lookup, common install paths,
/// then npm global install dirs.
pub fn resolve_binary(cmd: &str) -> ResolvedBinary {
    let mut lines = vec![format!("resolve '{cmd}':")];

    // Strategy 1: Login shell path lookup (primary)
    lines.push("  strategy 1 — login shell path lookup:".to_string());
    for (shell, lookup_cmd) in shell_lookup_commands(cmd) {
        match Command::new(shell).args(["-l", "-c", &lookup_cmd]).output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !output.status.success() {
                    lines.push(format!("    {shell} -l -c '{lookup_cmd}' => not found"));
                    continue;
                }

                let candidate_paths = candidate_paths_from_shell_output(stdout.as_ref());
                if let Some(path) = candidate_paths
                    .iter()
                    .rev()
                    .find(|path| is_executable_file(path))
                {
                    lines.push(format!(
                        "    {shell} -l -c '{lookup_cmd}' => {} (resolved)",
                        path.display()
                    ));
                    let install_source = Some(detect_install_source(path));
                    return ResolvedBinary {
                        path: Some(path.clone()),
                        search_output: lines.join("\n"),
                        install_source,
                    };
                }

                if let Some(path) = candidate_paths.first() {
                    lines.push(format!(
                        "    {shell} -l -c '{lookup_cmd}' => {} (ignored: not an executable file)",
                        path.display()
                    ));
                } else if stdout.trim().is_empty() {
                    lines.push(format!("    {shell} -l -c '{lookup_cmd}' => not found"));
                } else {
                    lines.push(format!(
                        "    {shell} -l -c '{lookup_cmd}' => {} (ignored: not an absolute path)",
                        summarize_output(stdout.as_ref())
                    ));
                }
            }
            Err(e) => {
                lines.push(format!("    {shell} -l -c '{lookup_cmd}' => error: {e}"));
            }
        }
    }

    // Strategy 2: Common install paths (fallback).
    lines.push("  strategy 2 — common install paths:".to_string());
    for dir in &[
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/home/linuxbrew/.linuxbrew/bin",
    ] {
        let path = PathBuf::from(dir).join(cmd);
        if is_executable_file(&path) {
            lines.push(format!("    {} => found (resolved)", path.display()));
            let install_source = Some(detect_install_source(&path));
            return ResolvedBinary {
                path: Some(path),
                search_output: lines.join("\n"),
                install_source,
            };
        }
        lines.push(format!("    {} => not found", path.display()));
    }

    // Strategy 3: npm global install dirs.
    //
    // Mirrors the dirs added by goose-internal's `services::path_env` (and the
    // upstream goose `config::search_path::SearchPaths::with_npm`) so that
    // bridges installed via `npm install -g` resolve here too. Without this,
    // npm-only installs are "found" by goose-internal's ACP inventory but
    // reported missing by doctor.
    lines.push("  strategy 3 — npm global install dirs:".to_string());
    let home = std::env::home_dir();
    if let Some(home) = home.as_deref() {
        for dir in npm_search_dirs(home) {
            let path = dir.join(cmd);
            if path.exists() {
                lines.push(format!("    {} => found (resolved)", path.display()));
                let install_source = Some(detect_install_source(&path));
                return ResolvedBinary {
                    path: Some(path),
                    search_output: lines.join("\n"),
                    install_source,
                };
            }
            lines.push(format!("    {} => not found", path.display()));
        }
    } else {
        lines.push("    (could not determine HOME)".to_string());
    }

    // Strategy 3 last resort: ask npm itself (the most authoritative answer,
    // but costs one subprocess — only invoked when the static probes above
    // didn't find the binary). `npm prefix -g` is the version-stable
    // equivalent of the older `npm bin -g`; the bin dir is `<prefix>/bin`.
    if let Some(npm_bin_dir) = npm_global_bin_dir(&mut lines) {
        let path = npm_bin_dir.join(cmd);
        if path.exists() {
            lines.push(format!("    {} => found (resolved)", path.display()));
            let install_source = Some(detect_install_source(&path));
            return ResolvedBinary {
                path: Some(path),
                search_output: lines.join("\n"),
                install_source,
            };
        }
        lines.push(format!("    {} => not found", path.display()));
    }

    lines.push("  not found in any location".to_string());
    ResolvedBinary {
        path: None,
        search_output: lines.join("\n"),
        install_source: None,
    }
}

fn shell_lookup_commands(cmd: &str) -> [(&'static str, String); 2] {
    let quoted = shell_quote(cmd);
    [
        ("/bin/zsh", format!("whence -p -- {quoted}")),
        ("/bin/bash", format!("type -P -- {quoted}")),
    ]
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn candidate_paths_from_shell_output(output: &str) -> Vec<PathBuf> {
    output
        .lines()
        .map(str::trim)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect()
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn summarize_output(output: &str) -> String {
    let trimmed = output.trim();
    const MAX_LEN: usize = 120;
    if trimmed.len() <= MAX_LEN {
        return trimmed.replace('\n', "\\n");
    }
    let summary: String = trimmed.chars().take(MAX_LEN).collect();
    format!("{}...", summary.replace('\n', "\\n"))
}

/// Candidate npm global install dirs to probe for a given $HOME.
///
/// Includes the standard per-user dirs plus all detected nvm node versions
/// (both `~/.nvm` and Homebrew's `/opt/homebrew/opt/nvm` layout).
fn npm_search_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![home.join(".npm-global/bin"), home.join(".npm/bin")];

    // ~/.nvm/versions/node/*/bin
    for version_dir in read_subdirs(&home.join(".nvm/versions/node")) {
        dirs.push(version_dir.join("bin"));
    }

    // macOS Homebrew nvm: /opt/homebrew/opt/nvm/versions/node/*/bin
    #[cfg(target_os = "macos")]
    for version_dir in read_subdirs(Path::new("/opt/homebrew/opt/nvm/versions/node")) {
        dirs.push(version_dir.join("bin"));
    }

    dirs
}

fn read_subdirs(parent: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(parent) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn npm_global_bin_dir(lines: &mut Vec<String>) -> Option<PathBuf> {
    let output = Command::new("npm").args(["prefix", "-g"]).output().ok()?;
    if !output.status.success() {
        lines.push("    npm prefix -g => failed".to_string());
        return None;
    }
    let prefix = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if prefix.is_empty() {
        lines.push("    npm prefix -g => empty output".to_string());
        return None;
    }
    let bin = PathBuf::from(prefix).join("bin");
    lines.push(format!("    npm prefix -g => {}", bin.display()));
    Some(bin)
}

/// Infer how a binary was installed from its path alone — no subprocess or
/// network probes. Path-prefix heuristics cover Brew, Cargo, Mise, Asdf, Npm
/// (mirroring the dirs in [`npm_search_dirs`]), and the System dirs; anything
/// else (including `~/.local/bin` curl-pipe installs, which can't be
/// fingerprinted reliably from the path) is reported as [`InstallSource::Unknown`].
pub(crate) fn detect_install_source(path: &Path) -> InstallSource {
    let home = std::env::home_dir();
    detect_install_source_with_home(path, home.as_deref())
}

/// Testable inner: same logic as [`detect_install_source`] but takes the home
/// directory as a parameter so unit tests can inject a fixed value.
fn detect_install_source_with_home(path: &Path, home: Option<&Path>) -> InstallSource {
    // Homebrew-managed nvm — checked before the generic `/opt/homebrew/`
    // Brew prefix, since this path is a strict subset of it.
    if path.starts_with("/opt/homebrew/opt/nvm/versions/node") {
        return InstallSource::Npm;
    }

    // Brew (path-prefix). `/usr/local/Cellar` covers Intel-mac brew; if a
    // binary appears as `/usr/local/bin/<x>` that's a symlink into Cellar, we
    // only follow the chain if `canonicalize` succeeds cheaply.
    if path.starts_with("/opt/homebrew/")
        || path.starts_with("/usr/local/Cellar/")
        || path.starts_with("/home/linuxbrew/.linuxbrew/")
    {
        return InstallSource::Brew;
    }
    if path.starts_with("/usr/local/bin/") {
        if let Ok(canonical) = path.canonicalize() {
            if canonical.starts_with("/usr/local/Cellar/") {
                return InstallSource::Brew;
            }
        }
    }

    if let Some(home) = home {
        if path.starts_with(home.join(".cargo/bin")) {
            return InstallSource::Cargo;
        }
        if path.starts_with(home.join(".local/share/mise")) || path.starts_with(home.join(".mise"))
        {
            return InstallSource::Mise;
        }
        if path.starts_with(home.join(".asdf")) {
            return InstallSource::Asdf;
        }
        if path.starts_with(home.join(".npm-global/bin"))
            || path.starts_with(home.join(".npm/bin"))
            || path.starts_with(home.join(".nvm/versions/node"))
        {
            return InstallSource::Npm;
        }
    }

    // System dirs. Checked last so e.g. a Brew binary surfacing at
    // `/usr/local/bin/x` was already classified above.
    if path.starts_with("/usr/bin/")
        || path.starts_with("/bin/")
        || path.starts_with("/usr/sbin/")
        || path.starts_with("/sbin/")
    {
        return InstallSource::System;
    }

    InstallSource::Unknown
}

/// Format the raw output of a command invocation for debug diagnostics.
pub fn format_command_output(cmd_desc: &str, output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut raw = format!("$ {cmd_desc}\nexit code: {}", output.status);
    if !stdout.trim().is_empty() {
        raw.push_str(&format!("\nstdout:\n{}", stdout.trim()));
    }
    if !stderr.trim().is_empty() {
        raw.push_str(&format!("\nstderr:\n{}", stderr.trim()));
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    #[test]
    fn candidate_accepts_single_absolute_path() {
        assert_eq!(
            candidate_paths_from_shell_output("/opt/homebrew/bin/git\n"),
            vec![PathBuf::from("/opt/homebrew/bin/git")]
        );
    }

    #[test]
    fn candidate_tolerates_startup_output_before_absolute_path() {
        assert_eq!(
            candidate_paths_from_shell_output("hello from shell init\n/opt/homebrew/bin/git\n"),
            vec![PathBuf::from("/opt/homebrew/bin/git")]
        );
    }

    #[test]
    fn candidate_rejects_function_body_output() {
        let output = "git () {\n\tcommand git \"$@\"\n}\n";
        assert_eq!(
            candidate_paths_from_shell_output(output),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn candidate_rejects_relative_or_command_name_output() {
        assert_eq!(
            candidate_paths_from_shell_output("git\n"),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn shell_quote_handles_single_quotes() {
        assert_eq!(shell_quote("git'bad"), "'git'\\''bad'");
    }

    #[test]
    fn picks_last_executable_when_rc_file_echoes_absolute_path() {
        // Simulates a rc file printing an absolute path of an unrelated
        // executable before the shell builtin prints the real lookup answer.
        let dir = std::env::temp_dir().join(format!("doctor-resolve-last-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let decoy = dir.join("decoy");
        let real = dir.join("real");
        File::create(&decoy).unwrap();
        File::create(&real).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&decoy, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let stdout = format!("{}\n{}\n", decoy.display(), real.display());
        let candidates = candidate_paths_from_shell_output(&stdout);
        let picked = candidates.iter().rev().find(|p| is_executable_file(p));

        assert_eq!(picked, Some(&real));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn executable_file_validation_checks_file_and_mode() {
        let dir = std::env::temp_dir().join(format!("doctor-resolve-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let executable = dir.join("tool");
        File::create(&executable).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&executable, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!is_executable_file(&executable));

            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(is_executable_file(&executable));
        }

        #[cfg(not(unix))]
        {
            assert!(is_executable_file(&executable));
        }

        assert!(!is_executable_file(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn npm_search_dirs_includes_expected_paths_for_fixed_home() {
        let home = PathBuf::from("/home/test");
        let dirs = npm_search_dirs(&home);

        assert!(
            dirs.iter().any(|p| p == &home.join(".npm-global/bin")),
            "missing ~/.npm-global/bin in {dirs:?}"
        );
        assert!(
            dirs.iter().any(|p| p == &home.join(".npm/bin")),
            "missing ~/.npm/bin in {dirs:?}"
        );
    }

    #[test]
    fn detect_install_source_classifies_brew() {
        assert_eq!(
            detect_install_source_with_home(Path::new("/opt/homebrew/bin/git"), None),
            InstallSource::Brew,
        );
        assert_eq!(
            detect_install_source_with_home(Path::new("/home/linuxbrew/.linuxbrew/bin/git"), None),
            InstallSource::Brew,
        );
    }

    #[test]
    fn detect_install_source_classifies_system() {
        assert_eq!(
            detect_install_source_with_home(Path::new("/usr/bin/git"), None),
            InstallSource::System,
        );
        assert_eq!(
            detect_install_source_with_home(Path::new("/bin/sh"), None),
            InstallSource::System,
        );
    }

    #[test]
    fn detect_install_source_classifies_cargo_under_home() {
        let home = PathBuf::from("/home/test");
        assert_eq!(
            detect_install_source_with_home(&home.join(".cargo/bin/cargo"), Some(home.as_path())),
            InstallSource::Cargo,
        );
    }

    #[test]
    fn detect_install_source_classifies_npm_dirs() {
        let home = PathBuf::from("/home/test");
        assert_eq!(
            detect_install_source_with_home(
                &home.join(".npm-global/bin/foo"),
                Some(home.as_path())
            ),
            InstallSource::Npm,
        );
        assert_eq!(
            detect_install_source_with_home(
                &home.join(".nvm/versions/node/v20.10.0/bin/foo"),
                Some(home.as_path())
            ),
            InstallSource::Npm,
        );
        assert_eq!(
            detect_install_source_with_home(
                Path::new("/opt/homebrew/opt/nvm/versions/node/v20.10.0/bin/foo"),
                None,
            ),
            InstallSource::Npm,
        );
    }

    #[test]
    fn detect_install_source_classifies_mise_and_asdf() {
        let home = PathBuf::from("/home/test");
        assert_eq!(
            detect_install_source_with_home(
                &home.join(".local/share/mise/installs/node/20/bin/foo"),
                Some(home.as_path()),
            ),
            InstallSource::Mise,
        );
        assert_eq!(
            detect_install_source_with_home(
                &home.join(".asdf/installs/nodejs/20/bin/foo"),
                Some(home.as_path()),
            ),
            InstallSource::Asdf,
        );
    }

    #[test]
    fn detect_install_source_unknown_for_curl_pipe_and_other() {
        let home = PathBuf::from("/home/test");
        // ~/.local/bin and ~/bin are common curl-pipe targets but unreliable to
        // fingerprint from path alone — Unknown beats false positives.
        assert_eq!(
            detect_install_source_with_home(&home.join(".local/bin/foo"), Some(home.as_path())),
            InstallSource::Unknown,
        );
        assert_eq!(
            detect_install_source_with_home(Path::new("/tmp/weird/foo"), Some(home.as_path())),
            InstallSource::Unknown,
        );
    }

    #[test]
    fn search_output_includes_npm_dirs_when_binary_not_found() {
        let resolved = resolve_binary("definitely-not-a-real-binary-xyz-123abc");
        assert!(
            resolved.path.is_none(),
            "did not expect to find a real binary"
        );
        assert!(
            resolved
                .search_output
                .contains("strategy 3 — npm global install dirs"),
            "expected strategy 3 marker in search_output:\n{}",
            resolved.search_output
        );
        if let Some(home) = std::env::home_dir() {
            let expected = home.join(".npm-global/bin");
            assert!(
                resolved
                    .search_output
                    .contains(&expected.display().to_string()),
                "expected {} in search_output:\n{}",
                expected.display(),
                resolved.search_output
            );
        }
    }
}
