//! Per-check package identifiers for version-freshness lookups.
//!
//! Maps a doctor check ID to one or more `(InstallSource, package_id,
//! LatestSource, Role)` entries. The freshness module picks the entry whose
//! `InstallSource` matches the binary's detected install source AND whose
//! `Role` matches the readout being built (the agent's main CLI vs. its ACP
//! bridge), then fetches the latest version via the entry's `LatestSource`.
//! Checks not listed here (or with no matching source) skip the latest-version
//! probe.

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
            // Main CLI when installed via the official Homebrew cask
            // (`brew install --cask claude-code`). The cask name is
            // `claude-code` — queryable via `brew info --json=v2 claude-code`.
            (
                InstallSource::Brew,
                "claude-code",
                LatestSource::Brew,
                Role::Main,
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
            // Bridge ships via npm; main CLI via brew — distinct install
            // sources so role disambiguation isn't strictly needed, but tag it
            // anyway for clarity.
            (
                InstallSource::Npm,
                "@zed-industries/codex-acp",
                LatestSource::Npm,
                Role::Bridge,
            ),
            (InstallSource::Brew, "codex", LatestSource::Brew, Role::Main),
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
            (InstallSource::Brew, "amp", LatestSource::Brew, Role::Main),
        ],
    ),
    (
        "ai-agent-copilot",
        &[(
            InstallSource::Npm,
            "@github/copilot",
            LatestSource::Npm,
            Role::Any,
        )],
    ),
    // ai-agent-cursor: curl-pipe installer with no registry presence; its
    // releases are published on GitHub. The repo slug is a best-effort default
    // and the GitHub fetcher degrades to `None` on a miss. Cursor is
    // self-updating, so this latest is report-only (no update nag).
    (
        "ai-agent-cursor",
        &[(
            InstallSource::CurlPipe,
            "getcursor/cursor",
            LatestSource::GitHubReleases,
            Role::Any,
        )],
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Brew-cask installs of Claude (`/opt/homebrew/bin/claude` -> Caskroom)
    /// resolve to `claude-code` on brew so `populate_freshness` can fetch a
    /// real latest and `derive_update_command` can emit `brew upgrade
    /// claude-code`.
    #[test]
    fn claude_main_brew_resolves_to_cask_package() {
        assert_eq!(
            lookup_package_id("ai-agent-claude", InstallSource::Brew, Role::Main),
            Some(("claude-code", LatestSource::Brew)),
        );
    }

    /// Guard against an accidental table reshuffle: the existing npm-main and
    /// npm-bridge entries must keep their ids after the brew addition.
    #[test]
    fn claude_npm_entries_unchanged_after_brew_addition() {
        assert_eq!(
            lookup_package_id("ai-agent-claude", InstallSource::Npm, Role::Main),
            Some(("@anthropic-ai/claude-code", LatestSource::Npm)),
        );
        assert_eq!(
            lookup_package_id("ai-agent-claude", InstallSource::Npm, Role::Bridge),
            Some(("@agentclientprotocol/claude-agent-acp", LatestSource::Npm)),
        );
    }

    /// `Role::Any` entries match any query role — confirms copilot's single
    /// untagged entry is reachable from both Main and Bridge queries.
    #[test]
    fn copilot_any_entry_matches_main_and_bridge_queries() {
        assert!(lookup_package_id("ai-agent-copilot", InstallSource::Npm, Role::Main).is_some());
        assert!(lookup_package_id("ai-agent-copilot", InstallSource::Npm, Role::Bridge).is_some());
    }
}
