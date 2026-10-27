use super::*;
use crate::doctor::runtime_checks::node_runtime_doctor_check;
use crate::doctor::test_support::*;
use crate::doctor::*;

#[test]
fn format_age_rounds_to_the_largest_whole_unit() {
    assert_eq!(format_age(1_000, 1_000), "just now");
    assert_eq!(format_age(0, 59_000), "just now");
    assert_eq!(format_age(0, 60_000), "1 minute ago");
    assert_eq!(format_age(0, 2 * 3_600_000 + 5), "2 hours ago");
    assert_eq!(format_age(0, 3 * 86_400_000), "3 days ago");
    // A clock that went backwards is not a panic.
    assert_eq!(format_age(5_000, 1_000), "just now");
}

fn claude_tool() -> ManagedTool {
    ManagedTool {
        id: "claude-acp",
        binary: "claude-agent-acp",
        package: "@agentclientprotocol/claude-agent-acp",
    }
}

const SHIM_BIN_DIR: &str = "/data/.staged/packages/bin";

/// The vendored Claude Code version the crate's freshness pass probes
/// through `claude-agent-acp --cli --version` — a different identity from
/// the ACP package's own version.
const VENDORED_CLI_VERSION: &str = "2.1.205";

/// A Claude row as the doctor crate reports the managed shim: bundled
/// `main`, no `bridge`. With `freshness`, the vendored CLI's probed version
/// and the crate's bundled suppression (`self_updating`, no update).
fn managed_claude_check(path: &str, freshness: bool) -> DoctorCheck {
    DoctorCheck {
        id: "ai-agent-claude".to_string(),
        label: "Claude Code".to_string(),
        status: CheckStatus::Pass,
        message: "Installed".to_string(),
        fix_url: None,
        fix_command: None,
        fix_type: None,
        path: Some(path.to_string()),
        bridge_path: None,
        raw_output: None,
        auth_status: Some(AuthStatus::Authenticated),
        login_command: Some("claude-agent-acp --cli auth login".to_string()),
        installed_version: freshness.then(|| VENDORED_CLI_VERSION.to_string()),
        latest_version: None,
        update_available: None,
        install_source: Some(InstallSource::Bundled),
        self_updating: freshness.then_some(true),
        main: Some(AgentVersionInfo {
            install_source: Some(InstallSource::Bundled),
            bundled: Some(true),
            installed_version: freshness.then(|| VENDORED_CLI_VERSION.to_string()),
            self_updating: freshness.then_some(true),
            ..AgentVersionInfo::default()
        }),
        bridge: None,
    }
}

fn managed_shim_path() -> String {
    Path::new(SHIM_BIN_DIR)
        .join("claude-agent-acp")
        .display()
        .to_string()
}

/// The ACP package version a copy the user installed themselves reports.
const USER_COPY_VERSION: &str = "0.16.2";

/// A Claude row as the doctor crate reports a copy the *user* installed —
/// found on PATH before the launch reconcile has written the managed shim,
/// or for as long as that reconcile keeps failing. Labeled by source rather
/// than bundled, with the crate's freshness fields: `latest` as the registry
/// answered it, and `command` the source-aware update the crate derived
/// (`None` for a source it cannot update, such as `Unknown`).
fn user_copy_claude_check(
    path: &str,
    source: &InstallSource,
    latest: Option<&str>,
    command: Option<&str>,
) -> DoctorCheck {
    let main = AgentVersionInfo {
        install_source: Some(source.clone()),
        bundled: None,
        installed_version: Some(USER_COPY_VERSION.to_string()),
        latest_version: latest.map(str::to_string),
        update_available: latest.map(|latest| latest != USER_COPY_VERSION),
        self_updating: Some(false),
        update_command: command.map(str::to_string),
        update_fix_type: command.map(|_| FixType::UpdateMain),
    };
    DoctorCheck {
        installed_version: main.installed_version.clone(),
        latest_version: main.latest_version.clone(),
        update_available: main.update_available,
        install_source: Some(source.clone()),
        self_updating: Some(false),
        main: Some(main),
        ..managed_claude_check(path, true)
    }
}

/// Run the report-wide pass over a single check, so the tests exercise the
/// managed-readout and redirect branches through the same dispatch the
/// report does.
fn apply_to(
    check: DoctorCheck,
    versions: &ManagedBridgeVersions,
    check_freshness: bool,
) -> DoctorCheck {
    apply_with(check, versions, check_freshness, &clean_diagnostics())
}

/// A reconcile that succeeded a minute ago and left nothing to report:
/// the managed copy is then simply missing, with no error to name.
fn clean_diagnostics() -> ManagedInstallDiagnostics {
    ManagedInstallDiagnostics {
        last_reconcile: Some(reconcile_record(true, vec![])),
        npm_registry: Some("https://registry.npmjs.org/ (per npm config)".to_string()),
        now_ms: 61_000,
    }
}

fn apply_with(
    check: DoctorCheck,
    versions: &ManagedBridgeVersions,
    check_freshness: bool,
    diagnostics: &ManagedInstallDiagnostics,
) -> DoctorCheck {
    let mut report = DoctorReport {
        checks: vec![check],
    };
    apply_managed_bridge_readouts(
        &mut report,
        std::slice::from_ref(versions),
        Path::new(SHIM_BIN_DIR),
        check_freshness,
        diagnostics,
    );
    report.checks.pop().unwrap()
}

/// Undo what [`flag_missing_managed_copy`] sets on a user-copy row, so a
/// test can assert that *nothing else* changed.
fn without_missing_copy_flag(mut check: DoctorCheck) -> DoctorCheck {
    check.status = CheckStatus::Pass;
    check.message = "Installed".to_string();
    check.fix_type = None;
    check.fix_command = None;
    check.raw_output = None;
    check
}

fn versions(installed: Option<&str>, latest: Option<&str>) -> ManagedBridgeVersions {
    ManagedBridgeVersions {
        tool: claude_tool(),
        installed: installed.map(str::to_string),
        latest: latest.map(str::to_string),
    }
}

fn json(check: &DoctorCheck) -> serde_json::Value {
    serde_json::to_value(check).unwrap()
}

/// The initial (no-network) report already carries both readouts: the
/// crate's `main`, untouched, and the ACP package's installed version
/// under `bridge`, with `bridge_path` set so the row renders it — the
/// same structure the freshness report has, so the frontend's merge (which
/// copies `bridge` but not `bridge_path`) never hides a readout.
#[test]
fn initial_report_adds_the_acp_readout_without_update_fields() {
    let path = managed_shim_path();
    let mut check = managed_claude_check(&path, false);
    let main_before = check.main.clone();

    assert!(apply_managed_bridge_readout(
        &mut check,
        &versions(Some("0.16.2"), None),
        Path::new(SHIM_BIN_DIR),
        false,
    ));

    assert_eq!(
        check.main, main_before,
        "the vendored-agent readout is preserved"
    );
    assert_eq!(check.bridge_path.as_deref(), Some(path.as_str()));
    let bridge = check.bridge.as_ref().expect("the ACP readout is added");
    assert_eq!(bridge.install_source, Some(InstallSource::Bundled));
    assert_eq!(bridge.bundled, Some(true));
    assert_eq!(bridge.installed_version.as_deref(), Some("0.16.2"));
    assert_eq!(bridge.latest_version, None);
    assert_eq!(bridge.update_available, None);
    assert_eq!(bridge.self_updating, None);
    assert_eq!(bridge.update_command, None);
    assert_eq!(bridge.update_fix_type, None);
    // The flat fields mirror the ACP readout, as the crate does for any
    // check with a bridge.
    assert_eq!(check.installed_version.as_deref(), Some("0.16.2"));
    assert_eq!(check.update_available, None);
}

/// The freshness report shows the two different versions side by side and
/// attaches exactly one update — to the ACP readout — when the registry has
/// a newer release of the same package. The text describes updating
/// Staged's managed copy; it is not an npm command.
#[test]
fn freshness_with_a_newer_release_offers_one_update_on_the_acp_readout() {
    let path = managed_shim_path();
    let mut check = managed_claude_check(&path, true);

    assert!(apply_managed_bridge_readout(
        &mut check,
        &versions(Some("0.16.2"), Some("0.17.0")),
        Path::new(SHIM_BIN_DIR),
        true,
    ));

    let main = check.main.as_ref().unwrap();
    let bridge = check.bridge.as_ref().unwrap();
    assert_eq!(
        main.installed_version.as_deref(),
        Some(VENDORED_CLI_VERSION)
    );
    assert_eq!(bridge.installed_version.as_deref(), Some("0.16.2"));
    assert_eq!(bridge.latest_version.as_deref(), Some("0.17.0"));
    assert_eq!(bridge.update_available, Some(true));
    assert_eq!(bridge.self_updating, Some(false));
    assert_eq!(bridge.update_fix_type, Some(FixType::UpdateBridge));
    let command = bridge.update_command.as_deref().expect("actionable update");
    assert!(
        command.contains("@agentclientprotocol/claude-agent-acp"),
        "{command}"
    );
    assert!(command.contains("0.17.0"), "{command}");
    assert!(command.contains("~/.staged/packages"), "{command}");
    assert!(!command.contains("npm install"), "{command}");
    // Exactly one actionable readout: the agent vendored inside the same
    // package is never offered a second install of its own.
    assert_eq!(main.update_available, None);
    assert_eq!(main.update_command, None);
    assert_eq!(main.update_fix_type, None);
    assert_eq!(main.self_updating, Some(true));
    // The flat headline follows the ACP readout.
    assert_eq!(check.installed_version.as_deref(), Some("0.16.2"));
    assert_eq!(check.latest_version.as_deref(), Some("0.17.0"));
    assert_eq!(check.update_available, Some(true));
    assert_eq!(check.self_updating, Some(false));
}

#[test]
fn freshness_with_the_current_release_reports_no_update() {
    let path = managed_shim_path();
    let mut check = managed_claude_check(&path, true);

    assert!(apply_managed_bridge_readout(
        &mut check,
        &versions(Some("0.17.0"), Some("0.17.0")),
        Path::new(SHIM_BIN_DIR),
        true,
    ));

    let bridge = check.bridge.as_ref().unwrap();
    assert_eq!(bridge.installed_version.as_deref(), Some("0.17.0"));
    assert_eq!(bridge.latest_version.as_deref(), Some("0.17.0"));
    assert_eq!(bridge.update_available, Some(false));
    assert_eq!(bridge.update_command, None);
    assert_eq!(bridge.update_fix_type, None);
    assert_eq!(check.update_available, Some(false));

    // A registry that is *behind* the install (a just-published version
    // not yet on the mirror) is likewise not an update.
    let mut check = managed_claude_check(&path, true);
    apply_managed_bridge_readout(
        &mut check,
        &versions(Some("0.17.1"), Some("0.17.0")),
        Path::new(SHIM_BIN_DIR),
        true,
    );
    assert_eq!(check.bridge.as_ref().unwrap().update_available, Some(false));
}

/// A failed or absent registry answer — and an unreadable installed
/// version — leave `update_available` unknown, never "up to date", while
/// whatever *was* read is still shown.
#[test]
fn unknown_installed_or_latest_versions_stay_unknown() {
    let path = managed_shim_path();

    let mut check = managed_claude_check(&path, true);
    assert!(apply_managed_bridge_readout(
        &mut check,
        &versions(Some("0.16.2"), None),
        Path::new(SHIM_BIN_DIR),
        true,
    ));
    let bridge = check.bridge.as_ref().unwrap();
    assert_eq!(bridge.installed_version.as_deref(), Some("0.16.2"));
    assert_eq!(bridge.latest_version, None);
    assert_eq!(bridge.update_available, None);
    assert_eq!(bridge.update_command, None);
    assert_eq!(check.update_available, None);

    let mut check = managed_claude_check(&path, true);
    assert!(apply_managed_bridge_readout(
        &mut check,
        &versions(None, Some("0.17.0")),
        Path::new(SHIM_BIN_DIR),
        true,
    ));
    let bridge = check.bridge.as_ref().unwrap();
    assert_eq!(bridge.installed_version, None);
    assert_eq!(bridge.latest_version.as_deref(), Some("0.17.0"));
    assert_eq!(bridge.update_available, None);
    assert_eq!(bridge.update_command, None);

    // Unparseable output on either side is not compared as a string.
    let mut check = managed_claude_check(&path, true);
    apply_managed_bridge_readout(
        &mut check,
        &versions(Some("0.16.2"), Some("latest")),
        Path::new(SHIM_BIN_DIR),
        true,
    );
    assert_eq!(check.bridge.as_ref().unwrap().update_available, None);
}

/// A copy the user installed themselves never gets the managed readout.
/// It is flagged — the managed copy is missing, and the managed install is
/// its fix — but nothing else changes: its own location, source and
/// version are the honest ones for what is on PATH, and with nothing to
/// update no update action is invented.
#[test]
fn a_user_copy_without_an_update_is_flagged_and_otherwise_left_alone() {
    let installed = versions(Some("0.16.2"), Some("0.17.0"));
    let path = "/opt/homebrew/bin/claude-agent-acp";

    // The registry answered, and the user copy is current.
    let check = user_copy_claude_check(path, &InstallSource::Brew, Some(USER_COPY_VERSION), None);
    let before = json(&check);
    let after = apply_to(check, &installed, true);
    assert_eq!(after.status, CheckStatus::Warn);
    assert_eq!(
        after.message,
        "Using /opt/homebrew/bin/claude-agent-acp — Staged's managed copy is missing"
    );
    assert_eq!(after.fix_type, Some(FixType::Bridge));
    assert_eq!(
        after.fix_command.as_deref(),
        Some(
            "install Staged's managed @agentclientprotocol/claude-agent-acp 0.17.0 in \
             ~/.staged/packages (the copy at /opt/homebrew/bin/claude-agent-acp is left as is)"
        )
    );
    let detail = after.raw_output.as_deref().expect("detail");
    assert!(
        detail.contains(
            "Staged-managed copy: missing (expected /data/.staged/packages/bin/claude-agent-acp)"
        ),
        "{detail}"
    );
    assert!(
        detail.contains("last managed install: succeeded 1 minute ago"),
        "{detail}"
    );
    assert!(!detail.contains("hint:"), "{detail}");
    assert_eq!(json(&without_missing_copy_flag(after)), before);

    // The registry lookup failed: unknown, so no action is invented.
    let check = user_copy_claude_check(path, &InstallSource::Brew, None, None);
    let before = json(&check);
    let after = apply_to(check, &installed, true);
    assert_eq!(after.fix_type, Some(FixType::Bridge));
    assert_eq!(json(&without_missing_copy_flag(after)), before);

    // The initial (no-network) pass sets no update fields at all, so there
    // is nothing to redirect there either — but the flag and fix do not
    // wait for the network: the base report already carries them.
    let mut check = managed_claude_check(path, false);
    check.main.as_mut().unwrap().bundled = None;
    check.main.as_mut().unwrap().install_source = Some(InstallSource::Npm);
    let before = json(&check);
    let after = apply_to(check, &installed, false);
    assert_eq!(after.status, CheckStatus::Warn);
    assert_eq!(after.fix_type, Some(FixType::Bridge));
    assert_eq!(json(&without_missing_copy_flag(after)), before);
}

/// Before the first reconcile has finished — a fresh profile whose launch
/// install is still downloading — a Homebrew copy on PATH is expected, so
/// the row must not read as a failure: it says the managed install hasn't
/// completed yet, the detail says when it runs, and the Fix stays
/// available for whoever would rather not wait. An already-warning row
/// keeps its own message in front.
#[test]
fn a_user_copy_before_the_first_reconcile_is_not_called_a_failure() {
    let installed = versions(None, Some("0.17.0"));
    let path = "/opt/homebrew/bin/claude-agent-acp";
    let diagnostics = ManagedInstallDiagnostics {
        last_reconcile: None,
        npm_registry: Some("https://registry.npmjs.org/ (per npm config)".to_string()),
        now_ms: 0,
    };

    let check = user_copy_claude_check(path, &InstallSource::Brew, Some(USER_COPY_VERSION), None);
    let after = apply_with(check, &installed, true, &diagnostics);
    assert_eq!(after.status, CheckStatus::Warn);
    assert_eq!(
        after.message,
        format!("Using {path} — Staged's managed install hasn't completed yet")
    );
    assert!(!after.message.contains("missing"), "{}", after.message);
    assert_eq!(after.fix_type, Some(FixType::Bridge));
    assert_eq!(
        after.fix_command.as_deref(),
        Some(
            "install Staged's managed @agentclientprotocol/claude-agent-acp 0.17.0 in \
             ~/.staged/packages (the copy at /opt/homebrew/bin/claude-agent-acp is left as is)"
        )
    );
    let detail = after.raw_output.as_deref().expect("detail");
    assert!(
        detail.contains("last managed install: not run yet (runs at launch and daily)"),
        "{detail}"
    );
    assert!(!detail.contains("hint:"), "{detail}");
    assert!(
        detail.contains("npm registry: https://registry.npmjs.org/ (per npm config)"),
        "{detail}"
    );

    let mut check =
        user_copy_claude_check(path, &InstallSource::Brew, Some(USER_COPY_VERSION), None);
    check.status = CheckStatus::Warn;
    check.message = "Installed, not authenticated".to_string();
    let after = apply_with(check, &installed, true, &diagnostics);
    assert_eq!(
        after.message,
        format!(
            "Installed, not authenticated; using {path} — Staged's managed install hasn't completed yet"
        )
    );
    assert_eq!(after.fix_type, Some(FixType::Bridge));
}

/// With a failed reconcile on record the row says why the managed copy is
/// missing — only the `npm error code` of the recorded error for *this*
/// tool, the rest of which (naming the registry it used) goes to the
/// detail with the classified hint and the registry Doctor detected. The
/// other tool's error stays off this row.
#[test]
fn a_user_copy_names_the_recorded_install_error_and_its_hint() {
    let installed = versions(None, None);
    let path = "/opt/homebrew/bin/claude-agent-acp";
    let claude_error = "npm install failed: npm install exited with exit status: 1 \
         (registry: npm default (per npm config)): npm error code FETCH_ERROR; \
         invalid json response body at \
         https://registry.npmjs.org/@agentclientprotocol%2fclaude-agent-acp reason: \
         Unexpected token '<'";
    let diagnostics = ManagedInstallDiagnostics {
        last_reconcile: Some(ReconcileRecord {
            at_ms: 0,
            ok: false,
            errors: vec![
                format!("claude-acp: {claude_error}"),
                "codex-acp: npm install failed: npm install exited with exit status: 1 \
                 (registry: npm default (per npm config)): npm error code E404"
                    .to_string(),
            ],
        }),
        npm_registry: Some("https://registry.npmjs.org/ (per npm config)".to_string()),
        now_ms: 5 * 60_000,
    };

    let check = user_copy_claude_check(path, &InstallSource::Brew, Some(USER_COPY_VERSION), None);
    let after = apply_with(check, &installed, true, &diagnostics);

    assert_eq!(after.status, CheckStatus::Warn);
    assert_eq!(
        after.message,
        format!("Using {path} — Staged's managed copy is missing: npm error code FETCH_ERROR")
    );
    assert!(
        !after.message.contains("invalid json"),
        "the full error stays out of the one-line message: {}",
        after.message
    );
    assert_eq!(after.fix_type, Some(FixType::Bridge));
    let detail = after.raw_output.as_deref().expect("detail");
    assert!(
        detail.contains("last managed install: failed 5 minutes ago"),
        "{detail}"
    );
    assert!(
        detail.contains(&format!("claude-acp: {claude_error}")),
        "{detail}"
    );
    assert!(!detail.contains("codex-acp"), "{detail}");
    assert!(
        detail.contains(
            "hint: this network blocks registry.npmjs.org and npm has no registry configured"
        ),
        "{detail}"
    );
    assert!(
        detail.contains("npm registry: https://registry.npmjs.org/ (per npm config)"),
        "{detail}"
    );

    // A row that already warns (installed, not signed in) keeps its own
    // message in front and its status.
    let mut check =
        user_copy_claude_check(path, &InstallSource::Brew, Some(USER_COPY_VERSION), None);
    check.status = CheckStatus::Warn;
    check.message = "Installed, not authenticated".to_string();
    check.raw_output = Some("strategy 0: PATH => /opt/homebrew/bin/claude-agent-acp".to_string());
    let after = apply_with(check, &installed, true, &diagnostics);
    assert_eq!(after.status, CheckStatus::Warn);
    assert_eq!(
        after.message,
        format!(
            "Installed, not authenticated; using {path} — Staged's managed copy is missing: npm error code FETCH_ERROR"
        )
    );
    let detail = after.raw_output.as_deref().unwrap();
    assert!(
        detail.starts_with("strategy 0: PATH => /opt/homebrew/bin/claude-agent-acp\n\nStaged-managed copy: missing"),
        "the crate's trace stays on top: {detail}"
    );
    assert!(
        detail.contains(&format!("claude-acp: {claude_error}")),
        "{detail}"
    );

    // An error with no npm code is ellipsized on the row and whole in the
    // detail; the registry line says when the probe did not run.
    let long_error = format!(
        "installed ACP bridge is incomplete: @agentclientprotocol/claude-agent-acp: {}",
        "validation probe exited with 1 and printed a very long explanation ".repeat(3)
    );
    let diagnostics = ManagedInstallDiagnostics {
        last_reconcile: Some(reconcile_record(
            false,
            vec![&format!("claude-acp: {long_error}")],
        )),
        npm_registry: None,
        now_ms: 0,
    };
    let check = user_copy_claude_check(path, &InstallSource::Brew, Some(USER_COPY_VERSION), None);
    let after = apply_with(check, &installed, true, &diagnostics);
    let expected_fragment: String = long_error.chars().take(SHORT_ERROR_CHARS).collect();
    assert_eq!(
        after.message,
        format!(
            "Using {path} — Staged's managed copy is missing: {}…",
            expected_fragment.trim_end()
        )
    );
    let detail = after.raw_output.as_deref().unwrap();
    assert!(
        detail.contains(&format!("claude-acp: {long_error}")),
        "{detail}"
    );
    assert!(detail.contains("npm registry: not probed"), "{detail}");
}

#[test]
fn short_error_fragment_prefers_the_npm_code_then_ellipsizes() {
    assert_eq!(
        short_error_fragment(
            "npm install failed: npm install exited with exit status: 1 (registry: npm default \
             (per npm config)): npm error code FETCH_ERROR; invalid json response body at x"
        ),
        "npm error code FETCH_ERROR"
    );
    assert_eq!(
        short_error_fragment(
            "npm install failed: npm default registry (registry.npmjs.org) was blocked by a \
             network gateway; fallback https://f.example.test/ also failed: npm install exited \
             with exit status: 1 (registry: https://f.example.test/): npm error code E404"
        ),
        "npm error code E404"
    );
    assert_eq!(short_error_fragment("short"), "short");
    let exact: String = "é".repeat(SHORT_ERROR_CHARS);
    assert_eq!(
        short_error_fragment(&exact),
        exact,
        "no ellipsis at the limit"
    );
    let over = format!("{exact}x");
    assert_eq!(short_error_fragment(&over), format!("{exact}…"));
    assert_eq!(
        short_error_fragment(&format!(
            "{} {}",
            "a".repeat(SHORT_ERROR_CHARS - 1),
            "b".repeat(5)
        )),
        format!("{}…", "a".repeat(SHORT_ERROR_CHARS - 1)),
        "trailing whitespace before the cut is dropped"
    );
}

/// The registry probe is decided from the shim dir alone: any managed
/// shim absent means a Claude/Codex row may resolve elsewhere.
#[test]
fn registry_probe_runs_only_while_a_managed_shim_is_absent() {
    let tools = crate::managed_acp_tools::managed_tools();
    if tools.is_empty() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    assert!(any_managed_shim_missing(dir.path()));
    for tool in &tools {
        std::fs::write(dir.path().join(tool.binary), "#!/bin/sh\n").unwrap();
    }
    assert!(!any_managed_shim_missing(dir.path()));
    std::fs::remove_file(dir.path().join(tools[0].binary)).unwrap();
    assert!(any_managed_shim_missing(dir.path()));
}

/// When this build does not manage bridges at all, a Homebrew copy is by
/// design: the rows say so and keep their status, and no fix is added.
/// Rows for other agents are untouched.
#[test]
fn managed_installs_disabled_is_noted_on_the_bridge_rows_without_a_downgrade() {
    let path = "/opt/homebrew/bin/claude-agent-acp";
    let mut claude =
        user_copy_claude_check(path, &InstallSource::Brew, Some(USER_COPY_VERSION), None);
    claude.raw_output = Some("strategy 0: PATH => /opt/homebrew/bin/claude-agent-acp".to_string());
    let mut codex = user_copy_claude_check(
        "/opt/homebrew/bin/codex-acp",
        &InstallSource::Brew,
        None,
        None,
    );
    codex.id = "ai-agent-codex".to_string();
    codex.status = CheckStatus::Fail;
    codex.message = "Not installed (optional)".to_string();
    codex.path = None;
    let amp = DoctorCheck {
        id: "ai-agent-amp".to_string(),
        ..claude.clone()
    };
    let amp_before = json(&amp);
    let mut report = DoctorReport {
        checks: vec![claude, codex, amp],
    };

    note_managed_installs_disabled(&mut report, "STAGED_ACP_TOOLS_DIR is set to /dev/acp");

    let claude = &report.checks[0];
    assert_eq!(claude.status, CheckStatus::Pass);
    assert_eq!(
        claude.message,
        "Installed (managed installs disabled: STAGED_ACP_TOOLS_DIR is set to /dev/acp)"
    );
    assert!(claude.fix_type.is_none());
    assert_eq!(
        claude.raw_output.as_deref(),
        Some(
            "strategy 0: PATH => /opt/homebrew/bin/claude-agent-acp\n\n\
             managed installs disabled: STAGED_ACP_TOOLS_DIR is set to /dev/acp"
        )
    );
    let codex = &report.checks[1];
    assert_eq!(codex.status, CheckStatus::Fail);
    assert_eq!(
        codex.message,
        "Not installed (optional) (managed installs disabled: STAGED_ACP_TOOLS_DIR is set to /dev/acp)"
    );
    assert_eq!(json(&report.checks[2]), amp_before);
}

/// A user copy with a newer release keeps its readout but has its action
/// redirected: on a managing build the managed installer is the only writer
/// for these two packages, so the crate's `npm install -g` (which would
/// orphan a copy in the private prefix) and `brew upgrade` (which Staged
/// does not run for a copy it never uses) are replaced by a description of
/// the managed install. A source the crate cannot update gets the action it
/// had none for.
///
/// The version the description names is the managed probe's — what the
/// installer will actually fetch from the configured registry — not the
/// crate's per-source answer (`brew info` for a Homebrew copy), which the
/// readout itself keeps showing.
#[test]
fn a_user_copy_with_a_newer_release_is_redirected_to_the_managed_install() {
    // The configured registry is one release ahead of what the crate's
    // source-aware lookup found for the user's copy.
    let installed = versions(Some("0.16.2"), Some("0.17.1"));
    let cases = [
        (
            "/Users/u/.nvm/versions/node/v22.14.0/bin/claude-agent-acp",
            InstallSource::Npm,
            Some("npm install -g @agentclientprotocol/claude-agent-acp@latest"),
        ),
        (
            "/opt/homebrew/bin/claude-agent-acp",
            InstallSource::Brew,
            Some("brew upgrade claude-agent-acp"),
        ),
        (
            "/usr/local/bin/claude-agent-acp",
            InstallSource::Unknown,
            None,
        ),
    ];

    for (path, source, command) in &cases {
        let check = user_copy_claude_check(path, source, Some("0.17.0"), *command);
        let before = json(&check);
        let mut after = apply_to(check, &installed, true);

        assert!(after.bridge.is_none(), "{source:?}");
        assert!(after.bridge_path.is_none(), "{source:?}");
        let main = after.main.as_ref().unwrap();
        assert_eq!(
            main.update_fix_type,
            Some(FixType::UpdateMain),
            "{source:?}"
        );
        let description = main.update_command.as_deref().expect("an action");
        assert!(
            description.contains("@agentclientprotocol/claude-agent-acp"),
            "{description}"
        );
        assert!(
            description.contains("0.17.1"),
            "names the registry release the installer fetches: {description}"
        );
        assert!(
            !description.contains("0.17.0"),
            "not the crate's per-source answer: {description}"
        );
        assert!(description.contains("~/.staged/packages"), "{description}");
        assert!(description.contains(path), "{description}");
        assert!(!description.contains("npm install"), "{description}");
        assert!(!description.contains("brew upgrade"), "{description}");

        // The row's fix is the same managed install, word for word, so the
        // Update and Fix buttons never describe two different things.
        assert_eq!(after.fix_type, Some(FixType::Bridge), "{source:?}");
        assert_eq!(
            after.fix_command.as_deref(),
            Some(description),
            "{source:?}"
        );

        // The action (and the missing-copy flag) are the *only* changes:
        // path, source, both versions and `updateAvailable` still describe
        // the copy that is on PATH — the readout's `latestVersion` stays
        // the crate's, even though the description names the registry's.
        let main = after.main.as_mut().unwrap();
        main.update_command = command.map(str::to_string);
        main.update_fix_type = command.map(|_| FixType::UpdateMain);
        assert_eq!(
            json(&without_missing_copy_flag(after)),
            before,
            "{source:?}"
        );
    }
}

/// When the managed probe failed — registry unreachable, garbage, timed
/// out — the description falls back to the version the crate flagged the
/// copy stale against, rather than naming nothing: the row is still
/// redirected, since the installer's `@latest` needs no version up front.
#[test]
fn a_redirected_description_falls_back_to_the_crates_latest_when_the_probe_failed() {
    let probe_failed = versions(Some("0.16.2"), None);
    let path = "/opt/homebrew/bin/claude-agent-acp";
    let check = user_copy_claude_check(
        path,
        &InstallSource::Brew,
        Some("0.17.0"),
        Some("brew upgrade claude-agent-acp"),
    );

    let after = apply_to(check, &probe_failed, true);

    let main = after.main.as_ref().unwrap();
    assert_eq!(main.update_fix_type, Some(FixType::UpdateMain));
    assert_eq!(
        main.update_command.as_deref(),
        Some(
            "install Staged's managed @agentclientprotocol/claude-agent-acp 0.17.0 in \
             ~/.staged/packages (the copy at /opt/homebrew/bin/claude-agent-acp is left as is)"
        )
    );
}

/// Only the managed shim gets the managed readout. A row whose path sits
/// under the shim dir under another name, a row with no executable, a row
/// the crate did not label bundled, another tool's versions, and a build
/// with nothing managed all leave the check exactly as the crate produced
/// it.
#[test]
fn unmanaged_copies_and_other_checks_are_left_alone() {
    let shim_bin_dir = Path::new(SHIM_BIN_DIR);
    let installed = versions(Some("0.16.2"), Some("0.17.0"));

    // Under the shim dir but a different binary name.
    let other = Path::new(SHIM_BIN_DIR)
        .join("codex-acp")
        .display()
        .to_string();
    let mut check = managed_claude_check(&other, true);
    let before = json(&check);
    assert!(!apply_managed_bridge_readout(
        &mut check,
        &installed,
        shim_bin_dir,
        true
    ));
    assert_eq!(json(&check), before);

    // Not installed at all.
    let mut check = managed_claude_check(&managed_shim_path(), false);
    check.path = None;
    check.main = None;
    check.status = CheckStatus::Warn;
    let before = json(&check);
    assert!(!apply_managed_bridge_readout(
        &mut check,
        &installed,
        shim_bin_dir,
        false
    ));
    assert_eq!(json(&check), before);

    // The right path, but the crate did not call it bundled (a dev override
    // dir that happens to sit at the shim path is not a managed install).
    let mut check = managed_claude_check(&managed_shim_path(), true);
    check.main.as_mut().unwrap().bundled = None;
    let before = json(&check);
    assert!(!apply_managed_bridge_readout(
        &mut check,
        &installed,
        shim_bin_dir,
        true
    ));
    assert_eq!(json(&check), before);

    // Report-wide: nothing managed (dev override, disabled feature,
    // unsupported target) means no versions and no readout anywhere.
    let mut report = DoctorReport {
        checks: vec![
            managed_claude_check(&managed_shim_path(), true),
            node_runtime_doctor_check(CheckStatus::Pass, "ok".to_string(), None, None, None),
        ],
    };
    let before: Vec<_> = report.checks.iter().map(json).collect();
    apply_managed_bridge_readouts(
        &mut report,
        &[],
        shim_bin_dir,
        true,
        &ManagedInstallDiagnostics::default(),
    );
    let after: Vec<_> = report.checks.iter().map(json).collect();
    assert_eq!(after, before);

    // Report-wide: versions for one tool reach only its own check.
    let codex_versions = ManagedBridgeVersions {
        tool: ManagedTool {
            id: "codex-acp",
            binary: "codex-acp",
            package: "@agentclientprotocol/codex-acp",
        },
        installed: Some("1.1.2".to_string()),
        latest: Some("1.2.0".to_string()),
    };
    apply_managed_bridge_readouts(
        &mut report,
        &[codex_versions],
        shim_bin_dir,
        true,
        &ManagedInstallDiagnostics::default(),
    );
    let after: Vec<_> = report.checks.iter().map(json).collect();
    assert_eq!(after, before);
    apply_managed_bridge_readouts(
        &mut report,
        &[installed],
        shim_bin_dir,
        true,
        &ManagedInstallDiagnostics::default(),
    );
    assert!(report.checks[0].bridge.is_some());
    assert!(report.checks[1].bridge.is_none());
}

/// The redirected action's text names the package, the version, where the
/// managed copy goes, and the copy it leaves alone — an unknown latest
/// version drops the version rather than printing a placeholder.
#[test]
fn managed_install_description_names_the_package_version_and_replaced_copy() {
    let description = managed_install_description(
        &claude_tool(),
        Some("0.17.0"),
        "/opt/homebrew/bin/claude-agent-acp",
    );
    assert_eq!(
        description,
        "install Staged's managed @agentclientprotocol/claude-agent-acp 0.17.0 in \
         ~/.staged/packages (the copy at /opt/homebrew/bin/claude-agent-acp is left as is)"
    );

    let description =
        managed_install_description(&claude_tool(), None, "/opt/homebrew/bin/claude-agent-acp");
    assert_eq!(
        description,
        "install Staged's managed @agentclientprotocol/claude-agent-acp in \
         ~/.staged/packages (the copy at /opt/homebrew/bin/claude-agent-acp is left as is)"
    );
}

#[test]
fn newer_version_available_compares_semver_triples() {
    assert_eq!(
        newer_version_available(Some("0.16.2"), Some("0.17.0")),
        Some(true)
    );
    assert_eq!(
        newer_version_available(Some("0.17.0"), Some("0.17.0")),
        Some(false)
    );
    assert_eq!(
        newer_version_available(Some("1.0.0"), Some("0.99.99")),
        Some(false)
    );
    assert_eq!(
        newer_version_available(Some("v1.2.3"), Some("1.2.4-rc.1")),
        Some(true)
    );
    assert_eq!(newer_version_available(None, Some("1.0.0")), None);
    assert_eq!(newer_version_available(Some("1.0.0"), None), None);
    assert_eq!(newer_version_available(Some("1.0"), Some("1.0.1")), None);
    assert_eq!(newer_version_available(Some("1.0.0"), Some("latest")), None);
}

#[test]
fn is_managed_shim_path_requires_the_shim_dir_and_binary_name() {
    let dir = Path::new(SHIM_BIN_DIR);
    assert!(is_managed_shim_path(
        "/data/.staged/packages/bin/claude-agent-acp",
        dir,
        "claude-agent-acp"
    ));
    assert!(!is_managed_shim_path(
        "/data/.staged/packages/bin/codex-acp",
        dir,
        "claude-agent-acp"
    ));
    assert!(!is_managed_shim_path(
        "/data/.staged/packages/npm-prefix/bin/claude-agent-acp",
        dir,
        "claude-agent-acp"
    ));
    assert!(!is_managed_shim_path(
        "/usr/local/bin/claude-agent-acp",
        dir,
        "claude-agent-acp"
    ));
}
