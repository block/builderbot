//! AI agent checks and fix command lookup.

use std::path::{Path, PathBuf};

use crate::command::{
    run_command_with_timeout, CommandError, CommandTimeout, DEFAULT_PROBE_TIMEOUT,
};
use crate::environment::{apply_doctor_env, DoctorEnv};
use crate::resolve::format_command_output;
use crate::timeout_check::{command_timeout_check, TimeoutCheck};
use crate::types::{
    AgentVersionInfo, AuthStatus, CheckStatus, DoctorCheck, FixType, InstallSource, ResolvedBinary,
};
use crate::ExecOutcome;

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
    /// Per-tool override declaring this agent's install source when generic path
    /// and filesystem-fingerprint heuristics fall through to
    /// [`InstallSource::Unknown`]. Lets a curl/native-only agent (Cursor, Amp)
    /// report its true source instead of `Unknown`. Applied only on
    /// `Unknown`; a positively-detected source (Brew/Npm/…) and a missing binary
    /// (`None`) are left untouched.
    pub install_source_override: Option<InstallSource>,
    /// CLI args that print the vendored agent CLI's version through the ACP
    /// bridge's passthrough subcommand (e.g. `--cli --version` for
    /// `claude-agent-acp`). Used by the freshness pass for
    /// [`InstallSource::Bundled`] readouts, where the bridge version is pinned
    /// by the embedding app's lock and the version worth surfacing is the
    /// vendored harness CLI's (e.g. Claude Code 2.1.x).
    pub bundled_version_args: Option<&'static [&'static str]>,
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
        install_source_override: None,
        bundled_version_args: None,
    },
    // The claude-agent-acp bridge vendors the complete Claude Code CLI and
    // forwards `--cli <args>` to it, sharing the user's credential store
    // (macOS Keychain / ~/.claude). The bridge is therefore the only binary
    // this check needs — no separate `claude` main CLI is probed.
    AgentCheckInfo {
        id: "ai-agent-claude",
        label: "Claude Code",
        commands: &["claude-agent-acp"],
        main_command: None,
        install_url: Some("https://www.npmjs.com/package/@agentclientprotocol/claude-agent-acp"),
        install_command: Some("npm install -g @agentclientprotocol/claude-agent-acp"),
        bridge_install_url: None,
        bridge_install_command: None,
        auth_command: Some("claude-agent-acp --cli auth login"),
        auth_status_command: Some("claude-agent-acp --cli auth status"),
        install_source_override: None,
        bundled_version_args: Some(&["--cli", "--version"]),
    },
    // The codex-acp bridge vendors the full `codex` binary and forwards
    // `cli <args>` to it, sharing the user's ~/.codex/auth.json — same
    // single-binary shape as Claude above.
    AgentCheckInfo {
        id: "ai-agent-codex",
        label: "Codex",
        commands: &["codex-acp"],
        main_command: None,
        install_url: Some("https://www.npmjs.com/package/@agentclientprotocol/codex-acp"),
        install_command: Some("npm install -g @agentclientprotocol/codex-acp"),
        bridge_install_url: None,
        bridge_install_command: None,
        auth_command: Some("codex-acp cli login"),
        auth_status_command: Some("codex-acp cli login status"),
        install_source_override: None,
        // `-V`, not `--version`: the bridge entrypoint intercepts a literal
        // `--version` anywhere in argv and prints its own version, so only
        // codex's clap short flag reaches the vendored binary.
        bundled_version_args: Some(&["cli", "-V"]),
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
        install_source_override: None,
        bundled_version_args: None,
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
        // Main `amp` curl installer; bridge `amp-acp` is npm (detected positively).
        install_source_override: Some(InstallSource::CurlPipe),
        bundled_version_args: None,
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
        install_source_override: None,
        bundled_version_args: None,
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
        // Cursor has only a curl/native installer and no ACP bridge, so the
        // resolved binary is the curl install — the override's primary use case.
        install_source_override: Some(InstallSource::CurlPipe),
        bundled_version_args: None,
    },
];

/// Version-probe args for a bundled agent readout: the bridge-passthrough
/// invocation that prints the vendored harness CLI's version (e.g. Claude
/// Code 2.1.x instead of the bridge package's own version). `None` for
/// non-bundled sources — registry installs keep their registry-consistent
/// probes so `update_available` compares like with like — and for agents
/// without a passthrough.
pub(crate) fn bundled_version_probe_args(
    check_id: &str,
    install_source: Option<&InstallSource>,
) -> Option<&'static [&'static str]> {
    if !matches!(install_source, Some(InstallSource::Bundled)) {
        return None;
    }
    AI_AGENT_CHECKS
        .iter()
        .find(|info| info.id == check_id)
        .and_then(|info| info.bundled_version_args)
}

/// Derive a source-aware update command from a readout's install source and
/// package id. Returns `None` for self-updating sources (`CurlPipe`),
/// app-managed installs (`Bundled` — updates ship with the embedding app),
/// sources with no canonical update recipe (`Mise`/`Asdf`/`Unknown`/`System`),
/// or when the package id is unknown.
///
/// The caller is responsible for gating on `update_available == Some(true)` —
/// this function only knows how to update, not whether to. `apply_npm_registry`
/// runs over the final command string downstream, so npm commands automatically
/// pick up a registry override when one is configured.
pub fn derive_update_command(
    install_source: Option<&InstallSource>,
    package_id: Option<&str>,
) -> Option<String> {
    let pkg = package_id?;
    match install_source? {
        InstallSource::Npm => Some(format!("npm install -g {pkg}@latest")),
        InstallSource::Brew => Some(format!("brew upgrade {pkg}")),
        InstallSource::Cargo => Some(format!("cargo install --force {pkg}")),
        InstallSource::CurlPipe
        | InstallSource::Bundled
        | InstallSource::Mise
        | InstallSource::Asdf
        | InstallSource::Unknown
        | InstallSource::System => None,
    }
}

/// Append `--registry=<url>` to `command` when a registry override is supplied
/// and the command is npm-backed. Non-npm commands (curl-pipe installers, auth
/// commands, …) and the `None` registry case return the command unchanged.
///
/// The registry URL is always caller-supplied — the crate never bakes in a
/// Block-specific (or any other) registry.
pub(crate) fn apply_npm_registry(command: &str, registry: Option<&str>) -> String {
    match registry {
        Some(url) if is_npm_command(command) => format!("{command} --registry={url}"),
        _ => command.to_string(),
    }
}

/// Heuristic for whether `command` shells out to npm. Matches a leading `npm `
/// invocation as well as the `npm install` / `npm view` forms that may be
/// preceded by other tokens.
fn is_npm_command(command: &str) -> bool {
    command.starts_with("npm ") || command.contains("npm install") || command.contains("npm view")
}

/// Parent directories of every resolved binary behind a check, deduplicated.
/// Used to build a PATH prefix for the auth probe so the agent is findable
/// even when the parent process has a restricted PATH (Finder/launchd case).
/// Order: main first, then bridges, in the order they were resolved.
fn collect_resolved_bin_dirs(
    main: Option<&ResolvedBinary>,
    bridges: &[ResolvedBinary],
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: &Path| {
        if let Some(parent) = p.parent() {
            let pb = parent.to_path_buf();
            if !out.contains(&pb) {
                out.push(pb);
            }
        }
    };
    if let Some(m) = main {
        if let Some(p) = m.path.as_deref() {
            push(p);
        }
    }
    for b in bridges {
        if let Some(p) = b.path.as_deref() {
            push(p);
        }
    }
    out
}

/// Build the resolve-trace portion of an agent check's `raw_output`. When the
/// agent has a separate `main_command`, prepends the main CLI's resolve trace
/// so the happy path explains *both* binaries' resolution, not just the
/// bridge's. Single-binary agents (no `main_command`) leave only the bridge
/// trace, which is the binary that was searched for.
fn build_raw_with_main_search(
    header: &str,
    info: &AgentCheckInfo,
    resolved_main: Option<&ResolvedBinary>,
    bridge_search: &str,
) -> String {
    match (info.main_command, resolved_main) {
        (Some(main_cmd), Some(rm)) => {
            let bridge_cmd = info.commands.first().copied().unwrap_or("");
            format!(
                "{header}\n# main CLI ({main_cmd}):\n{}\n# ACP bridge ({bridge_cmd}):\n{bridge_search}",
                rm.search_output,
            )
        }
        _ => format!("{header}\n{bridge_search}"),
    }
}

/// Resolve the effective install source for an agent binary. Generic path and
/// filesystem fingerprinting (in [`crate::resolve`]) win when they identify a
/// source; only when they fall through to [`InstallSource::Unknown`] does the
/// agent's declared `override_hint` take effect. A missing binary (`None`) is
/// never fabricated into a source.
fn apply_install_source_override(
    detected: Option<InstallSource>,
    override_hint: Option<&InstallSource>,
) -> Option<InstallSource> {
    match detected {
        Some(InstallSource::Unknown) => override_hint.cloned().or(Some(InstallSource::Unknown)),
        other => other,
    }
}

/// Build a per-binary version readout from a resolved install source. Returns
/// `Some` only when the binary was resolved (i.e. a source was detected); the
/// version fields stay `None` until the freshness pass fills them in. A binary
/// that wasn't found (`None`) yields `None` — no empty readout is fabricated.
/// A `Bundled` source also stamps the readout's `bundled` flag so the UI can
/// label the binary without re-deriving the app-bundle prefix match.
fn version_readout(source: Option<InstallSource>) -> Option<AgentVersionInfo> {
    source.map(|src| AgentVersionInfo {
        bundled: (src == InstallSource::Bundled).then_some(true),
        install_source: Some(src),
        ..AgentVersionInfo::default()
    })
}

/// Check whether a single AI agent is installed.
///
/// `npm_registry`, when `Some`, routes any npm-backed install/bridge fix
/// command through that registry (see [`apply_npm_registry`]).
pub fn check_single_ai_agent(
    info: &AgentCheckInfo,
    any_agent_found: bool,
    resolved_cmds: &[ResolvedBinary],
    resolved_main: Option<&ResolvedBinary>,
    npm_registry: Option<&str>,
    env: Option<&DoctorEnv>,
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
    let bridge_install_source = apply_install_source_override(
        resolved_bridge.and_then(|rb| rb.install_source.clone()),
        info.install_source_override.as_ref(),
    );

    if let Some(ref path_str) = resolved_path {
        if info.id == "ai-agent-goose" {
            let mut command = std::process::Command::new(path_str);
            command.arg("acp").arg("--help");
            apply_doctor_env(&mut command, env);
            match run_command_with_timeout(command, "goose acp --help", DEFAULT_PROBE_TIMEOUT) {
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
                        self_updating: None,
                        // Goose has no separate ACP bridge — the single binary
                        // is the agent CLI, reported under `main`.
                        main: version_readout(bridge_install_source.clone()),
                        bridge: None,
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
                        self_updating: None,
                        // Goose has no separate ACP bridge — the single binary
                        // is the agent CLI, reported under `main`.
                        main: version_readout(bridge_install_source.clone()),
                        bridge: None,
                    }
                }
                Err(CommandError::Timeout { command, timeout }) => command_timeout_check(
                    TimeoutCheck::new(
                        info.id,
                        info.label,
                        CheckStatus::Fail,
                        &header,
                        command,
                        timeout,
                    )
                    .path(resolved_path)
                    .install_source(bridge_install_source.clone())
                    .main(version_readout(bridge_install_source.clone()))
                    .raw_suffix(Some(&search)),
                ),
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
                    self_updating: None,
                    main: version_readout(bridge_install_source.clone()),
                    bridge: None,
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

            // Surface the agent CLI and its ACP bridge as two independent
            // readouts. When the agent has a separate `main_command`, the
            // install-source override (a curl/native hint) applies to the main
            // CLI; the bridge keeps its positively-detected source. When there
            // is no separate bridge, the single resolved binary is the agent
            // CLI itself, reported under `main` (bridge stays `None`).
            let (main_readout, bridge_readout) = if info.main_command.is_some() {
                let main_src = apply_install_source_override(
                    resolved_main.and_then(|rb| rb.install_source.clone()),
                    info.install_source_override.as_ref(),
                );
                let bridge_src = resolved_bridge.and_then(|rb| rb.install_source.clone());
                (version_readout(main_src), version_readout(bridge_src))
            } else {
                (version_readout(bridge_install_source.clone()), None)
            };

            // Build a PATH prefix from the resolved binary directories so the
            // auth probe can find the agent even when the parent process was
            // launched with a restricted PATH (Finder/launchd case). The probe
            // command itself is left intact — only PATH is augmented.
            let auth_path_prefix = collect_resolved_bin_dirs(resolved_main, resolved_cmds);

            let (auth_status, auth_output) = match info.auth_status_command {
                Some(cmd) => match crate::execute_command_with_path_prefix_with_env(
                    cmd,
                    &auth_path_prefix,
                    env,
                ) {
                    ExecOutcome::Ok => (
                        Some(AuthStatus::Authenticated),
                        Some(format!("$ {cmd}\n(exit 0)")),
                    ),
                    // Shell itself couldn't be spawned — we have no signal at
                    // all about auth state.
                    ExecOutcome::Spawn(e) => (
                        Some(AuthStatus::Unknown),
                        Some(format!("$ {cmd}\n(spawn failure: {e})")),
                    ),
                    ExecOutcome::Timeout { command, timeout } => {
                        let timeout = CommandTimeout::new(info.label, command, timeout);
                        (Some(AuthStatus::Unknown), Some(timeout.raw_output()))
                    }
                    // Exit 127 means the shell couldn't find the command —
                    // PATH-shadowed or uninstalled. Not the same as "signed
                    // out"; report Unknown so the UI doesn't offer a useless
                    // `... auth login` fix.
                    ExecOutcome::Exit {
                        code: Some(127),
                        stderr,
                    } => (
                        Some(AuthStatus::Unknown),
                        Some(format!(
                            "$ {cmd}\n(command not found / exit 127)\n{}",
                            stderr.trim_end(),
                        )),
                    ),
                    // Genuine non-zero exit — the command ran and rejected us.
                    ExecOutcome::Exit { code, stderr } => (
                        Some(AuthStatus::NotAuthenticated),
                        Some(format!(
                            "$ {cmd}\n(exit {})\n{}",
                            code.map(|c| c.to_string())
                                .unwrap_or_else(|| "signal".to_string()),
                            stderr.trim_end(),
                        )),
                    ),
                },
                None if info.auth_command.is_some() => (Some(AuthStatus::NotApplicable), None),
                None => (None, None),
            };

            let mut raw = build_raw_with_main_search(&header, info, resolved_main, &search);
            if let Some(extra) = auth_output {
                raw.push('\n');
                raw.push_str(&extra);
            }

            let (status, message, fix_type, fix_command) = match auth_status {
                Some(AuthStatus::NotAuthenticated) => (
                    CheckStatus::Warn,
                    "Installed, not authenticated".to_string(),
                    info.auth_command.map(|_| FixType::Auth),
                    info.auth_command.map(|s| s.to_string()),
                ),
                // PATH-shadowed / command not found: we can't tell. Surface
                // the state but don't offer an auth fix — `claude auth login`
                // won't help if `claude` isn't on PATH.
                Some(AuthStatus::Unknown) => (
                    CheckStatus::Warn,
                    "Installed, auth status unknown".to_string(),
                    None,
                    None,
                ),
                _ => (CheckStatus::Pass, "Installed".to_string(), None, None),
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
                self_updating: None,
                main: main_readout,
                bridge: bridge_readout,
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
            let main_install_source = apply_install_source_override(
                resolved_main
                    .as_ref()
                    .and_then(|rm| rm.install_source.clone()),
                info.install_source_override.as_ref(),
            );
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
                fix_command: info
                    .bridge_install_command
                    .map(|s| apply_npm_registry(s, npm_registry)),
                fix_type: info.bridge_install_command.map(|_| FixType::Bridge),
                path: Some(main_path.to_string_lossy().to_string()),
                bridge_path: None,
                raw_output: Some(format!("{header}\n{search}\n{main_search}")),
                auth_status: None,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: main_install_source.clone(),
                self_updating: None,
                // Bridge is absent; only the agent CLI resolved.
                main: version_readout(main_install_source),
                bridge: None,
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
            fix_command: info
                .install_command
                .map(|s| apply_npm_registry(s, npm_registry)),
            fix_type: info.install_command.map(|_| FixType::Command),
            path: None,
            bridge_path: None,
            raw_output: Some(format!("{header}\n{search}{extra_search}")),
            auth_status: None,
            installed_version: None,
            latest_version: None,
            update_available: None,
            install_source: None,
            self_updating: None,
            main: None,
            bridge: None,
        }
    }
}

/// Look up the shell command for a given check ID and fix type.
///
/// Returns `None` if the check ID is unknown or has no fix of the requested type.
pub fn lookup_fix_command(check_id: &str, fix_type: &FixType) -> Option<String> {
    // AI agent checks
    for info in AI_AGENT_CHECKS {
        if info.id == check_id {
            return match fix_type {
                FixType::Command => info.install_command.map(|s| s.to_string()),
                FixType::Bridge => info.bridge_install_command.map(|s| s.to_string()),
                FixType::Auth => info.auth_command.map(|s| s.to_string()),
                // Update commands are derived per-readout from
                // `(install_source, package_id)` and supplied to the executor
                // via `command_override`; there is no static fallback.
                FixType::UpdateMain | FixType::UpdateBridge => None,
            };
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn resolved(path: Option<&str>, source: Option<InstallSource>) -> ResolvedBinary {
        ResolvedBinary {
            path: path.map(PathBuf::from),
            search_output: String::new(),
            install_source: source,
        }
    }

    fn agent(id: &str) -> &'static AgentCheckInfo {
        AI_AGENT_CHECKS.iter().find(|i| i.id == id).unwrap()
    }

    fn unique_tmp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "doctor-agent-{name}-{}-{nonce}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn write_executable(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create executable parent");
        }
        std::fs::write(path, contents).expect("write executable");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).expect("chmod executable");
        }
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    fn write_login_path_rewrite_profiles(home: &Path, path: &Path) {
        let profile = format!(
            "export PATH={}\n",
            shell_quote(&format!("{}:/usr/bin:/bin", path.to_string_lossy())),
        );
        std::fs::write(home.join(".zprofile"), &profile).expect("write zprofile");
        std::fs::write(home.join(".bash_profile"), profile).expect("write bash_profile");
    }

    /// An agent with a separate ACP bridge surfaces both binaries as independent
    /// readouts, each carrying its own install source. Uses Pi (no auth
    /// commands) so the check runs without shelling out.
    #[test]
    fn agent_check_populates_main_and_bridge_readouts() {
        let bridge = resolved(Some("/n/bin/pi-acp"), Some(InstallSource::Npm));
        let main = resolved(Some("/c/bin/pi"), Some(InstallSource::Cargo));
        let check = check_single_ai_agent(
            agent("ai-agent-pi"),
            true,
            std::slice::from_ref(&bridge),
            Some(&main),
            None,
            None,
        );

        let m = check.main.expect("main readout present");
        assert_eq!(m.install_source, Some(InstallSource::Cargo));
        let b = check.bridge.expect("bridge readout present");
        assert_eq!(b.install_source, Some(InstallSource::Npm));
        // Flat field mirrors the bridge (headline) readout for back-compat.
        assert_eq!(check.install_source, Some(InstallSource::Npm));
        // Version fields stay empty on the cheap (no-freshness) path.
        assert!(m.installed_version.is_none());
        assert!(b.installed_version.is_none());
        assert!(m.self_updating.is_none());
    }

    /// When only the agent CLI resolves (no bridge), the main readout carries
    /// the CLI's source — upgraded from `Unknown` via the per-agent override —
    /// and the bridge readout is `None`. Amp's main-only path returns before
    /// any auth probe, so this runs without shelling out.
    #[test]
    fn agent_check_main_only_applies_override_and_omits_bridge() {
        let bridge_missing = resolved(None, None);
        let main = resolved(Some("/h/.local/bin/amp"), Some(InstallSource::Unknown));
        let check = check_single_ai_agent(
            agent("ai-agent-amp"),
            true,
            std::slice::from_ref(&bridge_missing),
            Some(&main),
            None,
            None,
        );

        let m = check.main.expect("main readout present");
        assert_eq!(
            m.install_source,
            Some(InstallSource::CurlPipe),
            "Unknown main CLI upgraded to CurlPipe via override",
        );
        assert!(check.bridge.is_none(), "no bridge resolved");
        assert_eq!(check.install_source, Some(InstallSource::CurlPipe));
    }

    /// A binary resolved from the embedding app's bundled tools dir carries
    /// `InstallSource::Bundled` and the readout's `bundled` flag, so the UI can
    /// label it without re-deriving the prefix match. Uses Copilot — the same
    /// single-binary shape as claude/codex (`main_command: None`, so the
    /// readout lands under `main`) but with no auth-status probe, so the check
    /// runs without shelling out.
    #[test]
    fn agent_check_bundled_source_stamps_bundled_readout() {
        let bridge = resolved(
            Some("/Applications/App.app/resources/acp/bin/copilot"),
            Some(InstallSource::Bundled),
        );
        let check = check_single_ai_agent(
            agent("ai-agent-copilot"),
            true,
            std::slice::from_ref(&bridge),
            None,
            None,
            None,
        );

        let m = check.main.expect("main readout present");
        assert_eq!(m.install_source, Some(InstallSource::Bundled));
        assert_eq!(m.bundled, Some(true));
        assert_eq!(check.install_source, Some(InstallSource::Bundled));
    }

    /// The claude/codex checks are single-binary: the ACP bridge vendors the
    /// full harness CLI, so no separate main command is resolved and all
    /// auth/install commands go through the bridge passthrough.
    #[test]
    fn claude_and_codex_probe_through_bridge_passthrough() {
        let claude = agent("ai-agent-claude");
        assert_eq!(claude.main_command, None);
        assert_eq!(
            claude.auth_status_command,
            Some("claude-agent-acp --cli auth status"),
        );
        assert_eq!(
            claude.auth_command,
            Some("claude-agent-acp --cli auth login")
        );
        assert_eq!(
            claude.install_command,
            Some("npm install -g @agentclientprotocol/claude-agent-acp"),
        );
        assert_eq!(
            claude.bundled_version_args,
            Some(&["--cli", "--version"][..])
        );

        let codex = agent("ai-agent-codex");
        assert_eq!(codex.main_command, None);
        assert_eq!(
            codex.auth_status_command,
            Some("codex-acp cli login status")
        );
        assert_eq!(codex.auth_command, Some("codex-acp cli login"));
        assert_eq!(
            codex.install_command,
            Some("npm install -g @agentclientprotocol/codex-acp"),
        );
        // `--version` would be swallowed by the bridge's own version handler;
        // clap's `-V` passes through to the vendored codex.
        assert_eq!(codex.bundled_version_args, Some(&["cli", "-V"][..]));
    }

    #[test]
    fn bundled_version_probe_args_apply_only_to_bundled_sources() {
        assert_eq!(
            bundled_version_probe_args("ai-agent-claude", Some(&InstallSource::Bundled)),
            Some(&["--cli", "--version"][..]),
        );
        assert_eq!(
            bundled_version_probe_args("ai-agent-codex", Some(&InstallSource::Bundled)),
            Some(&["cli", "-V"][..]),
        );
        // Registry installs keep their registry-consistent probes.
        assert_eq!(
            bundled_version_probe_args("ai-agent-claude", Some(&InstallSource::Npm)),
            None,
        );
        assert_eq!(bundled_version_probe_args("ai-agent-claude", None), None);
        // Bundled agents without a passthrough fall back to the default probe.
        assert_eq!(
            bundled_version_probe_args("ai-agent-goose", Some(&InstallSource::Bundled)),
            None,
        );
    }

    /// An agent with no separate bridge reports its single binary under `main`,
    /// leaving `bridge` as `None`. Copilot has an auth command but no
    /// auth-status command, so it reports NotApplicable without shelling out.
    #[test]
    fn agent_without_bridge_reports_single_binary_as_main() {
        let single = resolved(Some("/n/bin/copilot"), Some(InstallSource::Npm));
        let check = check_single_ai_agent(
            agent("ai-agent-copilot"),
            true,
            std::slice::from_ref(&single),
            None,
            None,
            None,
        );

        let m = check.main.expect("main readout present");
        assert_eq!(m.install_source, Some(InstallSource::Npm));
        assert!(check.bridge.is_none());
        assert_eq!(check.install_source, Some(InstallSource::Npm));
    }

    #[test]
    fn agent_auth_status_uses_doctor_env_over_login_shell_path_rewrite() {
        let tmp = unique_tmp_dir("auth-env-vs-hermit");
        let snapshot_bin = tmp.join("snapshot/bin");
        let hermit_bin = tmp.join("hermit/bin");
        let bridge = snapshot_bin.join("claude-agent-acp");
        // The auth probe is the bridge passthrough (`--cli auth status`).
        write_executable(
            &bridge,
            "#!/bin/sh\n\
             test \"$DOCTOR_AGENT_ENV\" = snapshot || exit 43\n\
             test \"$1\" = --cli || exit 44\n\
             test \"$2\" = auth || exit 45\n\
             test \"$3\" = status || exit 46\n\
             exit 0\n",
        );
        write_executable(&hermit_bin.join("claude-agent-acp"), "#!/bin/sh\nexit 42\n");
        write_login_path_rewrite_profiles(&tmp, &hermit_bin);

        let bridge_resolved = ResolvedBinary {
            path: Some(bridge.clone()),
            search_output: "BRIDGE-SNAPSHOT-SEARCH".to_string(),
            install_source: Some(InstallSource::Npm),
        };
        let env = DoctorEnv::new(vec![
            ("DOCTOR_AGENT_ENV".to_string(), "snapshot".to_string()),
            (
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin", snapshot_bin.to_string_lossy()),
            ),
            ("HOME".to_string(), tmp.to_string_lossy().to_string()),
            ("USER".to_string(), "doctor-test".to_string()),
            ("ZDOTDIR".to_string(), tmp.to_string_lossy().to_string()),
        ]);

        let check = check_single_ai_agent(
            agent("ai-agent-claude"),
            true,
            std::slice::from_ref(&bridge_resolved),
            None,
            None,
            Some(&env),
        );

        assert_eq!(check.status, CheckStatus::Pass);
        assert_eq!(check.auth_status, Some(AuthStatus::Authenticated));
        // Single-binary shape: the bridge reports under `path`/`main`.
        assert_eq!(
            check.path.as_deref(),
            Some(bridge.to_string_lossy().as_ref())
        );
        assert!(check.bridge_path.is_none());
        let raw = check.raw_output.unwrap_or_default();
        assert!(
            !raw.contains(&hermit_bin.to_string_lossy().to_string()),
            "DoctorEnv-backed auth should not execute through Hermit/login PATH; raw:\n{raw}",
        );

        let _ = std::fs::remove_dir_all(tmp);
    }

    /// A fully unresolved agent carries neither readout.
    #[test]
    fn unresolved_agent_has_no_readouts() {
        let missing = resolved(None, None);
        let check = check_single_ai_agent(
            agent("ai-agent-pi"),
            false,
            std::slice::from_ref(&missing),
            Some(&missing),
            None,
            None,
        );
        assert!(check.main.is_none());
        assert!(check.bridge.is_none());
    }

    #[test]
    fn auth_fix_lookup_returns_agent_auth_command() {
        assert_eq!(
            lookup_fix_command("ai-agent-claude", &FixType::Auth).as_deref(),
            Some("claude-agent-acp --cli auth login"),
        );
        assert_eq!(
            lookup_fix_command("ai-agent-codex", &FixType::Auth).as_deref(),
            Some("codex-acp cli login"),
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

    #[test]
    fn override_applies_only_to_unknown() {
        // Unknown + declared override => override wins.
        assert_eq!(
            apply_install_source_override(
                Some(InstallSource::Unknown),
                Some(&InstallSource::CurlPipe),
            ),
            Some(InstallSource::CurlPipe),
        );
        // Unknown + no override => stays Unknown.
        assert_eq!(
            apply_install_source_override(Some(InstallSource::Unknown), None),
            Some(InstallSource::Unknown),
        );
        // A positively-detected source is never overridden.
        assert_eq!(
            apply_install_source_override(Some(InstallSource::Npm), Some(&InstallSource::CurlPipe),),
            Some(InstallSource::Npm),
        );
        // A missing binary is never fabricated into a source.
        assert_eq!(
            apply_install_source_override(None, Some(&InstallSource::CurlPipe)),
            None,
        );
    }

    #[test]
    fn curl_native_agents_declare_curl_pipe_override() {
        for id in ["ai-agent-amp", "ai-agent-cursor"] {
            let info = AI_AGENT_CHECKS.iter().find(|i| i.id == id).unwrap();
            assert_eq!(
                info.install_source_override,
                Some(InstallSource::CurlPipe),
                "{id} should declare a CurlPipe override",
            );
        }
        // Bridge-only and registry-installed agents declare no override.
        for id in ["ai-agent-claude", "ai-agent-codex"] {
            let info = AI_AGENT_CHECKS.iter().find(|i| i.id == id).unwrap();
            assert_eq!(info.install_source_override, None, "{id}");
        }
    }

    #[test]
    fn apply_npm_registry_appends_to_npm_install() {
        assert_eq!(
            apply_npm_registry("npm install -g amp-acp", Some("https://artifactory/npm")),
            "npm install -g amp-acp --registry=https://artifactory/npm",
        );
    }

    #[test]
    fn apply_npm_registry_appends_to_npm_view() {
        assert_eq!(
            apply_npm_registry("npm view amp-acp version", Some("https://artifactory/npm")),
            "npm view amp-acp version --registry=https://artifactory/npm",
        );
    }

    #[test]
    fn apply_npm_registry_leaves_non_npm_commands_unchanged() {
        let curl = "curl -fsSL https://cursor.com/install | bash";
        assert_eq!(
            apply_npm_registry(curl, Some("https://artifactory/npm")),
            curl,
        );
        assert_eq!(
            apply_npm_registry(
                "claude-agent-acp --cli auth login",
                Some("https://artifactory/npm"),
            ),
            "claude-agent-acp --cli auth login",
        );
    }

    /// On the happy path (both main and bridge resolved), `raw_output` includes
    /// the main CLI's resolve trace alongside the bridge's, with clear labels
    /// and a stable main-first ordering — diagnoses PATH-shadowed agents from
    /// the Copy details payload alone.
    #[test]
    fn raw_output_includes_main_and_bridge_resolve_traces() {
        let bridge = ResolvedBinary {
            path: Some(PathBuf::from("/n/bin/pi-acp")),
            search_output: "BRIDGE-SEARCH-MARKER".to_string(),
            install_source: Some(InstallSource::Npm),
        };
        let main = ResolvedBinary {
            path: Some(PathBuf::from("/c/bin/pi")),
            search_output: "MAIN-SEARCH-MARKER".to_string(),
            install_source: Some(InstallSource::Cargo),
        };
        let check = check_single_ai_agent(
            agent("ai-agent-pi"),
            true,
            std::slice::from_ref(&bridge),
            Some(&main),
            None,
            None,
        );
        let raw = check.raw_output.expect("raw_output set");
        let main_at = raw
            .find("MAIN-SEARCH-MARKER")
            .expect("main trace in raw_output");
        let bridge_at = raw
            .find("BRIDGE-SEARCH-MARKER")
            .expect("bridge trace in raw_output");
        assert!(
            main_at < bridge_at,
            "main should appear before bridge; raw_output:\n{raw}",
        );
        assert!(
            raw.contains("# main CLI") && raw.contains("# ACP bridge"),
            "expected labeled sections; raw_output:\n{raw}",
        );
    }

    #[test]
    fn collect_resolved_bin_dirs_dedupes_and_preserves_order() {
        let main = ResolvedBinary {
            path: Some(PathBuf::from("/tmp/a/main")),
            search_output: String::new(),
            install_source: None,
        };
        let b1 = ResolvedBinary {
            path: Some(PathBuf::from("/tmp/b/bridge1")),
            search_output: String::new(),
            install_source: None,
        };
        // Same dir as main — should dedupe.
        let b2 = ResolvedBinary {
            path: Some(PathBuf::from("/tmp/a/another")),
            search_output: String::new(),
            install_source: None,
        };
        let dirs = collect_resolved_bin_dirs(Some(&main), &[b1, b2]);
        assert_eq!(dirs, vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")],);
    }

    #[test]
    fn apply_npm_registry_none_is_passthrough() {
        assert_eq!(
            apply_npm_registry("npm install -g amp-acp", None),
            "npm install -g amp-acp",
        );
    }

    #[test]
    fn derive_update_command_npm_emits_at_latest() {
        assert_eq!(
            derive_update_command(
                Some(&InstallSource::Npm),
                Some("@agentclientprotocol/claude-agent-acp"),
            )
            .as_deref(),
            Some("npm install -g @agentclientprotocol/claude-agent-acp@latest"),
        );
    }

    #[test]
    fn derive_update_command_brew_emits_upgrade() {
        assert_eq!(
            derive_update_command(Some(&InstallSource::Brew), Some("codex")).as_deref(),
            Some("brew upgrade codex"),
        );
    }

    #[test]
    fn derive_update_command_cargo_emits_install_force() {
        assert_eq!(
            derive_update_command(Some(&InstallSource::Cargo), Some("some-crate")).as_deref(),
            Some("cargo install --force some-crate"),
        );
    }

    #[test]
    fn derive_update_command_self_updating_and_opaque_sources_return_none() {
        for src in [
            InstallSource::CurlPipe,
            InstallSource::Bundled,
            InstallSource::Mise,
            InstallSource::Asdf,
            InstallSource::Unknown,
            InstallSource::System,
        ] {
            assert_eq!(
                derive_update_command(Some(&src), Some("pkg")),
                None,
                "expected None for {src:?}",
            );
        }
    }

    #[test]
    fn derive_update_command_returns_none_without_package_id() {
        assert_eq!(derive_update_command(Some(&InstallSource::Npm), None), None,);
    }

    #[test]
    fn update_fix_lookup_returns_none_for_update_variants() {
        // Update commands are derived per-readout, not statically registered.
        assert_eq!(
            lookup_fix_command("ai-agent-claude", &FixType::UpdateMain),
            None,
        );
        assert_eq!(
            lookup_fix_command("ai-agent-claude", &FixType::UpdateBridge),
            None,
        );
    }
}
