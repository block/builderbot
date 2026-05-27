//! Per-check package identifiers for version-freshness lookups.
//!
//! Maps a doctor check ID to one or more `(InstallSource, package_id)` pairs.
//! The freshness module dispatches to the registry that matches the binary's
//! detected install source. Checks not listed here (or with no matching
//! source) skip the latest-version probe.

use crate::types::InstallSource;

/// Static table of `check_id -> &[(install_source, package_id)]`.
///
/// A single check can have multiple entries when the same agent ships through
/// different registries (e.g. brew for the main binary, npm for the ACP bridge).
/// `lookup_package_id` picks the first entry whose `install_source` matches the
/// binary's detected source.
pub(crate) const PACKAGE_IDS: &[(&str, &[(InstallSource, &str)])] = &[
    ("git", &[(InstallSource::Brew, "git")]),
    ("gh", &[(InstallSource::Brew, "gh")]),
    ("git-lfs", &[(InstallSource::Brew, "git-lfs")]),
    // ai-agent-goose: brew tap exists but no canonical formula yet — skip.
    // TODO: revisit when block/goose lands a stable brew formula.
    (
        "ai-agent-claude",
        &[(InstallSource::Npm, "@agentclientprotocol/claude-agent-acp")],
    ),
    (
        "ai-agent-codex",
        &[
            (InstallSource::Npm, "@zed-industries/codex-acp"),
            (InstallSource::Brew, "codex"),
        ],
    ),
    ("ai-agent-pi", &[(InstallSource::Npm, "pi-acp")]),
    (
        "ai-agent-amp",
        &[
            (InstallSource::Npm, "amp-acp"),
            (InstallSource::Brew, "amp"),
        ],
    ),
    (
        "ai-agent-copilot",
        &[(InstallSource::Npm, "@github/copilot")],
    ),
    // ai-agent-cursor: curl-pipe installer with no registry presence.
    // TODO: GitHub releases would be a reasonable follow-up source.
];

/// Pick the package id whose source matches `source`. Returns `None` if the
/// check isn't in the table, or if no entry matches.
pub(crate) fn lookup_package_id(check_id: &str, source: InstallSource) -> Option<&'static str> {
    for (id, entries) in PACKAGE_IDS {
        if *id == check_id {
            for (entry_source, pkg) in entries.iter() {
                if entry_source == &source {
                    return Some(*pkg);
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
        assert_eq!(lookup_package_id("git", InstallSource::Brew), Some("git"),);
        assert_eq!(
            lookup_package_id("ai-agent-claude", InstallSource::Npm),
            Some("@agentclientprotocol/claude-agent-acp"),
        );
    }

    #[test]
    fn lookup_returns_none_for_mismatched_source() {
        // git is registered under Brew only — Npm should miss.
        assert_eq!(lookup_package_id("git", InstallSource::Npm), None);
    }

    #[test]
    fn lookup_returns_none_for_unknown_check() {
        assert_eq!(
            lookup_package_id("ai-agent-cursor", InstallSource::CurlPipe),
            None,
        );
        assert_eq!(lookup_package_id("nonexistent", InstallSource::Brew), None);
    }

    #[test]
    fn codex_has_both_npm_and_brew_entries() {
        assert!(lookup_package_id("ai-agent-codex", InstallSource::Npm).is_some());
        assert!(lookup_package_id("ai-agent-codex", InstallSource::Brew).is_some());
    }
}
