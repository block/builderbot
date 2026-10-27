use super::*;
use crate::managed_acp_tools::test_support::*;
use crate::managed_acp_tools::*;

#[test]
fn prune_removes_installs_dropped_from_the_managed_set() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path();
    let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
    let kept = test_tool();
    let dropped = ManagedTool {
        id: "codex-acp",
        binary: "codex-acp",
        package: "@agentclientprotocol/codex-acp",
    };
    write_installed_tool(packages_root, &node_install_dir, &kept);
    write_installed_tool(packages_root, &node_install_dir, &dropped);
    // A crashed install with no state entry.
    std::fs::create_dir_all(tools_root(packages_root).join("ghost-acp")).unwrap();

    prune_stale_managed_tools(packages_root, std::slice::from_ref(&kept));

    let state = read_state(packages_root);
    assert!(state.tools.contains_key(kept.id));
    assert!(!state.tools.contains_key(dropped.id));
    assert!(shim_bin_dir(packages_root).join(kept.binary).exists());
    assert!(!shim_bin_dir(packages_root).join(dropped.binary).exists());
    assert!(tools_root(packages_root).join(kept.id).exists());
    assert!(!tools_root(packages_root).join(dropped.id).exists());
    assert!(!tools_root(packages_root).join("ghost-acp").exists());
}

#[test]
fn record_reconcile_stamps_the_state() {
    let dir = tempfile::tempdir().unwrap();
    record_reconcile(dir.path(), vec!["codex-acp: boom".to_string()]);
    let record = read_state(dir.path()).last_reconcile.unwrap();
    assert!(!record.ok);
    assert_eq!(record.errors, vec!["codex-acp: boom".to_string()]);
    assert!(record.at_ms > 0);

    record_reconcile(dir.path(), Vec::new());
    let record = read_state(dir.path()).last_reconcile.unwrap();
    assert!(record.ok);
    assert!(record.errors.is_empty());
}

#[tokio::test]
async fn clean_reconcile_prunes_the_superseded_node_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let (packages_root, pinned_dir, superseded_dir) = write_node_bump_leftovers(dir.path());
    let tool = test_tool();
    write_installed_tool(&packages_root, &pinned_dir, &tool);

    finish_reconcile_at(&packages_root, std::slice::from_ref(&tool), Vec::new()).await;

    assert!(pinned_dir.exists());
    assert!(!superseded_dir.exists());
    assert!(read_state(&packages_root).last_reconcile.unwrap().ok);
}

#[tokio::test]
async fn partial_failure_keeps_the_superseded_node_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let (packages_root, pinned_dir, superseded_dir) = write_node_bump_leftovers(dir.path());
    let tool = test_tool();
    // The failed bridge's shim was never rewritten: it still execs the
    // superseded runtime, which must therefore survive the epilogue.
    write_installed_tool(&packages_root, &superseded_dir, &tool);

    finish_reconcile_at(
        &packages_root,
        std::slice::from_ref(&tool),
        vec![format!("{}: npm install failed", tool.id)],
    )
    .await;

    assert!(pinned_dir.exists());
    assert!(superseded_dir.exists());
    let shim = std::fs::read_to_string(shim_bin_dir(&packages_root).join(tool.binary)).unwrap();
    assert!(shim.contains(&superseded_dir.to_string_lossy().into_owned()));
    assert!(!read_state(&packages_root).last_reconcile.unwrap().ok);
}
