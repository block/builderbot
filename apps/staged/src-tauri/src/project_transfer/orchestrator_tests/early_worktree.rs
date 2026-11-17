//! Coming to local, a branch's worktree is added while its capture from
//! the workspace still runs, then switched onto the branch once that has
//! landed — or, when anything about it goes wrong, discarded for a worktree
//! added the way it always was.
//!
//! A worktree that went the early way shows it in its `HEAD` reflog: one
//! added on the branch never moved from another commit.

use std::time::{Duration, Instant};

use super::super::orchestrator::FailingEarlyAdds;
use super::super::tests::lfs::{lfs_installed, track_psd, POINTER_PREFIX};
use super::super::undo::AddedWorktree;
use super::lfs::lfs_project;
use super::*;

/// Take `f`'s one branch, `feature`, to a workspace: pushed, then a commit
/// ahead of origin, with an untracked file. Returns the pushed tip and the
/// head.
fn send_to_a_workspace(f: &Fixture) -> (String, String) {
    let worktree = f.worktree(0);
    write(&worktree, "pushed.txt", "pushed\n");
    let pushed = commit(&worktree, "pushed");
    git(&worktree, &["push", "-q", "-u", "origin", "feature"]);
    write(&worktree, "ahead.txt", "ahead\n");
    let head = commit(&worktree, "ahead");
    write(&worktree, "notes.txt", "untracked\n");
    f.transfer(true).result.expect("to remote");
    (pushed, head)
}

/// Where the branch's worktree goes when the project comes back.
fn worktree_path(f: &Fixture) -> PathBuf {
    f.host
        .local_worktree_path(&f.project.id, WIDGETS, "feature")
        .unwrap()
}

/// Hold every snapshot in the workspace until `worktree` is there (one
/// that never comes fails the wait after 20 s). Returns, per snapshot,
/// whether it was.
fn hold_the_snapshot_for(f: &Fixture, worktree: &Path) -> Arc<Mutex<Vec<bool>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (held, gitfile) = (Arc::clone(&seen), worktree.join(".git"));
    f.host.on_every_script(move |side, script| {
        if side != Side::Workspace || script != SNAPSHOT_SCRIPT {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while !gitfile.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        held.lock().unwrap().push(gitfile.exists());
    });
    seen
}

/// Every move of the worktree's `HEAD`, as its reflog words them.
fn head_moves(worktree: &Path) -> Vec<String> {
    git(worktree, &["reflog", "--format=%gs", "HEAD"])
        .lines()
        .map(str::to_string)
        .collect()
}

/// Whether the worktree was added early, at `start`, and switched onto
/// `feature` from there.
fn switched_from(worktree: &Path, start: &str) -> bool {
    head_moves(worktree).contains(&format!("checkout: moving from {start} to feature"))
}

/// Whether the worktree was added on its branch, never having been
/// anywhere else.
fn added_on_its_branch(worktree: &Path) -> bool {
    !head_moves(worktree)
        .iter()
        .any(|m| m.starts_with("checkout: "))
}

/// The branch's worktree, which must be on `feature` at `head`.
fn worktree_on_feature_at(f: &Fixture, head: &str) -> PathBuf {
    let worktree = f.worktree(0);
    assert_eq!(worktree, worktree_path(f));
    assert_eq!(
        git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "feature"
    );
    assert_eq!(rev(&worktree, "HEAD"), head);
    assert_eq!(rev(&f.host.clone_of(WIDGETS), "refs/heads/feature"), head);
    worktree
}

/// The snapshot waits here for the worktree, so it only finishes because
/// the worktree is added beside it. The workspace's head is new to the
/// clone, so the worktree starts at what origin has of the branch.
#[test]
fn the_worktree_is_added_while_the_snapshot_runs() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let (pushed, _) = send_to_a_workspace(&f);
    let checkout = f.checkout(0);
    write(&checkout, "remote.txt", "made remotely\n");
    let head = commit(&checkout, "made remotely");
    write(
        &checkout,
        "partial.txt",
        "one\ntwo\nthree\nfour\nfive\nsix\n",
    );
    let expected = observe(&checkout);
    let worktree = worktree_path(&f);
    let seen = hold_the_snapshot_for(&f, &worktree);

    f.transfer(true).result.expect("to local");

    assert_eq!(*seen.lock().unwrap(), vec![true], "added by the snapshot");
    assert_eq!(worktree_on_feature_at(&f, &head), worktree);
    assert_eq!(observe(&worktree), expected);
    assert!(
        switched_from(&worktree, &pushed),
        "{:?}",
        head_moves(&worktree)
    );
    assert_eq!(temp_refs(&worktree), "");
}

/// The worktree is in the undo list before it is made, which is now before
/// the snapshot: a transfer cut off anywhere after has it to remove.
#[test]
fn the_undo_list_names_the_worktree_before_the_snapshot_starts() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    send_to_a_workspace(&f);
    let recorded = Mutex::new(None);

    let transfer = f.transfer_with(true, &|progress| {
        if progress.phase != TransferPhase::Snapshotting || recorded.lock().unwrap().is_some() {
            return;
        }
        let dir = f.host.root.join("transfers").join(&progress.transfer_id);
        let undo = backups::read_manifest(&dir).expect("manifest").undo;
        *recorded.lock().unwrap() = Some(undo.worktrees);
    });

    transfer.result.expect("to local");
    assert_eq!(
        recorded.into_inner().unwrap(),
        Some(vec![AddedWorktree {
            clone: f.host.clone_of(WIDGETS),
            worktree: worktree_path(&f),
        }])
    );
}

/// The snapshot fails once the early worktree is there: the leg waits for
/// the add, and the rollback removes what it made.
#[test]
fn a_snapshot_that_fails_after_the_early_add_leaves_no_worktree() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    send_to_a_workspace(&f);
    let clone = f.host.clone_of(WIDGETS);
    let (db, before) = (f.db_state(), clone_state(&clone));
    let source = source_fingerprint(&f.checkout(0));
    let seen = hold_the_snapshot_for(&f, &worktree_path(&f));
    f.host
        .arm(Fault::Script(Side::Workspace, SNAPSHOT_SCRIPT, 1));

    let transfer = f.transfer(true);

    let err = transfer.result.unwrap_err();
    assert!(f.host.fired(), "{err}");
    assert_eq!(*seen.lock().unwrap(), vec![true], "the worktree was there");
    assert!(f.worktree_dirs().is_empty(), "{:?}", f.worktree_dirs());
    assert_eq!(clone_state(&clone), before);
    assert_eq!(f.db_state(), db);
    assert_eq!(source_fingerprint(&f.checkout(0)), source);
    let manifest = f.manifest(&transfer.transfer_id).expect("manifest");
    assert_eq!(manifest.status, "rolledBack");
}

/// The early worktree starts at the head the inspection saw; a commit made
/// in the workspace after that comes with the snapshot, and the switch
/// takes the worktree to it.
#[test]
fn a_head_that_moves_after_the_inspection_still_lands_in_the_worktree() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let (_, inspected) = send_to_a_workspace(&f);
    let checkout = f.checkout(0);
    let moved = Arc::new(Mutex::new(None));
    let (source, late) = (checkout.clone(), Arc::clone(&moved));
    f.host.before(Side::Workspace, SNAPSHOT_SCRIPT, 1, move || {
        write(&source, "late.txt", "after the inspection\n");
        write(&source, "a.txt", "alpha, later\n");
        let head = commit(&source, "late");
        *late.lock().unwrap() = Some((head, observe(&source)));
    });

    f.transfer(true).result.expect("to local");

    let (head, expected) = moved.lock().unwrap().take().expect("the head moved");
    assert_ne!(head, inspected);
    let worktree = worktree_on_feature_at(&f, &head);
    assert_eq!(observe(&worktree), expected);
    assert!(
        switched_from(&worktree, &inspected),
        "{:?}",
        head_moves(&worktree)
    );
}

/// An early add that fails part way, leaving a directory behind, costs
/// nothing but the head start: the leg clears it and adds the worktree on
/// the branch at the same path.
#[test]
fn an_early_add_that_fails_falls_back_to_adding_the_worktree_late() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let (_, head) = send_to_a_workspace(&f);
    let expected = observe(&f.checkout(0));
    let failing = FailingEarlyAdds::for_this_thread();

    let transfer = f.transfer(true);
    drop(failing);

    transfer.result.expect("to local");
    // At the path that was recorded, and nowhere else.
    let worktree = worktree_on_feature_at(&f, &head);
    assert_eq!(f.worktree_dirs(), vec![worktree.clone()]);
    assert_eq!(observe(&worktree), expected);
    assert!(
        added_on_its_branch(&worktree),
        "{:?}",
        head_moves(&worktree)
    );
    assert!(!worktree.join("half-added").exists());
}

/// The inspection saw no LFS objects, so the early worktree was added with
/// the smudge filter on; by the snapshot the checkout has an LFS file. The
/// leg discards the early worktree and adds one the way a snapshot with LFS
/// objects needs: pointers first, for the finish to replace.
#[test]
fn a_wrong_guess_about_lfs_falls_back_to_adding_the_worktree_late() {
    if !lfs_installed("a_wrong_guess_about_lfs_falls_back_to_adding_the_worktree_late") {
        return;
    }
    let f = lfs_project();
    let (_, head) = send_to_a_workspace(&f);
    assert_eq!(f.preflight().repos[0].lfs_present, 0);
    let art = noise(1800, 41);
    let (source, content) = (f.checkout(0), art.clone());
    let seen = hold_the_snapshot_for(&f, &worktree_path(&f));
    f.host.before(Side::Workspace, SNAPSHOT_SCRIPT, 1, move || {
        track_psd(&source);
        write(&source, "art/new.psd", &content);
    });

    let result = f.transfer(true).result.expect("to local");

    assert_eq!(*seen.lock().unwrap(), vec![true], "an early worktree");
    let worktree = worktree_on_feature_at(&f, &head);
    assert!(
        added_on_its_branch(&worktree),
        "{:?}",
        head_moves(&worktree)
    );
    let actual = fs::read(worktree.join("art/new.psd")).unwrap();
    assert!(!actual.starts_with(POINTER_PREFIX), "a pointer");
    assert!(actual == art, "other content");
    assert!(
        !codes(&result.repos[0].warnings)
            .iter()
            .any(|c| c.starts_with("lfs")),
        "{:?}",
        result.repos[0].warnings
    );
}
