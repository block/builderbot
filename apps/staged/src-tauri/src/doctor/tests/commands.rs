use super::*;

/// Bundled-readout labeling lives in the doctor crate; Staged's job is
/// only to hand the managed shim dir into the run options.
#[test]
fn run_checks_options_carries_bundled_tools_dir() {
    let dir = PathBuf::from("/data/packages/bin");
    let opts = run_checks_options(false, Vec::new(), Vec::new(), Some(dir.clone()));
    assert_eq!(opts.bundled_tools_dir, Some(dir));

    let opts = run_checks_options(false, Vec::new(), Vec::new(), None);
    assert!(opts.bundled_tools_dir.is_none());
}

/// The per-agent variables ride on the snapshot as per-check overlays: an
/// agent's probes see its rows over the shared snapshot, nothing else does.
#[test]
fn run_checks_options_overlays_agent_env_on_that_agents_check() {
    let snapshot = vec![("KEY".to_string(), "from snapshot".to_string())];
    let overlays = vec![(
        "ai-agent-claude".to_string(),
        vec![("KEY".to_string(), "from doctor row".to_string())],
    )];
    let env = run_checks_options(false, snapshot, overlays, None)
        .env
        .expect("the snapshot is always set");
    assert_eq!(
        env.for_check("ai-agent-claude").get("KEY"),
        Some("from doctor row")
    );
    assert_eq!(
        env.for_check("ai-agent-codex").get("KEY"),
        Some("from snapshot")
    );
    assert_eq!(env.get("KEY"), Some("from snapshot"));
}

/// The variables are stored by provider id, the part of the check id after
/// `ai-agent-` (the frontend derives it the same way), and only agent checks
/// have one — so every agent's rows can reach its probes and no other check
/// picks any up.
#[test]
fn agent_env_is_keyed_by_the_provider_id_behind_the_check() {
    assert_eq!(provider_id_for_check("ai-agent-claude"), Some("claude"));
    assert_eq!(provider_id_for_check("gh-auth"), None);
    assert!(agent_env_for_check("gh-auth").is_empty());
    for info in doctor::agents::AI_AGENT_CHECKS {
        assert!(
            provider_id_for_check(info.id).is_some(),
            "{} has no provider id",
            info.id
        );
    }
}

/// All overlays are derived from one parsed `preferences.json`: each agent
/// check gets its own enabled rows, an agent with none (or with only
/// disabled rows) is left out, and nothing is re-read per agent.
#[test]
fn agent_env_overlays_come_from_one_parsed_preferences_value() {
    let preferences = serde_json::json!({
        "agent-env-vars": {
            "claude": [
                { "name": "ANTHROPIC_API_KEY", "value": "k", "enabled": true },
                { "name": "OFF", "value": "x", "enabled": false }
            ],
            "codex": [
                { "name": "OPENAI_API_KEY", "value": "o", "enabled": false }
            ],
            "not-an-agent": [
                { "name": "IGNORED", "value": "i", "enabled": true }
            ]
        }
    });
    let overlays = agent_env_overlays_from(&preferences);
    assert_eq!(
        overlays,
        vec![(
            "ai-agent-claude".to_string(),
            vec![("ANTHROPIC_API_KEY".to_string(), "k".to_string())]
        )]
    );
    assert!(agent_env_overlays_from(&serde_json::json!({})).is_empty());
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
        run_checks_options(false, Vec::new(), Vec::new(), None).npm_registry,
        expected
    );
    assert_eq!(execute_fix_options(None, Vec::new()).npm_registry, expected);
}
