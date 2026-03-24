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
    /// If non-None, the resolved path to the executable on disk.
    pub path: Option<String>,
    /// Raw debug output: command stdout/stderr, search paths tried, etc.
    /// Used by the "Copy debug info" feature for support diagnostics.
    pub raw_output: Option<String>,
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

/// Format the raw output of a command invocation for debug diagnostics.
fn format_command_output(cmd_desc: &str, output: &std::process::Output) -> String {
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

/// Format the search results for a command, mirroring the `find_command` strategy.
///
/// Tries login shell `which` first, then falls back to common paths. Stops
/// searching once a match is found, so the output reflects the real resolution.
fn format_search_output(cmd: &str) -> String {
    let mut lines = vec![format!("resolve '{cmd}':")];

    // Strategy 1: Login shell `which` (primary)
    lines.push("  strategy 1 — login shell `which`:".to_string());
    for shell in &["/bin/zsh", "/bin/bash"] {
        let which_cmd = format!("which {cmd}");
        match Command::new(shell).args(["-l", "-c", &which_cmd]).output() {
            Ok(output) => {
                let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if output.status.success() && !result.is_empty() {
                    lines.push(format!(
                        "    {shell} -l -c 'which {cmd}' => {result} (resolved)"
                    ));
                    return lines.join("\n");
                }
                lines.push(format!("    {shell} -l -c 'which {cmd}' => not found"));
            }
            Err(e) => {
                lines.push(format!("    {shell} -l -c 'which {cmd}' => error: {e}"));
            }
        }
    }

    // Strategy 2: Common install paths (fallback)
    lines.push("  strategy 2 — common install paths (fallback):".to_string());
    for dir in &[
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/home/linuxbrew/.linuxbrew/bin",
    ] {
        let path = std::path::PathBuf::from(dir).join(cmd);
        if path.exists() {
            lines.push(format!("    {} => found (resolved)", path.display()));
            return lines.join("\n");
        }
        lines.push(format!("    {} => not found", path.display()));
    }

    lines.push("  not found in any location".to_string());
    lines.join("\n")
}

/// Check that `git` is installed and reachable.
fn check_git() -> DoctorCheck {
    let label = "Git".to_string();
    let id = "git".to_string();
    let search = format_search_output("git");
    let header = "# Check: Git — verify git is installed and reachable";

    match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let path = find_command("git").map(|p| p.to_string_lossy().to_string());
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("git --version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Pass,
                message: version,
                fix_url: None,
                fix_command: None,
                path,
                raw_output: Some(raw),
            }
        }
        Ok(output) => {
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("git --version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Fail,
                message: "Git not found".to_string(),
                fix_url: Some("https://git-scm.com/downloads".to_string()),
                fix_command: None,
                path: None,
                raw_output: Some(raw),
            }
        }
        Err(e) => DoctorCheck {
            id,
            label,
            status: CheckStatus::Fail,
            message: "Git not found".to_string(),
            fix_url: Some("https://git-scm.com/downloads".to_string()),
            fix_command: None,
            path: None,
            raw_output: Some(format!("{header}\n$ git --version\nerror: {e}\n{search}")),
        },
    }
}

/// Check that the GitHub CLI (`gh`) is installed.
fn check_gh() -> DoctorCheck {
    let label = "GitHub CLI".to_string();
    let id = "gh".to_string();
    let search = format_search_output("gh");
    let header = "# Check: GitHub CLI — verify gh is installed";

    match Command::new("gh").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            let first_line = version.lines().next().unwrap_or("gh").trim().to_string();
            let path = find_command("gh").map(|p| p.to_string_lossy().to_string());
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("gh --version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Pass,
                message: first_line,
                fix_url: None,
                fix_command: None,
                path,
                raw_output: Some(raw),
            }
        }
        Ok(output) => {
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("gh --version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Fail,
                message: "GitHub CLI not found".to_string(),
                fix_url: Some("https://cli.github.com".to_string()),
                fix_command: None,
                path: None,
                raw_output: Some(raw),
            }
        }
        Err(e) => DoctorCheck {
            id,
            label,
            status: CheckStatus::Fail,
            message: "GitHub CLI not found".to_string(),
            fix_url: Some("https://cli.github.com".to_string()),
            fix_command: None,
            path: None,
            raw_output: Some(format!("{header}\n$ gh --version\nerror: {e}\n{search}")),
        },
    }
}

/// Check that `gh auth status` succeeds (user is logged in).
fn check_gh_auth() -> DoctorCheck {
    let label = "GitHub Auth".to_string();
    let id = "gh-auth".to_string();
    let header = "# Check: GitHub Auth — verify user is logged in to GitHub";

    let auth = git::check_github_auth();
    // Capture raw gh auth status output for diagnostics.
    let raw = match Command::new("gh").args(["auth", "status"]).output() {
        Ok(output) => format!(
            "{header}\n{}",
            format_command_output("gh auth status", &output)
        ),
        Err(e) => format!("{header}\n$ gh auth status\nerror: {e}"),
    };

    if auth.authenticated {
        DoctorCheck {
            id,
            label,
            status: CheckStatus::Pass,
            message: "Authenticated".to_string(),
            fix_url: None,
            fix_command: None,
            path: None,
            raw_output: Some(raw),
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
            path: None,
            raw_output: Some(raw),
        }
    }
}

/// Check that Git LFS is installed.
fn check_git_lfs() -> DoctorCheck {
    let label = "Git LFS".to_string();
    let id = "git-lfs".to_string();
    let search = format_search_output("git-lfs");
    let header =
        "# Check: Git LFS — verify git-lfs is installed (optional, needed for large files)";

    match Command::new("git").args(["lfs", "version"]).output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let path = find_command("git-lfs").map(|p| p.to_string_lossy().to_string());
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("git lfs version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Pass,
                message: version,
                fix_url: None,
                fix_command: None,
                path,
                raw_output: Some(raw),
            }
        }
        Ok(output) => {
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("git lfs version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Warn,
                message: "Git LFS not installed (optional, needed for large files)".to_string(),
                fix_url: Some("https://git-lfs.com".to_string()),
                fix_command: None,
                path: None,
                raw_output: Some(raw),
            }
        }
        Err(e) => DoctorCheck {
            id,
            label,
            status: CheckStatus::Warn,
            message: "Git LFS not installed (optional, needed for large files)".to_string(),
            fix_url: Some("https://git-lfs.com".to_string()),
            fix_command: None,
            path: None,
            raw_output: Some(format!("{header}\n$ git lfs version\nerror: {e}\n{search}")),
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
    let header = "# Check: Copy on Write Git Clones — verify core.clonefile is enabled for disk space savings";

    match Command::new("git")
        .args(["config", "--global", "core.clonefile"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let raw = format!(
                "{header}\n{}",
                format_command_output("git config --global core.clonefile", &output)
            );
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
                    path: None,
                    raw_output: Some(raw),
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
                    path: None,
                    raw_output: Some(raw),
                }
            }
        }
        // Key not set — treat as not enabled
        Ok(output) => {
            let raw = format!(
                "{header}\n{}",
                format_command_output("git config --global core.clonefile", &output)
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Warn,
                message: "Not set — enable to reduce disk space used by new worktrees".to_string(),
                fix_url: None,
                fix_command: Some(fix_cmd),
                path: None,
                raw_output: Some(raw),
            }
        }
        Err(e) => DoctorCheck {
            id,
            label,
            status: CheckStatus::Warn,
            message: "Not set — enable to reduce disk space used by new worktrees".to_string(),
            fix_url: None,
            fix_command: Some(fix_cmd),
            path: None,
            raw_output: Some(format!(
                "{header}\n$ git config --global core.clonefile\nerror: {e}"
            )),
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
    let header = format!(
        "# Check: {} — verify {} agent is installed",
        info.label, info.label
    );
    // Collect search output for all commands.
    let search_lines: Vec<String> = info
        .commands
        .iter()
        .map(|cmd| format_search_output(cmd))
        .collect();
    let search = search_lines.join("\n");

    // Resolve the path for the first matching command.
    let resolved_path = info
        .commands
        .iter()
        .find_map(|cmd| find_command(cmd))
        .map(|p| p.to_string_lossy().to_string());

    if resolved_path.is_some() {
        // Special handling for Goose: verify ACP subcommand is available
        if info.id == "ai-agent-goose" {
            match Command::new("goose").arg("acp").arg("--help").output() {
                Ok(output) if output.status.success() => {
                    let raw = format!(
                        "{header}\n{}\n{}",
                        format_command_output("goose acp --help", &output),
                        search
                    );
                    DoctorCheck {
                        id: info.id.to_string(),
                        label: info.label.to_string(),
                        status: CheckStatus::Pass,
                        message: "Installed".to_string(),
                        fix_url: None,
                        fix_command: None,
                        path: resolved_path,
                        raw_output: Some(raw),
                    }
                }
                Ok(output) => {
                    let raw = format!(
                        "{header}\n{}\n{}",
                        format_command_output("goose acp --help", &output),
                        search
                    );
                    DoctorCheck {
                        id: info.id.to_string(),
                        label: info.label.to_string(),
                        status: CheckStatus::Fail,
                        message: "Goose ACP subcommand not available — upgrade required"
                            .to_string(),
                        fix_url: Some("https://github.com/block/goose".to_string()),
                        fix_command: None,
                        path: resolved_path,
                        raw_output: Some(raw),
                    }
                }
                Err(e) => DoctorCheck {
                    id: info.id.to_string(),
                    label: info.label.to_string(),
                    status: CheckStatus::Fail,
                    message: "Goose ACP subcommand not available — upgrade required".to_string(),
                    fix_url: Some("https://github.com/block/goose".to_string()),
                    fix_command: None,
                    path: resolved_path,
                    raw_output: Some(format!(
                        "{header}\n$ goose acp --help\nerror: {e}\n{search}"
                    )),
                },
            }
        } else {
            DoctorCheck {
                id: info.id.to_string(),
                label: info.label.to_string(),
                status: CheckStatus::Pass,
                message: "Installed".to_string(),
                fix_url: None,
                fix_command: None,
                path: resolved_path,
                raw_output: Some(format!("{header}\n{search}")),
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
            path: None,
            raw_output: Some(format!("{header}\n{search}")),
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
