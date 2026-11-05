//! Ignored files through the whole transfer: backed up as every source comes
//! down, carried when asked, and what stops either.

use super::super::backups::ManifestIgnored;
use super::super::scripts::IGNORED_ARCHIVE_SCRIPT;
use super::*;

fn with_ignored() -> TransferOptions {
    TransferOptions {
        include_uncommitted: true,
        include_ignored: true,
    }
}

/// The ignored files a transfer's backup kept for `bundle_stem`, unpacked.
fn backed_up(f: &Fixture, transfer_id: &str, bundle_stem: &str) -> PathBuf {
    let archive = f
        .host
        .root
        .join("transfers")
        .join(transfer_id)
        .join(format!("{bundle_stem}.ignored.tar"));
    assert!(archive.is_file(), "no {}", archive.display());
    let into = f
        .host
        .root
        .join("unpacked")
        .join(transfer_id)
        .join(bundle_stem);
    fs::create_dir_all(&into).unwrap();
    let status = std::process::Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&into)
        .status()
        .unwrap();
    assert!(status.success());
    into
}

fn read(dir: &Path, path: &str) -> Option<String> {
    fs::read_to_string(dir.join(path)).ok()
}

#[test]
fn ignored_files_are_backed_up_as_each_source_comes_down() {
    let f = two_repo_project();
    let widgets = f.worktree(0);
    write(&widgets, "logs/today.log", "today\n");
    write(&widgets, "node_modules/pkg/x.log", "never\n");
    write(&widgets, ".gitignore", "*.log\nnode_modules/\n");
    let gadgets = f.worktree(1);
    write(&gadgets, "gadget.log", "gadget\n");

    let preflight = f.preflight();
    // debug.log (from the mess) and logs/today.log.
    assert_eq!(preflight.repos[0].ignored_files, 2);
    assert_eq!(
        preflight.repos[0].ignored_bytes,
        ("ignored, must stay behind\n".len() + "today\n".len()) as u64
    );
    assert_eq!(preflight.repos[0].ignored_dirs, 1);
    assert_eq!(preflight.repos[1].ignored_files, 1);

    let to_remote = f.transfer(true);
    let result = to_remote.result.expect("to remote");
    let tid = &to_remote.transfer_id;
    for repo in &result.repos {
        assert!(
            !codes(&repo.warnings)
                .iter()
                .any(|c| c.starts_with("ignored")),
            "{:?}",
            repo.warnings
        );
    }
    // Not carried...
    let clone = f.checkout(0);
    assert!(!clone.join("debug.log").exists());
    assert!(!clone.join("logs/today.log").exists());
    // ...but kept, and the worktrees went.
    assert!(!widgets.exists() && !gadgets.exists());
    let kept = backed_up(&f, tid, "0-widgets");
    assert_eq!(
        read(&kept, "debug.log").as_deref(),
        Some("ignored, must stay behind\n")
    );
    assert_eq!(read(&kept, "logs/today.log").as_deref(), Some("today\n"));
    assert!(!kept.join("node_modules").exists());
    let kept = backed_up(&f, tid, "1-gadgets");
    assert_eq!(read(&kept, "gadget.log").as_deref(), Some("gadget\n"));
    let manifest = f.manifest(tid).unwrap();
    assert!(!manifest.include_ignored);
    assert_eq!(
        manifest.repos[0].ignored,
        Some(ManifestIgnored {
            archive: Some("0-widgets.ignored.tar".to_string()),
            files: 2,
            bytes: preflight.repos[0].ignored_bytes,
            over_cap: false,
            placed: 0,
        })
    );

    // And back: the workspace's ignored files come down to the backup before
    // the workspace is deleted.
    write(&clone, "remote.log", "remote\n");
    let to_local = f.transfer(true);
    let result = to_local.result.expect("to local");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert!(f.host.workspaces().is_empty());
    assert!(!f.worktree(0).join("remote.log").exists());
    let kept = backed_up(&f, &to_local.transfer_id, "0-widgets");
    assert_eq!(read(&kept, "remote.log").as_deref(), Some("remote\n"));
    let manifest = f.manifest(&to_local.transfer_id).unwrap();
    assert_eq!(manifest.repos[0].ignored.as_ref().map(|i| i.files), Some(1));
    // Gadgets' workspace clone had none, and says nothing of them.
    assert_eq!(manifest.repos[1].ignored, None);
}

#[test]
fn ignored_files_go_to_a_workspace_and_back_when_asked() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let worktree = f.worktree(0);
    write(&worktree, "secret.log", "s3cret\n");
    write(&worktree, "cache/deep/x.log", "x\n");
    make_a_mess(&worktree);
    let expected = observe(&worktree);

    let to_remote = f.transfer_options(with_ignored(), &|_| {});
    let result = to_remote.result.expect("to remote");
    assert!(
        result.repos[0].warnings.is_empty(),
        "{:?}",
        result.repos[0].warnings
    );
    let clone = f.checkout(0);
    // Ignored files don't show, so the checkout reads as it did.
    assert_eq!(observe(&clone), expected);
    for (path, content) in [
        ("secret.log", "s3cret\n"),
        ("cache/deep/x.log", "x\n"),
        ("debug.log", "ignored, must stay behind\n"),
    ] {
        assert_eq!(read(&clone, path).as_deref(), Some(content), "{path}");
    }
    let manifest = f.manifest(&to_remote.transfer_id).unwrap();
    assert!(manifest.include_ignored);
    let ignored = manifest.repos[0].ignored.clone().unwrap();
    assert_eq!((ignored.files, ignored.placed), (3, 3));
    assert!(ignored.archive.is_some(), "and kept in the backup");

    write(&clone, "cache/deep/x.log", "x, in the workspace\n");
    let to_local = f.transfer_options(with_ignored(), &|_| {});
    let result = to_local.result.expect("to local");
    assert!(
        result.repos[0].warnings.is_empty(),
        "{:?}",
        result.repos[0].warnings
    );
    let worktree = f.worktree(0);
    assert_eq!(observe(&worktree), expected);
    assert_eq!(
        read(&worktree, "cache/deep/x.log").as_deref(),
        Some("x, in the workspace\n")
    );
    assert_eq!(read(&worktree, "secret.log").as_deref(), Some("s3cret\n"));
    let manifest = f.manifest(&to_local.transfer_id).unwrap();
    assert_eq!(
        manifest.repos[0].ignored.as_ref().map(|i| i.placed),
        Some(3)
    );
}

#[test]
fn an_ignored_file_already_at_the_destination_is_kept_and_said_so() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    write(&f.worktree(0), "secret.log", "ours\n");
    write(&f.worktree(0), "other.log", "other\n");
    f.host
        .bootstrap_with(|clone| write(clone, "secret.log", "theirs\n"));

    let result = f
        .transfer_options(with_ignored(), &|_| {})
        .result
        .expect("to remote");

    let clone = f.checkout(0);
    assert_eq!(read(&clone, "secret.log").as_deref(), Some("theirs\n"));
    assert_eq!(read(&clone, "other.log").as_deref(), Some("other\n"));
    let warnings = &result.repos[0].warnings;
    assert_eq!(codes(warnings), vec!["ignored_not_placed"]);
    assert!(warnings[0].message.contains("secret.log"), "{warnings:?}");
}

#[test]
fn ignored_files_over_the_cap_stop_a_transfer_asked_to_carry_them_and_are_not_backed_up() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let worktree = f.worktree(0);
    write(&worktree, "big.log", "x".repeat(100));
    *f.host.ignored_cap.lock().unwrap() = Some(10);

    let preflight = f.preflight();
    assert!(preflight.can_transfer, "a warning, not a blocker");
    assert_eq!(
        codes(&preflight.repos[0].warnings),
        vec!["ignored_too_large"]
    );
    assert_eq!(preflight.repos[0].ignored_bytes, 100);

    let before = f.db_state();
    let asked = f.transfer_options(with_ignored(), &|_| {});
    let err = asked.result.unwrap_err();
    assert!(err.contains("Can't transfer the ignored files"), "{err}");
    assert_eq!(f.db_state(), before);
    assert!(worktree.join("big.log").exists());
    assert!(f.host.workspaces().is_empty());

    let result = f.transfer(true).result.expect("without them");
    let warnings = &result.repos[0].warnings;
    assert_eq!(
        codes(warnings),
        vec!["ignored_too_large", "ignored_backup_skipped"]
    );
    assert!(
        warnings[1].message.contains("1 ignored file ("),
        "{warnings:?}"
    );
    let transfer_dir = f.host.root.join("transfers").join(&result.transfer_id);
    assert!(!transfer_dir.join("0-widgets.ignored.tar").exists());
    let manifest = f.manifest(&result.transfer_id).unwrap();
    let ignored = manifest.repos[0].ignored.clone().unwrap();
    assert!(ignored.over_cap);
    assert_eq!(ignored.archive, None);
}

#[test]
fn a_source_whose_ignored_files_cant_be_backed_up_stays() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let worktree = f.worktree(0);
    write(&worktree, "secret.log", "s3cret\n");

    // The first archive run at this end is the backup: the transfer has
    // switched, but the worktree is kept.
    f.host
        .arm(Fault::Script(Side::Local, IGNORED_ARCHIVE_SCRIPT, 1));
    let result = f.transfer(true).result.expect("to remote");
    assert!(f.host.fired());
    assert_eq!(codes(&result.warnings), vec!["worktree_left"]);
    assert!(
        result.warnings[0].message.contains("ignored files"),
        "{:?}",
        result.warnings
    );
    assert_eq!(read(&worktree, "secret.log").as_deref(), Some("s3cret\n"));

    // From a workspace: it isn't deleted.
    write(&f.checkout(0), "remote.log", "remote\n");
    f.host
        .arm(Fault::Script(Side::Workspace, IGNORED_ARCHIVE_SCRIPT, 1));
    let result = f.transfer(true).result.expect("to local");
    assert!(f.host.fired());
    assert_eq!(codes(&result.warnings), vec!["workspace_left"]);
    assert!(
        result.warnings[0].message.contains("ignored files"),
        "{:?}",
        result.warnings
    );
    assert_eq!(f.host.workspaces(), vec![WORKSPACE.to_string()]);
    assert!(!f.host.calls().iter().any(|c| c.starts_with("delete")));
}

/// Entries a transfer to local adds to a clone's `info/exclude` — for
/// ignored files only the workspace's own rules ignored — are recorded in
/// the manifest as the finish noted them, and a rollback takes exactly those
/// back out: the clone's other worktrees read that file too.
#[test]
fn a_rolled_back_transfer_to_local_takes_its_info_exclude_entries_back_out() {
    let f = two_repo_project();
    f.transfer_options(with_ignored(), &|_| {})
        .result
        .expect("to remote");
    let checkout = f.checkout(0);
    // Only the workspace's clone ignores it (a name no global excludes file
    // here should match, as the fixture's ends share this machine's).
    let mut rules = fs::read_to_string(checkout.join(".git/info/exclude")).unwrap_or_default();
    rules.push_str("only-in-the-workspace.txt\n");
    fs::write(checkout.join(".git/info/exclude"), rules).unwrap();
    write(&checkout, "only-in-the-workspace.txt", "pen\n");
    let exclude = f.host.clone_of(WIDGETS).join(".git/info/exclude");
    let before = fs::read(&exclude).unwrap_or_default();
    // The second repo's landing fails, after the first one finished.
    f.host.arm(Fault::Script(Side::Local, APPLY_SCRIPT, 3));
    let checked = Mutex::new(false);

    let failed = f.transfer_options(with_ignored(), &|progress| {
        if progress.phase == TransferPhase::Applying
            && progress.branch_id.as_deref() == Some(&f.branches[1].id)
        {
            let undo = f.manifest(&progress.transfer_id).unwrap().undo;
            let [added] = undo.excludes.as_slice() else {
                panic!("{:?}", undo.excludes);
            };
            assert!(added.file.is_some(), "{added:?}");
            assert_eq!(added.lines, vec!["/only-in-the-workspace.txt".to_string()]);
            let now = fs::read_to_string(&exclude).unwrap();
            assert!(
                now.lines().any(|l| l == "/only-in-the-workspace.txt"),
                "{now}"
            );
            *checked.lock().unwrap() = true;
        }
    });

    assert!(f.host.fired());
    assert!(*checked.lock().unwrap());
    assert!(failed.result.is_err());
    assert_eq!(
        fs::read(&exclude).unwrap_or_default(),
        before,
        "the clone's info/exclude is as it was"
    );
    let manifest = f.manifest(&failed.transfer_id).unwrap();
    assert_eq!(manifest.status, backups::ROLLED_BACK);

    // Through, it stays: the new worktree ignores what it was given.
    let done = f.transfer_options(with_ignored(), &|_| {});
    done.result.expect("to local");
    let worktree = f.worktree(0);
    assert_eq!(
        read(&worktree, "only-in-the-workspace.txt").as_deref(),
        Some("pen\n")
    );
    assert!(!git(
        &worktree,
        &["status", "--porcelain", "--untracked-files=all"]
    )
    .contains("only-in-the-workspace.txt"));
    let after = fs::read_to_string(&exclude).unwrap();
    assert!(
        after.lines().any(|l| l == "/only-in-the-workspace.txt"),
        "{after}"
    );
}

/// An ignored file only the workspace ignored, at a path another worktree of
/// the local clone has, stays out of the new worktree: ignoring it through
/// the shared `info/exclude` would hide that worktree's file too.
#[test]
fn an_ignored_file_another_local_worktree_has_is_not_placed_and_said_so() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    f.transfer_options(with_ignored(), &|_| {})
        .result
        .expect("to remote");
    let checkout = f.checkout(0);
    let mut rules = fs::read_to_string(checkout.join(".git/info/exclude")).unwrap_or_default();
    rules.push_str("only-in-the-workspace.txt\n");
    fs::write(checkout.join(".git/info/exclude"), rules).unwrap();
    write(&checkout, "only-in-the-workspace.txt", "workspace's\n");
    let clone = f.host.clone_of(WIDGETS);
    write(&clone, "only-in-the-workspace.txt", "the clone's own\n");
    let exclude = clone.join(".git/info/exclude");
    let before = fs::read(&exclude).unwrap_or_default();

    let result = f
        .transfer_options(with_ignored(), &|_| {})
        .result
        .expect("to local");

    let warnings = &result.repos[0].warnings;
    assert_eq!(codes(warnings), vec!["ignored_not_placed"]);
    assert!(
        warnings[0].message.contains("another worktree")
            && warnings[0].message.contains("only-in-the-workspace.txt"),
        "{warnings:?}"
    );
    assert_eq!(read(&f.worktree(0), "only-in-the-workspace.txt"), None);
    assert_eq!(fs::read(&exclude).unwrap_or_default(), before);
    assert!(git(&clone, &["status", "--porcelain"]).contains("?? only-in-the-workspace.txt"));
}

/// Past the retention, a transfer that left only ignored files behind keeps
/// just their archive and its manifest: its bundles and backup refs go like
/// any other's. An interrupted one whose bundles went is still dealt with
/// at the next start, and goes whole once it no longer holds them.
#[test]
fn past_the_retention_only_the_ignored_files_left_behind_are_kept() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let worktree = f.worktree(0);
    write(&worktree, "secret.log", "s3cret\n");
    let to_remote = f.transfer(true);
    let result = to_remote.result.expect("to remote");
    let tid = &to_remote.transfer_id;
    let transfers = f.host.root.join("transfers");
    let dir = transfers.join(tid);
    let manifest = f.manifest(tid).unwrap();
    assert!(!manifest.keep_until_removed, "nothing uncommitted stayed");
    assert!(manifest.holds_left_behind_ignored(), "{:?}", manifest.repos);
    let backup_ref = result.repos[0].backup_ref.clone().unwrap();
    let clone = f.host.clone_of(WIDGETS);
    assert!(try_git(&clone, &["rev-parse", "-q", "--verify", &backup_ref]).is_ok());
    assert_eq!(f.bundles(tid), 1);
    let later =
        std::time::SystemTime::now() + backups::RETENTION + std::time::Duration::from_secs(60);

    let pruned = backups::prune(&transfers, backups::RETENTION, later);

    assert_eq!(pruned.trimmed, vec![dir.clone()]);
    assert!(pruned.removed.is_empty());
    assert_eq!(f.bundles(tid), 0);
    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "0-widgets.ignored.excludes")
        .collect();
    names.sort();
    assert_eq!(names, vec!["0-widgets.ignored.tar", backups::MANIFEST_FILE]);
    assert_eq!(
        read(&backed_up(&f, tid, "0-widgets"), "secret.log").as_deref(),
        Some("s3cret\n")
    );
    assert!(try_git(&clone, &["rev-parse", "-q", "--verify", &backup_ref]).is_err());
    let trimmed = f.manifest(tid).unwrap();
    assert!(trimmed.bundles_pruned_at.is_some());
    assert_eq!(trimmed.repos[0].backup_ref, None);

    // Cut off after the switch: marked, and still kept.
    let mut cut = trimmed.clone();
    cut.status = backups::COMMITTED.to_string();
    cut.owner_pid = Some(dead_pid());
    backups::write_manifest(&dir, &cut).unwrap();
    let recovered = recovery::recover(&f.store, &f.host, &recovery::is_running);
    assert!(
        matches!(recovered.as_slice(), [(id, Recovered::Marked(_))] if id == tid),
        "{recovered:?}"
    );
    let marked = f.manifest(tid).unwrap();
    assert_eq!(marked.status, backups::COMPLETED);
    assert_eq!(marked.bundles_pruned_at, trimmed.bundles_pruned_at);
    assert_eq!(
        backups::prune(&transfers, backups::RETENTION, later),
        backups::Pruned::default()
    );

    // Cut off before it, with nothing to undo: rolled back, so the source
    // still has the files and the rest goes too.
    let mut cut = marked;
    cut.status = backups::CARRYING.to_string();
    cut.direction = "toLocal".to_string();
    cut.undo = Undo::default();
    backups::write_manifest(&dir, &cut).unwrap();
    let recovered = recovery::recover(&f.store, &f.host, &recovery::is_running);
    assert_eq!(
        recovered,
        vec![(tid.clone(), Recovered::RolledBack { left: vec![] })]
    );
    assert_eq!(
        backups::prune(&transfers, backups::RETENTION, later).removed,
        vec![dir.clone()]
    );
    assert!(!dir.exists());
}
