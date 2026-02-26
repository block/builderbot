//! Health Check ("Doctor") — backend checks for external dependencies.
//!
//! Each check probes a single external dependency and returns a status
//! (pass / warn / fail) with a human-readable summary and an optional
//! URL the user can visit to install or configure the dependency.

use serde::{Deserialize, Serialize};
use std::process::Command;

use acp_client::find_command;

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
    /// If non-None, the UI shows an "Install" link that opens this URL.
    pub fix_url: Option<String>,
    /// If non-None, the UI shows a "Fix" button that runs this shell command.
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
                fix_url: None,
                fix_command: None,
            }
        }
        _ => DoctorCheck {
            id,
            label,
            status: CheckStatus::Fail,
            message: "Git not found".to_string(),
            fix_url: Some("https://git-scm.com/downloads".to_string()),
            fix_command: None,
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
                fix_url: None,
                fix_command: None,
            }
        }
        _ => DoctorCheck {
            id,
            label,
            status: CheckStatus::Fail,
            message: "GitHub CLI not found".to_string(),
            fix_url: Some("https://cli.github.com".to_string()),
            fix_command: None,
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
            fix_url: None,
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
            fix_url: Some("https://cli.github.com/manual/gh_auth_login".to_string()),
            fix_command: None,
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
                fix_url: None,
                fix_command: None,
            }
        }
        _ => DoctorCheck {
            id,
            label,
            status: CheckStatus::Warn,
            message: "Git LFS not installed (optional, needed for large files)".to_string(),
            fix_url: Some("https://git-lfs.com".to_string()),
            fix_command: None,
        },
    }
}

/// Check that `core.clonefile` is enabled in the global git config.
///
/// On macOS (APFS), `core.clonefile = true` enables copy-on-write clones
/// which makes git worktrees use significantly less disk space.
fn check_clonefile() -> DoctorCheck {
    let label = "Copy on Write Git Clones".to_string();
    let id = "git-clonefile".to_string();
    let fix_cmd = "git config --global core.clonefile true".to_string();

    match Command::new("git")
        .args(["config", "--global", "core.clonefile"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_lowercase();
            if value == "true" {
                DoctorCheck {
                    id,
                    label,
                    status: CheckStatus::Pass,
                    message: "Enabled — reduces disk space used by new worktrees".to_string(),
                    fix_url: None,
                    fix_command: None,
                }
            } else {
                DoctorCheck {
                    id,
                    label,
                    status: CheckStatus::Warn,
                    message: "Disabled — enable to reduce disk space used by new worktrees"
                        .to_string(),
                    fix_url: None,
                    fix_command: Some(fix_cmd),
                }
            }
        }
        // Key not set — treat as not enabled
        _ => DoctorCheck {
            id,
            label,
            status: CheckStatus::Warn,
            message: "Not set — enable to reduce disk space used by new worktrees".to_string(),
            fix_url: None,
            fix_command: Some(fix_cmd),
        },
    }
}

/// Metadata for an individual AI agent check.
struct AgentCheckInfo {
    /// Check ID used in the doctor report, e.g. "ai-agent-goose".
    id: &'static str,
    /// Human-readable label, e.g. "Goose".
    label: &'static str,
    /// CLI command names to search for (first entry is preferred/current).
    commands: &'static [&'static str],
    /// URL to open when the agent is not found (None if no install page).
    install_url: Option<&'static str>,
}

/// All AI agents we check for individually.
/// At least one must be installed; the rest are optional.
const AI_AGENT_CHECKS: &[AgentCheckInfo] = &[
    AgentCheckInfo {
        id: "ai-agent-goose",
        label: "Goose",
        commands: &["goose"],
        install_url: Some("https://github.com/block/goose"),
    },
    AgentCheckInfo {
        id: "ai-agent-claude",
        label: "Claude Code",
        commands: &["claude-agent-acp"],
        install_url: Some("https://github.com/zed-industries/claude-agent-acp#installation"),
    },
    AgentCheckInfo {
        id: "ai-agent-codex",
        label: "Codex",
        commands: &["codex-acp"],
        install_url: Some("https://github.com/openai/codex#getting-started"),
    },
    AgentCheckInfo {
        id: "ai-agent-pi",
        label: "Pi",
        commands: &["pi-acp"],
        install_url: None,
    },
    AgentCheckInfo {
        id: "ai-agent-amp",
        label: "Amp",
        commands: &["amp-acp"],
        install_url: Some("https://www.npmjs.com/package/amp-acp"),
    },
];

fn agent_installed(info: &AgentCheckInfo) -> bool {
    info.commands.iter().any(|cmd| find_command(cmd).is_some())
}

/// Check whether a single AI agent is installed.
///
/// If at least one other agent is already found (`any_agent_found`), missing
/// agents get `Warn`; otherwise the first missing agent gets `Warn` too since
/// only one agent is required overall.
fn check_single_ai_agent(info: &AgentCheckInfo, any_agent_found: bool) -> DoctorCheck {
    if agent_installed(info) {
        // Special handling for Goose: verify ACP subcommand is available
        if info.id == "ai-agent-goose" {
            match Command::new("goose").arg("acp").arg("--help").output() {
                Ok(output) if output.status.success() => DoctorCheck {
                    id: info.id.to_string(),
                    label: info.label.to_string(),
                    status: CheckStatus::Pass,
                    message: "Installed".to_string(),
                    fix_url: None,
                    fix_command: None,
                },
                _ => {
                    // Goose is installed but ACP subcommand is not available
                    DoctorCheck {
                        id: info.id.to_string(),
                        label: info.label.to_string(),
                        status: CheckStatus::Fail,
                        message: "Goose ACP subcommand not available — upgrade required"
                            .to_string(),
                        fix_url: Some("https://github.com/block/goose".to_string()),
                        fix_command: None,
                    }
                }
            }
        } else {
            DoctorCheck {
                id: info.id.to_string(),
                label: info.label.to_string(),
                status: CheckStatus::Pass,
                message: "Installed".to_string(),
                fix_url: None,
                fix_command: None,
            }
        }
    } else {
        DoctorCheck {
            id: info.id.to_string(),
            label: info.label.to_string(),
            status: CheckStatus::Warn,
            message: if any_agent_found {
                "Not installed (optional)".to_string()
            } else {
                "Not installed — at least one AI agent is needed".to_string()
            },
            fix_url: info.install_url.map(|s| s.to_string()),
            fix_command: None,
        }
    }
}

/// Produce a `DoctorCheck` for each known AI agent.
///
/// Each agent gets its own row. At least one must be installed for full
/// functionality, but each individual missing agent is only a warning.
fn check_ai_agents() -> Vec<DoctorCheck> {
    // First pass: determine which agents are installed.
    let installed: Vec<bool> = AI_AGENT_CHECKS.iter().map(agent_installed).collect();
    let any_found = installed.iter().any(|&b| b);

    // Second pass: build the checks with appropriate messaging.
    AI_AGENT_CHECKS
        .iter()
        .map(|info| check_single_ai_agent(info, any_found))
        .collect()
}

// =============================================================================
// Tauri commands
// =============================================================================

/// Run all health checks and return the report.
#[tauri::command]
pub async fn run_doctor() -> DoctorReport {
    // Run checks on a blocking thread since they shell out.
    tokio::task::spawn_blocking(|| {
        let mut checks = vec![
            check_git(),
            check_gh(),
            check_gh_auth(),
            check_git_lfs(),
            check_clonefile(),
        ];
        checks.extend(check_ai_agents());
        DoctorReport { checks }
    })
    .await
    .unwrap_or_else(|_| DoctorReport { checks: vec![] })
}

/// Run a fix command from a doctor check.
///
/// Executes the given shell command and returns Ok(()) on success,
/// or an error message if the command fails.
#[tauri::command]
pub async fn run_doctor_fix(command: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let output = Command::new("sh")
            .args(["-c", &command])
            .output()
            .map_err(|e| format!("Failed to run command: {e}"))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("Command failed with exit code {}", output.status)
            } else {
                stderr
            })
        }
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {e}")))
}
