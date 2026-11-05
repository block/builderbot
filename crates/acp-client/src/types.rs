//! Agent types, discovery, and metadata.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

/// Directory holding app-bundled ACP bridge executables, registered once at
/// app startup via [`set_bundled_tools_dir`].
static BUNDLED_TOOLS_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Register the directory holding app-bundled ACP bridge executables.
///
/// [`find_command`] consults this directory before any other resolution
/// strategy, so the pinned bridges an app ships as resources win
/// deterministically over user-installed copies. Set once at app startup;
/// later calls are ignored.
pub fn set_bundled_tools_dir(dir: PathBuf) {
    let _ = BUNDLED_TOOLS_DIR.set(dir);
}

/// Find a CLI binary by command name.
///
/// The app-bundled tools dir (if registered) wins; everything else delegates
/// to doctor so ACP and Doctor share one binary resolution policy.
pub fn find_command(cmd: &str) -> Option<PathBuf> {
    if let Some(path) = command_in_dir(BUNDLED_TOOLS_DIR.get().map(PathBuf::as_path), cmd) {
        return Some(path);
    }
    doctor::resolve::resolve_binary(cmd).path
}

/// Resolve `cmd` to an executable file inside an optional directory.
fn command_in_dir(dir: Option<&Path>, cmd: &str) -> Option<PathBuf> {
    let dir = dir?;
    doctor::resolve::resolve_executable_from_path(cmd, &dir.to_string_lossy())
}

/// What `blox acp` runs on the workstation when given no `--command`.
pub(crate) const BLOX_DEFAULT_ACP_ARGV: &[&str] = &["goose", "acp"];

/// The workstation process one Staged session's agent runs as.
///
/// `blox acp` reuses a running process whose argv equals the requested one
/// exactly (env, tags and cwd play no part), so an argv that names the Staged
/// session gives each session a process of its own, and gives the same
/// session (a resume, a reattach) that process back. A dedicated process is
/// what makes SIGINT at teardown safe (it stops this session's turn only) and
/// makes the process status (`RUNNING`/`IDLE`) this session's alone.
///
/// Forms, verified against a live workstation:
/// - Claude, Codex, Pi and Amp take a trailing `--staged-session=<id>`
///   argument they ignore. Blox keys its per-agent env defaults and the
///   `blox:agent` tag off the command (and its catalog args as a prefix), so
///   both survive. Claude runs `claude-agent-acp` directly: Blox swaps in its
///   `BLOX_FF_CLAUDE_ACP_PI` `sh -c` wrapper only for the exact argv
///   `claude-agent-acp`, and no unique argv can keep that wrapper. Running
///   the bridge directly is also what gives Staged the `_claude/sdkMessage`
///   frames, steering and `bypassPermissions` mode it relies on.
/// - Goose's CLI rejects unknown flags, so its argv is wrapped as
///   `sh -c 'exec "$@"' staged-session-<id> goose acp …`: the session id is
///   the wrapper's `$0`, and goose runs with its usual args. Blox no longer
///   recognises the process as goose, so it loses the `blox:agent` display
///   tag; goose has no catalog env to lose, and `GOOSE_MODE` is still passed
///   with `--env-vars`.
/// - With no known provider the flag is left off and Blox runs its default,
///   which every such session (and any other plain `blox acp` client)
///   shares. Teardown of a shared process must never SIGINT it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BloxAcpCommand {
    /// The argv Blox records for the process (`processCommand` followed by
    /// `processArgs`), and finds it again by.
    pub(crate) argv: Vec<String>,
    /// Whether `argv` is passed as `--command`, rather than being Blox's
    /// own default.
    pub(crate) explicit: bool,
    /// Whether the process serves this Staged session alone.
    pub(crate) dedicated: bool,
}

impl BloxAcpCommand {
    /// The `--command` value: argv joined with commas (the CLI's list
    /// separator), or `None` for Blox's default.
    pub(crate) fn flag_value(&self) -> Option<String> {
        self.explicit.then(|| self.argv.join(","))
    }
}

/// The [`BloxAcpCommand`] for `staged_session_id` running `agent_id`.
pub(crate) fn blox_acp_command(agent_id: Option<&str>, staged_session_id: &str) -> BloxAcpCommand {
    let Some(agent) = agent_id.and_then(|id| KNOWN_AGENTS.iter().find(|a| a.id == id)) else {
        return BloxAcpCommand {
            argv: BLOX_DEFAULT_ACP_ARGV
                .iter()
                .map(|s| s.to_string())
                .collect(),
            explicit: false,
            dedicated: false,
        };
    };
    // `--command` is split on commas, so keep the id to characters that
    // cannot split it (Staged's ids are UUIDs, which already are).
    let session_tag: String = staged_session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let agent_argv = std::iter::once(agent.command)
        .chain(agent.acp_args.iter().copied())
        .map(str::to_string);
    let argv = if agent.id == "goose" {
        ["sh", "-c", "exec \"$@\""]
            .into_iter()
            .map(str::to_string)
            .chain(std::iter::once(format!("staged-session-{session_tag}")))
            .chain(agent_argv)
            .collect()
    } else {
        agent_argv
            .chain(std::iter::once(format!("--staged-session={session_tag}")))
            .collect()
    };
    BloxAcpCommand {
        argv,
        explicit: true,
        dedicated: true,
    }
}

/// Whether `argv` is a [`BloxAcpCommand`] dedicated to one Staged session,
/// in either of [`blox_acp_command`]'s forms: a trailing
/// `--staged-session=<id>`, or Goose's `sh -c 'exec "$@"' staged-session-<id>`
/// wrapper. Blox's shared default names no session.
pub(crate) fn names_a_staged_session(argv: &[String]) -> bool {
    match argv {
        [sh, c, exec, tag, ..] if sh == "sh" && c == "-c" && exec == "exec \"$@\"" => {
            tag.starts_with("staged-session-")
        }
        _ => argv
            .last()
            .is_some_and(|arg| arg.starts_with("--staged-session=")),
    }
}

#[cfg(test)]
mod tests {
    use super::{blox_acp_command, command_in_dir, names_a_staged_session};
    use std::fs;
    use std::path::Path;

    fn write_executable(path: &Path) {
        fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn command_in_dir_resolves_executable() {
        let dir = std::env::temp_dir().join(format!("acp-bundled-dir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_executable(&dir.join("claude-agent-acp"));

        assert_eq!(
            command_in_dir(Some(&dir), "claude-agent-acp").as_deref(),
            Some(dir.join("claude-agent-acp").as_path()),
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_in_dir_skips_missing_and_unregistered() {
        let dir = std::env::temp_dir().join(format!("acp-bundled-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        assert_eq!(command_in_dir(Some(&dir), "claude-agent-acp"), None);
        assert_eq!(command_in_dir(None, "claude-agent-acp"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn command_in_dir_skips_non_executable_files() {
        let dir = std::env::temp_dir().join(format!("acp-bundled-nonexec-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("claude-agent-acp");
        fs::write(&file, "not executable").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(command_in_dir(Some(&dir), "claude-agent-acp"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn blox_commands_name_the_session_for_every_known_agent() {
        let claude = blox_acp_command(Some("claude"), "6f1c-aa");
        assert_eq!(
            claude.argv,
            ["claude-agent-acp", "--staged-session=6f1c-aa"]
        );
        assert_eq!(
            claude.flag_value().as_deref(),
            Some("claude-agent-acp,--staged-session=6f1c-aa")
        );
        assert!(claude.dedicated);
        for (agent, command) in [("codex", "codex-acp"), ("pi", "pi-acp"), ("amp", "amp-acp")] {
            let blox = blox_acp_command(Some(agent), "s1");
            assert_eq!(blox.argv, [command, "--staged-session=s1"], "{agent}");
            assert!(blox.dedicated, "{agent}");
        }
    }

    #[test]
    fn goose_runs_under_an_exec_wrapper_carrying_the_session() {
        let goose = blox_acp_command(Some("goose"), "s1");
        assert_eq!(
            goose.argv,
            [
                "sh",
                "-c",
                "exec \"$@\"",
                "staged-session-s1",
                "goose",
                "acp",
                "--with-builtin",
                "developer",
                "--with-builtin",
                "extensionmanager",
            ]
        );
        assert_eq!(
            goose.flag_value().as_deref(),
            Some(
                "sh,-c,exec \"$@\",staged-session-s1,goose,acp,--with-builtin,developer,\
                 --with-builtin,extensionmanager"
            )
        );
        assert!(goose.dedicated);
    }

    #[test]
    fn an_unknown_provider_shares_blox_default_process() {
        for agent in [None, Some("mystery")] {
            let blox = blox_acp_command(agent, "s1");
            assert_eq!(blox.argv, ["goose", "acp"]);
            assert_eq!(blox.flag_value(), None);
            assert!(!blox.dedicated);
        }
    }

    #[test]
    fn a_session_id_cannot_split_the_command_list() {
        let blox = blox_acp_command(Some("codex"), "a,b c");
        assert_eq!(blox.argv, ["codex-acp", "--staged-session=abc"]);
    }

    /// Every dedicated form names its session; Blox's shared default doesn't.
    #[test]
    fn dedicated_argv_names_its_staged_session() {
        for agent in [
            Some("claude"),
            Some("codex"),
            Some("goose"),
            Some("pi"),
            Some("amp"),
        ] {
            let blox = blox_acp_command(agent, "s1");
            assert_eq!(
                names_a_staged_session(&blox.argv),
                blox.dedicated,
                "{agent:?}"
            );
            assert!(blox.dedicated, "{agent:?}");
        }
        let shared = blox_acp_command(None, "s1");
        assert!(!names_a_staged_session(&shared.argv));
        assert!(!names_a_staged_session(&[]));
        let goose_unwrapped: Vec<String> = ["goose", "acp"].map(String::from).to_vec();
        assert!(!names_a_staged_session(&goose_unwrapped));
    }
}
