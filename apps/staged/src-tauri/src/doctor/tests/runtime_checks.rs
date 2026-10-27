use super::*;
use crate::doctor::test_support::*;

fn queue_health(stranded: Vec<sweep::StrandedRow>, swept_total: usize) -> QueueHealth {
    QueueHealth {
        owned_running: stranded.len() + 1,
        live: 1,
        stranded,
        swept_total,
    }
}

#[test]
fn session_queue_check_passes_when_nothing_is_stranded() {
    let check = build_session_queue_check(&queue_health(Vec::new(), 0));

    assert_eq!(check.id, SESSION_QUEUE_CHECK_ID);
    assert_eq!(check.status, CheckStatus::Pass);
    assert_eq!(check.message, "No stranded sessions");
    assert!(check.fix_type.is_none());
    let detail = check.raw_output.expect("detail is attached");
    assert!(detail.contains("owned running rows: 1"), "{detail}");
    assert!(detail.contains("stranded now: 0"), "{detail}");
    assert!(detail.contains("ended by the sweep so far: 0"), "{detail}");
}

/// A sweep that has already acted leaves nothing stranded, so the status
/// stays `Pass` — but the count is what makes a recurrence visible.
#[test]
fn session_queue_check_reports_past_sweeps_without_warning() {
    let check = build_session_queue_check(&queue_health(Vec::new(), 2));

    assert_eq!(check.status, CheckStatus::Pass);
    assert_eq!(
        check.message,
        "No stranded sessions; the sweep has ended 2 sessions so far"
    );
}

#[test]
fn session_queue_check_warns_while_a_row_is_stranded() {
    let row = sweep::StrandedRow {
        session_id: "s-1".to_string(),
        branch_id: Some("b-1".to_string()),
    };
    let check = build_session_queue_check(&queue_health(vec![row], 0));

    assert_eq!(check.status, CheckStatus::Warn);
    assert_eq!(
        check.message,
        "1 session is running with nothing in this process owning it; the stranded-session sweep ends it within a minute"
    );
    let detail = check.raw_output.expect("detail is attached");
    assert!(detail.contains("- session s-1 (branch b-1)"), "{detail}");
    assert!(
        detail.contains("of which owned by a registered runner or a liveness hold: 1"),
        "{detail}"
    );
}

#[test]
fn ready_runtime_passes_without_a_fix() {
    let check = build_node_runtime_check(
        ManagedNodeRuntimeState::Ready,
        Path::new("/data/packages/node/v9.9.9/plat"),
        &["copilot".to_string()],
        None,
    )
    .expect("ready runtime is reported");

    assert_eq!(check.status, CheckStatus::Pass);
    assert_eq!(
        check.message,
        format!("Staged-managed Node.js {} is installed", pinned_version())
    );
    assert_eq!(
        check.path.as_deref(),
        Some("/data/packages/node/v9.9.9/plat/bin/node")
    );
    assert!(check.fix_type.is_none());
    assert!(check.fix_command.is_none());
    assert!(check.fix_url.is_none());
    let output = check.raw_output.as_deref().expect("raw output");
    assert!(output.contains("state: ready"));
    assert!(output.contains("- copilot"));
}

#[test]
fn damaged_runtime_warns_with_a_native_reinstall_fix() {
    let check = build_node_runtime_check(
        ManagedNodeRuntimeState::Broken,
        Path::new("/data/packages/node/v9.9.9/plat"),
        &[],
        None,
    )
    .expect("damaged runtime is reported");

    assert_eq!(check.status, CheckStatus::Warn);
    assert!(check.message.contains("is damaged"));
    assert!(check.path.is_none());
    assert_eq!(check.fix_type, Some(FixType::Command));
    let fix_command = check.fix_command.as_deref().expect("fix command");
    assert!(fix_command.contains(&pinned_version()));
    assert!(fix_command.contains("~/.staged/packages"));
    let output = check.raw_output.as_deref().expect("raw output");
    assert!(output.contains("state: broken"));
    assert!(output.contains("Staged-installed npm tools: none"));
}

#[test]
fn missing_runtime_warns_only_when_installed_tools_need_it() {
    // Tools installed into the private prefix need the runtime: warn with
    // the reinstall fix.
    let check = build_node_runtime_check(
        ManagedNodeRuntimeState::Missing,
        Path::new("/data/packages/node/v9.9.9/plat"),
        &["amp-acp".to_string(), "copilot".to_string()],
        None,
    )
    .expect("needed-but-missing runtime is reported");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(check.message.contains("is not installed"));
    assert_eq!(check.fix_type, Some(FixType::Command));
    let output = check.raw_output.as_deref().expect("raw output");
    assert!(output.contains("- amp-acp"));
    assert!(output.contains("- copilot"));
    assert!(!output.contains("last managed install"), "{output}");

    // Nothing installed that needs it, and no reconcile on record (or a
    // clean one): stay silent.
    for record in [None, Some(reconcile_record(true, Vec::new()))] {
        assert!(build_node_runtime_check(
            ManagedNodeRuntimeState::Missing,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &[],
            record.as_ref(),
        )
        .is_none());
    }
}

/// A runtime that is missing *because* the last reconcile failed is the
/// install that could not happen, not one that was never needed: the row
/// warns with the recorded error even with nothing installed yet, lists
/// every error (and the URL they name) in its detail, and keeps the
/// reinstall fix.
#[test]
fn missing_runtime_after_a_failed_reconcile_warns_with_the_recorded_errors() {
    let node_error = "claude-acp: Node.js runtime download failed: \
         https://nodejs.org/dist/v9.9.9/node-v9.9.9-darwin-arm64.tar.gz: \
         error sending request for url";
    let record = reconcile_record(
        false,
        vec![
            "codex-acp: npm install failed: npm install exited with exit status: 1",
            node_error,
        ],
    );
    let check = build_node_runtime_check(
        ManagedNodeRuntimeState::Missing,
        Path::new("/data/packages/node/v9.9.9/plat"),
        &[],
        Some(&record),
    )
    .expect("a failed install is reported");

    assert_eq!(check.status, CheckStatus::Warn);
    assert_eq!(
        check.message,
        format!(
            "Staged-managed Node.js {} could not be installed: Node.js runtime download \
             failed: https://nodejs.org/dist/v9.9.9/node-v9.9.9-darwin-arm64.tar.gz: \
             error sending request for url",
            pinned_version()
        ),
        "leads with the runtime download error, minus the tool id"
    );
    assert_eq!(check.fix_type, Some(FixType::Command));
    assert!(check.fix_command.is_some());
    let output = check.raw_output.as_deref().expect("raw output");
    assert!(output.contains("state: missing"), "{output}");
    assert!(output.contains("last managed install: failed"), "{output}");
    assert!(output.contains("with 2 error(s):"), "{output}");
    assert!(output.contains(&format!("- {node_error}")), "{output}");
    assert!(
        output.contains("- codex-acp: npm install failed"),
        "{output}"
    );
    assert!(
        output.contains("hint: the configured registry/host did not respond"),
        "{output}"
    );

    // With no runtime-specific error the first recorded one is the
    // headline, still without its tool id.
    let record = reconcile_record(
        false,
        vec!["claude-acp: installed ACP bridge is incomplete: probe exited with 1"],
    );
    let check = build_node_runtime_check(
        ManagedNodeRuntimeState::Missing,
        Path::new("/data/packages/node/v9.9.9/plat"),
        &[],
        Some(&record),
    )
    .unwrap();
    assert!(
        check.message.ends_with(
            "could not be installed: installed ACP bridge is incomplete: probe exited with 1"
        ),
        "{}",
        check.message
    );
    assert!(!check.raw_output.as_deref().unwrap().contains("hint:"));
}

/// The hint is the runtime row's own only when the headline is a runtime
/// download failure: a bridge's npm 404 is still reported, but never as
/// "the configured registry does not carry Node.js". A 4xx from the dist
/// host points at the download URL.
#[test]
fn runtime_row_hints_only_on_its_own_download_failure() {
    let bridge_404 = "codex-acp: npm install failed: npm install exited with exit status: 1 \
         (registry: https://mirror.example.test/npm/): npm error code E404; 404 Not Found - \
         GET https://mirror.example.test/npm/@agentclientprotocol%2fcodex-acp";
    let check = build_node_runtime_check(
        ManagedNodeRuntimeState::Missing,
        Path::new("/data/packages/node/v9.9.9/plat"),
        &[],
        Some(&reconcile_record(false, vec![bridge_404])),
    )
    .unwrap();
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(
        check.message.ends_with(
            "could not be installed: npm install failed: npm install exited with exit status: 1 \
             (registry: https://mirror.example.test/npm/): npm error code E404; 404 Not Found - \
             GET https://mirror.example.test/npm/@agentclientprotocol%2fcodex-acp"
        ),
        "{}",
        check.message
    );
    let output = check.raw_output.as_deref().unwrap();
    assert!(output.contains(&format!("- {bridge_404}")), "{output}");
    assert!(!output.contains("hint:"), "{output}");
    assert!(!output.contains("Node.js\n"), "{output}");

    let check = build_node_runtime_check(
        ManagedNodeRuntimeState::Missing,
        Path::new("/data/packages/node/v9.9.9/plat"),
        &[],
        Some(&reconcile_record(
            false,
            vec![
                bridge_404,
                "claude-acp: Node.js runtime download failed: HTTP 404",
            ],
        )),
    )
    .unwrap();
    assert!(
        check
            .message
            .ends_with("could not be installed: Node.js runtime download failed: HTTP 404"),
        "{}",
        check.message
    );
    let output = check.raw_output.as_deref().unwrap();
    assert!(
        output.contains(
            "hint: the Node.js download URL returned HTTP 404 — check STAGED_NODE_DIST_URL"
        ),
        "{output}"
    );

    // A checksum mismatch is the headline but not a download failure Doctor
    // has advice for.
    let check = build_node_runtime_check(
        ManagedNodeRuntimeState::Missing,
        Path::new("/data/packages/node/v9.9.9/plat"),
        &[],
        Some(&reconcile_record(
            false,
            vec!["claude-acp: Node.js runtime archive SHA-256 mismatch: expected a, got b"],
        )),
    )
    .unwrap();
    assert!(
        check
            .message
            .ends_with("Node.js runtime archive SHA-256 mismatch: expected a, got b"),
        "{}",
        check.message
    );
    assert!(!check.raw_output.as_deref().unwrap().contains("hint:"));
}

#[test]
fn installed_npm_tool_names_lists_visible_entries_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("copilot"), "").unwrap();
    std::fs::write(dir.path().join(".copilot.tmp"), "").unwrap();

    let names = installed_npm_tool_names(dir.path());
    assert_eq!(names, vec!["copilot".to_string()]);

    // An absent dir reads as no tools, not an error.
    assert!(installed_npm_tool_names(&dir.path().join("absent")).is_empty());
}
