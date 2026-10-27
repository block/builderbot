use super::*;

/// Bundled-readout labeling lives in the doctor crate; Staged's job is
/// only to hand the managed shim dir into the run options.
#[test]
fn run_checks_options_carries_bundled_tools_dir() {
    let dir = PathBuf::from("/data/packages/bin");
    let opts = run_checks_options(false, Vec::new(), Some(dir.clone()));
    assert_eq!(opts.bundled_tools_dir, Some(dir));

    let opts = run_checks_options(false, Vec::new(), None);
    assert!(opts.bundled_tools_dir.is_none());
}

/// Fixes and updates for the two bridge checks route to the managed
/// installer exactly when this build manages bridges; every other check
/// keeps the doctor crate's regular fix commands.
#[test]
fn managed_bridge_checks_route_to_the_managed_installer() {
    let managed = crate::managed_acp_tools::managed_tools_enabled();
    assert_eq!(
        managed_tool_for_check("ai-agent-claude"),
        managed.then_some("claude-acp")
    );
    assert_eq!(
        managed_tool_for_check("ai-agent-codex"),
        managed.then_some("codex-acp")
    );
    assert_eq!(managed_tool_for_check("ai-agent-copilot"), None);
    assert_eq!(managed_tool_for_check("ai-agent-amp"), None);
    assert_eq!(managed_tool_for_check(NODE_RUNTIME_CHECK_ID), None);
}

/// Both update fix types on a managed check dispatch to the managed
/// installer — `UpdateBridge` from the managed row's ACP readout, and the
/// `UpdateMain` a copy that resolved outside the managed dir carries, whose
/// own `npm install -g` would orphan a copy in the private prefix. Non-update
/// fix types take the command-validated path, and nothing is managed when
/// bridge management is off (dev override, `no-managed-acp-tools`,
/// unsupported target).
#[test]
fn both_update_fix_types_dispatch_to_the_managed_installer() {
    let managed = crate::managed_acp_tools::managed_tools_enabled();
    for fix_type in [FixType::UpdateMain, FixType::UpdateBridge] {
        assert_eq!(
            managed_update_target("ai-agent-claude", &fix_type),
            managed.then_some("claude-acp"),
            "{fix_type:?}"
        );
        assert_eq!(
            managed_update_target("ai-agent-codex", &fix_type),
            managed.then_some("codex-acp"),
            "{fix_type:?}"
        );
        // Checks whose bridge Staged never manages keep the crate's own
        // fixes for both update types.
        assert_eq!(
            managed_update_target("ai-agent-amp", &fix_type),
            None,
            "{fix_type:?}"
        );
        assert_eq!(
            managed_update_target("ai-agent-copilot", &fix_type),
            None,
            "{fix_type:?}"
        );
    }
    for fix_type in [FixType::Command, FixType::Bridge, FixType::Auth] {
        assert_eq!(
            managed_update_target("ai-agent-claude", &fix_type),
            None,
            "{fix_type:?}"
        );
    }
}

/// Checks and fixes must agree on the registry: both option builders take
/// it from the same `managed_acp_tools::npm_registry_override()` — the
/// explicit `STAGED_NPM_REGISTRY`, or `None` so npm resolves its own
/// config.
#[test]
fn doctor_options_route_npm_through_the_registry_override() {
    let expected = crate::managed_acp_tools::npm_registry_override();
    assert_eq!(
        run_checks_options(false, Vec::new(), None).npm_registry,
        expected
    );
    assert_eq!(execute_fix_options(None, Vec::new()).npm_registry, expected);
}
