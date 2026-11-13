use super::*;
use crate::managed_acp_tools::test_support::*;
use crate::managed_acp_tools::*;

/// Names of the entries directly inside the cache dir, sorted.
fn cache_dir_entries(packages_root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(npm_cache_dir(packages_root))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn the_cache_dir_is_the_one_the_managed_npm_env_names() {
    let packages_root = Path::new("/staged/packages");
    let cache_dir = npm_cache_dir(packages_root);
    assert_eq!(cache_dir, packages_root.join("npm-prefix").join("cache"));

    let env = managed_npm_env_at(&packages_root.join("npm-prefix"));
    for key in ["NPM_CONFIG_CACHE", "npm_config_cache"] {
        let value = env
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str());
        assert_eq!(value, Some(cache_dir.to_string_lossy().as_ref()), "{key}");
    }
}

#[test]
fn a_cache_within_the_cap_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path();
    let tarball = write_cacache_file(packages_root, "content-v2/sha512/aa/bb/tarball", 60);
    let index = write_cacache_file(packages_root, "index-v5/cc/dd/entry", 40);

    // Under the cap, and exactly at it: only a cache *over* the cap goes.
    prune_npm_cache_at(packages_root, 1000);
    prune_npm_cache_at(packages_root, 100);

    assert_eq!(std::fs::read(&tarball).unwrap().len(), 60);
    assert_eq!(std::fs::read(&index).unwrap().len(), 40);
    assert_eq!(cache_dir_entries(packages_root), vec!["_cacache"]);
}

#[test]
fn a_cache_over_the_cap_is_removed_and_nothing_else_is() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path();
    write_cacache_file(packages_root, "content-v2/sha512/aa/bb/tarball", 60);
    write_cacache_file(packages_root, "index-v5/cc/dd/entry", 40);
    write_cacache_file(packages_root, "tmp/leftover", 1);

    // Everything else that lives under the packages root.
    let npm_prefix = packages_root.join("npm-prefix");
    let npm_log = npm_cache_dir(packages_root)
        .join("_logs")
        .join("debug-0.log");
    let global_package = npm_prefix
        .join("lib")
        .join("node_modules")
        .join("@github")
        .join("copilot")
        .join("package.json");
    let global_bin = npm_prefix.join("bin").join("copilot");
    for path in [&npm_log, &global_package, &global_bin] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "keep").unwrap();
    }
    let tool = test_tool();
    let node_install_dir = packages_root
        .join("node")
        .join(TEST_NODE_VERSION)
        .join("plat");
    write_installed_tool(packages_root, &node_install_dir, &tool);
    let state_before = std::fs::read_to_string(state_path(packages_root)).unwrap();

    prune_npm_cache_at(packages_root, 100);

    assert!(!cacache_dir(packages_root).exists());
    // No trash dir is left behind either: `_logs` is all that remains.
    assert_eq!(cache_dir_entries(packages_root), vec!["_logs"]);
    for path in [&npm_log, &global_package, &global_bin] {
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "keep",
            "{}",
            path.display()
        );
    }
    let install_dir = tool_install_dir(packages_root, tool.id);
    assert!(
        crate::managed_acp_tools::install::npm_entrypoint(&install_dir, tool.package).is_file()
    );
    assert!(shim_bin_dir(packages_root).join(tool.binary).is_file());
    assert_eq!(
        std::fs::read_to_string(state_path(packages_root)).unwrap(),
        state_before
    );
}

#[test]
fn leftover_trash_is_removed_even_when_the_cache_is_within_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path();
    let tarball = write_cacache_file(packages_root, "content-v2/sha512/aa/bb/tarball", 10);
    let cache_dir = npm_cache_dir(packages_root);
    // An interrupted prune from another (dead) process, mid-delete.
    let trash_dir = cache_dir.join("_cacache.trash-4242");
    std::fs::create_dir_all(trash_dir.join("content-v2").join("sha512")).unwrap();
    std::fs::write(trash_dir.join("content-v2").join("half-deleted"), "x").unwrap();
    let trash_file = cache_dir.join("_cacache.trash-7");
    std::fs::write(&trash_file, "x").unwrap();
    // A neighbour that only resembles the trash prefix stays.
    let lookalike = cache_dir.join("_cacache.trashed");
    std::fs::create_dir_all(&lookalike).unwrap();

    prune_npm_cache_at(packages_root, 1000);

    assert!(!trash_dir.exists());
    assert!(!trash_file.exists());
    assert!(lookalike.is_dir());
    assert_eq!(std::fs::read(&tarball).unwrap().len(), 10);
}

#[test]
fn a_missing_cache_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");

    // No packages root at all, then a cache dir with no `_cacache` in it.
    prune_npm_cache_at(&packages_root, 0);
    assert!(!packages_root.exists());

    std::fs::create_dir_all(npm_cache_dir(&packages_root)).unwrap();
    prune_npm_cache_at(&packages_root, 0);
    assert!(cache_dir_entries(&packages_root).is_empty());
}

#[test]
fn a_cacache_that_is_a_plain_file_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path();
    let cacache = cacache_dir(packages_root);
    std::fs::create_dir_all(cacache.parent().unwrap()).unwrap();
    std::fs::write(&cacache, "not a directory").unwrap();

    prune_npm_cache_at(packages_root, 0);

    assert_eq!(
        std::fs::read_to_string(&cacache).unwrap(),
        "not a directory"
    );
    assert_eq!(cache_dir_entries(packages_root), vec!["_cacache"]);
}

#[test]
fn symlinks_inside_the_cache_are_neither_sized_nor_followed() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    // Outside the cache: far more than the cap, reachable only through links.
    let outside_dir = dir.path().join("outside");
    std::fs::create_dir_all(&outside_dir).unwrap();
    let outside_file = outside_dir.join("big");
    std::fs::write(&outside_file, vec![b'x'; 5000]).unwrap();

    let tarball = write_cacache_file(&packages_root, "content-v2/tarball", 10);
    let cacache = cacache_dir(&packages_root);
    std::os::unix::fs::symlink(&outside_dir, cacache.join("dir-link")).unwrap();
    std::os::unix::fs::symlink(&outside_file, cacache.join("content-v2").join("file-link"))
        .unwrap();

    // Sizing: the 5000 linked bytes do not count, so 10 bytes is within a
    // 100-byte cap.
    prune_npm_cache_at(&packages_root, 100);
    assert!(tarball.is_file());
    assert!(cacache.join("dir-link").is_symlink());

    // Deleting: the links go with the cache, their targets stay.
    prune_npm_cache_at(&packages_root, 5);
    assert!(!cacache.exists());
    assert_eq!(std::fs::read(&outside_file).unwrap().len(), 5000);
}

#[test]
fn a_cacache_that_is_itself_a_symlink_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let packages_root = dir.path().join("packages");
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let elsewhere_file = elsewhere.join("tarball");
    std::fs::write(&elsewhere_file, vec![b'x'; 50]).unwrap();
    let cacache = cacache_dir(&packages_root);
    std::fs::create_dir_all(cacache.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &cacache).unwrap();

    prune_npm_cache_at(&packages_root, 0);

    assert!(cacache.is_symlink());
    assert_eq!(std::fs::read(&elsewhere_file).unwrap().len(), 50);
}
