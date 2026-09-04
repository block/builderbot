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
        if let Some(path) = resolve_executable_from_path(cmd, path_value) {
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

/// Resolve an executable name against a PATH value.
///
/// This is intentionally PATH-only: callers that need doctor's full fallback
/// strategy should use [`resolve_binary`] or [`resolve_binary_with_env`].
pub fn resolve_executable_from_path(cmd: &str, path_value: &str) -> Option<PathBuf> {
    std::env::split_paths(path_value)
        .map(|dir| dir.join(cmd))
        .find(|path| is_executable_file(path))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvShebangLauncher {
    pub interpreter: String,
    pub bin_dir: PathBuf,
}

pub fn env_shebang_interpreter(binary_path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(binary_path).ok()?;
    let mut buf = [0_u8; 512];
    let len = std::io::Read::read(&mut file, &mut buf).ok()?;
    let first_line = std::str::from_utf8(&buf[..len]).ok()?.lines().next()?;
    let shebang = first_line.strip_prefix("#!")?.trim();

    let mut parts = shebang.split_whitespace();
    let command = parts.next()?;
    let command_name = Path::new(command).file_name()?.to_str()?;
    if command_name != "env" {
        return None;
    }

    for part in parts {
        if part == "-S" || part.starts_with('-') || part.contains('=') {
            continue;
        }
        if part.contains('/') {
            return None;
        }
        return Some(part.to_string());
    }

    None
}

pub fn resolve_env_shebang_interpreter_from_path(
    binary_path: &Path,
    path_value: &str,
) -> Option<PathBuf> {
    let interpreter = env_shebang_interpreter(binary_path)?;
    resolve_executable_from_path(&interpreter, path_value)
}

pub fn env_shebang_launcher(binary_path: &Path) -> Option<EnvShebangLauncher> {
    let interpreter = env_shebang_interpreter(binary_path)?;
    let bin_dir = binary_path.parent()?.to_path_buf();
    if !is_executable_file(&bin_dir.join(&interpreter)) {
        return None;
    }

    Some(EnvShebangLauncher {
        interpreter,
        bin_dir,
    })
}

pub fn is_broad_toolchain_dir(path: &Path) -> bool {
    matches!(
        path.to_str(),
        Some("/opt/homebrew/bin" | "/usr/local/bin" | "/usr/bin" | "/bin" | "/opt/local/bin")
    )
}

pub fn path_with_inserted_launcher_bin_dir(existing_path: &str, bin_dir: &Path) -> Option<String> {
    let mut entries: Vec<PathBuf> = if existing_path.is_empty() {
        Vec::new()
    } else {
        std::env::split_paths(existing_path).collect()
    };

    if entries.iter().any(|entry| entry == bin_dir) {
        return None;
    }

    if is_broad_toolchain_dir(bin_dir) {
        entries.push(bin_dir.to_path_buf());
    } else {
        entries.insert(0, bin_dir.to_path_buf());
    }

    std::env::join_paths(entries).ok()?.into_string().ok()
}

pub fn guarded_path_for_env_shebang_launcher(
    binary_path: &Path,
    existing_path: &str,
) -> Option<String> {
    let launcher = env_shebang_launcher(binary_path)?;
    if resolve_executable_from_path(&launcher.interpreter, existing_path).is_some() {
        return None;
    }

    path_with_inserted_launcher_bin_dir(existing_path, &launcher.bin_dir)
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
/// attempted to recognise the versioned-symlink layout native installers use
/// (Cursor, Claude), using the caller snapshot's `HOME` when one is supplied.
fn detect_install_source_with_env(path: &Path, env: Option<&DoctorEnv>) -> InstallSource {
    let home = env
        .and_then(|env| env.get("HOME").map(PathBuf::from))
        .or_else(std::env::home_dir);
    let pnpm_home = env_or_process_var(env, "PNPM_HOME").map(PathBuf::from);
    let bun_install = env_or_process_var(env, "BUN_INSTALL").map(PathBuf::from);
    let base = detect_install_source_inner(
        path,
        home.as_deref(),
        pnpm_home.as_deref(),
        bun_install.as_deref(),
    );
    if base == InstallSource::Unknown {
        if let Some(home) = home.as_deref() {
            if fingerprint_curl_pipe(path, home) {
                return InstallSource::CurlPipe;
            }
        }
    }
    base
}

/// Read a variable from the caller snapshot when one is supplied, otherwise
/// from the process environment. Empty values are treated as absent.
fn env_or_process_var(env: Option<&DoctorEnv>, name: &str) -> Option<String> {
    let value = if let Some(env) = env {
        env.get(name).map(str::to_string)
    } else {
        std::env::var(name).ok()
    };
    value.filter(|s| !s.is_empty())
}

fn shell_lookup_commands(cmd: &str) -> [(&'static str, String); 2] {
    let quoted = shell_quote(cmd);
    [
        ("/bin/zsh", format!("whence -p -- {quoted}")),
        ("/bin/bash", format!("type -P -- {quoted}")),
    ]
}

/// Single-quote `value` for safe interpolation into a `sh -c` command line.
pub(crate) fn shell_quote(value: &str) -> String {
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

fn in_user_local_bin(path: &Path, home: &Path) -> bool {
    path.starts_with(home.join(".local/bin")) || path.starts_with(home.join("bin"))
}

/// Cheap filesystem fingerprint for native installs that path-prefix heuristics
/// can't classify. Only considers binaries inside a user-local bin dir
/// (`~/.local/bin`, `~/bin`), and on a single low-false-positive signal: the bin
/// entry is a symlink into a *versioned* install dir under `$HOME` — the layout
/// Cursor's and Claude's native installers use
/// (`~/.local/bin/<tool>` → `…/versions/<ver>/<tool>`).
///
/// Every input is local to the resolved binary: what this path *is*, never
/// whether some marker happens to exist under `$HOME`. An ambient marker
/// describes the machine rather than the binary PATH resolved to — with a tool
/// installed both by an installer and by npm, the marker is present either way.
///
/// No subprocess or network access — only `read_link`/`canonicalize`.
fn fingerprint_curl_pipe(path: &Path, home: &Path) -> bool {
    if !in_user_local_bin(path, home) {
        return false;
    }

    let Ok(target) = std::fs::read_link(path) else {
        return false;
    };
    let resolved = if target.is_absolute() {
        target
    } else if let Some(parent) = path.parent() {
        parent.join(target)
    } else {
        target
    };
    let resolved = resolved.canonicalize().unwrap_or(resolved);
    // `resolved` is canonical, so `$HOME` must be too before comparing: a home
    // reached through a symlinked ancestor (macOS `/tmp` → `/private/tmp`) never
    // matches as written. Falls back to `home` as-is if canonicalize fails.
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    resolved.starts_with(&home) && resolved.components().any(|c| c.as_os_str() == "versions")
}

/// The npm prefix that owns `path`, derived from where the package tree actually
/// lives: `<prefix>/lib/node_modules/<pkg>/…` → `<prefix>`.
///
/// The global bin entry is a symlink into the package tree, so the path is
/// canonicalized first (falling back to the path as-is, like
/// [`looks_like_npm_global`]). Anchors on the *first* adjacent
/// `lib`/`node_modules` component pair so a nested dependency tree
/// (`<prefix>/lib/node_modules/a/node_modules/b`) still yields `<prefix>`.
///
/// Returns `None` when the path has no such pair — callers then fall back to a
/// prefix-less `npm install -g`, which targets npm's configured global prefix.
pub fn npm_prefix_for_binary(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize();
    let target = canonical.as_deref().unwrap_or(path);
    let components: Vec<_> = target.components().collect();
    let lib = components
        .windows(2)
        .position(|pair| pair[0].as_os_str() == "lib" && pair[1].as_os_str() == "node_modules")?;
    let prefix: PathBuf = components[..lib].iter().collect();
    (!prefix.as_os_str().is_empty()).then_some(prefix)
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

/// Whether the binary lives in pnpm's global install dir — `$PNPM_HOME` when
/// the caller environment declares it, else the platform defaults
/// (`~/Library/pnpm` on macOS, `~/.local/share/pnpm` on Linux). pnpm links
/// global bins directly inside this dir, so a prefix check on the resolved
/// path is sufficient; no symlink chasing needed.
fn looks_like_pnpm_global(path: &Path, home: Option<&Path>, pnpm_home: Option<&Path>) -> bool {
    if pnpm_home.is_some_and(|dir| path.starts_with(dir)) {
        return true;
    }
    let Some(home) = home else {
        return false;
    };
    path.starts_with(home.join("Library/pnpm")) || path.starts_with(home.join(".local/share/pnpm"))
}

/// Whether the binary lives in bun's install dir — `$BUN_INSTALL` when the
/// caller environment declares it, else the default `~/.bun`. `bun add -g`
/// links bins at `<install>/bin/<tool>` pointing into
/// `<install>/install/global/node_modules/…`.
fn looks_like_bun_global(path: &Path, home: Option<&Path>, bun_install: Option<&Path>) -> bool {
    if bun_install.is_some_and(|dir| path.starts_with(dir)) {
        return true;
    }
    home.is_some_and(|home| path.starts_with(home.join(".bun")))
}

/// Test-only shorthand for [`detect_install_source_inner`] with no
/// environment-declared pnpm/bun dirs — unit tests inject a fixed home and
/// rely on the platform-default locations.
#[cfg(test)]
fn detect_install_source_with_home(path: &Path, home: Option<&Path>) -> InstallSource {
    detect_install_source_inner(path, home, None, None)
}

fn detect_install_source_inner(
    path: &Path,
    home: Option<&Path>,
    pnpm_home: Option<&Path>,
    bun_install: Option<&Path>,
) -> InstallSource {
    // Homebrew cask ownership beats npm internals. In a mixed Claude install,
    // the active `/opt/homebrew/bin/claude` can be a cask symlink while an
    // unrelated npm global package also exists; update planning must follow
    // the active Caskroom binary back to Brew.
    if looks_like_homebrew_cask(path) {
        return InstallSource::Brew;
    }

    // pnpm/bun global installs also canonicalize into `node_modules/` trees,
    // but `npm install -g` doesn't own them either — classify by their global
    // install dirs before the npm check.
    if looks_like_pnpm_global(path, home, pnpm_home) {
        return InstallSource::Pnpm;
    }
    if looks_like_bun_global(path, home, bun_install) {
        return InstallSource::Bun;
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

    fn write_executable(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn quoted(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    /// A caller snapshot declaring only `HOME`. Classifying through
    /// [`detect_install_source_with_env`] runs the full chain — unlike the
    /// [`detect_install_source_with_home`] shorthand, it also reaches the
    /// [`fingerprint_curl_pipe`] fallback.
    fn home_env(home: &Path) -> crate::DoctorEnv {
        crate::DoctorEnv::new(vec![("HOME".to_string(), home.display().to_string())])
    }

    fn write_login_path_rewrite_profiles(home: &Path, path: &Path) {
        let profile = format!(
            "export PATH={}\n",
            quoted(&format!("{}:/usr/bin:/bin", path.to_string_lossy())),
        );
        fs::write(home.join(".zprofile"), &profile).unwrap();
        fs::write(home.join(".bash_profile"), profile).unwrap();
    }

    fn join_path_entries(entries: &[PathBuf]) -> String {
        std::env::join_paths(entries)
            .expect("join path entries")
            .into_string()
            .expect("path entries should be utf8")
    }

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
    fn env_shebang_interpreter_detects_env_launcher() {
        let dir = std::env::temp_dir().join(format!("doctor-env-shebang-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let launcher = dir.join("codex-acp");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        assert_eq!(env_shebang_interpreter(&launcher).as_deref(), Some("node"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn guarded_path_keeps_snapshot_when_interpreter_is_already_available() {
        let dir = std::env::temp_dir().join(format!(
            "doctor-guarded-path-present-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let project_bin = dir.join("project-bin");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("codex-acp");
        write_executable(&project_bin.join("node"), "#!/bin/sh\n");
        write_executable(&agent_bin.join("node"), "#!/bin/sh\n");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let snapshot_path = join_path_entries(&[project_bin.clone(), PathBuf::from("/usr/bin")]);

        assert_eq!(
            guarded_path_for_env_shebang_launcher(&launcher, &snapshot_path),
            None,
            "launcher bin dir must not be inserted ahead of a snapshot PATH that already provides node"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn guarded_path_prepends_private_launcher_dir_when_interpreter_is_missing() {
        let dir = std::env::temp_dir().join(format!(
            "doctor-guarded-path-missing-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let snapshot_bin = dir.join("snapshot-bin");
        let agent_bin = dir.join("agent-bin");
        let launcher = agent_bin.join("claude-agent-acp");
        fs::create_dir_all(&snapshot_bin).expect("create snapshot bin");
        write_executable(&agent_bin.join("node"), "#!/bin/sh\n");
        write_executable(&launcher, "#!/usr/bin/env node\n");

        let snapshot_path = join_path_entries(std::slice::from_ref(&snapshot_bin));
        let updated = guarded_path_for_env_shebang_launcher(&launcher, &snapshot_path)
            .expect("missing interpreter should add launcher bin dir");
        let entries: Vec<PathBuf> = std::env::split_paths(&updated).collect();

        assert_eq!(entries, vec![agent_bin, snapshot_bin]);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn broad_toolchain_dirs_are_appended_not_prepended() {
        let hermit_bin = PathBuf::from("/repo/bin");
        let existing_path = join_path_entries(std::slice::from_ref(&hermit_bin));
        let updated =
            path_with_inserted_launcher_bin_dir(&existing_path, Path::new("/opt/homebrew/bin"))
                .expect("broad dir should still be added as a fallback");
        let entries: Vec<PathBuf> = std::env::split_paths(&updated).collect();

        assert!(is_broad_toolchain_dir(Path::new("/opt/homebrew/bin")));
        assert_eq!(
            entries,
            vec![hermit_bin, PathBuf::from("/opt/homebrew/bin")]
        );
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
    fn resolve_binary_with_env_prefers_snapshot_path_over_login_shell_rewrite() {
        let dir = std::env::temp_dir().join(format!(
            "doctor-resolve-env-vs-hermit-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let snapshot_bin = dir.join("snapshot/bin");
        let hermit_bin = dir.join("hermit/bin");
        let snapshot_tool = snapshot_bin.join("doctor-agent-cli");
        let hermit_tool = hermit_bin.join("doctor-agent-cli");
        write_executable(&snapshot_tool, "#!/bin/sh\nexit 0\n");
        write_executable(&hermit_tool, "#!/bin/sh\nexit 42\n");
        write_login_path_rewrite_profiles(&dir, &hermit_bin);

        let env = crate::DoctorEnv::new(vec![
            (
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin", snapshot_bin.to_string_lossy()),
            ),
            ("HOME".to_string(), dir.to_string_lossy().to_string()),
            ("USER".to_string(), "doctor-test".to_string()),
            ("ZDOTDIR".to_string(), dir.to_string_lossy().to_string()),
        ]);
        let resolved = resolve_binary_with_env("doctor-agent-cli", &env);

        assert_eq!(resolved.path.as_deref(), Some(snapshot_tool.as_path()));
        assert!(
            !resolved
                .search_output
                .contains(&hermit_bin.to_string_lossy().to_string()),
            "DoctorEnv resolution should not fall through to the Hermit/login PATH:\n{}",
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

    /// Pi's installer fallback layout: `npm install -g --prefix ~/.local` leaves
    /// `~/.local/bin/pi` symlinked into
    /// `~/.local/lib/node_modules/@earendil-works/pi-coding-agent/…`. The tree is
    /// npm-shaped because it *is* an npm install, and it classifies `Npm` —
    /// updatable with an npm recipe aimed at the prefix that owns it
    /// ([`npm_prefix_for_binary`]). No `$HOME` marker gets a say: living under
    /// `~/.local` is not evidence about how a binary was installed.
    #[test]
    fn pi_user_local_npm_layout_classifies_as_npm() {
        #[cfg(unix)]
        {
            let home = std::env::temp_dir().join(format!("doctor-pi-local-{}", std::process::id()));
            let _ = fs::remove_dir_all(&home);
            let pkg = home.join(".local/lib/node_modules/@earendil-works/pi-coding-agent/dist");
            fs::create_dir_all(&pkg).unwrap();
            let real = pkg.join("cli.js");
            File::create(&real).unwrap();
            fs::create_dir_all(home.join(".local/bin")).unwrap();
            let link = home.join(".local/bin/pi");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            // The standalone-node runtime the installer drops when it finds no
            // usable node. Present or absent, it changes nothing.
            fs::create_dir_all(home.join(".local/share/pi-node")).unwrap();

            assert_eq!(
                detect_install_source_with_env(&link, Some(&home_env(&home))),
                InstallSource::Npm,
            );
            assert_eq!(
                npm_prefix_for_binary(&link),
                Some(home.join(".local").canonicalize().unwrap()),
            );
            let _ = fs::remove_dir_all(&home);
        }
    }

    /// An npm-global install under a user-configured `~/.local` prefix stays
    /// `Npm` even for a tool whose installer also writes runtime dirs there
    /// (`~/.cache/amp`, `~/.local/share/amp` — both created by `amp` on first
    /// run, whatever installed it). Classification looks at the resolved binary,
    /// so an ambient data dir can no longer flip a user-managed npm install to
    /// `CurlPipe` and silence its update nag.
    #[test]
    fn amp_npm_layout_under_user_local_prefix_stays_npm_with_runtime_dirs_present() {
        #[cfg(unix)]
        {
            let home =
                std::env::temp_dir().join(format!("doctor-amp-local-{}", std::process::id()));
            let _ = fs::remove_dir_all(&home);
            let pkg = home.join(".local/lib/node_modules/@ampcode/cli/bin");
            fs::create_dir_all(&pkg).unwrap();
            let real = pkg.join("amp.exe");
            File::create(&real).unwrap();
            fs::create_dir_all(home.join(".local/bin")).unwrap();
            let link = home.join(".local/bin/amp");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            fs::create_dir_all(home.join(".cache/amp")).unwrap();
            fs::create_dir_all(home.join(".local/share/amp")).unwrap();

            assert_eq!(
                detect_install_source_with_env(&link, Some(&home_env(&home))),
                InstallSource::Npm,
            );
            let _ = fs::remove_dir_all(&home);
        }
    }

    /// Regression guard for the one fingerprint signal that survives: Cursor's
    /// native installer layout (`~/.local/bin/cursor-agent` → a versioned dir)
    /// classifies `CurlPipe` all the way through `detect_install_source_*`, with
    /// no installer-marker table involved. The same shape covers the native
    /// `claude` install (`~/.local/share/claude/versions/<ver>`).
    #[test]
    fn cursor_agent_versioned_symlink_classifies_as_curl_pipe() {
        #[cfg(unix)]
        {
            let home =
                std::env::temp_dir().join(format!("doctor-cursor-vers-{}", std::process::id()));
            let _ = fs::remove_dir_all(&home);
            let versioned = home.join(".local/share/cursor-agent/versions/2025.09.18-7ae6800");
            fs::create_dir_all(&versioned).unwrap();
            let real = versioned.join("cursor-agent");
            File::create(&real).unwrap();
            fs::create_dir_all(home.join(".local/bin")).unwrap();
            let link = home.join(".local/bin/cursor-agent");
            std::os::unix::fs::symlink(&real, &link).unwrap();

            assert_eq!(
                detect_install_source_with_env(&link, Some(&home_env(&home))),
                InstallSource::CurlPipe,
            );
            let _ = fs::remove_dir_all(&home);
        }
    }

    /// The npm global bin entry is a *relative* symlink into the package tree, so
    /// the prefix has to come out of the canonicalized target.
    #[test]
    fn npm_prefix_for_binary_follows_relative_bin_symlink() {
        #[cfg(unix)]
        {
            let root =
                std::env::temp_dir().join(format!("doctor-npm-prefix-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let prefix = root.join("prefix");
            let pkg = prefix.join("lib/node_modules/@earendil-works/pi-coding-agent/dist");
            fs::create_dir_all(&pkg).unwrap();
            File::create(pkg.join("cli.js")).unwrap();
            fs::create_dir_all(prefix.join("bin")).unwrap();
            std::os::unix::fs::symlink(
                "../lib/node_modules/@earendil-works/pi-coding-agent/dist/cli.js",
                prefix.join("bin/pi"),
            )
            .unwrap();

            assert_eq!(
                npm_prefix_for_binary(&prefix.join("bin/pi")),
                Some(prefix.canonicalize().unwrap()),
            );
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// Prefix derivation over the layouts seen in the wild. These paths need not
    /// exist — with canonicalize unavailable the components are read as given.
    #[test]
    fn npm_prefix_for_binary_derives_prefix_from_package_tree() {
        for (path, expected) in [
            (
                "/opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent/dist/cli.js",
                "/opt/homebrew",
            ),
            (
                "/Users/test/.local/lib/node_modules/@earendil-works/pi-coding-agent/dist/cli.js",
                "/Users/test/.local",
            ),
            (
                "/Users/test/.nvm/versions/node/v22.14.0/lib/node_modules/pi-acp/dist/index.js",
                "/Users/test/.nvm/versions/node/v22.14.0",
            ),
            // A nested dependency tree resolves to the *outer* prefix, not the
            // inner `node_modules`.
            (
                "/opt/homebrew/lib/node_modules/outer/node_modules/inner/cli.js",
                "/opt/homebrew",
            ),
        ] {
            assert_eq!(
                npm_prefix_for_binary(Path::new(path)),
                Some(PathBuf::from(expected)),
                "{path}",
            );
        }
    }

    /// No `lib/node_modules` pair — no prefix, and the caller falls back to a
    /// bare `npm install -g`. Covers non-npm binaries and pnpm's global shims
    /// (which are generated scripts, not symlinks into a `lib/` tree).
    #[test]
    fn npm_prefix_for_binary_none_without_package_tree() {
        for path in [
            "/usr/local/bin/goose",
            "/Users/test/Library/pnpm/pi",
            // `node_modules` without the `lib` parent: a project-local install,
            // which is nobody's global prefix.
            "/Users/test/project/node_modules/.bin/tsc",
        ] {
            assert_eq!(npm_prefix_for_binary(Path::new(path)), None, "{path}");
        }
    }

    /// pnpm global installs live under `$PNPM_HOME` (defaults `~/Library/pnpm`
    /// on macOS, `~/.local/share/pnpm` on Linux) and must classify `Pnpm`, not
    /// `Npm` — `npm install -g` neither owns nor updates them.
    #[test]
    fn pnpm_default_dirs_classify_as_pnpm() {
        let home = PathBuf::from("/home/test");
        assert_eq!(
            detect_install_source_with_home(&home.join("Library/pnpm/pi"), Some(home.as_path())),
            InstallSource::Pnpm,
        );
        assert_eq!(
            detect_install_source_with_home(
                &home.join(".local/share/pnpm/pi"),
                Some(home.as_path()),
            ),
            InstallSource::Pnpm,
        );
    }

    /// A custom `$PNPM_HOME` from the caller snapshot wins even outside the
    /// default locations.
    #[test]
    fn custom_pnpm_home_from_env_classifies_as_pnpm() {
        let env = crate::DoctorEnv::new(vec![
            ("HOME".to_string(), "/home/test".to_string()),
            ("PNPM_HOME".to_string(), "/data/pnpm".to_string()),
        ]);
        assert_eq!(
            detect_install_source_with_env(Path::new("/data/pnpm/pi"), Some(&env)),
            InstallSource::Pnpm,
        );
    }

    /// The pnpm check must beat the npm-layout check even when the bin entry is
    /// a real symlink into pnpm's global `node_modules` store.
    #[test]
    fn pnpm_symlink_into_global_store_classifies_as_pnpm_not_npm() {
        #[cfg(unix)]
        {
            let home = std::env::temp_dir().join(format!("doctor-pnpm-{}", std::process::id()));
            let _ = fs::remove_dir_all(&home);
            let pkg = home
                .join("Library/pnpm/global/5/node_modules/@earendil-works/pi-coding-agent/dist");
            fs::create_dir_all(&pkg).unwrap();
            let real = pkg.join("cli.js");
            File::create(&real).unwrap();
            fs::create_dir_all(home.join("Library/pnpm")).unwrap();
            let link = home.join("Library/pnpm/pi");
            std::os::unix::fs::symlink(&real, &link).unwrap();

            assert!(looks_like_npm_global(&link), "pnpm layout is npm-shaped");
            assert_eq!(
                detect_install_source_with_home(&link, Some(home.as_path())),
                InstallSource::Pnpm,
            );
            let _ = fs::remove_dir_all(&home);
        }
    }

    /// bun global installs live under `~/.bun` (or `$BUN_INSTALL`) with bin
    /// symlinks into `~/.bun/install/global/node_modules` — `Bun`, not `Npm`.
    #[test]
    fn bun_global_symlink_classifies_as_bun_not_npm() {
        #[cfg(unix)]
        {
            let home = std::env::temp_dir().join(format!("doctor-bun-{}", std::process::id()));
            let _ = fs::remove_dir_all(&home);
            let pkg =
                home.join(".bun/install/global/node_modules/@earendil-works/pi-coding-agent/dist");
            fs::create_dir_all(&pkg).unwrap();
            let real = pkg.join("cli.js");
            File::create(&real).unwrap();
            fs::create_dir_all(home.join(".bun/bin")).unwrap();
            let link = home.join(".bun/bin/pi");
            std::os::unix::fs::symlink(&real, &link).unwrap();

            assert!(looks_like_npm_global(&link), "bun layout is npm-shaped");
            assert_eq!(
                detect_install_source_with_home(&link, Some(home.as_path())),
                InstallSource::Bun,
            );
            let _ = fs::remove_dir_all(&home);
        }
    }

    /// A custom `$BUN_INSTALL` from the caller snapshot classifies as `Bun`.
    #[test]
    fn custom_bun_install_from_env_classifies_as_bun() {
        let env = crate::DoctorEnv::new(vec![
            ("HOME".to_string(), "/home/test".to_string()),
            ("BUN_INSTALL".to_string(), "/data/bun".to_string()),
        ]);
        assert_eq!(
            detect_install_source_with_env(Path::new("/data/bun/bin/pi"), Some(&env)),
            InstallSource::Bun,
        );
    }

    #[test]
    fn fingerprint_curl_pipe_no_match_for_plain_user_local_binary() {
        let home = std::env::temp_dir().join(format!("doctor-fp-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        // A real file in ~/.local/bin — not a versioned symlink — stays Unknown.
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        let bin = home.join(".local/bin/mytool");
        File::create(&bin).unwrap();

        assert!(!fingerprint_curl_pipe(&bin, &home));
        let _ = fs::remove_dir_all(&home);
    }

    /// The fingerprint only claims user-local bin dirs: the same versioned-symlink
    /// layout parked somewhere else on PATH is left to the path-prefix checks.
    #[test]
    fn fingerprint_curl_pipe_ignores_binaries_outside_user_local_bin() {
        #[cfg(unix)]
        {
            let root =
                std::env::temp_dir().join(format!("doctor-fp-outside-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let home = root.join("home");
            let versioned = home.join(".local/share/cursor-agent/versions/1.0.0");
            fs::create_dir_all(&versioned).unwrap();
            let real = versioned.join("cursor-agent");
            File::create(&real).unwrap();
            let elsewhere = root.join("elsewhere");
            fs::create_dir_all(&elsewhere).unwrap();
            let link = elsewhere.join("cursor-agent");
            std::os::unix::fs::symlink(&real, &link).unwrap();

            assert!(!fingerprint_curl_pipe(&link, &home));
            let _ = fs::remove_dir_all(&root);
        }
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
