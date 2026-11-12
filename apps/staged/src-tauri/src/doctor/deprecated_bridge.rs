//! Doctor check for the deprecated `@zed-industries/claude-code-acp` bridge.
//!
//! The old bridge swallows structured provider errors, which hides rate-limit
//! failures from Staged's detection and auto-recovery.

use super::{
    execute_fix_options, AgentVersionInfo, CheckStatus, DoctorCheck, DoctorReport, FixType,
};

pub(super) const DEPRECATED_CLAUDE_ACP_CHECK_ID: &str = "ai-agent-claude-deprecated-acp";
const DEPRECATED_CLAUDE_ACP_COMMAND: &str = "claude-code-acp";
const DEPRECATED_CLAUDE_ACP_PACKAGE: &str = "@zed-industries/claude-code-acp";
const CURRENT_CLAUDE_ACP_PACKAGE: &str = "@agentclientprotocol/claude-agent-acp";
const CURRENT_CLAUDE_ACP_FIX_COMMAND: &str =
    "npm uninstall -g @zed-industries/claude-code-acp && npm install -g @agentclientprotocol/claude-agent-acp";

/// Warn when the deprecated `@zed-industries/claude-code-acp` bridge is still
/// installed.
pub(super) async fn deprecated_claude_acp_check(
    report: &DoctorReport,
    env_vars: &[(String, String)],
) -> Option<DoctorCheck> {
    if report
        .checks
        .iter()
        .any(|check| check.id == DEPRECATED_CLAUDE_ACP_CHECK_ID)
    {
        return None;
    }

    let env = doctor::DoctorEnv::new(env_vars.to_vec());
    let deprecated = tokio::task::spawn_blocking(move || {
        doctor::resolve::resolve_binary_with_env(DEPRECATED_CLAUDE_ACP_COMMAND, &env)
    })
    .await
    .ok()?;

    let current_bridge_installed = report
        .checks
        .iter()
        .any(|check| check.id == "ai-agent-claude" && check.bridge_path.is_some());
    deprecated_claude_acp_warning(&deprecated, current_bridge_installed)
}

fn deprecated_claude_acp_warning(
    deprecated: &doctor::types::ResolvedBinary,
    current_bridge_installed: bool,
) -> Option<DoctorCheck> {
    let path = deprecated.path.as_ref()?.to_string_lossy().to_string();
    let message = if current_bridge_installed {
        format!(
            "Deprecated {DEPRECATED_CLAUDE_ACP_PACKAGE} is installed; keep using {CURRENT_CLAUDE_ACP_PACKAGE} and remove the old bridge because it hides rate-limit failures"
        )
    } else {
        format!(
            "Deprecated {DEPRECATED_CLAUDE_ACP_PACKAGE} is installed; install {CURRENT_CLAUDE_ACP_PACKAGE} so rate-limit recovery can see provider errors"
        )
    };

    Some(DoctorCheck {
        id: DEPRECATED_CLAUDE_ACP_CHECK_ID.to_string(),
        label: "Deprecated Claude ACP".to_string(),
        status: CheckStatus::Warn,
        message,
        fix_url: Some(format!(
            "https://www.npmjs.com/package/{CURRENT_CLAUDE_ACP_PACKAGE}"
        )),
        fix_command: Some(CURRENT_CLAUDE_ACP_FIX_COMMAND.to_string()),
        fix_type: Some(FixType::Bridge),
        path: None,
        bridge_path: Some(path),
        raw_output: Some(format!(
            "# Check: Deprecated Claude ACP — warn when {DEPRECATED_CLAUDE_ACP_COMMAND} is installed\n{}",
            deprecated.search_output
        )),
        auth_status: None,
        login_command: None,
        installed_version: None,
        latest_version: None,
        update_available: None,
        install_source: deprecated.install_source.clone(),
        self_updating: None,
        main: None,
        bridge: deprecated
            .install_source
            .clone()
            .map(|install_source| AgentVersionInfo {
                install_source: Some(install_source),
                ..AgentVersionInfo::default()
            }),
    })
}

/// The deprecated-bridge fix carries its own command rather than a static
/// check definition, so the npm-backed gate in `run_doctor_fix` can't see it;
/// the caller ensures the managed runtime first, since this shells out to npm.
pub(super) async fn run_deprecated_claude_acp_fix(
    fix_type: FixType,
    env_vars: Vec<(String, String)>,
) -> Result<(), String> {
    if fix_type != FixType::Bridge {
        return Err(format!(
            "{fix_type:?} is not supported for {DEPRECATED_CLAUDE_ACP_CHECK_ID}"
        ));
    }

    doctor::execute_fix_with_env_options(
        DEPRECATED_CLAUDE_ACP_CHECK_ID.to_string(),
        fix_type,
        execute_fix_options(Some(CURRENT_CLAUDE_ACP_FIX_COMMAND.to_string()), env_vars),
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::doctor::InstallSource;
    use doctor::types::ResolvedBinary;

    fn resolved_deprecated() -> ResolvedBinary {
        ResolvedBinary {
            path: Some(PathBuf::from("/tmp/bin/claude-code-acp")),
            search_output: "resolve 'claude-code-acp': found".to_string(),
            install_source: Some(InstallSource::Npm),
        }
    }

    #[test]
    fn deprecated_claude_acp_warning_recommends_current_bridge() {
        let check = deprecated_claude_acp_warning(&resolved_deprecated(), false).unwrap();

        assert_eq!(check.id, DEPRECATED_CLAUDE_ACP_CHECK_ID);
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.message.contains(DEPRECATED_CLAUDE_ACP_PACKAGE));
        assert!(check.message.contains(CURRENT_CLAUDE_ACP_PACKAGE));
        assert_eq!(
            check.fix_command.as_deref(),
            Some(CURRENT_CLAUDE_ACP_FIX_COMMAND)
        );
        assert_eq!(check.fix_type, Some(FixType::Bridge));
        assert_eq!(
            check.bridge.as_ref().and_then(|b| b.install_source.clone()),
            Some(InstallSource::Npm)
        );
    }

    #[test]
    fn deprecated_claude_acp_warning_is_absent_when_binary_missing() {
        let missing = ResolvedBinary {
            path: None,
            search_output: "not found".to_string(),
            install_source: None,
        };

        assert!(deprecated_claude_acp_warning(&missing, false).is_none());
    }
}
