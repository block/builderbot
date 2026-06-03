//! Per-check package identifiers for version-freshness lookups.
//!
//! Maps a doctor check ID to one or more `(InstallSource, package_id,
//! LatestSource)` entries. The freshness module picks the entry whose
//! `InstallSource` matches the binary's detected install source, then fetches
//! the latest version via the entry's `LatestSource`. Checks not listed here
//! (or with no matching source) skip the latest-version probe.

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

/// One freshness entry: the install source it applies to, the package id to
/// query, and how to fetch that package's latest version.
type PackageEntry = (InstallSource, &'static str, LatestSource);

/// Static table of `check_id -> &[PackageEntry]`.
///
/// A single check can have multiple entries when the same agent ships through
/// different registries (e.g. brew for the main binary, npm for the ACP bridge).
/// `lookup_package_id` picks the first entry whose `install_source` matches the
/// binary's detected source.
pub(crate) const PACKAGE_IDS: &[(&str, &[PackageEntry])] = &[
    ("git", &[(InstallSource::Brew, "git", LatestSource::Brew)]),
    ("gh", &[(InstallSource::Brew, "gh", LatestSource::Brew)]),
    (
        "git-lfs",
        &[(InstallSource::Brew, "git-lfs", LatestSource::Brew)],
    ),
    // ai-agent-goose: brew tap exists but no canonical formula yet — skip.
    // TODO: revisit when block/goose lands a stable brew formula.
    (
        "ai-agent-claude",
        &[(
            InstallSource::Npm,
            "@agentclientprotocol/claude-agent-acp",
            LatestSource::Npm,
        )],
        // TODO: the main `claude` native (CurlPipe) install has no registry
        // entry — its latest is published via the native installer's channel
        // manifest, which we don't parse yet. Claude native is self-updating
        // (see `freshness::is_self_updating`), so it stays report-only for now.
    ),
    (
        "ai-agent-codex",
        &[
            (
                InstallSource::Npm,
                "@zed-industries/codex-acp",
                LatestSource::Npm,
            ),
            (InstallSource::Brew, "codex", LatestSource::Brew),
        ],
    ),
    (
        "ai-agent-pi",
        &[(InstallSource::Npm, "pi-acp", LatestSource::Npm)],
    ),
    (
        "ai-agent-amp",
        &[
            (InstallSource::Npm, "amp-acp", LatestSource::Npm),
            (InstallSource::Brew, "amp", LatestSource::Brew),
        ],
    ),
    (
        "ai-agent-copilot",
        &[(InstallSource::Npm, "@github/copilot", LatestSource::Npm)],
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
        )],
    ),
];

/// Pick the package id and its latest-version source for the entry whose
/// install source matches `source`. Returns `None` if the check isn't in the
/// table, or if no entry matches.
pub(crate) fn lookup_package_id(
    check_id: &str,
    source: InstallSource,
) -> Option<(&'static str, LatestSource)> {
    for (id, entries) in PACKAGE_IDS {
        if *id == check_id {
            for (entry_source, pkg, latest) in entries.iter() {
                if entry_source == &source {
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
    fn lookup_matches_source() {
        assert_eq!(
            lookup_package_id("git", InstallSource::Brew),
            Some(("git", LatestSource::Brew)),
        );
        assert_eq!(
            lookup_package_id("ai-agent-claude", InstallSource::Npm),
            Some(("@agentclientprotocol/claude-agent-acp", LatestSource::Npm)),
        );
    }

    #[test]
    fn lookup_returns_none_for_mismatched_source() {
        // git is registered under Brew only — Npm should miss.
        assert_eq!(lookup_package_id("git", InstallSource::Npm), None);
    }

    #[test]
    fn lookup_returns_none_for_unknown_check() {
        assert_eq!(lookup_package_id("nonexistent", InstallSource::Brew), None);
    }

    #[test]
    fn codex_has_both_npm_and_brew_entries() {
        assert_eq!(
            lookup_package_id("ai-agent-codex", InstallSource::Npm),
            Some(("@zed-industries/codex-acp", LatestSource::Npm)),
        );
        assert_eq!(
            lookup_package_id("ai-agent-codex", InstallSource::Brew),
            Some(("codex", LatestSource::Brew)),
        );
    }

    #[test]
    fn cursor_curl_pipe_resolves_to_github_releases() {
        let (_, latest) =
            lookup_package_id("ai-agent-cursor", InstallSource::CurlPipe).expect("cursor entry");
        assert_eq!(latest, LatestSource::GitHubReleases);
    }
}
