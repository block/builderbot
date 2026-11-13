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

/// The Doctor-path cap: a fix that holds the npm write lock wipes an
/// oversized cache before releasing it, and nothing else under the root.
#[tokio::test]
async fn npm_write_lock_prunes_an_oversized_cache_while_still_held() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    write_cacache_file(&packages_root, "content-v2/sha512/aa/bb/tarball", 60);
    write_cacache_file(&packages_root, "index-v5/cc/dd/entry", 41);
    // What the fix's `npm install -g` just put in the private prefix.
    let global_package = packages_root
        .join("npm-prefix")
        .join("lib")
        .join("node_modules")
        .join("@github")
        .join("copilot")
        .join("package.json");
    std::fs::create_dir_all(global_package.parent().unwrap()).unwrap();
    std::fs::write(&global_package, "keep").unwrap();

    let lock = lock_npm_writes_at(&packages_root).await.unwrap();
    lock.prune_npm_cache_over(100).await;

    assert!(!cacache_dir(&packages_root).exists());
    assert_eq!(std::fs::read_to_string(&global_package).unwrap(), "keep");
    // The prune did not give the locks up: no other Staged process can start
    // an install until the fix drops them.
    let contender = open_packages_lock_contender(&packages_root);
    assert!(!try_packages_flock(&contender));
    assert!(tool_install_lock().try_lock().is_err());

    drop(lock);
    acquire_packages_flock(&contender).await;
}

#[tokio::test]
async fn npm_write_lock_leaves_a_cache_within_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let tarball = write_cacache_file(&packages_root, "content-v2/sha512/aa/bb/tarball", 100);

    let lock = lock_npm_writes_at(&packages_root).await.unwrap();
    lock.prune_npm_cache_over(100).await;
    // A root with no npm prefix at all — a fix that failed before npm wrote
    // anything — is a no-op too.
    drop(lock);
    let empty_root = dir.path().join("empty");
    let lock = lock_npm_writes_at(&empty_root).await.unwrap();
    lock.prune_npm_cache_over(0).await;

    assert_eq!(std::fs::read(&tarball).unwrap().len(), 100);
    assert!(!empty_root.join("npm-prefix").exists());
}
