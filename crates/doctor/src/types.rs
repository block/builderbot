//! Types shared across the doctor sub-modules.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Severity level for a single check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

/// The type of fix available for a check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FixType {
    /// A shell command to install or configure the dependency.
    Command,
    /// A shell command to install the ACP bridge binary.
    Bridge,
    /// A shell command that triggers an authentication flow.
    Auth,
    /// A shell command to update the agent's main CLI. Source-aware: derived
    /// from the readout's `(install_source, package_id)` so an npm-installed
    /// agent is updated via `npm install -g …@latest`, not the install-time
    /// curl-pipe recipe. The command is not in the static AI_AGENT_CHECKS
    /// table; the executor receives it via `command_override`.
    UpdateMain,
    /// A shell command to update the agent's ACP bridge. Same shape as
    /// `UpdateMain` — derived per-readout, passed via `command_override`.
    UpdateBridge,
}

/// Authentication state for a check that probes credentials.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AuthStatus {
    Authenticated,
    NotAuthenticated,
    NotApplicable,
    /// Probe couldn't determine auth state because the agent's
    /// `auth_status_command` failed to spawn or exited with 127 (command not
    /// found). Distinct from `NotAuthenticated`: a PATH-shadowed agent isn't
    /// signed out, we just can't tell. Downstream consumers should treat this
    /// as informational (no auth fix to offer).
    Unknown,
}

/// How a binary was installed on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InstallSource {
    Brew,
    Npm,
    Cargo,
    Mise,
    Asdf,
    CurlPipe,
    System,
    Unknown,
}

/// Version + install-source readout for one binary behind an agent check.
///
/// An AI-agent check may front two distinct binaries — the agent's own CLI
/// (`main`) and its ACP bridge (`bridge`) — each installed and versioned
/// independently. This struct carries one binary's readout so the two can be
/// surfaced side by side instead of collapsed into a single source/version.
///
/// All fields default to `None`; the cheap (no-freshness) path populates only
/// `install_source`, and the freshness pass fills in the version fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersionInfo {
    /// How this binary was installed (brew, npm, curl/native, …), if detected.
    pub install_source: Option<InstallSource>,
    /// Installed version string, if detected by the freshness pass.
    pub installed_version: Option<String>,
    /// Latest available version string, if known.
    pub latest_version: Option<String>,
    /// Whether a newer version is available. Suppressed (`None`) for
    /// self-updating installs — see [`DoctorCheck::update_available`].
    pub update_available: Option<bool>,
    /// Whether this binary keeps itself up to date (curl/native installers).
    pub self_updating: Option<bool>,
    /// Source-aware update command for this readout, derived from
    /// `(install_source, package_id)`. `Some` only when an update is both
    /// computable (the install source has a known update recipe and a
    /// registered package id) and actionable (`update_available == Some(true)`
    /// and the binary is not self-updating). Pairs with `update_fix_type`.
    pub update_command: Option<String>,
    /// `FixType::UpdateMain` or `FixType::UpdateBridge` matching this readout's
    /// slot. Always paired with `update_command`: both `Some` or both `None`.
    pub update_fix_type: Option<FixType>,
}

/// A single health-check result shown in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheck {
    /// Short identifier, e.g. "git"
    pub id: String,
    /// Human-readable label, e.g. "Git"
    pub label: String,
    pub status: CheckStatus,
    /// One-line explanation shown next to the status badge.
    pub message: String,
    /// If non-None, the UI shows an "Install" link that opens this URL.
    pub fix_url: Option<String>,
    /// If non-None, the UI shows the command text in a confirmation dialog.
    /// Display-only — never sent back to the backend for execution.
    pub fix_command: Option<String>,
    /// The type of fix available for this check.
    /// Sent back to the backend along with the check ID to execute the fix.
    pub fix_type: Option<FixType>,
    /// If non-None, the resolved path to the main executable on disk.
    pub path: Option<String>,
    /// If non-None, the resolved path to the ACP bridge executable on disk.
    pub bridge_path: Option<String>,
    /// Raw debug output: command stdout/stderr, search paths tried, etc.
    /// Used by the "Copy details" feature for support diagnostics.
    pub raw_output: Option<String>,
    /// Authentication status, when the check probes credentials.
    pub auth_status: Option<AuthStatus>,
    /// Installed version string, if detected.
    pub installed_version: Option<String>,
    /// Latest available version string, if known.
    pub latest_version: Option<String>,
    /// Whether a newer version is available than the installed one.
    ///
    /// For self-updating tools (see `self_updating`) this is never `Some(true)`:
    /// the tool manages its own freshness, so we don't raise an update nag even
    /// when a newer version exists upstream.
    pub update_available: Option<bool>,
    /// How the binary was installed (brew, npm, cargo, etc.), if detected.
    pub install_source: Option<InstallSource>,
    /// Whether this tool keeps itself up to date (curl/native installers such as
    /// Claude native, Cursor, and Amp-curl). When `Some(true)`, the freshness
    /// pass reports the installed/latest versions for display but suppresses
    /// `update_available` — the update is "managed by the tool", not actionable.
    /// Only populated by the freshness pass; `None` on the default cheap path.
    pub self_updating: Option<bool>,
    /// Independent readout for the agent's own CLI (e.g. `claude`, `codex`).
    ///
    /// For AI-agent checks this carries the main CLI's install source and (when
    /// freshness runs) its versions, separate from the ACP bridge. `None` for
    /// non-agent checks and for agents with no resolvable main CLI.
    ///
    /// The flat fields above remain populated for backward compatibility: they
    /// mirror the `bridge` readout when a bridge exists, otherwise `main`.
    pub main: Option<AgentVersionInfo>,
    /// Independent readout for the agent's ACP bridge (e.g. `claude-agent-acp`,
    /// `codex-acp`). `None` for non-agent checks and for agents that have no
    /// separate bridge binary (the single binary is reported under `main`).
    pub bridge: Option<AgentVersionInfo>,
}

/// The full report returned to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

/// A resolved binary: the path (if found) and the diagnostic search trace.
#[derive(Clone)]
pub struct ResolvedBinary {
    pub path: Option<PathBuf>,
    pub search_output: String,
    pub install_source: Option<InstallSource>,
}
