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

/// Which binary an entry describes. An AI-agent check can front two distinct
/// binaries — the agent's own CLI (`Main`) and its ACP bridge (`Bridge`) —
/// and the role keeps their entries from answering each other's lookups when
/// they could share an install source. Non-agent checks and single-binary
/// agent checks use `Any`.
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
/// different registries (e.g. Amp: brew for the main binary, npm for the ACP
/// bridge). `lookup_package_id` picks the first entry whose
/// `(install_source, role)` matches the query.
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
    //
    // ai-agent-claude / ai-agent-codex: the ACP bridge is the only binary the
    // check fronts (it vendors the full harness CLI), so the single npm entry
    // is untagged. Bundled installs have no registry entry — their versions
    // are pinned by the embedding app's lock.
    (
        "ai-agent-claude",
        &[(
            InstallSource::Npm,
            "@agentclientprotocol/claude-agent-acp",
            LatestSource::Npm,
            Role::Any,
        )],
    ),
    (
        "ai-agent-codex",
        &[(
            InstallSource::Npm,
            "@agentclientprotocol/codex-acp",
            LatestSource::Npm,
            Role::Any,
        )],
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
    fn cursor_curl_pipe_resolves_to_github_releases() {
        let (_, latest) = lookup_package_id("ai-agent-cursor", InstallSource::CurlPipe, Role::Any)
            .expect("cursor entry");
        assert_eq!(latest, LatestSource::GitHubReleases);
    }

    /// Claude's single binary is the ACP bridge, reported under the main slot:
    /// its untagged entry must answer both Main and Any queries with the bridge
    /// npm package.
    #[test]
    fn claude_npm_resolves_to_bridge_package_for_main_and_any() {
        for role in [Role::Main, Role::Any] {
            assert_eq!(
                lookup_package_id("ai-agent-claude", InstallSource::Npm, role),
                Some(("@agentclientprotocol/claude-agent-acp", LatestSource::Npm)),
            );
        }
    }

    /// Same single-binary shape for codex — and the package id must be the
    /// maintained `@agentclientprotocol` scope, not the retired
    /// `@zed-industries` one.
    #[test]
    fn codex_npm_resolves_to_bridge_package_for_main_and_any() {
        for role in [Role::Main, Role::Any] {
            assert_eq!(
                lookup_package_id("ai-agent-codex", InstallSource::Npm, role),
                Some(("@agentclientprotocol/codex-acp", LatestSource::Npm)),
            );
        }
    }

    /// Bundled installs are pinned by the embedding app's lock and have no
    /// registry entry — no latest-version probe, no update nag.
    #[test]
    fn bundled_installs_have_no_registry_entry() {
        for id in ["ai-agent-claude", "ai-agent-codex"] {
            assert_eq!(
                lookup_package_id(id, InstallSource::Bundled, Role::Any),
                None,
                "{id}",
            );
        }
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
}
