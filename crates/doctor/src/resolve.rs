//! Binary resolution and command output formatting helpers.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::command::{
    format_duration, run_command_with_timeout, CommandError, CommandTimeout, DEFAULT_PROBE_TIMEOUT,
};
use crate::environment::{apply_doctor_env, DoctorEnv};

use super::types::{InstallSource, ResolvedBinary};

/// Resolve a binary by trying login shell path lookup, common install paths,
/// then npm global install dirs.
pub fn resolve_binary(cmd: &str) -> ResolvedBinary {
    resolve_binary_with_diagnostics(cmd, None).0
}

/// Resolve a binary using a caller-provided environment snapshot.
pub fn resolve_binary_with_env(cmd: &str, env: &DoctorEnv) -> ResolvedBinary {
    resolve_binary_with_diagnostics(cmd, Some(env)).0
}

pub(crate) fn resolve_binary_with_diagnostics(
    cmd: &str,
    env: Option<&DoctorEnv>,
) -> (ResolvedBinary, Vec<CommandTimeout>) {
    let mut lines = vec![format!("resolve '{cmd}':")];
    let mut timeouts = Vec::new();

    if let Some(path_value) = env.and_then(|env| env.get("PATH")) {
        lines.push("  strategy 0 — caller environment PATH:".to_string());
        if let Some(path) = resolve_from_path(cmd, path_value) {
            lines.push(format!("    PATH => {} (resolved)", path.display()));
            return resolved_binary(path, &lines, timeouts, env);
        }
        lines.push("    PATH => not found".to_string());
    }

    // Strategy 1: Login shell path lookup (primary)
    lines.push("  strategy 1 — login shell path lookup:".to_string());
    for (shell, lookup_cmd) in shell_lookup_commands(cmd) {
        let display_command = format!("{shell} -l -c '{lookup_cmd}'");
        let mut command = Command::new(shell);
        command.args(["-l", "-c", &lookup_cmd]);
        apply_doctor_env(&mut command, env);
        match run_command_with_timeout(command, &display_command, DEFAULT_PROBE_TIMEOUT) {
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
                    return resolved_binary(path.clone(), &lines, timeouts, env);
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
            Err(CommandError::Timeout { command, timeout }) => {
                lines.push(format!(
                    "    {display_command} => timed out after {}",
                    format_duration(timeout),
                ));
                timeouts.push(CommandTimeout::new(
                    format!("{cmd} binary resolution"),
                    command,
                    timeout,
                ));
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
            return resolved_binary(path, &lines, timeouts, env);
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
    let home = env
        .and_then(|env| env.get("HOME").map(PathBuf::from))
        .or_else(std::env::home_dir);
    if let Some(home) = home.as_deref() {
        for dir in npm_search_dirs(home) {
            let path = dir.join(cmd);
            if path.exists() {
                lines.push(format!("    {} => found (resolved)", path.display()));
                return resolved_binary(path, &lines, timeouts, env);
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
    if let Some(npm_bin_dir) = npm_global_bin_dir(&mut lines, &mut timeouts, env) {
        let path = npm_bin_dir.join(cmd);
        if path.exists() {
            lines.push(format!("    {} => found (resolved)", path.display()));
            return resolved_binary(path, &lines, timeouts, env);
        }
        lines.push(format!("    {} => not found", path.display()));
    }

    lines.push("  not found in any location".to_string());
    (
        ResolvedBinary {
            path: None,
            search_output: lines.join("\n"),
            install_source: None,
        },
        timeouts,
    )
}

fn resolve_from_path(cmd: &str, path_value: &str) -> Option<PathBuf> {
    std::env::split_paths(path_value)
        .map(|dir| dir.join(cmd))
        .find(|path| is_executable_file(path))
}

fn resolved_binary(
    path: PathBuf,
    lines: &[String],
    timeouts: Vec<CommandTimeout>,
    env: Option<&DoctorEnv>,
) -> (ResolvedBinary, Vec<CommandTimeout>) {
    let install_source = Some(detect_install_source_with_env(&path, env));
    (
        ResolvedBinary {
            path: Some(path),
            search_output: lines.join("\n"),
            install_source,
        },
        timeouts,
    )
}

/// Infer how a binary was installed. First applies path-prefix heuristics (no
/// subprocess or network probes) covering Brew, Cargo, Mise, Asdf, Npm
/// (mirroring the dirs in [`npm_search_dirs`]), and the System dirs. When those
/// fall through to [`InstallSource::Unknown`] for a binary in a user-local bin
/// dir, a cheap filesystem fingerprint (see [`fingerprint_curl_pipe`]) is
/// attempted to recognise curl/native installers (Claude native, Cursor, Amp),
/// using the caller snapshot's `HOME` when one is supplied.
fn detect_install_source_with_env(path: &Path, env: Option<&DoctorEnv>) -> InstallSource {
    let home = env
        .and_then(|env| env.get("HOME").map(PathBuf::from))
        .or_else(std::env::home_dir);
    let base = detect_install_source_with_home(path, home.as_deref());
    if base == InstallSource::Unknown {
        if let Some(home) = home.as_deref() {
            if fingerprint_curl_pipe(path, home) {
                return InstallSource::CurlPipe;
            }
        }
    }
    base
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

fn npm_global_bin_dir(
    lines: &mut Vec<String>,
    timeouts: &mut Vec<CommandTimeout>,
    env: Option<&DoctorEnv>,
) -> Option<PathBuf> {
    let mut command = Command::new("npm");
    command.args(["prefix", "-g"]);
    apply_doctor_env(&mut command, env);
    let output = match run_command_with_timeout(command, "npm prefix -g", DEFAULT_PROBE_TIMEOUT) {
        Ok(output) => output,
        Err(CommandError::Timeout { command, timeout }) => {
            lines.push(format!(
                "    npm prefix -g => timed out after {}",
                format_duration(timeout),
            ));
            timeouts.push(CommandTimeout::new("npm global prefix", command, timeout));
            return None;
        }
        Err(e) => {
            lines.push(format!("    npm prefix -g => error: {e}"));
            return None;
        }
    };
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

/// A known curl/native installer footprint for a binary that lands in a
/// user-local bin dir. Pairs the binary name with marker paths (relative to
/// `$HOME`) that the installer also creates; if the binary lives in a user-local
/// bin dir and any marker exists, the install is fingerprinted as a curl-pipe
/// install.
struct CurlInstallerFootprint {
    /// Binary file name as it appears in `~/.local/bin` or `~/bin`.
    binary: &'static str,
    /// Marker paths relative to `$HOME`; if any exists the fingerprint matches.
    markers: &'static [&'static str],
}

/// Known footprints of curl/native installers whose binaries land in a
/// user-local bin dir. Conservative on purpose — only well-known data dirs are
/// listed so a bare `~/.local/bin/<x>` with no installer footprint stays
/// [`InstallSource::Unknown`].
const CURL_INSTALLER_FOOTPRINTS: &[CurlInstallerFootprint] = &[
    // Claude native installer — claude.ai/install.sh.
    CurlInstallerFootprint {
        binary: "claude",
        markers: &[".local/share/claude", ".claude/local", ".claude/bin"],
    },
    // Cursor CLI installer — cursor.com/install.
    CurlInstallerFootprint {
        binary: "cursor-agent",
        markers: &[".local/share/cursor-agent/versions", ".cursor/bin"],
    },
    // Amp installer — ampcode.com/install.sh.
    CurlInstallerFootprint {
        binary: "amp",
        markers: &[".local/share/amp", ".cache/amp"],
    },
];

/// Cheap filesystem fingerprint for curl/native installs that path-prefix
/// heuristics can't classify. Only considers binaries inside a user-local bin
/// dir (`~/.local/bin`, `~/bin`) and uses two low-false-positive signals:
///
/// 1. A known installer footprint marker (see [`CURL_INSTALLER_FOOTPRINTS`])
///    exists under `$HOME` and the binary name matches that installer.
/// 2. The bin entry is a symlink into a *versioned* install dir under `$HOME`
///    (the layout Cursor's and Claude's native installers use:
///    `~/.local/bin/<tool>` → `…/versions/<ver>/<tool>`).
///
/// No subprocess or network access — only `read_link`/`exists`/`canonicalize`.
fn fingerprint_curl_pipe(path: &Path, home: &Path) -> bool {
    let in_user_local_bin =
        path.starts_with(home.join(".local/bin")) || path.starts_with(home.join("bin"));
    if !in_user_local_bin {
        return false;
    }

    // Signal 1 — a known installer footprint marker exists under $HOME.
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        for fp in CURL_INSTALLER_FOOTPRINTS {
            if fp.binary == name && fp.markers.iter().any(|m| home.join(m).exists()) {
                return true;
            }
        }
    }

    // Signal 2 — the bin entry is a symlink into a versioned install dir under
    // $HOME.
    if let Ok(target) = std::fs::read_link(path) {
        let resolved = if target.is_absolute() {
            target
        } else if let Some(parent) = path.parent() {
            parent.join(target)
        } else {
            target
        };
        let resolved = resolved.canonicalize().unwrap_or(resolved);
        if resolved.starts_with(home) && resolved.components().any(|c| c.as_os_str() == "versions")
        {
            return true;
        }
    }

    false
}

/// Whether the active binary path is owned by a Homebrew cask.
///
/// Cask binaries commonly appear as `<brew-prefix>/bin/<tool>` symlinks into
/// `<brew-prefix>/Caskroom/<token>/<version>/<tool>`. Treat the Caskroom path
/// (or the bin symlink's immediate target) as the owning install source before
/// following the full canonical chain for npm detection: cask wrappers may
/// include node-based internals, but the user updates the active binary through
/// `brew upgrade`, not an unrelated or inactive global npm package.
fn looks_like_homebrew_cask(path: &Path) -> bool {
    if path_has_caskroom_component(path) {
        return true;
    }

    let Ok(target) = std::fs::read_link(path) else {
        return false;
    };
    let target = if target.is_absolute() {
        target
    } else {
        path.parent()
            .map(|parent| parent.join(&target))
            .unwrap_or(target)
    };

    path_has_caskroom_component(&target)
}

fn path_has_caskroom_component(path: &Path) -> bool {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .any(|component| component.eq_ignore_ascii_case("Caskroom"))
}

/// Whether the canonicalized binary path lives inside a `node_modules/`
/// directory — a clean positive signal for `npm install -g`, regardless of
/// which node distribution (brew-shipped, nvm, fnm, asdf, mise) hosts the npm
/// prefix. Brew formulae and cargo installs never place binaries inside
/// `node_modules/`, so this dominates the path-prefix heuristics below.
///
/// The npm global bin entry is a symlink (e.g. `/opt/homebrew/bin/claude` →
/// `../lib/node_modules/@anthropic-ai/claude-code/...`), so the check resolves
/// the symlink via [`fs::canonicalize`] before inspecting components. If
/// canonicalize fails (broken symlink, permissions), fall back to the path
/// as-is — better to attempt the check than skip it. No subprocess or network.
fn looks_like_npm_global(path: &Path) -> bool {
    let resolved = std::fs::canonicalize(path);
    let target = resolved.as_deref().unwrap_or(path);
    target.components().any(|c| c.as_os_str() == "node_modules")
}

/// Testable inner: same logic as [`detect_install_source`] but takes the home
/// directory as a parameter so unit tests can inject a fixed value.
fn detect_install_source_with_home(path: &Path, home: Option<&Path>) -> InstallSource {
    // Homebrew cask ownership beats npm internals. In a mixed Claude install,
    // the active `/opt/homebrew/bin/claude` can be a cask symlink while an
    // unrelated npm global package also exists; update planning must follow
    // the active Caskroom binary back to Brew.
    if looks_like_homebrew_cask(path) {
        return InstallSource::Brew;
    }

    // npm global install (any node distribution). Checked first: the bin entry
    // is a symlink into `node_modules/`, which may live under a brew prefix
    // (`npm config get prefix = /opt/homebrew`), so this must win over the
    // `/opt/homebrew/` Brew path-prefix check below.
    if looks_like_npm_global(path) {
        return InstallSource::Npm;
    }

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
    fn resolve_binary_with_env_finds_binary_only_in_snapshot_path() {
        let dir =
            std::env::temp_dir().join(format!("doctor-resolve-env-path-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let bin = dir.join("doctor-env-only-tool");
        fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let env = crate::DoctorEnv::new(vec![
            ("PATH".to_string(), dir.to_string_lossy().to_string()),
            ("HOME".to_string(), dir.to_string_lossy().to_string()),
        ]);
        let resolved = resolve_binary_with_env("doctor-env-only-tool", &env);

        assert_eq!(resolved.path.as_deref(), Some(bin.as_path()));
        assert!(
            resolved
                .search_output
                .contains("strategy 0 — caller environment PATH"),
            "search trace should mention the snapshot PATH strategy:\n{}",
            resolved.search_output,
        );
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
    fn npm_global_under_brew_prefix_classifies_as_npm() {
        // `npm config get prefix = /opt/homebrew` lands the package under
        // `<prefix>/lib/node_modules/...` with a bin symlink at `<prefix>/bin`.
        // The brew path-prefix check must not win — node_modules in the
        // canonicalized target is the authoritative npm signal.
        let root = std::env::temp_dir().join(format!("doctor-npm-brew-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let pkg = root.join("lib/node_modules/@anthropic-ai/claude-code/bin");
        fs::create_dir_all(&pkg).unwrap();
        let real = pkg.join("claude.exe");
        File::create(&real).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        let link = root.join("bin/claude");
        std::os::unix::fs::symlink(
            "../lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe",
            &link,
        )
        .unwrap();

        assert!(looks_like_npm_global(&link));
        assert_eq!(
            detect_install_source_with_home(&link, None),
            InstallSource::Npm,
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn genuine_brew_cellar_not_misclassified_as_npm() {
        // A real Cellar binary (no node_modules in its canonicalized path) must
        // stay Brew — `looks_like_npm_global` returns false for it.
        let root = std::env::temp_dir().join(format!("doctor-brew-cellar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cellar = root.join("Cellar/git/2.44.0/bin");
        fs::create_dir_all(&cellar).unwrap();
        let bin = cellar.join("git");
        File::create(&bin).unwrap();

        assert!(!looks_like_npm_global(&bin));
        // The real `/opt/homebrew` prefix check is exercised by
        // `detect_install_source_classifies_brew`; here we only assert the new
        // npm layer leaves non-npm paths to fall through.
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn npm_global_under_nvm_classifies_as_npm() {
        // The nvm layout: ~/.local/bin/<name> → ~/.nvm/versions/node/<v>/lib/
        // node_modules/<pkg>/... Path-prefix already handled ~/.nvm directly;
        // the node_modules layer must keep classifying the symlinked bin too.
        let home = std::env::temp_dir().join(format!("doctor-npm-nvm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let pkg = home.join(".nvm/versions/node/v20.10.0/lib/node_modules/@scope/tool/bin");
        fs::create_dir_all(&pkg).unwrap();
        let real = pkg.join("tool.js");
        File::create(&real).unwrap();
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        let link = home.join(".local/bin/tool");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(looks_like_npm_global(&link));
        assert_eq!(
            detect_install_source_with_home(&link, Some(home.as_path())),
            InstallSource::Npm,
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn homebrew_caskroom_symlink_classifies_as_brew_with_inactive_npm_package() {
        #[cfg(unix)]
        {
            let root =
                std::env::temp_dir().join(format!("doctor-cask-mixed-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);

            let cask_bin = root.join("Caskroom/claude-code/2.1.152/claude");
            fs::create_dir_all(cask_bin.parent().unwrap()).unwrap();
            File::create(&cask_bin).unwrap();
            fs::create_dir_all(root.join("bin")).unwrap();
            let active = root.join("bin/claude");
            std::os::unix::fs::symlink(&cask_bin, &active).unwrap();

            // Unrelated global npm package exists under nvm, but it is not the
            // active/resolved `claude` binary.
            let home = root.join("home");
            let npm_pkg = home
                .join(".nvm/versions/node/v23.7.0/lib/node_modules/@anthropic-ai/claude-code/cli");
            fs::create_dir_all(&npm_pkg).unwrap();
            File::create(npm_pkg.join("claude.js")).unwrap();
            fs::create_dir_all(home.join(".nvm/versions/node/v23.7.0/bin")).unwrap();
            std::os::unix::fs::symlink(
                npm_pkg.join("claude.js"),
                home.join(".nvm/versions/node/v23.7.0/bin/claude"),
            )
            .unwrap();

            assert!(looks_like_homebrew_cask(&active));
            assert_eq!(
                detect_install_source_with_home(&active, Some(home.as_path())),
                InstallSource::Brew,
            );

            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn homebrew_caskroom_ownership_wins_over_node_modules_canonical_target() {
        #[cfg(unix)]
        {
            let root = std::env::temp_dir()
                .join(format!("doctor-cask-node-modules-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);

            let home = root.join("home");
            let npm_pkg = home
                .join(".nvm/versions/node/v23.7.0/lib/node_modules/@anthropic-ai/claude-code/cli");
            fs::create_dir_all(&npm_pkg).unwrap();
            let npm_entry = npm_pkg.join("claude.js");
            File::create(&npm_entry).unwrap();

            let cask_bin = root.join("Caskroom/claude-code/2.1.152/claude");
            fs::create_dir_all(cask_bin.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&npm_entry, &cask_bin).unwrap();
            fs::create_dir_all(root.join("bin")).unwrap();
            let active = root.join("bin/claude");
            std::os::unix::fs::symlink(&cask_bin, &active).unwrap();

            assert!(looks_like_homebrew_cask(&active));
            assert!(
                looks_like_npm_global(&active),
                "canonical node_modules target should not override Caskroom ownership",
            );
            assert_eq!(
                detect_install_source_with_home(&active, Some(home.as_path())),
                InstallSource::Brew,
            );

            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn non_node_modules_path_falls_through_to_existing_detection() {
        // A plain binary (no symlink, no node_modules) is not npm; the new layer
        // returns false and existing path-prefix detection decides.
        let root = std::env::temp_dir().join(format!("doctor-no-npm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("usr/local/bin");
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("foo");
        File::create(&bin).unwrap();

        assert!(!looks_like_npm_global(&bin));
        // System dirs still classify correctly through the unchanged fall-through.
        assert_eq!(
            detect_install_source_with_home(Path::new("/usr/bin/foo"), None),
            InstallSource::System,
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fingerprint_curl_pipe_matches_known_installer_marker() {
        let home = std::env::temp_dir().join(format!("doctor-fp-marker-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        // Claude native installer: ~/.local/bin/claude + ~/.local/share/claude.
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir_all(home.join(".local/share/claude")).unwrap();
        let bin = home.join(".local/bin/claude");
        File::create(&bin).unwrap();

        assert!(fingerprint_curl_pipe(&bin, &home));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn fingerprint_curl_pipe_matches_versioned_symlink() {
        #[cfg(unix)]
        {
            let home =
                std::env::temp_dir().join(format!("doctor-fp-symlink-{}", std::process::id()));
            let _ = fs::remove_dir_all(&home);
            // Cursor layout: ~/.local/bin/cursor-agent -> ~/.local/share/cursor-agent/versions/1.0.0/cursor-agent
            let versioned = home.join(".local/share/cursor-agent/versions/1.0.0");
            fs::create_dir_all(&versioned).unwrap();
            let real = versioned.join("cursor-agent");
            File::create(&real).unwrap();
            fs::create_dir_all(home.join(".local/bin")).unwrap();
            let link = home.join(".local/bin/cursor-agent");
            std::os::unix::fs::symlink(&real, &link).unwrap();

            assert!(fingerprint_curl_pipe(&link, &home));
            let _ = fs::remove_dir_all(&home);
        }
    }

    #[test]
    fn fingerprint_curl_pipe_no_match_without_footprint() {
        let home = std::env::temp_dir().join(format!("doctor-fp-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        // A bare ~/.local/bin binary with no installer footprint stays Unknown.
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        let bin = home.join(".local/bin/mytool");
        File::create(&bin).unwrap();

        assert!(!fingerprint_curl_pipe(&bin, &home));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn fingerprint_curl_pipe_ignores_binaries_outside_user_local_bin() {
        let home = std::env::temp_dir().join(format!("doctor-fp-outside-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        // Marker exists, but the binary is elsewhere — must not fingerprint.
        fs::create_dir_all(home.join(".local/share/claude")).unwrap();
        let bin = PathBuf::from("/tmp/elsewhere/claude");

        assert!(!fingerprint_curl_pipe(&bin, &home));
        let _ = fs::remove_dir_all(&home);
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
