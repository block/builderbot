//! Tauri command wrappers for the doctor health-check system.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

pub use doctor::types::{AuthStatus, InstallSource};
pub use doctor::{
    AgentVersionInfo, CheckStatus, DoctorCheck, DoctorReport, ExecuteFixOptions, FixType,
    RunChecksOptions,
};

/// Environment snapshot for doctor checks and fixes. Shaped through
/// `apply_bundled_tools_env` so checks resolve binaries from the same PATH
/// the agent spawn path uses — a bridge Staged bundles must never be
/// reported missing (or prompt an install) just because the user has no
/// global copy.
async fn doctor_env_vars(bundled_dir: Option<&Path>) -> Vec<(String, String)> {
    let mut env_vars = crate::shell_env::home_env_vars_with_extended_path(
        crate::session_runner::shell_env_cache().as_ref(),
    )
    .await;
    if let Some(dir) = bundled_dir {
        crate::acp_tools::apply_bundled_tools_env(&mut env_vars, dir);
    }
    env_vars
}

fn run_checks_options(check_freshness: bool, env_vars: Vec<(String, String)>) -> RunChecksOptions {
    RunChecksOptions {
        check_freshness,
        offline: false,
        // Use the default public registries — Staged installs these agents
        // from public npm/brew/crates.io, not an internal mirror.
        npm_registry: None,
        env: None,
    }
    .with_env_snapshot(env_vars)
}

fn execute_fix_options(
    command_override: Option<String>,
    env_vars: Vec<(String, String)>,
) -> ExecuteFixOptions {
    ExecuteFixOptions {
        command_override,
        npm_registry: None,
        env: None,
    }
    .with_env_snapshot(env_vars)
}

/// Run all health checks and return the report.
///
/// This is the cheap, no-network path: it resolves binaries and reports
/// install/auth status but does not probe registries for version freshness.
/// The frontend calls this first for an instant paint, then follows up with
/// [`run_doctor_freshness`] to fill in version/update information.
#[tauri::command]
pub async fn run_doctor(app_handle: tauri::AppHandle) -> DoctorReport {
    run_doctor_report(&app_handle, false).await
}

/// Run all health checks with version freshness enabled.
///
/// This is the slower second pass: it probes each readout's installed version
/// and looks up the latest version from the relevant registry (npm, brew,
/// crates.io, GitHub releases), populating `installedVersion`, `latestVersion`,
/// `updateAvailable`, and the source-aware `updateCommand`/`updateFixType` on
/// each readout. Hits the network, so it must never block first paint.
#[tauri::command]
pub async fn run_doctor_freshness(app_handle: tauri::AppHandle) -> DoctorReport {
    run_doctor_report(&app_handle, true).await
}

/// Run the doctor crate's checks plus Staged-local ones (currently the
/// bundled ACP Node.js runtime check) over one shared env snapshot.
async fn run_doctor_report(app_handle: &tauri::AppHandle, check_freshness: bool) -> DoctorReport {
    let bundled_dir = crate::acp_tools::resolve_bundled_acp_tools_dir(app_handle);
    let env_vars = doctor_env_vars(bundled_dir.as_deref()).await;
    let (mut report, node_runtime) = tokio::join!(
        doctor::run_checks_with_options(run_checks_options(check_freshness, env_vars.clone())),
        run_node_runtime_check(&env_vars, bundled_dir.as_deref()),
    );
    if let Some(check) = node_runtime {
        report.checks.push(check);
    }
    mark_bundled_bridges(&mut report, bundled_dir.as_deref());
    report
}

/// Stamp `bundled` on bridge readouts whose resolved path lives inside the
/// app's bundled ACP tools dir. The doctor crate resolves those bridges via
/// the PATH prefix `apply_bundled_tools_env` injects, so a prefix match
/// against the same dir identifies them; the UI then presents them as
/// bundled instead of showing an app-internal resource path.
fn mark_bundled_bridges(report: &mut DoctorReport, bundled_dir: Option<&Path>) {
    let Some(dir) = bundled_dir else { return };
    for check in &mut report.checks {
        let in_bundled_dir = check
            .bridge_path
            .as_deref()
            .is_some_and(|p| Path::new(p).starts_with(dir));
        if in_bundled_dir {
            if let Some(bridge) = check.bridge.as_mut() {
                bridge.bundled = Some(true);
            }
        }
    }
}

/// Run a fix for a doctor check, identified by check ID and fix type.
///
/// The actual shell command is looked up from the static check definitions —
/// the caller never sends a raw command string.
#[tauri::command]
pub async fn run_doctor_fix(
    app_handle: tauri::AppHandle,
    check_id: String,
    fix_type: FixType,
) -> Result<(), String> {
    let bundled_dir = crate::acp_tools::resolve_bundled_acp_tools_dir(&app_handle);
    let env_vars = doctor_env_vars(bundled_dir.as_deref()).await;
    doctor::execute_fix_with_env_options(check_id, fix_type, execute_fix_options(None, env_vars))
        .await
}

/// Run a source-aware update for a single readout (main CLI or ACP bridge).
///
/// Unlike [`run_doctor_fix`], update commands (`UpdateMain`/`UpdateBridge`) are
/// derived per-readout at freshness time rather than living in the static check
/// table, so the executor needs the command passed in as an override.
///
/// **Trust boundary:** we do not execute the frontend-supplied `command`
/// blindly. We re-run freshness, re-derive the expected `updateCommand` for
/// `(check_id, fix_type)` backend-side, and only proceed if the two match. This
/// keeps `run_doctor_update` from becoming an arbitrary-shell-exec hole — the
/// `command` argument is effectively a confirmation of what the UI displayed,
/// validated against the authoritative backend derivation.
#[tauri::command]
pub async fn run_doctor_update(
    app_handle: tauri::AppHandle,
    check_id: String,
    fix_type: FixType,
    command: String,
) -> Result<(), String> {
    let bundled_dir = crate::acp_tools::resolve_bundled_acp_tools_dir(&app_handle);
    let env_vars = doctor_env_vars(bundled_dir.as_deref()).await;
    let expected = expected_update_command(&check_id, &fix_type, env_vars.clone()).await?;
    if expected != command {
        return Err(format!(
            "Update command mismatch for {check_id}: refusing to run a command \
             that does not match the backend-derived update command."
        ));
    }
    // Run the backend-derived `expected`, not the frontend-supplied `command`.
    // They are equal past the guard above, but executing `expected` makes the
    // command that runs provably the one the backend derived — no dependence on
    // the equality check surviving future edits.
    doctor::execute_fix_with_env_options(
        check_id,
        fix_type,
        execute_fix_options(Some(expected), env_vars),
    )
    .await
}

/// Re-run freshness and return the authoritative update command for the given
/// check + slot, or an error if no actionable update is derivable.
async fn expected_update_command(
    check_id: &str,
    fix_type: &FixType,
    env_vars: Vec<(String, String)>,
) -> Result<String, String> {
    let report = doctor::run_checks_with_options(run_checks_options(true, env_vars)).await;

    let check = report
        .checks
        .iter()
        .find(|c| c.id == check_id)
        .ok_or_else(|| format!("No such check: {check_id}"))?;

    // The fix type selects which readout's update applies.
    let readout = match fix_type {
        FixType::UpdateMain => check.main.as_ref(),
        FixType::UpdateBridge => check.bridge.as_ref(),
        _ => return Err(format!("{fix_type:?} is not an update fix type")),
    };

    readout
        .and_then(|r| r.update_command.clone())
        .ok_or_else(|| format!("No actionable update available for {check_id}"))
}

// =============================================================================
// Bundled ACP Node.js runtime check
// =============================================================================

const NODE_RUNTIME_CHECK_ID: &str = "node-runtime";
const NODE_RUNTIME_CHECK_LABEL: &str = "Node.js Runtime";
const NODE_RUNTIME_FIX_URL: &str = "https://nodejs.org/en/download";
const NODE_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// On-disk shape of `resources/acp/node-runtime.json`, written by
/// `scripts/prepare-acp-tools-resource.sh` while staging npm-sourced ACP
/// bridges. Each tool carries its own required Node major so bridges with
/// different engine ranges are checked independently.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeRuntimeManifest {
    #[serde(default)]
    tools: Vec<NodeRuntimeTool>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeRuntimeTool {
    binary: String,
    #[serde(default)]
    node_engine: Option<String>,
    required_node_major: u32,
}

impl NodeRuntimeTool {
    fn requirement_label(&self) -> String {
        self.node_engine
            .clone()
            .unwrap_or_else(|| format!(">={}", self.required_node_major))
    }
}

enum NodeRuntimeManifestState {
    /// No manifest next to the bundled tools dir: no npm-sourced bridges are
    /// bundled, so the check stays silent.
    Missing,
    Invalid {
        path: PathBuf,
        error: String,
    },
    Loaded {
        path: PathBuf,
        manifest: NodeRuntimeManifest,
    },
}

fn load_node_runtime_manifest(bundled_bin_dir: Option<&Path>) -> NodeRuntimeManifestState {
    let Some(path) = bundled_bin_dir.and_then(crate::acp_tools::node_runtime_manifest_path) else {
        return NodeRuntimeManifestState::Missing;
    };
    let contents = match std::fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return NodeRuntimeManifestState::Missing;
        }
        Err(error) => {
            return NodeRuntimeManifestState::Invalid {
                path,
                error: format!("failed to read manifest: {error}"),
            };
        }
    };
    match serde_json::from_slice::<NodeRuntimeManifest>(&contents) {
        Ok(manifest) => NodeRuntimeManifestState::Loaded { path, manifest },
        Err(error) => NodeRuntimeManifestState::Invalid {
            path,
            error: format!("failed to parse manifest JSON: {error}"),
        },
    }
}

/// Surface the bundled ACP bridges' Node.js runtime requirement at setup
/// time instead of letting the first session spawn die with a bare exit 127
/// (the bundled bridges are bash shims that exec node). Returns `None` when
/// no npm-sourced bridges are bundled; an unreadable manifest warns instead
/// of silently hiding a packaging break.
async fn run_node_runtime_check(
    env_vars: &[(String, String)],
    bundled_bin_dir: Option<&Path>,
) -> Option<DoctorCheck> {
    let (manifest_path, manifest) = match load_node_runtime_manifest(bundled_bin_dir) {
        NodeRuntimeManifestState::Missing => return None,
        NodeRuntimeManifestState::Invalid { path, error } => {
            return Some(node_runtime_doctor_check(
                CheckStatus::Warn,
                "Bundled ACP bridge Node.js manifest is unreadable; bridge runtime requirements cannot be verified".to_string(),
                Some(path.display().to_string()),
                Some(format!("error: {error}")),
            ));
        }
        NodeRuntimeManifestState::Loaded { path, manifest } => (path, manifest),
    };
    if manifest.tools.is_empty() {
        return None;
    }

    // Resolve node from the same PATH shape every other doctor check and the
    // agent spawn path use, so this check cannot disagree with what the
    // bundled wrapper shims will find at spawn time.
    let path_value = env_vars
        .iter()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| value.as_str())
        .unwrap_or_default();
    let node_path = doctor::resolve::resolve_executable_from_path("node", path_value)
        .map(|path| path.to_string_lossy().to_string());
    let node_version = match node_path.as_deref() {
        Some(path) => query_node_version(path).await,
        None => None,
    };

    Some(build_node_runtime_check(
        &manifest_path,
        &manifest.tools,
        node_path,
        node_version,
    ))
}

async fn query_node_version(node_path: &str) -> Option<String> {
    let mut command = tokio::process::Command::new(node_path);
    command
        .args(["-p", "process.versions.node"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(NODE_PROBE_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(String::from)
}

fn parse_node_major(version: &str) -> Option<u32> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

fn node_requirement_summary<'a>(tools: impl IntoIterator<Item = &'a NodeRuntimeTool>) -> String {
    tools
        .into_iter()
        .map(|tool| format!("{} needs Node.js {}", tool.binary, tool.requirement_label()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn build_node_runtime_check(
    manifest_path: &Path,
    tools: &[NodeRuntimeTool],
    node_path: Option<String>,
    node_version: Option<String>,
) -> DoctorCheck {
    let node_major = node_version.as_deref().and_then(parse_node_major);
    let unmet: Vec<&NodeRuntimeTool> = match node_major {
        Some(major) => tools
            .iter()
            .filter(|tool| major < tool.required_node_major)
            .collect(),
        None => Vec::new(),
    };

    let (status, message) = if node_path.is_none() {
        (
            CheckStatus::Warn,
            format!(
                "Node.js was not found on PATH; bundled ACP bridges require it ({})",
                node_requirement_summary(tools)
            ),
        )
    } else if node_major.is_none() {
        (
            CheckStatus::Warn,
            format!(
                "Could not determine the Node.js version; bundled ACP bridges require it ({})",
                node_requirement_summary(tools)
            ),
        )
    } else if unmet.is_empty() {
        (
            CheckStatus::Pass,
            format!(
                "Node.js {} satisfies the bundled ACP bridge requirements",
                node_version.as_deref().unwrap_or("unknown")
            ),
        )
    } else {
        (
            CheckStatus::Warn,
            format!(
                "Node.js {} is too old for bundled ACP bridges: {}",
                node_version.as_deref().unwrap_or("unknown"),
                node_requirement_summary(unmet.iter().copied())
            ),
        )
    };

    let mut detail = vec![
        "checked: bundled ACP bridge Node.js runtime requirement".to_string(),
        format!("manifest: {}", manifest_path.display()),
        format!(
            "node: {}",
            node_path.as_deref().unwrap_or("not found on PATH")
        ),
        format!("version: {}", node_version.as_deref().unwrap_or("unknown")),
        "requirements:".to_string(),
    ];
    for tool in tools {
        let verdict = match node_major {
            Some(major) if major >= tool.required_node_major => "satisfied",
            Some(_) => "unmet",
            None => "unknown",
        };
        detail.push(format!(
            "- {}: requires Node.js {} [{verdict}]",
            tool.binary,
            tool.requirement_label()
        ));
    }

    node_runtime_doctor_check(status, message, node_path, Some(detail.join("\n")))
}

fn node_runtime_doctor_check(
    status: CheckStatus,
    message: String,
    path: Option<String>,
    raw_output: Option<String>,
) -> DoctorCheck {
    DoctorCheck {
        id: NODE_RUNTIME_CHECK_ID.to_string(),
        label: NODE_RUNTIME_CHECK_LABEL.to_string(),
        status,
        message,
        // Only rendered by the frontend for non-pass statuses.
        fix_url: Some(NODE_RUNTIME_FIX_URL.to_string()),
        fix_command: None,
        fix_type: None,
        path,
        bridge_path: None,
        raw_output,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn node_tool(binary: &str, engine: &str, major: u32) -> NodeRuntimeTool {
        NodeRuntimeTool {
            binary: binary.to_string(),
            node_engine: Some(engine.to_string()),
            required_node_major: major,
        }
    }

    #[test]
    fn node_runtime_check_passes_when_all_bridges_are_satisfied() {
        let tools = [
            node_tool("claude-agent-acp", ">=22", 22),
            node_tool("codex-acp", ">=20", 20),
        ];

        let check = build_node_runtime_check(
            Path::new("/resources/acp/node-runtime.json"),
            &tools,
            Some("/usr/local/bin/node".to_string()),
            Some("22.17.0".to_string()),
        );

        assert_eq!(check.status, CheckStatus::Pass);
        assert_eq!(
            check.message,
            "Node.js 22.17.0 satisfies the bundled ACP bridge requirements"
        );
        assert_eq!(check.path.as_deref(), Some("/usr/local/bin/node"));
        let output = check.raw_output.as_deref().expect("raw output");
        assert!(output.contains("manifest: /resources/acp/node-runtime.json"));
        assert!(output.contains("- claude-agent-acp: requires Node.js >=22 [satisfied]"));
        assert!(output.contains("- codex-acp: requires Node.js >=20 [satisfied]"));
    }

    #[test]
    fn node_runtime_check_warns_only_for_bridges_with_unmet_majors() {
        // Bridges may require different Node majors; a Node 21 runtime
        // satisfies codex (>=20) but not claude (>=22).
        let tools = [
            node_tool("claude-agent-acp", ">=22", 22),
            node_tool("codex-acp", ">=20", 20),
        ];

        let check = build_node_runtime_check(
            Path::new("/resources/acp/node-runtime.json"),
            &tools,
            Some("/usr/local/bin/node".to_string()),
            Some("21.7.3".to_string()),
        );

        assert_eq!(check.status, CheckStatus::Warn);
        assert_eq!(
            check.message,
            "Node.js 21.7.3 is too old for bundled ACP bridges: claude-agent-acp needs Node.js >=22"
        );
        assert!(!check.message.contains("codex-acp"));
        let output = check.raw_output.as_deref().expect("raw output");
        assert!(output.contains("- claude-agent-acp: requires Node.js >=22 [unmet]"));
        assert!(output.contains("- codex-acp: requires Node.js >=20 [satisfied]"));
    }

    #[test]
    fn node_runtime_check_warns_when_node_is_missing() {
        let tools = [
            node_tool("claude-agent-acp", ">=22", 22),
            node_tool("codex-acp", ">=20", 20),
        ];

        let check = build_node_runtime_check(
            Path::new("/resources/acp/node-runtime.json"),
            &tools,
            None,
            None,
        );

        assert_eq!(check.status, CheckStatus::Warn);
        assert_eq!(
            check.message,
            "Node.js was not found on PATH; bundled ACP bridges require it (claude-agent-acp needs Node.js >=22, codex-acp needs Node.js >=20)"
        );
        assert!(check.path.is_none());
        assert_eq!(
            check.fix_url.as_deref(),
            Some("https://nodejs.org/en/download")
        );
    }

    #[test]
    fn node_runtime_check_warns_when_version_is_unknown() {
        let tools = [node_tool("claude-agent-acp", ">=22", 22)];

        let check = build_node_runtime_check(
            Path::new("/resources/acp/node-runtime.json"),
            &tools,
            Some("/usr/local/bin/node".to_string()),
            None,
        );

        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check
            .message
            .starts_with("Could not determine the Node.js version"));
        assert!(check
            .message
            .contains("claude-agent-acp needs Node.js >=22"));
    }

    #[test]
    fn node_runtime_manifest_loads_from_bundled_dir_parent() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap();
        std::fs::write(
            dir.path().join("node-runtime.json"),
            r#"{"tools":[{"id":"claude-acp","binary":"claude-agent-acp","nodeEngine":">=22","requiredNodeMajor":22},{"id":"codex-acp","binary":"codex-acp","nodeEngine":">=20","requiredNodeMajor":20}]}"#,
        )
        .unwrap();

        let NodeRuntimeManifestState::Loaded { path, manifest } =
            load_node_runtime_manifest(Some(&bin_dir))
        else {
            panic!("expected manifest to load");
        };

        assert_eq!(path, dir.path().join("node-runtime.json"));
        assert_eq!(manifest.tools.len(), 2);
        assert_eq!(manifest.tools[0].binary, "claude-agent-acp");
        assert_eq!(manifest.tools[0].required_node_major, 22);
        assert_eq!(manifest.tools[1].required_node_major, 20);
    }

    #[test]
    fn node_runtime_manifest_missing_or_invalid() {
        assert!(matches!(
            load_node_runtime_manifest(None),
            NodeRuntimeManifestState::Missing
        ));

        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap();
        assert!(matches!(
            load_node_runtime_manifest(Some(&bin_dir)),
            NodeRuntimeManifestState::Missing
        ));

        std::fs::write(dir.path().join("node-runtime.json"), "not json").unwrap();
        assert!(matches!(
            load_node_runtime_manifest(Some(&bin_dir)),
            NodeRuntimeManifestState::Invalid { .. }
        ));
    }

    #[tokio::test]
    async fn node_runtime_check_runs_end_to_end_from_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap();
        std::fs::write(
            dir.path().join("node-runtime.json"),
            r#"{"tools":[{"id":"claude-acp","binary":"claude-agent-acp","nodeEngine":">=0","requiredNodeMajor":0}]}"#,
        )
        .unwrap();
        let env_vars = vec![("PATH".to_string(), std::env::var("PATH").unwrap())];

        let check = run_node_runtime_check(&env_vars, Some(&bin_dir))
            .await
            .expect("check emitted for npm-bundled bridges");

        assert_eq!(check.id, "node-runtime");
        // With a zero required major any resolvable Node passes; on a host
        // without Node the check still surfaces as a warning instead of
        // vanishing.
        if check.path.is_some() {
            assert_eq!(check.status, CheckStatus::Pass);
        } else {
            assert_eq!(check.status, CheckStatus::Warn);
        }

        assert!(run_node_runtime_check(&env_vars, None).await.is_none());
    }

    #[test]
    fn parse_node_major_handles_plain_and_prefixed_versions() {
        assert_eq!(parse_node_major("22.17.0"), Some(22));
        assert_eq!(parse_node_major("v20.11.1\n"), Some(20));
        assert_eq!(parse_node_major("not-a-version"), None);
        assert_eq!(parse_node_major(""), None);
    }

    fn agent_check_with_bridge(id: &str, bridge_path: &str) -> DoctorCheck {
        let mut check =
            node_runtime_doctor_check(CheckStatus::Pass, "Installed".to_string(), None, None);
        check.id = id.to_string();
        check.bridge_path = Some(bridge_path.to_string());
        check.bridge = Some(AgentVersionInfo::default());
        check
    }

    #[test]
    fn mark_bundled_bridges_stamps_only_bridges_inside_bundled_dir() {
        let mut report = DoctorReport {
            checks: vec![
                agent_check_with_bridge(
                    "ai-agent-claude",
                    "/bundle/resources/acp/bin/claude-agent-acp",
                ),
                agent_check_with_bridge("ai-agent-pi", "/Users/me/.npm-global/bin/pi-acp"),
            ],
        };

        mark_bundled_bridges(&mut report, Some(Path::new("/bundle/resources/acp/bin")));

        assert_eq!(
            report.checks[0].bridge.as_ref().unwrap().bundled,
            Some(true),
        );
        assert_eq!(
            report.checks[1].bridge.as_ref().unwrap().bundled,
            None,
            "a user-installed bridge outside the bundled dir must not be stamped",
        );
    }

    #[test]
    fn mark_bundled_bridges_without_bundled_dir_is_a_no_op() {
        let mut report = DoctorReport {
            checks: vec![agent_check_with_bridge(
                "ai-agent-claude",
                "/bundle/resources/acp/bin/claude-agent-acp",
            )],
        };

        mark_bundled_bridges(&mut report, None);

        assert_eq!(report.checks[0].bridge.as_ref().unwrap().bundled, None);
    }
}
