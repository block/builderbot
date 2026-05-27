//! AI agent checks and fix command lookup.

use std::process::Command;

use crate::checks::CLONEFILE_FIX_COMMAND;
use crate::execute_command_blocking;
use crate::resolve::format_command_output;
use crate::types::{AuthStatus, CheckStatus, DoctorCheck, FixType, ResolvedBinary};

/// Metadata for an individual AI agent check.
pub struct AgentCheckInfo {
    /// Check ID used in the doctor report, e.g. "ai-agent-goose".
    pub id: &'static str,
    /// Human-readable label, e.g. "Goose".
    pub label: &'static str,
    /// ACP bridge binary names to search for (first entry is preferred/current).
    pub commands: &'static [&'static str],
    /// Main CLI tool name (e.g. "claude"), if separate from the ACP bridge.
    pub main_command: Option<&'static str>,
    /// URL to install the main tool.
    pub install_url: Option<&'static str>,
    /// Shell command to install the main tool.
    pub install_command: Option<&'static str>,
    /// URL to install the ACP bridge, when the main tool is present but the bridge is not.
    pub bridge_install_url: Option<&'static str>,
    /// Shell command to install the ACP bridge (used as fix_command for partial installs).
    pub bridge_install_command: Option<&'static str>,
    /// Shell command that triggers an interactive authentication flow for this agent.
    pub auth_command: Option<&'static str>,
    /// Shell command that reports whether the user is currently authenticated.
    pub auth_status_command: Option<&'static str>,
}

/// All AI agents we check for individually.
pub const AI_AGENT_CHECKS: &[AgentCheckInfo] = &[
    AgentCheckInfo {
        id: "ai-agent-goose",
        label: "Goose",
        commands: &["goose"],
        main_command: None,
        install_url: Some("https://github.com/block/goose"),
        install_command: None,
        bridge_install_url: None,
        bridge_install_command: None,
        auth_command: None,
        auth_status_command: None,
    },
    AgentCheckInfo {
        id: "ai-agent-claude",
        label: "Claude Code",
        commands: &["claude-agent-acp"],
        main_command: Some("claude"),
        install_url: Some("https://code.claude.com/docs/en/overview"),
        install_command: Some("curl -fsSL https://claude.ai/install.sh | bash"),
        bridge_install_url: Some("https://github.com/zed-industries/claude-agent-acp#installation"),
        bridge_install_command: Some("npm install -g @agentclientprotocol/claude-agent-acp"),
        auth_command: Some("claude auth login"),
        auth_status_command: Some("claude auth status"),
    },
    AgentCheckInfo {
        id: "ai-agent-codex",
        label: "Codex",
        commands: &["codex-acp"],
        main_command: Some("codex"),
        install_url: Some("https://github.com/openai/codex#quickstart"),
        install_command: Some("brew install --cask codex"),
        bridge_install_url: Some("https://github.com/zed-industries/codex-acp#installation"),
        bridge_install_command: Some("npm install -g @zed-industries/codex-acp"),
        auth_command: Some("codex login"),
        auth_status_command: Some("codex login status"),
    },
    AgentCheckInfo {
        id: "ai-agent-pi",
        label: "Pi",
        commands: &["pi-acp"],
        main_command: Some("pi"),
        install_url: None,
        install_command: None,
        bridge_install_url: None,
        bridge_install_command: None,
        auth_command: None,
        auth_status_command: None,
    },
    AgentCheckInfo {
        id: "ai-agent-amp",
        label: "Amp",
        commands: &["amp-acp"],
        main_command: Some("amp"),
        install_url: Some("https://ampcode.com"),
        install_command: Some("curl -fsSL https://ampcode.com/install.sh | bash"),
        bridge_install_url: Some("https://www.npmjs.com/package/amp-acp"),
        bridge_install_command: Some("npm install -g amp-acp"),
        auth_command: Some("amp login"),
        auth_status_command: Some("amp usage"),
    },
    AgentCheckInfo {
        id: "ai-agent-copilot",
        label: "GitHub Copilot",
        commands: &["copilot"],
        main_command: None,
        install_url: Some("https://github.com/github/copilot-cli"),
        install_command: Some("npm install -g @github/copilot"),
        bridge_install_url: None,
        bridge_install_command: None,
        auth_command: Some("copilot login"),
        auth_status_command: None,
    },
    AgentCheckInfo {
        id: "ai-agent-cursor",
        label: "Cursor Agent",
        commands: &["cursor-agent"],
        main_command: None,
        install_url: Some("https://cursor.com/"),
        install_command: Some("curl -fsSL https://cursor.com/install | bash"),
        bridge_install_url: None,
        bridge_install_command: None,
        auth_command: Some("cursor-agent login"),
        auth_status_command: Some("cursor-agent status"),
    },
];

/// Check whether a single AI agent is installed.
pub fn check_single_ai_agent(
    info: &AgentCheckInfo,
    any_agent_found: bool,
    resolved_cmds: &[ResolvedBinary],
    resolved_main: Option<&ResolvedBinary>,
) -> DoctorCheck {
    let header = format!(
        "# Check: {} — verify {} agent is installed",
        info.label, info.label
    );
    let search_lines: Vec<&str> = resolved_cmds
        .iter()
        .map(|rb| rb.search_output.as_str())
        .collect();
    let search = search_lines.join("\n");

    let resolved_bridge: Option<&ResolvedBinary> =
        resolved_cmds.iter().find(|rb| rb.path.is_some());
    let resolved_path = resolved_bridge
        .and_then(|rb| rb.path.as_ref())
        .map(|p| p.to_string_lossy().to_string());
    // Prefer the bridge's install_source — that's the binary the UI cares about
    // for "how did your acp adapter get installed". For goose (no separate
    // main_command) the bridge is the main binary itself, so this is correct.
    let bridge_install_source = resolved_bridge.and_then(|rb| rb.install_source.clone());

    if let Some(ref path_str) = resolved_path {
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
                        fix_type: None,
                        path: resolved_path,
                        bridge_path: None,
                        raw_output: Some(raw),
                        auth_status: None,
                        installed_version: None,
                        latest_version: None,
                        update_available: None,
                        install_source: bridge_install_source.clone(),
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
                        fix_type: None,
                        path: resolved_path,
                        bridge_path: None,
                        raw_output: Some(raw),
                        auth_status: None,
                        installed_version: None,
                        latest_version: None,
                        update_available: None,
                        install_source: bridge_install_source.clone(),
                    }
                }
                Err(e) => DoctorCheck {
                    id: info.id.to_string(),
                    label: info.label.to_string(),
                    status: CheckStatus::Fail,
                    message: "Goose ACP subcommand not available — upgrade required".to_string(),
                    fix_url: Some("https://github.com/block/goose".to_string()),
                    fix_command: None,
                    fix_type: None,
                    path: resolved_path,
                    bridge_path: None,
                    raw_output: Some(format!(
                        "{header}\n$ goose acp --help\nerror: {e}\n{search}"
                    )),
                    auth_status: None,
                    installed_version: None,
                    latest_version: None,
                    update_available: None,
                    install_source: bridge_install_source.clone(),
                },
            }
        } else {
            let (main_path, bridge_path) = if info.main_command.is_some() {
                let main_p = resolved_main
                    .and_then(|rb| rb.path.as_ref())
                    .map(|p| p.to_string_lossy().to_string());
                (main_p, resolved_path)
            } else {
                (resolved_path, None)
            };

            let (auth_status, auth_output) = match info.auth_status_command {
                Some(cmd) => match execute_command_blocking(cmd) {
                    Ok(()) => (
                        Some(AuthStatus::Authenticated),
                        Some(format!("$ {cmd}\n(exit 0)")),
                    ),
                    Err(err) => (
                        Some(AuthStatus::NotAuthenticated),
                        Some(format!("$ {cmd}\n{err}")),
                    ),
                },
                None if info.auth_command.is_some() => (Some(AuthStatus::NotApplicable), None),
                None => (None, None),
            };

            let mut raw = format!("{header}\n{search}");
            if let Some(extra) = auth_output {
                raw.push('\n');
                raw.push_str(&extra);
            }

            let (status, message, fix_type, fix_command) =
                if matches!(auth_status, Some(AuthStatus::NotAuthenticated)) {
                    (
                        CheckStatus::Warn,
                        "Installed, not authenticated".to_string(),
                        info.auth_command.map(|_| FixType::Auth),
                        info.auth_command.map(|s| s.to_string()),
                    )
                } else {
                    (CheckStatus::Pass, "Installed".to_string(), None, None)
                };

            DoctorCheck {
                id: info.id.to_string(),
                label: info.label.to_string(),
                status,
                message,
                fix_url: None,
                fix_command,
                fix_type,
                path: main_path,
                bridge_path,
                raw_output: Some(raw),
                auth_status,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: bridge_install_source,
            }
        }
    } else {
        // Bridge binary not found. If the main binary exists, suggest installing
        // just the bridge; otherwise report the agent as not installed.
        if let Some(main_path) = resolved_main.as_ref().and_then(|rm| rm.path.as_ref()) {
            let bridge_cmd = info.commands[0];
            let main_search = &resolved_main.as_ref().unwrap().search_output;
            // Bridge wasn't found, so fall back to the main binary's
            // install_source — the only resolved path we have to describe.
            let main_install_source = resolved_main
                .as_ref()
                .and_then(|rm| rm.install_source.clone());
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
                fix_command: info.bridge_install_command.map(|s| s.to_string()),
                fix_type: info.bridge_install_command.map(|_| FixType::Bridge),
                path: Some(main_path.to_string_lossy().to_string()),
                bridge_path: None,
                raw_output: Some(format!("{header}\n{search}\n{main_search}")),
                auth_status: None,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: main_install_source,
            };
        }

        // Neither bridge nor main binary found — agent is not installed.
        let extra_search = resolved_main
            .as_ref()
            .map(|rm| format!("\n{}", rm.search_output))
            .unwrap_or_default();

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
            fix_command: info.install_command.map(|s| s.to_string()),
            fix_type: info.install_command.map(|_| FixType::Command),
            path: None,
            bridge_path: None,
            raw_output: Some(format!("{header}\n{search}{extra_search}")),
            auth_status: None,
            installed_version: None,
            latest_version: None,
            update_available: None,
            install_source: None,
        }
    }
}

/// Look up the shell command for a given check ID and fix type.
///
/// Returns `None` if the check ID is unknown or has no fix of the requested type.
pub fn lookup_fix_command(check_id: &str, fix_type: &FixType) -> Option<String> {
    // Tool checks with hardcoded fix commands
    if check_id == "git-clonefile" && *fix_type == FixType::Command {
        return Some(CLONEFILE_FIX_COMMAND.to_string());
    }

    // AI agent checks
    for info in AI_AGENT_CHECKS {
        if info.id == check_id {
            return match fix_type {
                FixType::Command => info.install_command.map(|s| s.to_string()),
                FixType::Bridge => info.bridge_install_command.map(|s| s.to_string()),
                FixType::Auth => info.auth_command.map(|s| s.to_string()),
            };
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_fix_lookup_returns_agent_auth_command() {
        assert_eq!(
            lookup_fix_command("ai-agent-claude", &FixType::Auth).as_deref(),
            Some("claude auth login"),
        );
        assert_eq!(
            lookup_fix_command("ai-agent-copilot", &FixType::Auth).as_deref(),
            Some("copilot login"),
        );
    }

    #[test]
    fn auth_fix_lookup_returns_none_for_agents_without_auth_command() {
        assert_eq!(lookup_fix_command("ai-agent-goose", &FixType::Auth), None);
    }
}
