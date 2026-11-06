//! Git LFS objects carried beside the bundle, against a real `git lfs`. Each
//! test skips, saying so, when git-lfs isn't installed.
//!
//! The scripts run with a git config of the test's own, so the destination is
//! what Blox is: git-lfs installed, but no LFS filter configured anywhere.

use super::*;

/// Whether `git lfs` runs here; `test` says what is skipped without it.
pub(in crate::project_transfer) fn lfs_installed(test: &str) -> bool {
    let installed = Command::new("git")
        .args(["lfs", "version"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !installed {
        eprintln!("skipping {test}: git-lfs is not on PATH");
    }
    installed
}

/// Script variables that keep the user's and the system's git config out:
/// whatever LFS filter they set up, the scripts don't see it.
pub(in crate::project_transfer) fn isolated_config(root: &Path) -> [(&'static str, String); 2] {
    let global = root.join("global.gitconfig");
    fs::write(&global, "").unwrap();
    [
        ("GIT_CONFIG_GLOBAL", global.to_str().unwrap().to_string()),
        ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
    ]
}

/// Set the LFS filter up in `repo` (not its hooks), and track `*.psd`.
pub(in crate::project_transfer) fn track_psd(repo: &Path) {
    git(repo, &["lfs", "install", "--local", "--skip-repo"]);
    write(
        repo,
        ".gitattributes",
        "*.psd filter=lfs diff=lfs merge=lfs -text\n",
    );
}

pub(in crate::project_transfer) const POINTER_PREFIX: &[u8] =
    b"version https://git-lfs.github.com/spec/v1\n";

/// Whether `repo`'s LFS store has the object for `content`.
pub(in crate::project_transfer) fn has_lfs_object(repo: &Path, content: &[u8]) -> bool {
    let oid = hex::encode(Sha256::digest(content));
    let common = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    Path::new(common.trim())
        .join("lfs/objects")
        .join(&oid[..2])
        .join(&oid[2..4])
        .join(&oid)
        .is_file()
}

/// A pointer to an object nobody has.
fn ghost_pointer() -> String {
    format!(
        "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize 4242\n",
        "6".repeat(64)
    )
}

/// The LFS files [`lfs_branch`] leaves, by path, with their content.
type Files = Vec<(&'static str, Vec<u8>)>;

/// `feature` at the source: an unpushed commit that starts tracking `*.psd`
/// and adds two, one then modified in the working tree; a new one staged;
/// and an untracked one. Returns each file's content as the working tree has
/// it, and the committed version of the modified one.
fn lfs_branch(w: &World) -> (Files, Vec<u8>) {
    track_psd(&w.source);
    git(&w.source, &["checkout", "-q", "-b", "feature"]);
    let committed = noise(3000, 11);
    let changed_before = noise(2000, 12);
    write(&w.source, "art/committed.psd", &committed);
    write(&w.source, "art/changed.psd", &changed_before);
    commit(&w.source, "art");
    let changed = noise(2500, 13);
    let staged = noise(1200, 14);
    let new = noise(1500, 15);
    write(&w.source, "art/changed.psd", &changed);
    write(&w.source, "art/staged.psd", &staged);
    git(&w.source, &["add", "art/staged.psd"]);
    write(&w.source, "art/new.psd", &new);
    // Only pointers were committed.
    let blob = git(&w.source, &["cat-file", "-p", "HEAD:art/committed.psd"]);
    assert!(blob.as_bytes().starts_with(POINTER_PREFIX), "{blob}");
    let files = vec![
        ("art/committed.psd", committed),
        ("art/changed.psd", changed),
        ("art/staged.psd", staged),
        ("art/new.psd", new),
    ];
    (files, changed_before)
}

fn assert_content(checkout: &Path, files: &Files) {
    for (path, content) in files {
        let actual = fs::read(checkout.join(path)).unwrap();
        assert!(
            !actual.starts_with(POINTER_PREFIX),
            "{path} is still a pointer"
        );
        assert!(actual == *content, "{path} has other content");
    }
}

impl World {
    /// An end like [`Self::endpoint`] that sees only the repositories' own
    /// git config.
    fn isolated(&self, scratch: &Path) -> LocalEndpoint {
        let [(k1, v1), (k2, v2)] = isolated_config(&self.root);
        self.endpoint(scratch).with_var(k1, &v1).with_var(k2, &v2)
    }

    /// [`Self::to_remote`] and [`Self::to_local`] at isolated ends, the
    /// worktree added the way the orchestrator adds it. Returns the
    /// checkout, and what finishing in it reported.
    fn carry_lfs(
        &self,
        to_remote: bool,
        include_uncommitted: bool,
    ) -> Result<(CarryOutcome, PathBuf, ApplyReport), TransferError> {
        let (source, dest, landing) = if to_remote {
            (
                self.isolated(&self.local_scratch),
                self.isolated(&self.remote_scratch),
                Landing::Checkout,
            )
        } else {
            (
                self.isolated(&self.remote_scratch),
                self.isolated(&self.local_scratch),
                Landing::Branch,
            )
        };
        let plan = self.origin_plan(
            &source,
            &dest,
            landing,
            ("feature", "feature"),
            include_uncommitted,
            &["main", "feature"],
        );
        let outcome = carry_repo(&source, &dest, &plan)?;
        if to_remote {
            let applied = outcome.applied.clone();
            return Ok((outcome, self.dest.clone(), applied));
        }
        let worktree = self.root.join("wt-feature");
        crate::git::create_worktree_for_existing_branch_without_lfs_smudge(
            &self.dest, "feature", &worktree,
        )
        .unwrap();
        let finished = finish(&dest, &plan, &outcome, worktree.to_str().unwrap())?;
        Ok((outcome, worktree, finished))
    }
}

#[test]
fn lfs_files_arrive_with_their_content_both_ways() {
    if !lfs_installed("lfs_files_arrive_with_their_content_both_ways") {
        return;
    }
    for shell in shells() {
        for to_remote in [true, false] {
            let what = format!("{shell}, to remote: {to_remote}");
            let w = World::new(shell);
            let (files, _) = lfs_branch(&w);

            let (outcome, checkout, finished) = w.carry_lfs(to_remote, true).unwrap();

            let lfs = outcome.snapshot.lfs.as_ref().expect("an LFS payload");
            // committed, changed (both versions), staged and new.
            assert_eq!(
                (lfs.counts.total, lfs.counts.present, lfs.counts.missing),
                (5, 5, 0),
                "{what}"
            );
            assert_eq!(
                lfs.counts.present_bytes,
                3000 + 2000 + 2500 + 1200 + 1500,
                "{what}"
            );
            assert!(outcome.dest_lfs.is_some(), "{what}");
            assert_eq!(
                (outcome.applied.lfs_missing, outcome.applied.lfs_unavailable),
                (0, false),
                "{what}"
            );
            assert!(
                finished.warnings.is_empty(),
                "{what}: {:?}",
                finished.warnings
            );
            assert_content(&checkout, &files);
            assert_same_checkout(&w.source, &checkout);
            assert!(!snapshot_ref_exists(&checkout), "{what}");
            // The destination is set up to clean and smudge LFS files itself.
            let filter = git(&checkout, &["config", "--local", "filter.lfs.process"]);
            assert!(filter.contains("git-lfs"), "{what}: {filter}");
            // The archive stays beside the bundle, on this machine either
            // way, as the backup.
            assert!(
                w.local_scratch
                    .join(TID)
                    .join("repo.bundle.lfs.tar")
                    .is_file(),
                "{what}"
            );
        }
    }
}

#[test]
fn the_preflight_counts_the_lfs_objects_a_snapshot_would_carry() {
    if !lfs_installed("the_preflight_counts_the_lfs_objects_a_snapshot_would_carry") {
        return;
    }
    let w = World::new("/bin/sh");
    lfs_branch(&w);
    write(&w.source, "ghost.psd", ghost_pointer());
    git(&w.source, &["add", "ghost.psd"]);
    let before = source_fingerprint(&w.source);

    let output = w
        .isolated(&w.local_scratch)
        .run_script(
            scripts::PREFLIGHT_SCRIPT,
            &scripts::preflight_args([(w.source.to_str().unwrap(), "feature")]),
        )
        .unwrap();

    assert_eq!(source_fingerprint(&w.source), before, "read only");
    let section = &scripts::sections(&output.stdout)[0];
    let counts = LfsCounts::from_report(section).expect("LFS counts");
    // HEAD's two and the staged one; the new versions are only hashed by a
    // snapshot, so they are counted as changed files.
    assert_eq!(
        (counts.present, counts.present_bytes),
        (3, 3000 + 2000 + 1200)
    );
    assert_eq!((counts.missing, counts.missing_bytes), (1, 4242));
    assert_eq!(section.get("lfs_changed"), Some("2"));
    assert_eq!(section.get("lfs_changed_bytes"), Some("4000"));
}

#[test]
fn lfs_objects_the_source_lacks_are_fetched_from_origin_if_it_has_them() {
    if !lfs_installed("lfs_objects_the_source_lacks_are_fetched_from_origin_if_it_has_them") {
        return;
    }
    for to_remote in [true, false] {
        let w = World::new("/bin/sh");
        // Someone else pushes an LFS file, objects and all.
        let other = w.other_clone("other");
        track_psd(&other);
        let pushed = noise(1800, 21);
        write(&other, "pushed.psd", &pushed);
        commit(&other, "pushed");
        git(&other, &["lfs", "push", "origin", "main"]);
        git(&other, &["push", "-q", "origin", "main"]);
        // The source has the pointer but never downloaded the object.
        git(
            &w.source,
            &["lfs", "install", "--local", "--skip-repo", "--skip-smudge"],
        );
        git(&w.source, &["pull", "-q", "--ff-only", "origin", "main"]);
        assert!(fs::read(w.source.join("pushed.psd"))
            .unwrap()
            .starts_with(POINTER_PREFIX));
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        let local = noise(900, 22);
        write(&w.source, "local.psd", &local);
        // And one that nobody has.
        write(&w.source, "ghost.psd", ghost_pointer());
        commit(&w.source, "local and ghost");

        let (outcome, checkout, finished) = w.carry_lfs(to_remote, true).unwrap();

        let counts = outcome.snapshot.lfs.as_ref().unwrap().counts;
        assert_eq!((counts.total, counts.present, counts.missing), (3, 1, 2));
        let applied = if to_remote {
            &outcome.applied
        } else {
            // Fetched while landing the branch, checked out by `finish`.
            assert_eq!(finished.lfs_missing, 0);
            &outcome.applied
        };
        assert_eq!(
            (applied.lfs_missing, applied.lfs_missing_bytes),
            (1, 4242),
            "only the ghost is still missing"
        );
        assert_content(
            &checkout,
            &vec![("pushed.psd", pushed.clone()), ("local.psd", local.clone())],
        );
        assert_eq!(
            fs::read_to_string(checkout.join("ghost.psd")).unwrap(),
            ghost_pointer()
        );
        assert_eq!(rev(&checkout, "HEAD"), outcome.snapshot.head);
        assert_eq!(git(&checkout, &["status", "--porcelain"]), "");
    }
}

#[test]
fn an_lfs_filter_the_destination_already_has_is_kept() {
    if !lfs_installed("an_lfs_filter_the_destination_already_has_is_kept") {
        return;
    }
    let w = World::new("/bin/sh");
    let (files, _) = lfs_branch(&w);
    // Set up not to smudge on checkout, which `git lfs checkout` does
    // regardless.
    git(
        &w.dest,
        &["lfs", "install", "--local", "--skip-repo", "--skip-smudge"],
    );
    git(&w.dest, &["config", "--local", "filter.lfs.x-mine", "kept"]);
    let filters = ["config", "--local", "--get-regexp", "^filter\\.lfs\\."];
    let before = git(&w.dest, &filters);
    assert!(before.contains("--skip"), "{before}");

    let (_, checkout, _) = w.carry_lfs(true, true).unwrap();

    assert_content(&checkout, &files);
    assert_eq!(git(&w.dest, &filters), before);
}

#[test]
fn leaving_uncommitted_changes_behind_still_checks_out_lfs_content() {
    if !lfs_installed("leaving_uncommitted_changes_behind_still_checks_out_lfs_content") {
        return;
    }
    for to_remote in [true, false] {
        let w = World::new("/bin/sh");
        let (files, changed_before) = lfs_branch(&w);

        let (_, checkout, _) = w.carry_lfs(to_remote, false).unwrap();

        assert_content(
            &checkout,
            &vec![
                ("art/committed.psd", files[0].1.clone()),
                ("art/changed.psd", changed_before.clone()),
            ],
        );
        assert!(!checkout.join("art/new.psd").exists());
        assert_eq!(git(&checkout, &["status", "--porcelain"]), "");
    }
}

/// `main` pushed with its LFS objects: `*.psd` tracked, and `art.psd` and
/// `gone.psd`. Then `feature` at the source, where the index changes both: a
/// new version of `art.psd` is staged, and `gone.psd` is removed with `git
/// rm`. Neither pushed version is in the index, the working tree or an
/// unpushed commit; only `HEAD` has them. Returns them as pushed.
fn pushed_lfs_files_the_index_changes(w: &World) -> Files {
    track_psd(&w.source);
    let art = noise(2200, 31);
    let gone = noise(1700, 32);
    write(&w.source, "art.psd", &art);
    write(&w.source, "gone.psd", &gone);
    commit(&w.source, "art");
    git(&w.source, &["lfs", "push", "origin", "main"]);
    git(&w.source, &["push", "-q", "origin", "main"]);
    git(&w.dest, &["fetch", "-q", "origin"]);
    git(&w.source, &["checkout", "-q", "-b", "feature"]);
    write(&w.source, "art.psd", noise(2400, 33));
    git(&w.source, &["add", "art.psd"]);
    git(&w.source, &["rm", "-q", "gone.psd"]);
    vec![("art.psd", art), ("gone.psd", gone)]
}

#[test]
fn leaving_uncommitted_changes_behind_checks_out_pushed_lfs_files_the_index_changed() {
    if !lfs_installed(
        "leaving_uncommitted_changes_behind_checks_out_pushed_lfs_files_the_index_changed",
    ) {
        return;
    }
    for shell in shells() {
        for to_remote in [true, false] {
            let what = format!("{shell}, to remote: {to_remote}");
            let w = World::new(shell);
            let files = pushed_lfs_files_the_index_changes(&w);
            let preflight = w
                .isolated(&w.local_scratch)
                .run_script(
                    scripts::PREFLIGHT_SCRIPT,
                    &scripts::preflight_args([(w.source.to_str().unwrap(), "feature")]),
                )
                .unwrap();

            let (outcome, checkout, finished) = w.carry_lfs(to_remote, false).unwrap();

            // HEAD's two, though no unpushed commit has them, and the staged
            // version, which stays in the backup.
            let counts = outcome
                .snapshot
                .lfs
                .as_ref()
                .expect("an LFS payload")
                .counts;
            assert_eq!(
                (counts.total, counts.present, counts.missing),
                (3, 3, 0),
                "{what}"
            );
            let section = &scripts::sections(&preflight.stdout)[0];
            assert_eq!(
                LfsCounts::from_report(section).map(|c| (c.present, c.missing)),
                Some((3, 0)),
                "{what}: the preflight counts the same"
            );
            assert_eq!(
                (outcome.applied.lfs_missing, finished.lfs_missing),
                (0, 0),
                "{what}"
            );
            assert!(
                finished.warnings.is_empty(),
                "{what}: {:?}",
                finished.warnings
            );
            assert_content(&checkout, &files);
            assert_eq!(rev(&checkout, "HEAD"), outcome.snapshot.head, "{what}");
            assert_eq!(git(&checkout, &["status", "--porcelain"]), "", "{what}");
        }
    }
}

/// What the checkout uses is what has to arrive: `HEAD`'s objects when the
/// uncommitted changes stay behind, the working tree's when they come. One
/// the source never downloaded is fetched from origin only when the checkout
/// uses it.
#[test]
fn lfs_objects_the_checkout_uses_are_fetched_whichever_changes_come() {
    if !lfs_installed("lfs_objects_the_checkout_uses_are_fetched_whichever_changes_come") {
        return;
    }
    for to_remote in [true, false] {
        for include_uncommitted in [false, true] {
            let what = format!("to remote: {to_remote}, uncommitted: {include_uncommitted}");
            let w = World::new("/bin/sh");
            // Someone else pushes two LFS files, objects and all.
            let other = w.other_clone("other");
            track_psd(&other);
            let kept = noise(1300, 41);
            let gone = noise(1600, 42);
            write(&other, "kept.psd", &kept);
            write(&other, "gone.psd", &gone);
            commit(&other, "pushed");
            git(&other, &["lfs", "push", "origin", "main"]);
            git(&other, &["push", "-q", "origin", "main"]);
            git(&w.dest, &["fetch", "-q", "origin"]);
            // The source has the pointers but never downloaded the objects,
            // and stages one's removal.
            git(
                &w.source,
                &["lfs", "install", "--local", "--skip-repo", "--skip-smudge"],
            );
            git(&w.source, &["pull", "-q", "--ff-only", "origin", "main"]);
            git(&w.source, &["checkout", "-q", "-b", "feature"]);
            git(&w.source, &["rm", "-q", "gone.psd"]);

            let (outcome, checkout, finished) =
                w.carry_lfs(to_remote, include_uncommitted).unwrap();

            let counts = outcome.snapshot.lfs.as_ref().unwrap().counts;
            assert_eq!(
                (counts.total, counts.present, counts.missing),
                (2, 0, 2),
                "{what}"
            );
            assert_eq!(
                (outcome.applied.lfs_missing, finished.lfs_missing),
                (0, 0),
                "{what}"
            );
            if include_uncommitted {
                assert_content(&checkout, &vec![("kept.psd", kept.clone())]);
                assert!(!checkout.join("gone.psd").exists(), "{what}");
                assert!(!has_lfs_object(&checkout, &gone), "{what}: not used");
            } else {
                assert_content(
                    &checkout,
                    &vec![("kept.psd", kept.clone()), ("gone.psd", gone.clone())],
                );
                assert_eq!(git(&checkout, &["status", "--porcelain"]), "", "{what}");
            }
        }
    }
}

/// A source whose LFS archive loses an object the working tree uses on the
/// way, with the archive's size and checksum made to match what is left: one
/// in `W` and not `HEAD` when the uncommitted changes are restored, else one
/// in `HEAD` and not `W`.
struct DropsAnLfsObject {
    inner: LocalEndpoint,
    restore: bool,
}

fn tar(args: &[&str]) {
    let status = Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "tar {args:?}");
}

impl DropsAnLfsObject {
    fn tamper(&self, archive: &str, stdout: &str) -> String {
        let unpacked = PathBuf::from(format!("{archive}.unpacked"));
        fs::create_dir_all(&unpacked).unwrap();
        tar(&["-xf", archive, "-C", unpacked.to_str().unwrap()]);
        // `<oid> <size> <in W> <in HEAD> <at the source>`.
        let (used, unused) = if self.restore { (2, 3) } else { (3, 2) };
        let manifest = fs::read_to_string(unpacked.join("oids")).unwrap();
        let oid = manifest
            .lines()
            .map(|line| line.split(' ').collect::<Vec<_>>())
            .find(|fields| fields[used] == "1" && fields[unused] == "0" && fields[4] == "1")
            .expect("an object only the checkout uses")[0]
            .to_string();
        fs::remove_file(
            unpacked
                .join("objects")
                .join(&oid[..2])
                .join(&oid[2..4])
                .join(&oid),
        )
        .unwrap();
        fs::remove_file(archive).unwrap();
        tar(&[
            "-cf",
            archive,
            "-C",
            unpacked.to_str().unwrap(),
            "oids",
            "objects",
        ]);
        let bytes = fs::read(archive).unwrap();
        // Line by line, the trailing newline kept: a deferred bundle's
        // report is appended to this one.
        let mut out = stdout
            .lines()
            .map(|line| match line.split_once('=') {
                Some(("lfs_size", _)) => format!("lfs_size={}", bytes.len()),
                Some(("lfs_sha256", _)) => {
                    format!("lfs_sha256={}", hex::encode(Sha256::digest(&bytes)))
                }
                _ => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        out.push('\n');
        out
    }
}

impl TransferEndpoint for DropsAnLfsObject {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn run_script(&self, script: &str, args: &[String]) -> Result<ScriptOutput, TransferError> {
        let mut output = self.inner.run_script(script, args)?;
        if script == scripts::SNAPSHOT_SCRIPT && output.success {
            output.stdout = self.tamper(&format!("{}.lfs.tar", args[2]), &output.stdout);
        }
        Ok(output)
    }

    fn scratch_dir(&self, transfer_id: &str) -> Result<String, TransferError> {
        self.inner.scratch_dir(transfer_id)
    }

    fn put_file(&self, local: &Path, path: &str) -> Result<(), TransferError> {
        self.inner.put_file(local, path)
    }

    fn get_file(&self, path: &str, local: &Path, size: u64) -> Result<(), TransferError> {
        self.inner.get_file(path, local, size)
    }
}

#[test]
fn an_lfs_object_lost_on_the_way_fails_the_apply_before_the_branch_lands() {
    if !lfs_installed("an_lfs_object_lost_on_the_way_fails_the_apply_before_the_branch_lands") {
        return;
    }
    for shell in shells() {
        for (to_remote, restore) in [(true, true), (false, true), (true, false), (false, false)] {
            let what = format!("{shell}, to remote: {to_remote}, uncommitted: {restore}");
            let w = World::new(shell);
            lfs_branch(&w);
            let (source_scratch, dest_scratch, landing) = if to_remote {
                (&w.local_scratch, &w.remote_scratch, Landing::Checkout)
            } else {
                (&w.remote_scratch, &w.local_scratch, Landing::Branch)
            };
            let source = DropsAnLfsObject {
                inner: w.isolated(source_scratch),
                restore,
            };
            let dest = w.isolated(dest_scratch);
            let plan = w.plan(
                &source.inner,
                landing,
                ("feature", "feature"),
                restore,
                vec![],
            );
            let dest_before = source_fingerprint(&w.dest);

            let err = carry_repo(&source, &dest, &plan).unwrap_err();

            assert!(
                err.to_string()
                    .contains("1 LFS objects the working tree uses did not arrive"),
                "{what}: {err}"
            );
            assert!(
                try_git(&w.dest, &["rev-parse", "--verify", "refs/heads/feature"]).is_err(),
                "{what}"
            );
            let (observed, index, refs) = source_fingerprint(&w.dest);
            assert_eq!(observed, dest_before.0, "{what}");
            assert_eq!(index, dest_before.1, "{what}");
            // Only the snapshot ref, which the caller's cleanup removes.
            assert_eq!(refs, dest_before.2, "{what}");
        }
    }
}

/// A repository that doesn't use LFS, or only says it does, costs nothing
/// extra: no archive, no extra file to move, no extra script run.
#[test]
fn a_repo_without_lfs_objects_carries_no_lfs_payload() {
    for attributes in [false, true] {
        let w = World::new("/bin/sh");
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        if attributes {
            write(
                &w.source,
                ".gitattributes",
                "*.psd filter=lfs diff=lfs merge=lfs -text\n",
            );
        }
        write(&w.source, "a.txt", "changed\n");
        commit(&w.source, "change");
        write(&w.source, "notes.txt", "untracked\n");
        let source = Recording::new(w.endpoint(&w.local_scratch));
        let dest = Recording::new(w.endpoint(&w.remote_scratch));
        let plan = w.plan(
            &source.inner,
            Landing::Checkout,
            ("feature", "feature"),
            true,
            vec![rev(&w.dest, "origin/main")],
        );

        let outcome = carry_repo(&source, &dest, &plan).unwrap();

        assert!(outcome.snapshot.lfs.is_none(), "attributes: {attributes}");
        assert!(outcome.dest_lfs.is_none());
        fn names(runs: Vec<(&'static str, u64)>) -> Vec<&'static str> {
            runs.into_iter().map(|(n, _)| n).collect()
        }
        assert_eq!(names(source.runs()), ["snapshot"]);
        assert_eq!(names(dest.runs()), ["apply"]);
        assert_eq!(source.files(), ["get repo.bundle"]);
        assert_eq!(dest.files(), ["put repo.bundle"]);
        assert_same_checkout(&w.source, &w.dest);
        for scratch in [&w.local_scratch, &w.remote_scratch] {
            let names: Vec<_> = fs::read_dir(scratch.join(TID))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert!(names.iter().all(|n| !n.contains("lfs")), "{names:?}");
        }
    }
}
