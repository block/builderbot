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

    let git_path = match find_command("git") {
        Some(p) => p,
        None => {
            return DoctorCheck {
                id,
                label,
                status: CheckStatus::Fail,
                message: "Git not found".to_string(),
                fix_url: Some("https://git-scm.com/downloads".to_string()),
                fix_command: None,
                path: None,
                raw_output: Some(format!("{header}\nnot found via find_command\n{search}")),
            };
        }
    };
    let path_str = git_path.to_string_lossy().to_string();

    match Command::new(&git_path).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
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
                path: Some(path_str),
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
                path: Some(path_str),
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
            path: Some(path_str),
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

    let gh_path = match find_command("gh") {
        Some(p) => p,
        None => {
            return DoctorCheck {
                id,
                label,
                status: CheckStatus::Fail,
                message: "GitHub CLI not found".to_string(),
                fix_url: Some("https://cli.github.com".to_string()),
                fix_command: None,
                path: None,
                raw_output: Some(format!("{header}\nnot found via find_command\n{search}")),
            };
        }
    };
    let path_str = gh_path.to_string_lossy().to_string();

    match Command::new(&gh_path).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            let first_line = version.lines().next().unwrap_or("gh").trim().to_string();
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
                path: Some(path_str),
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
                path: Some(path_str),
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
            path: Some(path_str),
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
    // Capture raw gh auth status output for diagnostics, using find_command
    // to mirror the resolution strategy used at runtime.
    let raw = match find_command("gh") {
        Some(gh_path) => match Command::new(&gh_path).args(["auth", "status"]).output() {
            Ok(output) => format!(
                "{header}\n{}",
                format_command_output("gh auth status", &output)
            ),
            Err(e) => format!("{header}\n$ gh auth status\nerror: {e}"),
        },
        None => format!("{header}\ngh not found via find_command"),
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

    // Resolve git via find_command so we use the same binary the app uses at runtime.
    let git_path = match find_command("git") {
        Some(p) => p,
        None => {
            return DoctorCheck {
                id,
                label,
                status: CheckStatus::Warn,
                message: "Git LFS not installed (optional, needed for large files)".to_string(),
                fix_url: Some("https://git-lfs.com".to_string()),
                fix_command: None,
                path: None,
                raw_output: Some(format!(
                    "{header}\ngit not found via find_command\n{search}"
                )),
            };
        }
    };

    match Command::new(&git_path).args(["lfs", "version"]).output() {
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

    // Resolve git via find_command so we use the same binary the app uses at runtime.
    let git_path = find_command("git").unwrap_or_else(|| "git".into());

    match Command::new(&git_path)
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
    /// ACP bridge binary names to search for (first entry is preferred/current).
    commands: &'static [&'static str],
    /// Main CLI tool name (e.g. "claude"), if separate from the ACP bridge.
    main_command: Option<&'static str>,
    /// URL to install the main tool.
    install_url: Option<&'static str>,
    /// URL to install the ACP bridge, when the main tool is present but the bridge is not.
    bridge_install_url: Option<&'static str>,
    /// Shell command to install the ACP bridge (used as fix_command for partial installs).
    bridge_fix_command: Option<&'static str>,
}

/// All AI agents we check for individually.
/// At least one must be installed; the rest are optional.
const AI_AGENT_CHECKS: &[AgentCheckInfo] = &[
    AgentCheckInfo {
        id: "ai-agent-goose",
        label: "Goose",
        commands: &["goose"],
        main_command: None,
        install_url: Some("https://github.com/block/goose"),
        bridge_install_url: None,
        bridge_fix_command: None,
    },
    AgentCheckInfo {
        id: "ai-agent-claude",
        label: "Claude Code",
        commands: &["claude-agent-acp"],
        main_command: Some("claude"),
        install_url: Some("https://docs.anthropic.com/en/docs/claude-code/overview"),
        bridge_install_url: Some("https://github.com/anthropics/claude-agent-acp#installation"),
        bridge_fix_command: None,
    },
    AgentCheckInfo {
        id: "ai-agent-codex",
        label: "Codex",
        commands: &["codex-acp"],
        main_command: Some("codex"),
        install_url: Some("https://github.com/openai/codex#getting-started"),
        bridge_install_url: Some("https://github.com/openai/codex-acp#installation"),
        bridge_fix_command: None,
    },
    AgentCheckInfo {
        id: "ai-agent-pi",
        label: "Pi",
        commands: &["pi-acp"],
        main_command: Some("pi"),
        install_url: None,
        bridge_install_url: None,
        bridge_fix_command: None,
    },
    AgentCheckInfo {
        id: "ai-agent-amp",
        label: "Amp",
        commands: &["amp-acp"],
        main_command: Some("amp"),
        install_url: Some("https://ampcode.com"),
        bridge_install_url: None,
        bridge_fix_command: Some("npm install -g amp-acp"),
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

    if let Some(ref path_str) = resolved_path {
        // Special handling for Goose: verify ACP subcommand is available
        if info.id == "ai-agent-goose" {
            match Command::new(path_str).arg("acp").arg("--help").output() {
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
        // Bridge not found — check if the main CLI tool is installed (partial install).
        if let Some(main_cmd) = info.main_command {
            let main_search = format_search_output(main_cmd);
            if let Some(main_path) = find_command(main_cmd) {
                let bridge_cmd = info.commands[0];
                return DoctorCheck {
                    id: info.id.to_string(),
                    label: info.label.to_string(),
                    status: CheckStatus::Warn,
                    message: format!(
                        "{} is installed but {} also needs to be installed",
                        info.label, bridge_cmd
                    ),
                    fix_url: info
                        .bridge_install_url
                        .or(info.install_url)
                        .map(|s| s.to_string()),
                    fix_command: info.bridge_fix_command.map(|s| s.to_string()),
                    path: Some(main_path.to_string_lossy().to_string()),
                    raw_output: Some(format!("{header}\n{search}\n{main_search}")),
                };
            }
            // Main tool also not found — fall through to fully-missing case,
            // but include main_search in the debug output.
            return DoctorCheck {
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
                raw_output: Some(format!("{header}\n{search}\n{main_search}")),
            };
        }

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

/// Fallback check returned when a spawn_blocking task panics.
fn empty_check(id: &str) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        label: id.to_string(),
        status: CheckStatus::Fail,
        message: "Check failed to run".to_string(),
        fix_url: None,
        fix_command: None,
        path: None,
        raw_output: None,
    }
}

/// Run all health checks and return the report.
///
/// All checks run concurrently so the total wall time is roughly the
/// duration of the slowest individual check, not the sum of all of them.
#[tauri::command]
pub async fn run_doctor() -> DoctorReport {
    // Phase 1: spawn every system check and the agent-installed scan in parallel.
    let git = tokio::task::spawn_blocking(check_git);
    let gh = tokio::task::spawn_blocking(check_gh);
    let gh_auth = tokio::task::spawn_blocking(check_gh_auth);
    let git_lfs = tokio::task::spawn_blocking(check_git_lfs);
    let clonefile = tokio::task::spawn_blocking(check_clonefile);
    let any_agent = tokio::task::spawn_blocking(|| AI_AGENT_CHECKS.iter().any(agent_installed));

    let (git, gh, gh_auth, git_lfs, clonefile, any_agent) =
        tokio::join!(git, gh, gh_auth, git_lfs, clonefile, any_agent);

    let any_agent_found = any_agent.unwrap_or(false);

    // Phase 2: spawn individual agent checks in parallel (needs any_agent_found).
    let agent_handles: Vec<_> = AI_AGENT_CHECKS
        .iter()
        .map(|info| {
            let found = any_agent_found;
            tokio::task::spawn_blocking(move || check_single_ai_agent(info, found))
        })
        .collect();

    let mut checks = vec![
        git.unwrap_or_else(|_| empty_check("git")),
        gh.unwrap_or_else(|_| empty_check("gh")),
        gh_auth.unwrap_or_else(|_| empty_check("gh-auth")),
        git_lfs.unwrap_or_else(|_| empty_check("git-lfs")),
        clonefile.unwrap_or_else(|_| empty_check("clonefile")),
    ];

    for handle in agent_handles {
        if let Ok(check) = handle.await {
            checks.push(check);
        }
    }

    DoctorReport { checks }
}

/// Run a fix command from a doctor check.
///
/// Executes the given shell command and returns Ok(()) on success,
/// or an error message if the command fails.
#[tauri::command]
pub async fn run_doctor_fix(command: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        // Use a login shell so commands like `npm` installed via nvm are visible.
        let (shell, args) = if std::path::Path::new("/bin/zsh").exists() {
            ("/bin/zsh", vec!["-l", "-c", &command])
        } else {
            ("/bin/bash", vec!["-l", "-c", &command])
        };
        let output = Command::new(shell)
            .args(&args)
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
