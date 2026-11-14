use super::*;
use crate::managed_acp_tools::test_support::*;
use crate::managed_acp_tools::*;

/// The npm cache cap the epilogue tests run with; fixture caches are a few
/// bytes either side of it.
const TEST_NPM_CACHE_MAX_BYTES: u64 = 100;

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

    finish_reconcile_at(
        &packages_root,
        std::slice::from_ref(&tool),
        Vec::new(),
        TEST_NPM_CACHE_MAX_BYTES,
    )
    .await;

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
        TEST_NPM_CACHE_MAX_BYTES,
    )
    .await;

    assert!(pinned_dir.exists());
    assert!(superseded_dir.exists());
    let shim = std::fs::read_to_string(shim_bin_dir(&packages_root).join(tool.binary)).unwrap();
    assert!(shim.contains(&superseded_dir.to_string_lossy().into_owned()));
    assert!(!read_state(&packages_root).last_reconcile.unwrap().ok);
}

#[tokio::test]
async fn failed_reconcile_still_prunes_an_oversized_npm_cache() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let tool = test_tool();
    write_cacache_file(&packages_root, "content-v2/sha512/aa/bb/tarball", 150);
    let error = format!("{}: npm install failed", tool.id);

    finish_reconcile_at(
        &packages_root,
        std::slice::from_ref(&tool),
        vec![error.clone()],
        TEST_NPM_CACHE_MAX_BYTES,
    )
    .await;

    // The cleanup is not success-gated — a bridge that fails every pass must
    // not switch it off — and the failed pass is still recorded.
    assert!(!cacache_dir(&packages_root).exists());
    let record = read_state(&packages_root).last_reconcile.unwrap();
    assert!(!record.ok);
    assert_eq!(record.errors, vec![error]);
}

#[tokio::test]
async fn reconcile_keeps_an_npm_cache_within_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let tool = test_tool();
    let tarball = write_cacache_file(&packages_root, "content-v2/sha512/aa/bb/tarball", 50);

    finish_reconcile_at(
        &packages_root,
        std::slice::from_ref(&tool),
        Vec::new(),
        TEST_NPM_CACHE_MAX_BYTES,
    )
    .await;

    assert!(tarball.is_file());
    assert!(read_state(&packages_root).last_reconcile.unwrap().ok);
}

#[tokio::test]
async fn epilogue_prunes_the_npm_cache_only_once_it_holds_the_packages_flock() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let tool = test_tool();
    write_cacache_file(&packages_root, "content-v2/sha512/aa/bb/tarball", 150);

    // Another Staged process mid-install: a second open file description
    // holding the packages flock.
    let contender = open_packages_lock_contender(&packages_root);
    acquire_packages_flock(&contender).await;

    let epilogue = tokio::spawn({
        let packages_root = packages_root.clone();
        async move {
            finish_reconcile_at(
                &packages_root,
                std::slice::from_ref(&tool),
                Vec::new(),
                TEST_NPM_CACHE_MAX_BYTES,
            )
            .await;
        }
    });

    // While the other holder lives the epilogue stays parked on the flock:
    // nothing is recorded and the oversized cache is not touched.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!epilogue.is_finished());
    assert!(cacache_dir(&packages_root).is_dir());
    assert!(read_state(&packages_root).last_reconcile.is_none());

    drop(contender);
    tokio::time::timeout(std::time::Duration::from_secs(30), epilogue)
        .await
        .expect("epilogue never finished after the packages lock was released")
        .unwrap();

    assert!(!cacache_dir(&packages_root).exists());
    assert!(read_state(&packages_root).last_reconcile.unwrap().ok);
}
