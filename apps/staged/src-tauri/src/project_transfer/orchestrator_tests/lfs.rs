//! A project with Git LFS files goes to a workspace and back, through the
//! whole transfer, against a real `git lfs`. Skips, saying so, without one.

use super::super::tests::lfs::{
    has_lfs_object, isolated_config, lfs_installed, track_psd, POINTER_PREFIX,
};
use super::*;

/// A single-repo project whose scripts see no git config but the clones'
/// own, like Blox: git-lfs installed, no LFS filter set up anywhere.
fn lfs_project() -> Fixture {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    for (key, value) in isolated_config(&f.host.root) {
        f.host
            .script_vars
            .lock()
            .unwrap()
            .push((key.to_string(), value));
    }
    f
}

fn assert_real(checkout: &Path, path: &str, content: &[u8]) {
    let actual = fs::read(checkout.join(path)).unwrap();
    assert!(!actual.starts_with(POINTER_PREFIX), "{path} is a pointer");
    assert!(actual == content, "{path} has other content");
}

#[test]
fn lfs_files_go_to_a_workspace_and_back_with_their_content() {
    if !lfs_installed("lfs_files_go_to_a_workspace_and_back_with_their_content") {
        return;
    }
    let f = lfs_project();
    let worktree = f.worktree(0);
    track_psd(&worktree);
    let committed = noise(3000, 31);
    let changed_before = noise(2000, 32);
    write(&worktree, "art/committed.psd", &committed);
    write(&worktree, "art/changed.psd", &changed_before);
    commit(&worktree, "art");
    let changed = noise(2500, 33);
    let staged = noise(1200, 34);
    let new = noise(1500, 35);
    write(&worktree, "art/changed.psd", &changed);
    write(&worktree, "art/staged.psd", &staged);
    git(&worktree, &["add", "art/staged.psd"]);
    write(&worktree, "art/new.psd", &new);
    let expected = observe(&worktree);

    let preflight = f.preflight();
    assert!(preflight.can_transfer, "{:?}", preflight.blockers);
    // HEAD's two and the staged one are counted by object; the working
    // tree's new versions as changed files.
    assert_eq!(preflight.repos[0].lfs_present, 5);
    assert_eq!(
        preflight.repos[0].lfs_present_bytes,
        3000 + 2000 + 2500 + 1200 + 1500
    );
    assert_eq!(preflight.repos[0].lfs_missing, 0);
    assert!(
        !codes(&preflight.repos[0].warnings).contains(&"lfs_missing"),
        "{:?}",
        preflight.repos[0].warnings
    );

    let to_remote = f.transfer(true);
    let result = to_remote.result.expect("to remote");
    let clone = f.checkout(0);
    assert_eq!(observe(&clone), expected);
    for (path, content) in [
        ("art/committed.psd", &committed),
        ("art/changed.psd", &changed),
        ("art/staged.psd", &staged),
        ("art/new.psd", &new),
    ] {
        assert_real(&clone, path, content);
    }
    assert!(has_lfs_object(&clone, &changed_before));
    assert!(
        !codes(&result.repos[0].warnings)
            .iter()
            .any(|c| c.starts_with("lfs")),
        "{:?}",
        result.repos[0].warnings
    );
    assert!(!codes(&result.warnings).iter().any(|c| c.starts_with("lfs")));
    // The archive stays beside the bundle as the backup.
    let backup = f.host.root.join("transfers").join(&to_remote.transfer_id);
    assert!(backup.join("0-widgets.bundle.lfs.tar").is_file());
    assert_eq!(f.bundles(&to_remote.transfer_id), 1);

    // Back, to a clone that has lost its LFS objects in the meantime:
    // the workspace's are the only copy.
    let local_clone = f.host.clone_of(WIDGETS);
    let lfs_store = local_clone.join(".git/lfs");
    if lfs_store.exists() {
        fs::remove_dir_all(&lfs_store).unwrap();
    }
    let pushed_back = noise(800, 36);
    write(&clone, "art/new.psd", &pushed_back);
    let expected = observe(&clone);

    let to_local = f.transfer(true);
    let result = to_local.result.expect("to local");
    let worktree = f.checkout(0);
    assert_eq!(observe(&worktree), expected);
    for (path, content) in [
        ("art/committed.psd", &committed),
        ("art/changed.psd", &changed),
        ("art/staged.psd", &staged),
        ("art/new.psd", &pushed_back),
    ] {
        assert_real(&worktree, path, content);
    }
    assert!(
        !codes(&result.repos[0].warnings)
            .iter()
            .any(|c| c.starts_with("lfs")),
        "{:?}",
        result.repos[0].warnings
    );
    assert_eq!(f.bundles(&to_local.transfer_id), 1);
    assert!(f
        .host
        .root
        .join("transfers")
        .join(&to_local.transfer_id)
        .join("0-widgets.bundle.lfs.tar")
        .is_file());
}
