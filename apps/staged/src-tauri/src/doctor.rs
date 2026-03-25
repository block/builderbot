//! Health Check ("Doctor") — backend checks for external dependencies.
//!
//! Each check probes a single external dependency and returns a status
//! (pass / warn / fail) with a human-readable summary and an optional
//! URL the user can visit to install or configure the dependency.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

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
    /// Used by the "Copy details" feature for support diagnostics.
    pub raw_output: Option<String>,
}

/// The full report returned to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

// =============================================================================
// Binary resolution
// =============================================================================

/// A resolved binary: the path (if found) and the diagnostic search trace.
#[derive(Clone)]
struct ResolvedBinary {
    path: Option<PathBuf>,
    search_output: String,
}

/// Resolve a binary by trying login shell `which` then common install paths.
///
/// Combines the work of `find_command` and diagnostic search output into a
/// single pass, so each binary only needs to be resolved once.
fn resolve_binary(cmd: &str) -> ResolvedBinary {
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
                    return ResolvedBinary {
                        path: Some(PathBuf::from(&result)),
                        search_output: lines.join("\n"),
                    };
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
        let path = PathBuf::from(dir).join(cmd);
        if path.exists() {
            lines.push(format!("    {} => found (resolved)", path.display()));
            return ResolvedBinary {
                path: Some(path),
                search_output: lines.join("\n"),
            };
        }
        lines.push(format!("    {} => not found", path.display()));
    }

    lines.push("  not found in any location".to_string());
    ResolvedBinary {
        path: None,
        search_output: lines.join("\n"),
    }
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

/// Check that `git` is installed and reachable.
fn check_git(resolved: &ResolvedBinary) -> DoctorCheck {
    let label = "Git".to_string();
    let id = "git".to_string();
    let search = &resolved.search_output;
    let header = "# Check: Git — verify git is installed and reachable";

    let git_path = match &resolved.path {
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
                raw_output: Some(format!("{header}\nnot found via resolve_binary\n{search}")),
            };
        }
    };
    let path_str = git_path.to_string_lossy().to_string();

    match Command::new(git_path).arg("--version").output() {
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
fn check_gh(resolved: &ResolvedBinary) -> DoctorCheck {
    let label = "GitHub CLI".to_string();
    let id = "gh".to_string();
    let search = &resolved.search_output;
    let header = "# Check: GitHub CLI — verify gh is installed";

    let gh_path = match &resolved.path {
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
                raw_output: Some(format!("{header}\nnot found via resolve_binary\n{search}")),
            };
        }
    };
    let path_str = gh_path.to_string_lossy().to_string();

    match Command::new(gh_path).arg("--version").output() {
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
///
/// Uses the pre-resolved `gh` path to run `gh auth status` directly,
/// avoiding a redundant binary resolution and command invocation.
fn check_gh_auth(gh: &ResolvedBinary) -> DoctorCheck {
    let label = "GitHub Auth".to_string();
    let id = "gh-auth".to_string();
    let header = "# Check: GitHub Auth — verify user is logged in to GitHub";

    let gh_path = match &gh.path {
        Some(p) => p,
        None => {
            return DoctorCheck {
                id,
                label,
                status: CheckStatus::Fail,
                message: "GitHub CLI not found — install gh first".to_string(),
                fix_url: Some("https://cli.github.com".to_string()),
                fix_command: None,
                path: None,
                raw_output: Some(format!("{header}\ngh not found via resolve_binary")),
            };
        }
    };

    match Command::new(gh_path).args(["auth", "status"]).output() {
        Ok(output) => {
            let raw = format!(
                "{header}\n{}",
                format_command_output("gh auth status", &output)
            );
            if output.status.success() {
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
                let stderr = String::from_utf8_lossy(&output.stderr);
                let hint = if stderr.contains("not logged in") || stderr.contains("no oauth token")
                {
                    "Not authenticated — run `gh auth login`".to_string()
                } else {
                    "Not authenticated".to_string()
                };
                DoctorCheck {
                    id,
                    label,
                    status: CheckStatus::Fail,
                    message: hint,
                    fix_url: Some("https://cli.github.com/manual/gh_auth_login".to_string()),
                    fix_command: None,
                    path: None,
                    raw_output: Some(raw),
                }
            }
        }
        Err(e) => DoctorCheck {
            id,
            label,
            status: CheckStatus::Fail,
            message: "Not authenticated".to_string(),
            fix_url: Some("https://cli.github.com/manual/gh_auth_login".to_string()),
            fix_command: None,
            path: None,
            raw_output: Some(format!("{header}\n$ gh auth status\nerror: {e}")),
        },
    }
}

/// Check that Git LFS is installed.
fn check_git_lfs(git: &ResolvedBinary, git_lfs: &ResolvedBinary) -> DoctorCheck {
    let label = "Git LFS".to_string();
    let id = "git-lfs".to_string();
    let search = &git_lfs.search_output;
    let header =
        "# Check: Git LFS — verify git-lfs is installed (optional, needed for large files)";

    let git_path = match &git.path {
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
                    "{header}\ngit not found via resolve_binary\n{search}"
                )),
            };
        }
    };

    match Command::new(git_path).args(["lfs", "version"]).output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let path = git_lfs
                .path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
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
fn check_clonefile(git: &ResolvedBinary) -> DoctorCheck {
    let label = "Copy on Write Git Clones".to_string();
    let id = "git-clonefile".to_string();
    let fix_cmd = "git config --global core.clonefile true".to_string();
    let header = "# Check: Copy on Write Git Clones — verify core.clonefile is enabled for disk space savings";

    let git_path = match &git.path {
        Some(p) => p,
        None => {
            return DoctorCheck {
                id,
                label,
                status: CheckStatus::Warn,
                message: "Git not found — cannot check clonefile setting".to_string(),
                fix_url: Some("https://git-scm.com/downloads".to_string()),
                fix_command: None,
                path: None,
                raw_output: Some(format!("{header}\ngit not found via resolve_binary")),
            };
        }
    };

    match Command::new(git_path)
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

/// Check whether a single AI agent is installed.
///
/// Uses pre-resolved binaries from the resolution phase. If at least one other
/// agent is already found (`any_agent_found`), missing agents get `Warn`;
/// otherwise the first missing agent gets `Warn` too since only one agent is
/// required overall.
fn check_single_ai_agent(
    info: &AgentCheckInfo,
    any_agent_found: bool,
    resolved_cmds: &[ResolvedBinary],
    resolved_main: Option<&ResolvedBinary>,
) -> DoctorCheck {
    let header = format!(
        "# Check: {} — verify {} agent is installed",
        info.label, info.label
    );
    // Collect search output from pre-resolved binaries.
    let search_lines: Vec<&str> = resolved_cmds
        .iter()
        .map(|rb| rb.search_output.as_str())
        .collect();
    let search = search_lines.join("\n");

    // Find the first resolved path among the bridge commands.
    let resolved_path = resolved_cmds
        .iter()
        .find_map(|rb| rb.path.as_ref())
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
        if let Some(resolved_main) = resolved_main {
            let main_search = &resolved_main.search_output;
            if let Some(ref main_path) = resolved_main.path {
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

// =============================================================================
// Tauri commands
// =============================================================================

/// Fallback check returned when a spawn_blocking task panics.
fn empty_check(id: &str, label: &str) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        label: label.to_string(),
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
/// Binary resolution is done once upfront for all unique command names, then
/// all checks run concurrently using the pre-resolved paths. This avoids
/// redundant login shell spawns when multiple checks need the same binary.
#[tauri::command]
pub async fn run_doctor() -> DoctorReport {
    // Phase 0: Resolve all unique binaries concurrently.
    // Collect every command name we'll need across all checks.
    let mut binary_names: Vec<&'static str> = vec!["git", "gh", "git-lfs"];
    for info in AI_AGENT_CHECKS {
        for cmd in info.commands {
            if !binary_names.contains(cmd) {
                binary_names.push(cmd);
            }
        }
        if let Some(main) = info.main_command {
            if !binary_names.contains(&main) {
                binary_names.push(main);
            }
        }
    }

    let handles: Vec<_> = binary_names
        .iter()
        .map(|&name| tokio::task::spawn_blocking(move || (name, resolve_binary(name))))
        .collect();

    let mut resolved: HashMap<&str, ResolvedBinary> = HashMap::new();
    for handle in handles {
        if let Ok((name, rb)) = handle.await {
            resolved.insert(name, rb);
        }
    }

    let fallback = ResolvedBinary {
        path: None,
        search_output: "resolution task panicked".to_string(),
    };
    let r_git = resolved
        .get("git")
        .cloned()
        .unwrap_or_else(|| fallback.clone());
    let r_gh = resolved
        .get("gh")
        .cloned()
        .unwrap_or_else(|| fallback.clone());
    let r_git_lfs = resolved
        .get("git-lfs")
        .cloned()
        .unwrap_or_else(|| fallback.clone());

    // Determine if any agent is installed from the already-resolved data.
    let any_agent_found = AI_AGENT_CHECKS.iter().any(|info| {
        info.commands
            .iter()
            .any(|cmd| resolved.get(cmd).map_or(false, |rb| rb.path.is_some()))
    });

    // Phase 1: Run all checks concurrently using pre-resolved binaries.
    let git_r = r_git.clone();
    let gh_r = r_gh.clone();
    let gh_r2 = r_gh.clone();
    let git_r2 = r_git.clone();
    let git_lfs_r = r_git_lfs;
    let git_r3 = r_git;

    let c_git = tokio::task::spawn_blocking(move || check_git(&git_r));
    let c_gh = tokio::task::spawn_blocking(move || check_gh(&gh_r));
    let c_gh_auth = tokio::task::spawn_blocking(move || check_gh_auth(&gh_r2));
    let c_git_lfs = tokio::task::spawn_blocking(move || check_git_lfs(&git_r2, &git_lfs_r));
    let c_clonefile = tokio::task::spawn_blocking(move || check_clonefile(&git_r3));

    let agent_handles: Vec<_> = AI_AGENT_CHECKS
        .iter()
        .map(|info| {
            let found = any_agent_found;
            let cmds: Vec<ResolvedBinary> = info
                .commands
                .iter()
                .map(|cmd| {
                    resolved
                        .get(cmd)
                        .cloned()
                        .unwrap_or_else(|| fallback.clone())
                })
                .collect();
            let main = info.main_command.and_then(|cmd| resolved.get(cmd).cloned());
            tokio::task::spawn_blocking(move || {
                check_single_ai_agent(info, found, &cmds, main.as_ref())
            })
        })
        .collect();

    let (c_git, c_gh, c_gh_auth, c_git_lfs, c_clonefile) =
        tokio::join!(c_git, c_gh, c_gh_auth, c_git_lfs, c_clonefile);

    let mut checks = vec![
        c_git.unwrap_or_else(|_| empty_check("git", "Git")),
        c_gh.unwrap_or_else(|_| empty_check("gh", "GitHub CLI")),
        c_gh_auth.unwrap_or_else(|_| empty_check("gh-auth", "GitHub Auth")),
        c_git_lfs.unwrap_or_else(|_| empty_check("git-lfs", "Git LFS")),
        c_clonefile.unwrap_or_else(|_| empty_check("git-clonefile", "Copy on Write Git Clones")),
    ];

    for (i, handle) in agent_handles.into_iter().enumerate() {
        let info = &AI_AGENT_CHECKS[i];
        checks.push(
            handle
                .await
                .unwrap_or_else(|_| empty_check(info.id, info.label)),
        );
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
        // Clear the inherited environment so directory-local tool managers
        // (e.g. Hermit) don't intercept the command. The login shell will
        // rebuild PATH etc. from the user's profile.
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let user = std::env::var("USER").unwrap_or_default();
        let output = Command::new(shell)
            .args(&args)
            .env_clear()
            .env("HOME", &home)
            .env("USER", &user)
            .env("TERM", "xterm-256color")
            .current_dir(&home)
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
