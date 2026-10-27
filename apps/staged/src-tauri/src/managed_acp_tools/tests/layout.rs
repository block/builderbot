use super::*;
use crate::managed_acp_tools::test_support::*;

#[test]
fn managed_tools_disabled_reason_names_the_cause() {
    assert_eq!(
        managed_tools_disabled_reason_from_parts(None, false, true),
        None
    );
    assert_eq!(
        managed_tools_disabled_reason_from_parts(Some(Path::new("/dev/acp")), false, true)
            .as_deref(),
        Some("STAGED_ACP_TOOLS_DIR is set to /dev/acp")
    );
    assert_eq!(
        managed_tools_disabled_reason_from_parts(None, true, true).as_deref(),
        Some("this build was compiled without managed ACP tools")
    );
    assert_eq!(
        managed_tools_disabled_reason_from_parts(None, false, false).as_deref(),
        Some("no managed Node.js runtime is available for this platform")
    );
    // The override is the reason even on a build that is disabled anyway.
    assert!(
        managed_tools_disabled_reason_from_parts(Some(Path::new("/d")), true, false)
            .unwrap()
            .starts_with("STAGED_ACP_TOOLS_DIR")
    );
}

#[test]
fn path_helpers_lay_out_the_packages_tree() {
    let root = Path::new("/home/.staged/packages");
    assert_eq!(shim_bin_dir(root), root.join("bin"));
    assert_eq!(tools_root(root), root.join("tools"));
    assert_eq!(
        tool_install_dir(root, "claude-acp"),
        root.join("tools").join("claude-acp")
    );
    assert_eq!(state_path(root), root.join("state.json"));
}

#[test]
fn managed_tools_require_no_override_no_disable_and_a_supported_target() {
    assert!(managed_tools_enabled_from_parts(false, false, true));
    assert!(!managed_tools_enabled_from_parts(true, false, true));
    assert!(!managed_tools_enabled_from_parts(false, true, true));
    assert!(!managed_tools_enabled_from_parts(false, false, false));
}

#[test]
fn managed_npm_env_points_every_pair_into_the_prefix() {
    let env = managed_npm_env_at(Path::new("/data/packages/npm-prefix"));
    let expect = |key: &str, value: &str| {
        assert_eq!(
            env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()),
            Some(value),
            "{key}"
        );
    };
    expect("NPM_CONFIG_PREFIX", "/data/packages/npm-prefix");
    expect("npm_config_prefix", "/data/packages/npm-prefix");
    expect("NPM_CONFIG_CACHE", "/data/packages/npm-prefix/cache");
    expect("npm_config_cache", "/data/packages/npm-prefix/cache");
    expect("COREPACK_HOME", "/data/packages/npm-prefix/corepack");
    assert_eq!(env.len(), 5);
}

#[test]
fn apply_managed_npm_env_replaces_and_inserts() {
    let mut vars = vec![
        ("PATH".to_string(), "/usr/bin".to_string()),
        ("NPM_CONFIG_PREFIX".to_string(), "/stray/prefix".to_string()),
    ];

    apply_managed_npm_env(
        &mut vars,
        &managed_npm_env_at(Path::new("/data/npm-prefix")),
    );

    assert_eq!(vars.len(), 6);
    assert_eq!(vars[0], ("PATH".to_string(), "/usr/bin".to_string()));
    assert_eq!(
        vars[1],
        (
            "NPM_CONFIG_PREFIX".to_string(),
            "/data/npm-prefix".to_string()
        )
    );
    assert!(vars
        .iter()
        .any(|(k, v)| k == "COREPACK_HOME" && v == "/data/npm-prefix/corepack"));
}

#[test]
fn managed_prepend_dirs_orders_shims_then_prefix_then_node() {
    assert_eq!(
        managed_prepend_dirs_from_parts(
            None,
            Some(PathBuf::from("/data/packages/bin")),
            Some(PathBuf::from("/data/packages/npm-prefix/bin")),
            Some(PathBuf::from("/data/packages/node/v1/plat/bin")),
        ),
        vec![
            PathBuf::from("/data/packages/bin"),
            PathBuf::from("/data/packages/npm-prefix/bin"),
            PathBuf::from("/data/packages/node/v1/plat/bin"),
        ]
    );
    // The dev override replaces the managed shim dir and resolves first;
    // the prefix bin still resolves already-installed shims (host node
    // may run them).
    assert_eq!(
        managed_prepend_dirs_from_parts(
            Some(PathBuf::from("/dev/acp/bin")),
            None,
            Some(PathBuf::from("/data/packages/npm-prefix/bin")),
            None
        ),
        vec![
            PathBuf::from("/dev/acp/bin"),
            PathBuf::from("/data/packages/npm-prefix/bin"),
        ]
    );
}

#[test]
fn npm_backed_commands_are_detected() {
    for command in [
        "npm install -g @github/copilot",
        "npm install -g amp-acp@latest --registry=https://example.test/npm/",
        "sh -c 'npm install -g @agentclientprotocol/claude-agent-acp'",
    ] {
        assert!(is_npm_backed_command(command), "{command}");
    }
    for command in [
        "curl -fsSL https://cursor.com/install | bash",
        "brew install --cask codex",
        "claude /login",
    ] {
        assert!(!is_npm_backed_command(command), "{command}");
    }
}

#[test]
fn managed_tools_table_lists_the_two_bridges() {
    let ids: Vec<&str> = MANAGED_TOOLS.iter().map(|tool| tool.id).collect();
    assert_eq!(ids, vec!["claude-acp", "codex-acp"]);
    for tool in MANAGED_TOOLS {
        assert!(
            tool.package.starts_with("@agentclientprotocol/"),
            "{}",
            tool.package
        );
        assert!(!tool.binary.is_empty(), "{}", tool.id);
    }
}

#[test]
fn state_round_trips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = ManagedToolsState::default();
    state.tools.insert(
        "claude-acp".to_string(),
        InstalledToolPin {
            binary: "claude-agent-acp".to_string(),
            version: "1.2.3".to_string(),
            node_version: TEST_NODE_VERSION.to_string(),
        },
    );
    state.last_reconcile = Some(ReconcileRecord {
        at_ms: 42,
        ok: false,
        errors: vec!["codex-acp: boom".to_string()],
    });
    write_state(dir.path(), &state).unwrap();
    assert_eq!(read_state(dir.path()), state);

    // Missing and corrupt files read as the empty state.
    assert_eq!(
        read_state(&dir.path().join("absent")),
        ManagedToolsState::default()
    );
    std::fs::write(state_path(dir.path()), "not json").unwrap();
    assert_eq!(read_state(dir.path()), ManagedToolsState::default());
}
