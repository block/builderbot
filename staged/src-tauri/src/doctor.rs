//! System Health Check ("Doctor") — backend checks + optional auto-fix.
//!
//! Each check probes a single external dependency and returns a status
//! (pass / warn / fail) with a human-readable summary and optional fix.

use serde::{Deserialize, Serialize};
use std::process::Command;

use crate::agent;
use crate::git;

// =============================================================================
// Types
// =============================================================================

/// Severity level for a single check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
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
    /// If non-None, the UI can offer a "Fix" button.
    /// The value is a shell command the backend will execute.
    pub fix_command: Option<String>,
}

/// The full report returned to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

// =============================================================================
// Individual checks
// =============================================================================

/// Check that `git` is installed and reachable.
fn check_git() -> DoctorCheck {
    let label = "Git".to_string();
    let id = "git".to_string();

    match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Pass,
                message: version,
                fix_command: None,
            }
        }
        _ => DoctorCheck {
            id,
            label,
            status: CheckStatus::Fail,
            message: "Git not found".to_string(),
            fix_command: Some("brew install git".to_string()),
        },
    }
}

/// Check that the GitHub CLI (`gh`) is installed.
fn check_gh() -> DoctorCheck {
    let label = "GitHub CLI".to_string();
    let id = "gh".to_string();

    match Command::new("gh").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            let first_line = version.lines().next().unwrap_or("gh").trim().to_string();
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Pass,
                message: first_line,
                fix_command: None,
            }
        }
        _ => DoctorCheck {
            id,
            label,
            status: CheckStatus::Fail,
            message: "GitHub CLI not found".to_string(),
            fix_command: Some("brew install gh".to_string()),
        },
    }
}

/// Check that `gh auth status` succeeds (user is logged in).
fn check_gh_auth() -> DoctorCheck {
    let label = "GitHub Auth".to_string();
    let id = "gh-auth".to_string();

    let auth = git::check_github_auth();
    if auth.authenticated {
        DoctorCheck {
            id,
            label,
            status: CheckStatus::Pass,
            message: "Authenticated".to_string(),
            fix_command: None,
        }
    } else {
        DoctorCheck {
            id,
            label,
            status: CheckStatus::Fail,
            message: auth
                .setup_hint
                .unwrap_or_else(|| "Not authenticated".to_string()),
            fix_command: Some("gh auth login".to_string()),
        }
    }
}

/// Check that Git LFS is installed.
fn check_git_lfs() -> DoctorCheck {
    let label = "Git LFS".to_string();
    let id = "git-lfs".to_string();

    match Command::new("git").args(["lfs", "version"]).output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Pass,
                message: version,
                fix_command: None,
            }
        }
        _ => DoctorCheck {
            id,
            label,
            status: CheckStatus::Warn,
            message: "Git LFS not installed (optional, needed for large files)".to_string(),
            fix_command: Some("brew install git-lfs && git lfs install".to_string()),
        },
    }
}

/// Check that at least one ACP-compatible AI agent is discoverable.
fn check_ai_agent() -> DoctorCheck {
    let label = "AI Agent".to_string();
    let id = "ai-agent".to_string();

    let providers = agent::discover_providers();
    if providers.is_empty() {
        DoctorCheck {
            id,
            label,
            status: CheckStatus::Warn,
            message: "No AI agent found. Install one to enable AI features.".to_string(),
            fix_command: None,
        }
    } else {
        let names: Vec<String> = providers.iter().map(|p| p.label.clone()).collect();
        DoctorCheck {
            id,
            label,
            status: CheckStatus::Pass,
            message: format!("Found: {}", names.join(", ")),
            fix_command: None,
        }
    }
}

// =============================================================================
// Tauri commands
// =============================================================================

/// Run all health checks and return the report.
#[tauri::command]
pub async fn run_doctor() -> DoctorReport {
    // Run checks on a blocking thread since they shell out.
    tokio::task::spawn_blocking(|| DoctorReport {
        checks: vec![
            check_git(),
            check_gh(),
            check_gh_auth(),
            check_git_lfs(),
            check_ai_agent(),
        ],
    })
    .await
    .unwrap_or_else(|_| DoctorReport { checks: vec![] })
}

/// Execute a fix command for a specific check.
/// Returns the updated check after re-running it.
#[tauri::command]
pub async fn run_doctor_fix(check_id: String) -> Result<DoctorCheck, String> {
    tokio::task::spawn_blocking(move || {
        // Find the check and its fix command
        let check = match check_id.as_str() {
            "git" => check_git(),
            "gh" => check_gh(),
            "gh-auth" => check_gh_auth(),
            "git-lfs" => check_git_lfs(),
            "ai-agent" => check_ai_agent(),
            _ => return Err(format!("Unknown check: {check_id}")),
        };

        let fix_cmd = match &check.fix_command {
            Some(cmd) => cmd.clone(),
            None => return Err(format!("No fix available for: {check_id}")),
        };

        // Execute the fix command
        let output = Command::new("sh")
            .args(["-c", &fix_cmd])
            .output()
            .map_err(|e| format!("Failed to run fix: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Fix command failed: {}", stderr.trim()));
        }

        // Re-run the check to get the updated status
        let updated = match check_id.as_str() {
            "git" => check_git(),
            "gh" => check_gh(),
            "gh-auth" => check_gh_auth(),
            "git-lfs" => check_git_lfs(),
            "ai-agent" => check_ai_agent(),
            _ => unreachable!(),
        };

        Ok(updated)
    })
    .await
    .map_err(|e| format!("Task panicked: {e}"))?
}
