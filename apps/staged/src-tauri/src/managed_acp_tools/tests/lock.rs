use super::*;
use crate::managed_acp_tools::test_support::*;

#[tokio::test]
async fn npm_write_lock_holds_the_packages_flock_until_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");

    let lock = lock_npm_writes_at(&packages_root).await.unwrap();
    // Another Staged process — a bridge install, a reconcile epilogue about
    // to prune the npm cache — cannot take the flock while a fix holds it.
    let contender = open_packages_lock_contender(&packages_root);
    assert!(!try_packages_flock(&contender));
    // Nor can anything in this process take the tool-install mutex.
    assert!(tool_install_lock().try_lock().is_err());

    drop(lock);
    acquire_packages_flock(&contender).await;
}
