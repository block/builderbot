//! Per-check package identifiers for version-freshness lookups.
//!
//! Maps a doctor check ID to one or more `(InstallSource, package_id,
//! LatestSource, Role)` entries. The freshness module picks the entry whose
//! `InstallSource` matches the binary's detected install source AND whose
//! `Role` matches the readout being built (the agent's main CLI vs. its ACP
//! bridge), then fetches the latest version via the entry's `LatestSource`.
//! Checks not listed here (or with no matching source) skip the latest-version
//! probe.

use std::path::Path;

use crate::types::InstallSource;

/// How to fetch the "latest available" version for a package.
///
/// For registry installs this mirrors the install source 1:1 (a brew binary
/// checks brew, an npm binary checks npm). Non-registry installs (curl/native)
/// need an explicit mechanism that doesn't follow from the install source —
/// e.g. Cursor's curl install has no registry presence and is tracked via
/// GitHub releases, so its `CurlPipe` entry fetches from `GitHubReleases`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LatestSource {
    Brew,
    Npm,
    /// Retained so a future cargo-installed tool can reach `latest_crates_io`;
    /// no entry in `PACKAGE_IDS` selects it yet (no agent ships via crates.io).
    #[allow(dead_code)]
    CratesIo,
    GitHubReleases,
}

/// Which binary an entry describes. An AI-agent check fronts up to two distinct
/// binaries — the agent's own CLI (`Main`) and its ACP bridge (`Bridge`). When
/// both are installed from the same registry (e.g. both via npm, as with
/// Claude) the install source alone is ambiguous and the role is what
/// disambiguates them. Non-agent checks (and agents whose two binaries already
/// have distinct install sources) use `Any`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Main,
    Bridge,
    /// Entry applies to either readout (and to flat, non-agent checks).
    Any,
}

impl Role {
    /// `query` matches `entry` when:
    /// - the entry's role is `Any` (it applies anywhere), or
    /// - the query is `Any` (the caller doesn't care which role), or
    /// - both roles are equal.
    fn matches(entry: Role, query: Role) -> bool {
        matches!(entry, Role::Any) || matches!(query, Role::Any) || entry == query
    }
}

/// One freshness entry: the install source it applies to, the package id to
/// query, how to fetch that package's latest version, and which readout (main
/// CLI vs ACP bridge) it describes.
type PackageEntry = (InstallSource, &'static str, LatestSource, Role);

/// Static table of `check_id -> &[PackageEntry]`.
///
/// A single check can have multiple entries when the same agent ships through
/// different registries (e.g. brew for the main binary, npm for the ACP bridge)
/// or when both binaries share an install source but have distinct package ids
/// (Claude: `@anthropic-ai/claude-code` for the main CLI, `@agentclientprotocol
/// /claude-agent-acp` for the bridge — both npm). `lookup_package_id` picks the
/// first entry whose `(install_source, role)` matches the query.
pub(crate) const PACKAGE_IDS: &[(&str, &[PackageEntry])] = &[
    (
        "git",
        &[(InstallSource::Brew, "git", LatestSource::Brew, Role::Any)],
    ),
    (
        "gh",
        &[(InstallSource::Brew, "gh", LatestSource::Brew, Role::Any)],
    ),
    (
        "git-lfs",
        &[(
            InstallSource::Brew,
            "git-lfs",
            LatestSource::Brew,
            Role::Any,
        )],
    ),
    // ai-agent-goose: brew tap exists but no canonical formula yet — skip.
    // TODO: revisit when block/goose lands a stable brew formula.
    (
        "ai-agent-claude",
        &[
            // Official Homebrew cask (`brew install --cask claude-code`) for
            // the main Claude Code CLI.
            (
                InstallSource::Brew,
                "claude-code",
                LatestSource::Brew,
                Role::Main,
            ),
            // Main CLI when installed via npm (e.g. under nvm). The native
            // curl-pipe install is fingerprinted as `CurlPipe` (no registry
            // entry here, self-updating) so this only applies when Claude
            // landed via `npm i -g @anthropic-ai/claude-code`.
            (
                InstallSource::Npm,
                "@anthropic-ai/claude-code",
                LatestSource::Npm,
                Role::Main,
            ),
            // ACP bridge — separate npm package.
            (
                InstallSource::Npm,
                "@agentclientprotocol/claude-agent-acp",
                LatestSource::Npm,
                Role::Bridge,
            ),
        ],
        // TODO: the main `claude` native (CurlPipe) install has no registry
        // entry — its latest is published via the native installer's channel
        // manifest, which we don't parse yet. Claude native is self-updating
        // (see `freshness::is_self_updating`), so it stays report-only for now.
    ),
    (
        "ai-agent-codex",
        &[
            // Bridge ships via npm; main CLI via brew or npm. Role tags
            // disambiguate the two npm packages (bridge vs main).
            (
                InstallSource::Npm,
                "@zed-industries/codex-acp",
                LatestSource::Npm,
                Role::Bridge,
            ),
            (InstallSource::Brew, "codex", LatestSource::Brew, Role::Main),
            // Main CLI when installed via npm. WARNING: the unscoped `codex`
            // package on npm is an unrelated 2012 project; only the scoped
            // `@openai/codex` is OpenAI's CLI.
            (
                InstallSource::Npm,
                "@openai/codex",
                LatestSource::Npm,
                Role::Main,
            ),
        ],
    ),
    (
        "ai-agent-pi",
        &[(
            InstallSource::Npm,
            "pi-acp",
            LatestSource::Npm,
            Role::Bridge,
        )],
    ),
    (
        "ai-agent-amp",
        &[
            // Bridge: npm. Main: brew. Main curl-pipe install is `CurlPipe`,
            // not present in the table (self-updating, report-only).
            (
                InstallSource::Npm,
                "amp-acp",
                LatestSource::Npm,
                Role::Bridge,
            ),
            // Sourcegraph Amp ships from the `ampcode/tap` tap as `ampcode`.
            // WARNING: homebrew-core's `amp` formula is an unrelated GPL-3.0
            // terminal text editor — do NOT use that package id here, or
            // `brew upgrade amp` will silently swap in the text editor.
            (
                InstallSource::Brew,
                "ampcode",
                LatestSource::Brew,
                Role::Main,
            ),
        ],
    ),
    (
        "ai-agent-copilot",
        &[
            (
                InstallSource::Npm,
                "@github/copilot",
                LatestSource::Npm,
                Role::Any,
            ),
            // Official Homebrew cask (`brew install --cask copilot-cli`).
            (
                InstallSource::Brew,
                "copilot-cli",
                LatestSource::Brew,
                Role::Any,
            ),
        ],
    ),
    // ai-agent-cursor: curl-pipe installer with no registry presence; its
    // releases are published on GitHub. The repo slug is a best-effort default
    // and the GitHub fetcher degrades to `None` on a miss. Cursor is
    // self-updating, so this latest is report-only (no update nag).
    (
        "ai-agent-cursor",
        &[
            (
                InstallSource::CurlPipe,
                "getcursor/cursor",
                LatestSource::GitHubReleases,
                Role::Any,
            ),
            // Official Homebrew cask (`brew install --cask cursor-cli`) for
            // the headless `cursor-agent` CLI.
            (
                InstallSource::Brew,
                "cursor-cli",
                LatestSource::Brew,
                Role::Any,
            ),
        ],
    ),
];

/// Pick the package id and its latest-version source for the entry whose
/// `(install_source, role)` matches. Returns `None` if the check isn't in the
/// table, or if no entry matches both the source and the role.
pub(crate) fn lookup_package_id(
    check_id: &str,
    source: InstallSource,
    role: Role,
) -> Option<(&'static str, LatestSource)> {
    for (id, entries) in PACKAGE_IDS {
        if *id == check_id {
            for (entry_source, pkg, latest, entry_role) in entries.iter() {
                if entry_source == &source && Role::matches(*entry_role, role) {
                    return Some((*pkg, *latest));
                }
            }
            return None;
        }
    }
    None
}

/// Pick the package id and latest-version source for a resolved binary.
///
/// Most entries are static. Claude Code's Homebrew casks are the exception:
/// both `claude-code` and `claude-code@latest` expose the same `claude`
/// command, so the command name alone cannot distinguish the installed cask
/// channel. When the resolved binary points into Homebrew's Caskroom, preserve
/// the owning Claude cask token, but only for the known allowlisted tokens.
pub(crate) fn lookup_package_id_for_binary(
    check_id: &str,
    source: InstallSource,
    role: Role,
    binary_path: Option<&Path>,
) -> Option<(String, LatestSource)> {
    if let Some(package_id) = path_package_id_override(check_id, source.clone(), role, binary_path)
    {
        return Some((package_id.to_string(), LatestSource::Brew));
    }

    lookup_package_id(check_id, source, role)
        .map(|(package_id, latest)| (package_id.to_string(), latest))
}

fn path_package_id_override(
    check_id: &str,
    source: InstallSource,
    role: Role,
    binary_path: Option<&Path>,
) -> Option<&'static str> {
    if check_id != "ai-agent-claude" || source != InstallSource::Brew || role != Role::Main {
        return None;
    }

    let binary_path = binary_path?;
    claude_code_cask_token_from_path(binary_path)
}

fn claude_code_cask_token_from_path(path: &Path) -> Option<&'static str> {
    if let Some(token) = claude_code_cask_token_from_caskroom_path(path) {
        return Some(token);
    }

    if let Some(target) = immediate_symlink_target(path) {
        if let Some(token) = claude_code_cask_token_from_caskroom_path(&target) {
            return Some(token);
        }
    }

    if let Ok(canonical) = path.canonicalize() {
        if let Some(token) = claude_code_cask_token_from_caskroom_path(&canonical) {
            return Some(token);
        }
    }

    None
}

fn immediate_symlink_target(path: &Path) -> Option<std::path::PathBuf> {
    let target = std::fs::read_link(path).ok()?;
    Some(if target.is_absolute() {
        target
    } else {
        path.parent()
            .map(|parent| parent.join(&target))
            .unwrap_or(target)
    })
}

fn claude_code_cask_token_from_caskroom_path(path: &Path) -> Option<&'static str> {
    let mut components = path.components().filter_map(|c| c.as_os_str().to_str());
    while let Some(component) = components.next() {
        if !component.eq_ignore_ascii_case("Caskroom") {
            continue;
        }

        return allowed_claude_code_cask_token(components.next()?);
    }

    None
}

fn allowed_claude_code_cask_token(token: &str) -> Option<&'static str> {
    match token {
        "claude-code" => Some("claude-code"),
        "claude-code@latest" => Some("claude-code@latest"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "doctor-package-ids-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lookup_matches_source_with_any_role() {
        assert_eq!(
            lookup_package_id("git", InstallSource::Brew, Role::Any),
            Some(("git", LatestSource::Brew)),
        );
    }

    #[test]
    fn lookup_returns_none_for_mismatched_source() {
        // git is registered under Brew only — Npm should miss.
        assert_eq!(
            lookup_package_id("git", InstallSource::Npm, Role::Any),
            None
        );
    }

    #[test]
    fn lookup_returns_none_for_unknown_check() {
        assert_eq!(
            lookup_package_id("nonexistent", InstallSource::Brew, Role::Any),
            None
        );
    }

    #[test]
    fn codex_has_role_tagged_entries() {
        assert_eq!(
            lookup_package_id("ai-agent-codex", InstallSource::Npm, Role::Bridge),
            Some(("@zed-industries/codex-acp", LatestSource::Npm)),
        );
        assert_eq!(
            lookup_package_id("ai-agent-codex", InstallSource::Brew, Role::Main),
            Some(("codex", LatestSource::Brew)),
        );
    }

    #[test]
    fn cursor_curl_pipe_resolves_to_github_releases() {
        let (_, latest) = lookup_package_id("ai-agent-cursor", InstallSource::CurlPipe, Role::Any)
            .expect("cursor entry");
        assert_eq!(latest, LatestSource::GitHubReleases);
    }

    /// The whole point of the role split: claude's main CLI under npm must
    /// resolve to `@anthropic-ai/claude-code`, not the bridge package.
    #[test]
    fn claude_main_npm_resolves_to_main_package() {
        assert_eq!(
            lookup_package_id("ai-agent-claude", InstallSource::Npm, Role::Main),
            Some(("@anthropic-ai/claude-code", LatestSource::Npm)),
        );
    }

    #[test]
    fn claude_main_brew_resolves_to_claude_code_cask() {
        assert_eq!(
            lookup_package_id("ai-agent-claude", InstallSource::Brew, Role::Main),
            Some(("claude-code", LatestSource::Brew)),
        );
    }

    #[test]
    fn claude_main_brew_preserves_latest_cask_token_from_caskroom_path() {
        let path = Path::new("/opt/homebrew/Caskroom/claude-code@latest/2.1.153/claude");

        assert_eq!(
            lookup_package_id_for_binary(
                "ai-agent-claude",
                InstallSource::Brew,
                Role::Main,
                Some(path),
            ),
            Some(("claude-code@latest".to_string(), LatestSource::Brew)),
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_main_brew_preserves_latest_cask_token_through_bin_symlink() {
        let root = scratch_dir("claude-cask-symlink");
        let target = root.join("Caskroom/claude-code@latest/2.1.153/claude");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "#!/bin/sh\n").unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        let link = root.join("bin/claude");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert_eq!(
            lookup_package_id_for_binary(
                "ai-agent-claude",
                InstallSource::Brew,
                Role::Main,
                Some(&link),
            ),
            Some(("claude-code@latest".to_string(), LatestSource::Brew)),
        );

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn claude_main_brew_preserves_latest_cask_token_from_immediate_symlink_target() {
        let root = scratch_dir("claude-cask-immediate-symlink");
        let npm_entry = root.join(
            "home/.nvm/versions/node/v23.7.0/lib/node_modules/@anthropic-ai/claude-code/cli/claude.js",
        );
        fs::create_dir_all(npm_entry.parent().unwrap()).unwrap();
        fs::write(&npm_entry, "#!/usr/bin/env node\n").unwrap();

        let cask_bin = root.join("Caskroom/claude-code@latest/2.1.153/claude");
        fs::create_dir_all(cask_bin.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&npm_entry, &cask_bin).unwrap();

        fs::create_dir_all(root.join("bin")).unwrap();
        let active = root.join("bin/claude");
        std::os::unix::fs::symlink(&cask_bin, &active).unwrap();

        assert_eq!(
            lookup_package_id_for_binary(
                "ai-agent-claude",
                InstallSource::Brew,
                Role::Main,
                Some(&active),
            ),
            Some(("claude-code@latest".to_string(), LatestSource::Brew)),
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_main_brew_ignores_unallowlisted_caskroom_token() {
        let path = Path::new("/opt/homebrew/Caskroom/not-claude/1.0.0/claude");

        assert_eq!(
            lookup_package_id_for_binary(
                "ai-agent-claude",
                InstallSource::Brew,
                Role::Main,
                Some(path),
            ),
            Some(("claude-code".to_string(), LatestSource::Brew)),
        );
    }

    /// And the bridge readout still resolves to the bridge package even though
    /// they share an install source.
    #[test]
    fn claude_bridge_npm_resolves_to_bridge_package() {
        assert_eq!(
            lookup_package_id("ai-agent-claude", InstallSource::Npm, Role::Bridge),
            Some(("@agentclientprotocol/claude-agent-acp", LatestSource::Npm)),
        );
    }

    /// A `Role::Any` query on a role-tagged check returns the first matching
    /// entry — used by non-agent (flat) lookups and as a permissive fallback.
    #[test]
    fn claude_npm_with_any_role_returns_first_match() {
        // The Main entry appears first in the table; Any should hit it.
        let (pkg, _) = lookup_package_id("ai-agent-claude", InstallSource::Npm, Role::Any).unwrap();
        assert_eq!(pkg, "@anthropic-ai/claude-code");
    }

    /// `Role::Any` entries match any query role — confirms copilot's single
    /// untagged entry is reachable from both Main and Bridge queries.
    #[test]
    fn copilot_any_entry_matches_main_and_bridge_queries() {
        assert!(lookup_package_id("ai-agent-copilot", InstallSource::Npm, Role::Main).is_some());
        assert!(lookup_package_id("ai-agent-copilot", InstallSource::Npm, Role::Bridge).is_some());
    }

    /// Sourcegraph Amp's brew formula is `ampcode/tap/ampcode`. Homebrew-core's
    /// `amp` is an unrelated GPL-3.0 terminal text editor — if Brew/Main ever
    /// resolves to `"amp"`, `brew upgrade amp` would silently swap in that
    /// editor.
    #[test]
    fn amp_brew_resolves_to_ampcode_not_text_editor() {
        assert_eq!(
            lookup_package_id("ai-agent-amp", InstallSource::Brew, Role::Main),
            Some(("ampcode", LatestSource::Brew)),
        );
    }

    #[test]
    fn cursor_brew_resolves_to_cursor_cli_cask() {
        assert_eq!(
            lookup_package_id("ai-agent-cursor", InstallSource::Brew, Role::Any),
            Some(("cursor-cli", LatestSource::Brew)),
        );
    }

    /// Guard that adding the brew entry didn't displace the curl-pipe lookup.
    #[test]
    fn cursor_curl_pipe_still_resolves() {
        let (pkg, latest) =
            lookup_package_id("ai-agent-cursor", InstallSource::CurlPipe, Role::Any)
                .expect("cursor curl-pipe entry");
        assert_eq!(pkg, "getcursor/cursor");
        assert_eq!(latest, LatestSource::GitHubReleases);
    }

    #[test]
    fn copilot_brew_resolves_to_copilot_cli_cask() {
        assert_eq!(
            lookup_package_id("ai-agent-copilot", InstallSource::Brew, Role::Any),
            Some(("copilot-cli", LatestSource::Brew)),
        );
    }

    #[test]
    fn copilot_npm_still_resolves() {
        assert_eq!(
            lookup_package_id("ai-agent-copilot", InstallSource::Npm, Role::Any),
            Some(("@github/copilot", LatestSource::Npm)),
        );
    }

    #[test]
    fn codex_npm_main_resolves_to_openai_codex() {
        assert_eq!(
            lookup_package_id("ai-agent-codex", InstallSource::Npm, Role::Main),
            Some(("@openai/codex", LatestSource::Npm)),
        );
    }

    /// Guard that adding the Main npm entry didn't shadow the existing Bridge
    /// entry — the role-tagged lookup must still pick the bridge package.
    #[test]
    fn codex_npm_bridge_unchanged() {
        assert_eq!(
            lookup_package_id("ai-agent-codex", InstallSource::Npm, Role::Bridge),
            Some(("@zed-industries/codex-acp", LatestSource::Npm)),
        );
    }
}
