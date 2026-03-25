//! Agent types, discovery, and metadata.

use serde::Serialize;
use std::path::{Path, PathBuf};

// =============================================================================
// Known agents — the registry of ACP-compatible providers
// =============================================================================

/// Static metadata for each known ACP agent.
pub(crate) struct KnownAgent {
    /// Unique identifier used in preferences and IPC.
    pub id: &'static str,
    /// Human-readable label for the UI.
    pub label: &'static str,
    /// CLI command name to search for.
    pub command: &'static str,
    /// Arguments to pass when spawning in ACP mode.
    pub acp_args: &'static [&'static str],
}

/// All agents we know how to talk to, in display order.
pub(crate) const KNOWN_AGENTS: &[KnownAgent] = &[
    KnownAgent {
        id: "goose",
        label: "Goose",
        command: "goose",
        acp_args: &[
            "acp",
            "--with-builtin",
            "developer",
            "--with-builtin",
            "extensionmanager",
        ],
    },
    KnownAgent {
        id: "claude",
        label: "Claude Code",
        command: "claude-agent-acp",
        acp_args: &[],
    },
    KnownAgent {
        id: "codex",
        label: "Codex",
        command: "codex-acp",
        acp_args: &[],
    },
    KnownAgent {
        id: "pi",
        label: "Pi",
        command: "pi-acp",
        acp_args: &[],
    },
    KnownAgent {
        id: "amp",
        label: "Amp",
        command: "amp-acp",
        acp_args: &[],
    },
];

fn find_known_agent_binary(agent: &KnownAgent) -> Option<PathBuf> {
    find_command(agent.command)
}

// =============================================================================
// Provider discovery — public API
// =============================================================================

/// Information about a discovered ACP provider.
#[derive(Debug, Clone, Serialize)]
pub struct AcpProviderInfo {
    pub id: String,
    pub label: String,
}

/// Scan the system for all known ACP agents that are installed.
///
/// Returns only agents whose CLI binary can be found. The order matches
/// `KNOWN_AGENTS` (display order). All agents are probed concurrently.
pub fn discover_providers() -> Vec<AcpProviderInfo> {
    std::thread::scope(|s| {
        let handles: Vec<_> = KNOWN_AGENTS
            .iter()
            .map(|agent| s.spawn(move || (agent, find_known_agent_binary(agent).is_some())))
            .collect();

        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter(|(_, found)| *found)
            .map(|(agent, _)| AcpProviderInfo {
                id: agent.id.to_string(),
                label: agent.label.to_string(),
            })
            .collect()
    })
}

/// Find a specific ACP agent by provider ID (e.g., "goose", "claude").
pub fn find_acp_agent_by_id(provider_id: &str) -> Option<AcpAgent> {
    KNOWN_AGENTS
        .iter()
        .find(|a| a.id == provider_id)
        .and_then(|agent| {
            find_known_agent_binary(agent).map(|path| AcpAgent {
                binary_path: path,
                acp_args: agent.acp_args.iter().map(|s| s.to_string()).collect(),
                label: agent.label.to_string(),
            })
        })
}

/// Return the CLI command names of all known agents (for error messages).
pub fn known_agent_commands() -> Vec<&'static str> {
    KNOWN_AGENTS.iter().map(|a| a.command).collect()
}

/// Find the first available ACP agent.
///
/// Tries each known agent in order and returns the first one found.
pub fn find_acp_agent() -> Option<AcpAgent> {
    for agent in KNOWN_AGENTS {
        if let Some(path) = find_known_agent_binary(agent) {
            return Some(AcpAgent {
                binary_path: path,
                acp_args: agent.acp_args.iter().map(|s| s.to_string()).collect(),
                label: agent.label.to_string(),
            });
        }
    }
    None
}

// =============================================================================
// ACP Agent
// =============================================================================

/// An ACP-compatible agent with its binary path and configuration.
#[derive(Debug, Clone)]
pub struct AcpAgent {
    pub binary_path: PathBuf,
    pub acp_args: Vec<String>,
    pub label: String,
}

impl AcpAgent {
    /// Get the path to the agent executable.
    pub fn path(&self) -> &Path {
        &self.binary_path
    }

    /// Get the agent's label.
    pub fn name(&self) -> &str {
        &self.label
    }
}

// =============================================================================
// Binary discovery
// =============================================================================

/// Common paths where CLIs might be installed (GUI apps don't inherit shell PATH).
const COMMON_PATHS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/home/linuxbrew/.linuxbrew/bin",
];

/// Find a CLI binary by command name.
///
/// Searches in order:
/// 1. Login shell `which` (picks up user's PATH from `.zshrc` / `.bashrc`)
/// 2. Common install locations
pub fn find_command(cmd: &str) -> Option<PathBuf> {
    // Strategy 1: Login shell `which`
    if let Some(path) = find_via_login_shell(cmd) {
        if path.exists() {
            return Some(path);
        }
    }

    // Strategy 2: Common paths
    for dir in COMMON_PATHS {
        let path = PathBuf::from(dir).join(cmd);
        if path.exists() {
            return Some(path);
        }
    }

    None
}

fn find_via_login_shell(cmd: &str) -> Option<PathBuf> {
    let which_cmd = format!("which {cmd}");

    for shell in &["/bin/zsh", "/bin/bash"] {
        if let Ok(output) = std::process::Command::new(shell)
            .args(["-l", "-c", &which_cmd])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(path_str) = stdout.lines().rfind(|l| !l.is_empty()) {
                    let path_str = path_str.trim();
                    if !path_str.is_empty() && path_str.starts_with('/') {
                        return Some(PathBuf::from(path_str));
                    }
                }
            }
        }
    }
    None
}

/// Map an agent ID to the `--command` value for `blox acp`.
///
/// Returns `None` if the agent uses the workspace default (no flag needed).
pub(crate) fn blox_acp_command(agent_id: &str) -> Option<String> {
    KNOWN_AGENTS.iter().find(|a| a.id == agent_id).map(|a| {
        let mut parts = vec![a.command];
        parts.extend(a.acp_args.iter().copied());
        parts.join(",")
    })
}
