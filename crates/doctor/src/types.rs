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
#[serde(rename_all = "lowercase")]
pub enum FixType {
    /// A shell command to install or configure the dependency.
    Command,
    /// A shell command to install the ACP bridge binary.
    Bridge,
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
}
