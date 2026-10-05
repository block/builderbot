use super::*;

const TIMEOUT: Duration = super::super::validation::VALIDATION_TIMEOUT;
const MISSING_NATIVE: &str = "echo 'Claude native binary not found for darwin-arm64' >&2\nexit 1\n";

fn write_probe(node: &Path, template: &Path, tool: &ManagedTool, script: &str) {
    write_fixture_install(template, tool, "1.2.4");
    std::fs::write(npm_entrypoint(template, tool.package), script).unwrap();
    write_fake_node_with_npm(node, template, 0);
    // Execute the staged fixture itself. Check the absolute entrypoint and
    // arguments so probing the old live tree or plain --version cannot pass.
    write_shim(
        &node.join("bin"),
        "node",
        &format!(
            "#!/bin/sh\ncase \"$1\" in /*) ;; *) exit 91 ;; esac\n[ \"$1\" -ef \"$PWD/node_modules/{}/dist/index.js\" ] || exit 91\n[ \"$2\" = --cli ] && [ \"$3\" = --version ] && [ $# = 3 ] || exit 92\nexec /bin/sh \"$@\"\n",
            tool.package
        ),
    )
    .unwrap();
}

#[tokio::test]
async fn complete_claude_promotes_the_validated_staging_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("packages with spaces");
    let node = root.join("node");
    let tool = test_tool();
    write_probe(
        &node,
        &dir.path().join("template"),
        &tool,
        "[ -z \"${CLAUDE_CODE_EXECUTABLE:-}\" ] || exit 93\n[ -z \"${NODE_OPTIONS:-}\" ] || exit 94\nread input && exit 95\necho validated > validated\necho '2.1.287 (Claude Code)'\n",
    );

    install_npm_tool(
        &root,
        &node,
        TEST_NODE_VERSION,
        &tool,
        None,
        TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap();

    let live = tool_install_dir(&root, tool.id);
    assert_eq!(
        std::fs::read_to_string(live.join("validated")).unwrap(),
        "validated\n"
    );
    assert_eq!(
        installed_tool_version(&root, &tool).as_deref(),
        Some("1.2.4")
    );
    assert_eq!(
        read_state(&root).tools[tool.id].node_version,
        TEST_NODE_VERSION
    );
    assert_eq!(
        std::fs::read_to_string(shim_bin_dir(&root).join(tool.binary)).unwrap(),
        shim_contents(&node_binary(&node), &npm_entrypoint(&live, tool.package))
    );
    assert!(!staging_install_dir(&root, tool.id).exists());
}

#[tokio::test]
async fn incomplete_first_install_creates_no_live_tree_shim_or_state() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("packages");
    let node = root.join("node");
    let tool = test_tool();
    write_probe(&node, &dir.path().join("template"), &tool, MISSING_NATIVE);

    let error = install_npm_tool(
        &root,
        &node,
        TEST_NODE_VERSION,
        &tool,
        None,
        TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ManagedToolError::Incomplete(_)), "{error}");
    assert!(!tool_install_dir(&root, tool.id).exists());
    assert!(!shim_bin_dir(&root).join(tool.binary).exists());
    assert!(!state_path(&root).exists());
    assert!(!staging_install_dir(&root, tool.id).exists());
}

/// Every validation failure must preserve the old tree, shim and state before
/// the reconcile epilogue records it. Use an old Node path to catch premature
/// runtime migration as well as premature package promotion.
async fn rejected_upgrade(script: &str, timeout: Duration) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let (root, node, old_node) = write_node_bump_leftovers(dir.path());
    let tool = test_tool();
    write_installed_tool(&root, &old_node, &tool);
    let live = tool_install_dir(&root, tool.id);
    let entrypoint = npm_entrypoint(&live, tool.package);
    std::fs::write(&entrypoint, "echo '1.2.3'\n").unwrap();
    write_shim(
        &old_node.join("bin"),
        "node",
        "#!/bin/sh\nexec /bin/sh \"$@\"\n",
    )
    .unwrap();
    let shim = shim_bin_dir(&root).join(tool.binary);
    let shim_before = std::fs::read(&shim).unwrap();
    let state_before = std::fs::read(state_path(&root)).unwrap();
    write_probe(&node, &dir.path().join("template"), &tool, script);
    let lines = Mutex::new(Vec::new());

    let error = install_npm_tool(
        &root,
        &node,
        TEST_NODE_VERSION,
        &tool,
        None,
        timeout,
        &|line| lines.lock().unwrap().push(line.to_string()),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ManagedToolError::Incomplete(_)), "{error}");
    assert_eq!(std::fs::read(&shim).unwrap(), shim_before);
    assert_eq!(std::fs::read(state_path(&root)).unwrap(), state_before);
    assert_eq!(
        installed_tool_version(&root, &tool).as_deref(),
        Some("1.2.3")
    );
    let old_version = tokio::process::Command::new(&shim).output().await.unwrap();
    assert!(old_version.status.success());
    assert_eq!(old_version.stdout, b"1.2.3\n");
    assert!(!staging_install_dir(&root, tool.id).exists());
    assert!(!live.with_extension("old").exists());
    assert!(!lines
        .lock()
        .unwrap()
        .iter()
        .any(|line| line.contains("is ready")));
    (dir, error.to_string())
}

#[tokio::test]
async fn missing_native_binary_preserves_live_install_and_partial_reconcile_runtime() {
    let (dir, error) = rejected_upgrade(MISSING_NATIVE, TIMEOUT).await;
    assert!(error.contains("--cli --version"), "{error}");
    assert!(error.contains("Claude native binary not found"), "{error}");
    let root = dir.path().join("packages");
    let node = managed_node::pinned_install_dir(&root.join("node")).unwrap();
    let codex = MANAGED_TOOLS[1];
    let template = dir.path().join("codex-template");
    write_probe(&node, &template, &codex, "exit 99\n");
    // The next tool still installs; Codex must not run Claude's probe.
    install_npm_tool(
        &root,
        &node,
        TEST_NODE_VERSION,
        &codex,
        None,
        TIMEOUT,
        &|_| {},
    )
    .await
    .unwrap();
    finish_reconcile_at(&root, MANAGED_TOOLS, vec![format!("claude-acp: {error}")]).await;

    let state = read_state(&root);
    let record = state.last_reconcile.unwrap();
    assert!(!record.ok);
    assert_eq!(record.errors, vec![format!("claude-acp: {error}")]);
    assert_eq!(state.tools["claude-acp"].version, "1.2.3");
    assert_eq!(state.tools["codex-acp"].version, "1.2.4");
    assert!(root.join("node/v0.0.1/plat/bin/node").exists());
}

#[tokio::test]
async fn personal_claude_override_cannot_mask_missing_native_binary() {
    // Give only an isolated test subprocess the override: mutating this test
    // runner's environment would race other parallel tests.
    const MARKER: &str = "STAGED_TEST_CLAUDE_VALIDATION";
    if std::env::var_os(MARKER).is_none() {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "managed_acp_tools::tests::validation::personal_claude_override_cannot_mask_missing_native_binary", "--nocapture"])
            .env(MARKER, "1")
            .env("CLAUDE_CODE_EXECUTABLE", "/bin/true")
            .env("NODE_OPTIONS", "--invalid-node-option")
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    assert_eq!(
        std::env::var("CLAUDE_CODE_EXECUTABLE").unwrap(),
        "/bin/true"
    );
    let (_, error) = rejected_upgrade(
        &format!("[ -n \"${{CLAUDE_CODE_EXECUTABLE:-}}\" ] && exit 0\n[ -n \"${{NODE_OPTIONS:-}}\" ] && exit 0\n{MISSING_NATIVE}"),
        TIMEOUT,
    ).await;
    assert!(error.contains("Claude native binary not found"), "{error}");
}

// Model the real Node bridge: keep a distinct native child that inherits the
// output pipes, and record both PIDs only after spawning it.
const HANGING_NATIVE: &str =
    "/bin/sleep 60 &\necho \"$$ $!\" > ../../validation-pids\necho 'waiting for native CLI' >&2\n";

async fn probe_pids(root: &Path) -> Vec<i32> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(contents) = tokio::fs::read_to_string(root.join("validation-pids")).await {
                let pids: Vec<i32> = contents
                    .split_whitespace()
                    .map(|pid| pid.parse().unwrap())
                    .collect();
                if pids.len() == 2 {
                    return pids;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("probe did not spawn its native child")
}

async fn assert_probe_tree_stopped(pids: &[i32]) {
    let stopped = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if pids.iter().all(|&pid| {
                // Signal 0 only checks existence; allow asynchronous reaping
                // of the native grandchild by the OS after the group is killed.
                (unsafe { libc::kill(pid, 0) }) == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            }) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if stopped.is_err() {
        // Do not leave a hanging fixture behind when this regression fails.
        for &pid in pids {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
    assert!(
        stopped.is_ok(),
        "probe processes survived cleanup: {pids:?}"
    );
}

async fn timeout_preserves_install_and_kills_tree(bridge_exit: &str) {
    let started = Instant::now();
    let (dir, error) = rejected_upgrade(
        &format!("{HANGING_NATIVE}{bridge_exit}\n"),
        Duration::from_secs(1),
    )
    .await;
    let pids = probe_pids(&dir.path().join("packages")).await;
    assert_probe_tree_stopped(&pids).await;
    assert!(error.contains("timed out after 1 seconds"), "{error}");
    assert!(error.contains("waiting for native CLI"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn timeout_kills_bridge_and_native_child_and_preserves_previous_install() {
    timeout_preserves_install_and_kills_tree("wait").await;
}

#[tokio::test]
async fn timeout_kills_native_child_after_bridge_exits_with_open_pipes() {
    timeout_preserves_install_and_kills_tree("exit 0").await;
}

#[tokio::test]
async fn cancellation_kills_bridge_and_native_child() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("packages");
    let node = root.join("node");
    let tool = test_tool();
    let staging = staging_install_dir(&root, tool.id);
    write_probe(&node, &staging, &tool, &format!("{HANGING_NATIVE}wait\n"));
    let probe = tokio::spawn(async move {
        super::super::validation::validate_staged_tool(&node, &staging, &tool, TIMEOUT).await
    });
    let pids = probe_pids(&root).await;
    probe.abort();
    assert!(probe.await.unwrap_err().is_cancelled());
    assert_probe_tree_stopped(&pids).await;
}

#[tokio::test]
async fn verbose_failure_keeps_bounded_stdout_and_stderr_diagnostics() {
    let (_, error) = rejected_upgrade(
        "echo 'native binary missing' >&2\ni=0\nwhile [ $i -lt 5000 ]; do echo 'diagnostic output'; echo 'diagnostic error' >&2; i=$((i + 1)); done\nexit 1\n",
        TIMEOUT,
    ).await;
    assert!(error.contains("stderr: native binary missing"), "{error}");
    assert!(error.contains("stdout: diagnostic output"), "{error}");
    assert!(
        error.len() < 4500,
        "diagnostics were not bounded: {}",
        error.len()
    );
}
