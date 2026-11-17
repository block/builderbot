//! Coming to local, a branch's worktree is added while its capture from
//! the workspace still runs, then switched onto the branch once that has
//! landed — or, when anything about it goes wrong, discarded for a worktree
//! added the way it always was.
//!
//! A worktree that went the early way shows it in its `HEAD` reflog: one
//! added on the branch never moved from another commit.
//!
//! The shell-environment calls do nothing under test, so what these tests
//! see of them is which were made for the worktree, in what order, and
//! whether the worktree was there at the time.
//!
//! Whether a leg gets an early worktree, and whether the capture its add
//! starts is kept, depend on the `post-checkout` hooks git finds, and a
//! machine may have a hook directory of its own for every repo, or name a
//! hook in its config. So the git each transfer here runs reads no config
//! but the repos' own ([`own_config_only`]): a clone has the hooks a test
//! gives it, and otherwise the default hook directory with none in it.

use std::time::{Duration, Instant};

use super::super::orchestrator::{FailingEarlyAdds, GatedEarlyAdds, GitWithoutHookRun};
use super::super::tests::lfs::{lfs_installed, track_psd, POINTER_PREFIX};
use super::super::undo::AddedWorktree;
use super::lfs::lfs_project;
use super::*;
use crate::git::shell_env_calls::Watch;
use crate::git::OwnConfigOnly;

/// Keep the machine's git config out of the git `f`'s transfers run,
/// while what this returns lives.
fn own_config_only(f: &Fixture) -> OwnConfigOnly {
    OwnConfigOnly::under(&f.host.root)
}

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

/// Make the early add and the snapshot wait for each other (see
/// [`super::super::orchestrator::GatedEarlyAdds`]). Returns, per snapshot,
/// whether the add finished while it was held.
fn gate_the_early_add(f: &Fixture) -> (GatedEarlyAdds, Arc<Mutex<Vec<bool>>>) {
    let gated = GatedEarlyAdds::for_this_thread();
    let finished = Arc::new(Mutex::new(Vec::new()));
    let (gate, seen) = (gated.gate(), Arc::clone(&finished));
    f.host.on_every_script(move |side, script| {
        if side == Side::Workspace && script == SNAPSHOT_SCRIPT {
            let finished = gate.snapshot_started();
            seen.lock().unwrap().push(finished);
        }
    });
    (gated, finished)
}

/// Give the clone a `post-checkout` hook that logs its three arguments and
/// then runs `then`. Returns the log.
fn record_post_checkout(f: &Fixture, then: &str) -> PathBuf {
    let log = f.host.root.join("post-checkout.log");
    install_post_checkout(
        f,
        &format!(
            "#!/bin/sh\necho \"$1 $2 $3\" >> '{}'\n{then}\n",
            log.display()
        ),
    );
    log
}

/// Give the clone a hook directory holding only a `post-checkout` with
/// this content.
fn install_post_checkout(f: &Fixture, script: &str) {
    use std::os::unix::fs::PermissionsExt;

    let hooks = f.host.root.join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("post-checkout");
    fs::write(&hook, script).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    git(
        &f.host.clone_of(WIDGETS),
        &["config", "core.hooksPath", path(&hooks)],
    );
}

/// Put a `post-checkout` hook that logs its three arguments among the
/// files of the branch's worktree, in `.githooks`, for the next commit
/// there to take, and have the clone look for its hooks in that
/// directory of whichever worktree git runs in. Returns the log.
fn record_post_checkout_from_the_branch(f: &Fixture) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let log = f.host.root.join("post-checkout.log");
    let hooks = f.worktree(0).join(".githooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("post-checkout");
    fs::write(
        &hook,
        format!("#!/bin/sh\necho \"$1 $2 $3\" >> '{}'\n", log.display()),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    git(
        &f.host.clone_of(WIDGETS),
        &["config", "core.hooksPath", ".githooks"],
    );
    log
}

/// Name a `post-checkout` hook in the clone's config, one that logs its
/// three arguments each time git runs it. Returns the log, and whether
/// this git runs hooks that config names at all (git 2.54 and later).
fn record_post_checkout_named_in_config(f: &Fixture) -> (PathBuf, bool) {
    use std::os::unix::fs::PermissionsExt;

    let clone = f.host.clone_of(WIDGETS);
    let log = f.host.root.join("post-checkout.log");
    let script = f.host.root.join("record-post-checkout.sh");
    fs::write(
        &script,
        format!("#!/bin/sh\necho \"$1 $2 $3\" >> '{}'\n", log.display()),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    git(&clone, &["config", "hook.record.event", "post-checkout"]);
    git(
        &clone,
        &[
            "config",
            "hook.record.command",
            &format!("'{}'", path(&script)),
        ],
    );

    // Asked of the git the transfer will run, the way it will run it.
    let probe = ["hook", "run", "post-checkout", "--", "a", "probe", "run"];
    let runs = crate::git::cli_run(&clone, &probe).is_ok() && lines(&log) == ["a probe run"];
    let _ = fs::remove_file(&log);
    (log, runs)
}

fn lines(log: &Path) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// What `post-checkout` is told of a worktree new on a branch at `head`.
fn new_worktree_at(head: &str) -> String {
    format!("{} {head} 1", "0".repeat(head.len()))
}

/// The shell-environment calls made for `worktree`, in order, each with
/// whether the worktree existed then.
fn shell_env_calls(watch: &Watch, worktree: &Path) -> Vec<(&'static str, bool)> {
    watch
        .calls()
        .into_iter()
        .filter(|call| call.path == worktree)
        .map(|call| (call.what, call.existed))
        .collect()
}

/// The shell-environment calls made for `worktree`, in order, each with
/// whether the file `watch` notes was there by then.
fn shell_env_calls_noting(watch: &Watch, worktree: &Path) -> Vec<(&'static str, bool)> {
    watch
        .calls()
        .into_iter()
        .filter(|call| call.path == worktree)
        .map(|call| (call.what, call.noted))
        .collect()
}

/// A commit made in the workspace's checkout, which the clone has never
/// seen: the early worktree starts at what origin has, and the switch has
/// files to rewrite. `file` is one the commit adds.
fn commit_in_the_workspace(f: &Fixture, file: &str) -> String {
    let checkout = f.checkout(0);
    write(&checkout, file, "made remotely\n");
    commit(&checkout, "made remotely")
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

/// The add and the snapshot wait for each other: the add does nothing
/// until the snapshot has started, and the snapshot goes no further until
/// the add is done. So the transfer only gets through with the add running
/// inside the snapshot, on a thread of its own: an add made before the
/// capture would never see the snapshot start, and one made after it would
/// not finish while the snapshot waits. The workspace's head is new to the
/// clone, so the worktree starts at what origin has of the branch.
///
/// The clone has no `post-checkout` hook, so the capture the add started
/// is the one the finish uses.
#[test]
fn the_worktree_is_added_while_the_snapshot_runs() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    let (pushed, _) = send_to_a_workspace(&f);
    let checkout = f.checkout(0);
    let head = commit_in_the_workspace(&f, "remote.txt");
    write(
        &checkout,
        "partial.txt",
        "one\ntwo\nthree\nfour\nfive\nsix\n",
    );
    let expected = observe(&checkout);
    let worktree = worktree_path(&f);
    let watch = Watch::under(&f.host.root);
    let (gated, finished_in_the_snapshot) = gate_the_early_add(&f);

    f.transfer(true).result.expect("to local");

    assert_eq!(
        gated.gate().add_began_in_the_snapshot(),
        Some(true),
        "the add did not wait for the snapshot to start"
    );
    assert_eq!(
        *finished_in_the_snapshot.lock().unwrap(),
        vec![true],
        "the add did not finish while the snapshot was held"
    );
    assert_eq!(worktree_on_feature_at(&f, &head), worktree);
    assert_eq!(observe(&worktree), expected);
    assert!(
        switched_from(&worktree, &pushed),
        "{:?}",
        head_moves(&worktree)
    );
    assert_eq!(temp_refs(&worktree), "");
    // The capture started by the add is the one the finish waits for: no
    // hook runs, and nothing between the two commits changes what a
    // capture reads.
    assert_eq!(
        shell_env_calls(&watch, &worktree),
        [("invalidate", true), ("prewarm", true), ("wait", true)]
    );
}

/// `post-checkout` hears of the early worktree once, after it is on its
/// branch, and is told what git tells it of a worktree added on a branch:
/// no previous head. Neither the detached add nor the switch from the start
/// commit reaches it.
#[test]
fn post_checkout_fires_once_for_an_early_worktree_as_for_a_new_one() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    if !crate::git::can_run_hooks_on_request(&f.host.clone_of(WIDGETS)) {
        eprintln!("skipping: this git has no `git hook run`");
        return;
    }
    let (pushed, _) = send_to_a_workspace(&f);
    let head = commit_in_the_workspace(&f, "remote.txt");
    let log = record_post_checkout(&f, "");

    f.transfer(true).result.expect("to local");

    let worktree = worktree_on_feature_at(&f, &head);
    assert!(
        switched_from(&worktree, &pushed),
        "{:?}",
        head_moves(&worktree)
    );
    assert_eq!(lines(&log), [new_worktree_at(&head)]);
}

/// The commonest case: the clone already holds the head, so the worktree
/// is added at it and the switch moves no file. The hook is still told the
/// worktree is new, not that it went from the head to the head.
#[test]
fn post_checkout_sees_a_new_worktree_when_the_start_commit_is_the_head() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    if !crate::git::can_run_hooks_on_request(&f.host.clone_of(WIDGETS)) {
        eprintln!("skipping: this git has no `git hook run`");
        return;
    }
    let (_, head) = send_to_a_workspace(&f);
    let log = record_post_checkout(&f, "");

    f.transfer(true).result.expect("to local");

    let worktree = worktree_on_feature_at(&f, &head);
    assert!(
        switched_from(&worktree, &head),
        "{:?}",
        head_moves(&worktree)
    );
    assert_eq!(lines(&log), [new_worktree_at(&head)]);
}

/// A `post-checkout` hook may change what a shell picks up in the worktree,
/// so a capture made before it runs is no use to the finish. The clone's
/// hook directory is an absolute one, the same from every worktree, so the
/// hook is known before the add: the worktree is still added early, but no
/// capture is started in it, and the finish's is the first, after the
/// hook. The leg still asks for any capture to be waited out before the
/// hook fires, and finds none.
#[test]
fn a_post_checkout_hook_known_before_the_add_starts_no_early_capture() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    if !crate::git::can_run_hooks_on_request(&f.host.clone_of(WIDGETS)) {
        eprintln!("skipping: this git has no `git hook run`");
        return;
    }
    let (_, head) = send_to_a_workspace(&f);
    let log = record_post_checkout(&f, "");
    let worktree = worktree_path(&f);
    let watch = Watch::under(&f.host.root).noting(&log);

    f.transfer(true).result.expect("to local");

    assert_eq!(worktree_on_feature_at(&f, &head), worktree);
    assert!(
        switched_from(&worktree, &head),
        "{:?}",
        head_moves(&worktree)
    );
    assert_eq!(lines(&log), [new_worktree_at(&head)]);
    assert_eq!(
        shell_env_calls(&watch, &worktree),
        [
            ("invalidate", true),
            ("wait out and invalidate", true),
            ("wait", true)
        ]
    );
    // Each call with whether the hook had run by then.
    assert_eq!(
        shell_env_calls_noting(&watch, &worktree),
        [
            ("invalidate", false),
            ("wait out and invalidate", false),
            ("wait", true)
        ]
    );
}

/// A relative `core.hooksPath` makes the hook the branch's own file, which
/// the clone cannot know of before the worktree is on its branch, so the
/// early add starts its capture. Once the hook is found, that capture is
/// waited out and dropped before the hook fires, and the finish captures
/// after it: the order a worktree added late has, with no shell of an
/// earlier capture in the directory while the hook works. The clone holds
/// the head, so the two commits are the same and the hook is the only
/// reason.
#[test]
fn a_branchs_own_post_checkout_hook_runs_after_the_early_capture_and_before_the_finishs() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    if !crate::git::can_run_hooks_on_request(&f.host.clone_of(WIDGETS)) {
        eprintln!("skipping: this git has no `git hook run`");
        return;
    }
    let log = record_post_checkout_from_the_branch(&f);
    let (_, head) = send_to_a_workspace(&f);
    let worktree = worktree_path(&f);
    let watch = Watch::under(&f.host.root).noting(&log);

    f.transfer(true).result.expect("to local");

    assert_eq!(worktree_on_feature_at(&f, &head), worktree);
    assert!(
        switched_from(&worktree, &head),
        "{:?}",
        head_moves(&worktree)
    );
    assert_eq!(lines(&log), [new_worktree_at(&head)]);
    assert_eq!(
        shell_env_calls(&watch, &worktree),
        [
            ("invalidate", true),
            ("prewarm", true),
            ("wait out and invalidate", true),
            ("wait", true)
        ]
    );
    // Each call with whether the hook had run by then.
    assert_eq!(
        shell_env_calls_noting(&watch, &worktree),
        [
            ("invalidate", false),
            ("prewarm", false),
            ("wait out and invalidate", false),
            ("wait", true)
        ]
    );
}

/// Turning the hook directory off, as the early add and the switch do,
/// does not quiet a `post-checkout` hook that config names: git would run
/// it at the add, told of the start commit, at the switch, told the
/// worktree came from there, and a third time on request. So a clone whose
/// config names one gets no early worktree. The worktree is added on the
/// branch once that has landed, and the hook hears of it once, as new.
///
/// The workspace's head is new to the clone, so an early worktree would
/// have started at another commit and had files to switch.
#[test]
fn a_post_checkout_hook_named_in_config_fires_once_for_a_worktree_added_late() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    send_to_a_workspace(&f);
    let head = commit_in_the_workspace(&f, "remote.txt");
    let expected = observe(&f.checkout(0));
    let (log, git_runs_it) = record_post_checkout_named_in_config(&f);
    let watch = Watch::under(&f.host.root);

    f.transfer(true).result.expect("to local");

    let worktree = worktree_on_feature_at(&f, &head);
    assert_eq!(observe(&worktree), expected);
    assert!(
        added_on_its_branch(&worktree),
        "{:?}",
        head_moves(&worktree)
    );
    // Nothing was started ahead of the finish, in a worktree that was not
    // there yet.
    assert_eq!(
        shell_env_calls(&watch, &worktree),
        [("invalidate", false), ("wait", true)]
    );
    if git_runs_it {
        assert_eq!(lines(&log), [new_worktree_at(&head)]);
    } else {
        // The plan is made from config, whatever this git does with it.
        eprintln!("this git runs no hooks named in config: only the late add was checked");
        assert_eq!(lines(&log), [""; 0]);
    }
}

/// A clone with no `post-checkout` hook keeps the capture the early add
/// started, in the commonest case too: the clone holds the head, the switch
/// moves no file, and nothing fires.
#[test]
fn without_a_post_checkout_hook_the_early_capture_serves_the_finish() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    let (_, head) = send_to_a_workspace(&f);
    let worktree = worktree_path(&f);
    let watch = Watch::under(&f.host.root);

    f.transfer(true).result.expect("to local");

    assert_eq!(worktree_on_feature_at(&f, &head), worktree);
    assert!(
        switched_from(&worktree, &head),
        "{:?}",
        head_moves(&worktree)
    );
    assert_eq!(
        shell_env_calls(&watch, &worktree),
        [("invalidate", true), ("prewarm", true), ("wait", true)]
    );
}

/// A hook that is there but not executable is one git ignores, so it
/// costs nothing either.
#[test]
fn a_post_checkout_git_would_ignore_keeps_the_early_capture() {
    use std::os::unix::fs::PermissionsExt;

    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    let (_, head) = send_to_a_workspace(&f);
    let log = record_post_checkout(&f, "");
    let hook = f.host.root.join("hooks").join("post-checkout");
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o644)).unwrap();
    let worktree = worktree_path(&f);
    let watch = Watch::under(&f.host.root);

    f.transfer(true).result.expect("to local");

    assert_eq!(worktree_on_feature_at(&f, &head), worktree);
    assert_eq!(lines(&log), [""; 0], "the hook ran");
    assert_eq!(
        shell_env_calls(&watch, &worktree),
        [("invalidate", true), ("prewarm", true), ("wait", true)]
    );
}

/// The hook `git lfs install` writes only sets the write flag of lockable
/// files, so a clone whose one hook is that keeps the capture the early
/// add started, and the hook still fires.
#[test]
fn the_stock_git_lfs_post_checkout_keeps_the_early_capture() {
    if !lfs_installed("the_stock_git_lfs_post_checkout_keeps_the_early_capture") {
        return;
    }
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    if !crate::git::can_run_hooks_on_request(&f.host.clone_of(WIDGETS)) {
        eprintln!("skipping: this git has no `git hook run`");
        return;
    }
    let (_, head) = send_to_a_workspace(&f);
    install_post_checkout(&f, crate::git::STOCK_GIT_LFS_POST_CHECKOUT);
    let worktree = worktree_path(&f);
    let watch = Watch::under(&f.host.root);

    f.transfer(true).result.expect("to local");

    assert_eq!(worktree_on_feature_at(&f, &head), worktree);
    assert!(
        switched_from(&worktree, &head),
        "{:?}",
        head_moves(&worktree)
    );
    assert_eq!(
        shell_env_calls(&watch, &worktree),
        [("invalidate", true), ("prewarm", true), ("wait", true)]
    );
}

/// A git that cannot fire a hook on request gets no early worktree: the
/// worktree is added on the branch once that has landed, and git fires the
/// hook itself, the same once and with the same arguments.
#[test]
fn a_git_that_cannot_fire_hooks_on_request_adds_the_worktree_late() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    send_to_a_workspace(&f);
    let head = commit_in_the_workspace(&f, "remote.txt");
    let expected = observe(&f.checkout(0));
    let log = record_post_checkout(&f, "");
    let watch = Watch::under(&f.host.root);
    let old_git = GitWithoutHookRun::for_this_thread();

    let transfer = f.transfer(true);
    drop(old_git);

    transfer.result.expect("to local");
    let worktree = worktree_on_feature_at(&f, &head);
    assert_eq!(observe(&worktree), expected);
    assert!(
        added_on_its_branch(&worktree),
        "{:?}",
        head_moves(&worktree)
    );
    assert_eq!(lines(&log), [new_worktree_at(&head)]);
    // What is cached for the path is dropped before the worktree is there,
    // as on the early path, and nothing is started ahead of the finish.
    assert_eq!(
        shell_env_calls(&watch, &worktree),
        [("invalidate", false), ("wait", true)]
    );
}

/// A `post-checkout` that fails fails the transfer, as it does when it
/// fails `git worktree add`. It is not taken for a reason to fall back,
/// which would fire it a second time.
#[test]
fn a_post_checkout_that_fails_fails_the_transfer_having_fired_once() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    if !crate::git::can_run_hooks_on_request(&f.host.clone_of(WIDGETS)) {
        eprintln!("skipping: this git has no `git hook run`");
        return;
    }
    send_to_a_workspace(&f);
    let head = commit_in_the_workspace(&f, "remote.txt");
    let source = source_fingerprint(&f.checkout(0));
    let log = record_post_checkout(&f, "echo no good >&2\nexit 1");

    let transfer = f.transfer(true);

    let err = transfer.result.unwrap_err();
    assert!(err.contains("Cannot add the worktree for feature"), "{err}");
    assert!(err.contains("no good"), "{err}");
    assert_eq!(lines(&log), [new_worktree_at(&head)]);
    assert!(f.worktree_dirs().is_empty(), "{:?}", f.worktree_dirs());
    assert_eq!(source_fingerprint(&f.checkout(0)), source);
    let manifest = f.manifest(&transfer.transfer_id).expect("manifest");
    assert_eq!(manifest.status, "rolledBack");
}

/// The capture the early add started ran at the start commit. When the
/// branch's head differs from it in something a capture reads, that capture
/// is waited out and dropped before the finish asks for a new one, so two
/// are never running in the worktree and none is left untracked. No hook
/// is needed for that.
#[test]
fn a_stale_capture_is_waited_out_before_the_finish_captures_again() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    let (pushed, _) = send_to_a_workspace(&f);
    let head = commit_in_the_workspace(&f, ".nvmrc");
    let watch = Watch::under(&f.host.root);

    f.transfer(true).result.expect("to local");

    let worktree = worktree_on_feature_at(&f, &head);
    assert!(
        switched_from(&worktree, &pushed),
        "{:?}",
        head_moves(&worktree)
    );
    assert_eq!(
        shell_env_calls(&watch, &worktree),
        [
            ("invalidate", true),
            ("prewarm", true),
            ("wait out and invalidate", true),
            ("wait", true)
        ]
    );
}

/// A switch that fails after the early add went well: an untracked file
/// turned up in the worktree where the branch has one. The leg falls back,
/// and the capture the early add started is waited out while the worktree
/// is still there, before it is removed and added again on the branch.
#[test]
fn a_switch_that_fails_waits_out_the_capture_before_the_worktree_is_replaced() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    send_to_a_workspace(&f);
    let head = commit_in_the_workspace(&f, "remote.txt");
    let expected = observe(&f.checkout(0));
    let worktree = worktree_path(&f);
    let watch = Watch::under(&f.host.root);
    let gated = GatedEarlyAdds::for_this_thread();
    let (gate, early) = (gated.gate(), worktree.clone());
    f.host.on_every_script(move |side, script| {
        if side == Side::Workspace && script == SNAPSHOT_SCRIPT {
            assert!(gate.snapshot_started(), "no early worktree");
            write(&early, "remote.txt", "in the way\n");
            write(&early, "scratch.txt", "left by something else\n");
        }
    });

    f.transfer(true).result.expect("to local");

    assert_eq!(worktree_on_feature_at(&f, &head), worktree);
    assert_eq!(f.worktree_dirs(), vec![worktree.clone()]);
    assert!(
        added_on_its_branch(&worktree),
        "{:?}",
        head_moves(&worktree)
    );
    assert_eq!(observe(&worktree), expected);
    assert!(!worktree.join("scratch.txt").exists());
    assert_eq!(
        shell_env_calls(&watch, &worktree),
        [
            ("invalidate", true),
            ("prewarm", true),
            ("wait out and invalidate", true),
            ("wait", true)
        ]
    );
}

/// The worktree is in the undo list before it is made, which is now before
/// the snapshot: a transfer cut off anywhere after has it to remove.
#[test]
fn the_undo_list_names_the_worktree_before_the_snapshot_starts() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
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
/// the add, and the rollback removes what it made. The capture the add
/// started in the worktree is waited out first, while the worktree is
/// still there, and nothing asks for another.
#[test]
fn a_snapshot_that_fails_after_the_early_add_leaves_no_worktree() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let _own_config = own_config_only(&f);
    send_to_a_workspace(&f);
    let clone = f.host.clone_of(WIDGETS);
    let (db, before) = (f.db_state(), clone_state(&clone));
    let source = source_fingerprint(&f.checkout(0));
    let watch = Watch::under(&f.host.root);
    let seen = hold_the_snapshot_for(&f, &worktree_path(&f));
    f.host
        .arm(Fault::Script(Side::Workspace, SNAPSHOT_SCRIPT, 1));

    let transfer = f.transfer(true);

    let err = transfer.result.unwrap_err();
    assert!(f.host.fired(), "{err}");
    assert_eq!(*seen.lock().unwrap(), vec![true], "the worktree was there");
    assert!(f.worktree_dirs().is_empty(), "{:?}", f.worktree_dirs());
    assert_eq!(
        shell_env_calls(&watch, &worktree_path(&f)),
        [
            ("invalidate", true),
            ("prewarm", true),
            ("wait out and invalidate", true)
        ]
    );
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
    let _own_config = own_config_only(&f);
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
    let _own_config = own_config_only(&f);
    let (_, head) = send_to_a_workspace(&f);
    let expected = observe(&f.checkout(0));
    let watch = Watch::under(&f.host.root);
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
    // No capture was started in what the add left, so the removal had
    // nothing to wait for; it still asks, with the directory there.
    assert_eq!(
        shell_env_calls(&watch, &worktree),
        [("wait out and invalidate", true), ("wait", true)]
    );
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
    let _own_config = own_config_only(&f);
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
