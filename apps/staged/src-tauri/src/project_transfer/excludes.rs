//! The entries a transfer to local leaves in its clone's `info/exclude`, once
//! the transfer is over.
//!
//! A finish to local gives each ignored file only the source's own rules
//! ignored an anchored entry (`/<path>`) in the shared local clone's
//! `info/exclude`, so the worktree it made ignores the file too (see the
//! ignored-file placing in `scripts/ignored.rs`). Every worktree of the clone
//! reads that file, and it outlives the worktree: left there, an entry would
//! hide a file a later worktree has at the same path from `git add -A` and
//! from a transfer's snapshot. So a transfer that switched keeps them with
//! its repo in the manifest ([`WorktreeExcludes`]), and they are taken back
//! out once that worktree is gone — when a transfer back to remote removes
//! it, and when Staged removes it with its branch
//! ([`release_worktree_excludes`]) — or, for one that went some other way,
//! at the next start's prune ([`sweep`]). Taking them out is
//! [`undo::remove_exclude_lines`]: each line only while it is still there,
//! verbatim, and the file only when the transfer made it and nothing else is
//! left in it.
//!
//! An entry can outlive its own worktree's need for it, too. A later
//! transfer to local that places a file at the same path in another
//! worktree of the clone finds it ignored already, by this entry, and adds
//! none of its own; so, as the finish gives no entry to a path another
//! worktree has anything at, an entry isn't taken out while another
//! worktree of the clone has anything at its path. It stays recorded, its
//! worktree gone, and goes once nothing has: when Staged next removes a
//! worktree, or at a start's sweep, whichever sees it first.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::backups::{self, ManifestRepo, TransferManifest, MANIFEST_FILE};
use super::is_valid_transfer_id;
use super::undo::{self, AddedExcludes};

/// What a manifest holding [`WorktreeExcludes`] has in its JSON — and one
/// without has nowhere, as the field is left out when empty — so a manifest
/// without any is passed over unparsed.
const RECORD_KEY: &[u8] = b"\"addedExcludes\"";

/// Entries a transfer to local added to its clone's `info/exclude` for the
/// worktree it made, kept until that worktree goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeExcludes {
    /// The worktree they were added for.
    pub worktree: PathBuf,
    /// The `info/exclude` they went into: the clone's, shared by its
    /// worktrees.
    pub file: PathBuf,
    /// The transfer created the file.
    #[serde(default)]
    pub created: bool,
    /// The file's last line had no newline, and the transfer ended it before
    /// adding its own.
    #[serde(default)]
    pub terminated: bool,
    /// Each entry added, as written, that is still to come out.
    pub lines: Vec<String>,
}

impl WorktreeExcludes {
    /// `added`, as the finish's journal resolved it, for `worktree`; `None`
    /// when it added nothing.
    pub fn new(worktree: PathBuf, added: &AddedExcludes) -> Option<Self> {
        let file = added.file.clone()?;
        (!added.lines.is_empty()).then(|| Self {
            worktree,
            file,
            created: added.created,
            terminated: added.terminated,
            lines: added.lines.clone(),
        })
    }

    /// Take out the entries for paths none of `worktrees` — the clone's,
    /// as [`live_worktrees`] lists them — has anything at, other than the
    /// one these were added for, and keep the rest. Returns whether any came
    /// out.
    fn release(&mut self, worktrees: &[(PathBuf, Option<String>)]) -> Result<bool, String> {
        let others: Vec<&Path> = worktrees
            .iter()
            .map(|(path, _)| path.as_path())
            .filter(|path| !same_path(path, &self.worktree))
            .collect();
        let (held, free): (Vec<String>, Vec<String>) =
            self.lines.iter().cloned().partition(|line| {
                entry_path(line).is_some_and(|path| {
                    others
                        .iter()
                        .any(|w| std::fs::symlink_metadata(w.join(&path)).is_ok())
                })
            });
        if free.is_empty() {
            return Ok(false);
        }
        // The newline the transfer ended the file's last line with goes with
        // its last entry, not before.
        undo::remove_exclude_lines(
            &self.file,
            &free,
            self.created,
            self.terminated && held.is_empty(),
        )?;
        self.lines = held;
        Ok(true)
    }

    /// Whether these are `worktree`'s: the same path, or the same place
    /// through a symlink (`~/.staged` linked elsewhere, which git lists by
    /// its real path), whether or not it is still there.
    fn is_for(&self, worktree: &Path) -> bool {
        same_path(&self.worktree, worktree)
    }
}

/// Take out the entries any finished transfer under Staged's transfers
/// directory added for `worktree`, which is going. Best-effort and only
/// logged: see [`release_for_worktree`].
pub fn release_worktree_excludes(worktree: &Path) {
    let Some(transfers_dir) = crate::paths::transfers_dir() else {
        return;
    };
    for problem in release_for_worktree(&transfers_dir, worktree) {
        log::warn!(
            "[project_transfer] removing worktree {}: {problem}",
            worktree.display()
        );
    }
}

/// Take out the entries each completed transfer under `transfers_dir`
/// added for `worktree`, which has gone, but those another worktree of the
/// clone still has something at the path of (see the module documentation),
/// and clear each record left with none, so they aren't taken out twice.
/// A record whose entries couldn't be taken out stays, for a start's
/// [`sweep`] to try again. With the worktrees of the clone changed, the
/// entries other records kept for a worktree that went before are looked at
/// again too, and taken out once nothing needs them; a record whose
/// worktree moved is left for the sweep to follow. Returns what it couldn't
/// do for `worktree`, one sentence each (what it couldn't do for the others
/// is only logged); reads only the manifests that hold any record.
pub fn release_for_worktree(transfers_dir: &Path, worktree: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(transfers_dir) else {
        return problems;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_str().is_some_and(is_valid_transfer_id) {
            continue;
        }
        let dir = entry.path();
        let Some(mut manifest) = read_manifest_with_records(&dir) else {
            continue;
        };
        let mut changed = false;
        for repo in &mut manifest.repos {
            let Some(record) = &repo.added_excludes else {
                continue;
            };
            let gone = if record.is_for(worktree) {
                Gone::Removed
            } else if !record.worktree.exists() {
                Gone::Unseen(None)
            } else {
                continue;
            };
            let for_this = matches!(gone, Gone::Removed);
            match settle(repo, gone) {
                Ok(settled) => changed |= settled,
                Err(e) if for_this => problems.push(e),
                Err(e) => log::warn!("[project_transfer] {e}"),
            }
        }
        if changed {
            if let Err(e) = backups::write_manifest(&dir, &manifest) {
                problems.push(e);
            }
        }
    }
    problems
}

/// At the start's prune: take out the entries of each of `manifest`'s
/// records whose worktree is gone, but those another worktree of the clone
/// still has something at the path of, and clear a record left with none.
/// One whose worktree moved — a branch moved to another project takes its
/// worktree along — follows it instead: its branch is still checked out in
/// the clone, at a path that exists. That path is recorded as the store
/// names it, when one of `workdirs` (the store's) is that place, rather than
/// as git lists it. Returns whether the manifest changed, to be written.
pub fn sweep(manifest: &mut TransferManifest, workdirs: &[PathBuf]) -> bool {
    if manifest.status != backups::COMPLETED {
        return false;
    }
    let mut changed = false;
    for repo in &mut manifest.repos {
        let gone = repo
            .added_excludes
            .as_ref()
            .is_some_and(|record| !record.worktree.exists());
        if !gone {
            continue;
        }
        match settle(repo, Gone::Unseen(Some(workdirs))) {
            Ok(settled) => changed |= settled,
            Err(e) => log::warn!("[project_transfer] {e}"),
        }
    }
    changed
}

/// How a record's worktree came to be gone.
enum Gone<'a> {
    /// Staged just removed it.
    Removed,
    /// It isn't there, and it may have moved: with the store's workdir
    /// paths, the record follows it; without, it is left as it is.
    Unseen(Option<&'a [PathBuf]>),
}

/// Deal with `repo`'s record, whose worktree has gone as `gone` says: let go
/// of the entries no other worktree of the clone needs, and clear the
/// record once it has none left. Returns whether the record changed.
fn settle(repo: &mut ManifestRepo, gone: Gone<'_>) -> Result<bool, String> {
    let Some(record) = repo.added_excludes.as_mut() else {
        return Ok(false);
    };
    let worktrees = live_worktrees(repo.local_clone.as_deref(), &record.file).map_err(|e| {
        format!(
            "cannot tell what else needs the entries added for {}: {e}",
            record.worktree.display()
        )
    })?;
    if let Gone::Unseen(follow) = gone {
        let branch = Some(repo.dest_branch.as_str());
        if let Some((moved, _)) = worktrees.iter().find(|(_, on)| on.as_deref() == branch) {
            let Some(workdirs) = follow else {
                return Ok(false);
            };
            record.worktree = as_the_store_names(moved, workdirs);
            return Ok(true);
        }
    }
    let released = record.release(&worktrees)?;
    if record.lines.is_empty() {
        repo.added_excludes = None;
        return Ok(true);
    }
    Ok(released)
}

/// The worktrees of the clone at `clone` — or, without one, of the
/// repository whose `info/exclude` is `file` — that are there, its main
/// checkout among them, with the branch each has checked out. None at all
/// when the clone is gone.
fn live_worktrees(
    clone: Option<&str>,
    file: &Path,
) -> Result<Vec<(PathBuf, Option<String>)>, String> {
    // `<git dir>/info/exclude`, which git can list the worktrees from too.
    let clone = match clone {
        Some(clone) => Path::new(clone),
        None => match file.parent().and_then(Path::parent) {
            Some(git_dir) => git_dir,
            None => return Ok(Vec::new()),
        },
    };
    if !clone.exists() {
        return Ok(Vec::new());
    }
    let worktrees = crate::git::list_worktrees(clone).map_err(|e| e.to_string())?;
    Ok(worktrees
        .into_iter()
        .filter(|(path, _)| path.exists())
        .collect())
}

/// The path an entry the finish added is for: `/<path>`, with a backslash
/// before each character a pattern treats specially. `None` for a line
/// that isn't one.
fn entry_path(line: &str) -> Option<PathBuf> {
    let escaped = line.strip_prefix('/')?;
    let mut path = String::with_capacity(escaped.len());
    let mut chars = escaped.chars();
    while let Some(c) = chars.next() {
        path.push(if c == '\\' { chars.next()? } else { c });
    }
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Whether `a` and `b` are the same place: as written, or once whatever
/// part of each is there is resolved through its symlinks.
fn same_path(a: &Path, b: &Path) -> bool {
    a == b || resolved(a).is_some_and(|a| resolved(b) == Some(a))
}

/// `path`, its longest leading part that is there made canonical, so a path
/// that went resolves as it did while it was there.
fn resolved(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    let mut there = path;
    loop {
        if let Ok(real) = there.canonicalize() {
            return Some(rest.iter().rev().fold(real, |path, part| path.join(part)));
        }
        rest.push(there.file_name()?);
        there = there.parent()?;
    }
}

/// `path`, as git lists a worktree — by its real path — the way the store
/// names it: the one of `workdirs` that is the same place, if any.
fn as_the_store_names(path: &Path, workdirs: &[PathBuf]) -> PathBuf {
    workdirs
        .iter()
        .find(|workdir| same_path(workdir, path))
        .cloned()
        .unwrap_or_else(|| path.to_path_buf())
}

/// The manifest in `dir`, if it is a completed transfer's with any record;
/// `None`, without parsing it, when its JSON has none.
fn read_manifest_with_records(dir: &Path) -> Option<TransferManifest> {
    let bytes = std::fs::read(dir.join(MANIFEST_FILE)).ok()?;
    if !bytes.windows(RECORD_KEY.len()).any(|w| w == RECORD_KEY) {
        return None;
    }
    let manifest: TransferManifest = serde_json::from_slice(&bytes).ok()?;
    (manifest.status == backups::COMPLETED).then_some(manifest)
}

#[cfg(test)]
mod tests {
    use super::super::backups::{prune, read_manifest, write_manifest, ManifestRepo, RETENTION};
    use super::*;
    use std::time::{Duration, SystemTime};

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    fn now_ms(now: SystemTime) -> i64 {
        now.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    fn lines(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|e| e.to_string()).collect()
    }

    fn repo(clone: &Path, record: Option<WorktreeExcludes>) -> ManifestRepo {
        ManifestRepo {
            github_repo: "acme/widgets".to_string(),
            branch: "feature".to_string(),
            dest_branch: "feature".to_string(),
            bundle: "0-widgets.bundle".to_string(),
            head: "a".repeat(40),
            snapshot: "b".repeat(40),
            has_uncommitted_changes: false,
            local_clone: Some(clone.to_string_lossy().into_owned()),
            backup_ref: None,
            ignored: None,
            added_excludes: record,
        }
    }

    fn manifest(id: &str, created_at: i64, repos: Vec<ManifestRepo>) -> TransferManifest {
        TransferManifest {
            transfer_id: id.to_string(),
            project_id: "p".to_string(),
            direction: "toLocal".to_string(),
            created_at,
            include_uncommitted: true,
            include_ignored: true,
            status: backups::COMPLETED.to_string(),
            workspace_name: Some("stg-x".to_string()),
            repos,
            ..TransferManifest::default()
        }
    }

    fn record(worktree: &Path, file: &Path, entries: &[&str]) -> WorktreeExcludes {
        WorktreeExcludes {
            worktree: worktree.to_path_buf(),
            file: file.to_path_buf(),
            created: false,
            terminated: false,
            lines: lines(entries),
        }
    }

    /// A clone with an `info/exclude`, and the worktrees added to it.
    struct Clone {
        root: crate::test_utils::TempDir,
        /// Where the clone and its worktrees are: `root`, or a symlink to a
        /// directory in it.
        base: PathBuf,
        clone: PathBuf,
        exclude: PathBuf,
    }

    impl Clone {
        fn new(name: &str) -> Self {
            let root = crate::test_utils::TempDir::new(name);
            let base = root.path().to_path_buf();
            Self::at(root, base)
        }

        /// One under `<root>/staged`, a symlink to `<root>/real`: as a
        /// Staged whose `~/.staged` is linked elsewhere names it, while git
        /// lists its worktrees by their real paths.
        fn linked(name: &str) -> Self {
            let root = crate::test_utils::TempDir::new(name);
            let real = root.path().join("real");
            std::fs::create_dir_all(&real).unwrap();
            let base = root.path().join("staged");
            std::os::unix::fs::symlink(&real, &base).unwrap();
            Self::at(root, base)
        }

        fn at(root: crate::test_utils::TempDir, base: PathBuf) -> Self {
            let clone = base.join("clone");
            std::fs::create_dir_all(&clone).unwrap();
            let git = |args: &[&str]| crate::git::cli_run(&clone, args).unwrap();
            git(&["init", "-q", "-b", "main"]);
            git(&[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x",
            ]);
            let exclude = clone.join(".git/info/exclude");
            Self {
                root,
                base,
                clone,
                exclude,
            }
        }

        fn add_worktree(&self, name: &str, branch: &str) -> PathBuf {
            let path = self.base.join(name);
            crate::git::cli_run(
                &self.clone,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    branch,
                    path.to_str().unwrap(),
                ],
            )
            .unwrap();
            path
        }

        /// As `git worktree remove` does, leaving the branch.
        fn remove_worktree(&self, path: &Path) {
            crate::git::cli_run(
                &self.clone,
                &["worktree", "remove", "--force", path.to_str().unwrap()],
            )
            .unwrap();
        }

        fn transfers(&self) -> PathBuf {
            self.root.path().join("transfers")
        }

        fn exclude_text(&self) -> String {
            std::fs::read_to_string(&self.exclude).unwrap_or_default()
        }
    }

    /// The hook Staged's own worktree removal runs: the entries recorded for
    /// that worktree come out, once, with the lines anyone else wrote — and
    /// another live worktree's, a rolled-back transfer's and an unparsed
    /// legacy manifest's records — left as they were.
    #[test]
    fn removing_a_worktree_takes_out_only_the_entries_recorded_for_it() {
        let c = Clone::new("transfer-excludes-release");
        let gone = c.root.path().join("worktrees/feature");
        let other = c.add_worktree("other", "other");
        std::fs::write(
            &c.exclude,
            "# git ls-files --others --exclude-from=.git/info/exclude\n.penpal\n/a.txt\n\
             /b\\ c.txt\n/kept.txt\nmine-later\n",
        )
        .unwrap();
        let transfers = c.transfers();
        // No transfers at all: nothing to do, nothing said.
        assert!(release_for_worktree(&transfers, &gone).is_empty());

        let ours = manifest(
            "ours",
            0,
            vec![
                repo(
                    &c.clone,
                    Some(record(&gone, &c.exclude, &["/a.txt", "/b\\ c.txt"])),
                ),
                repo(&c.clone, Some(record(&other, &c.exclude, &["/kept.txt"]))),
            ],
        );
        write_manifest(&transfers.join("ours"), &ours).unwrap();
        // Rolled back: its rollback already took its entries out.
        let mut rolled_back = manifest(
            "rolled-back",
            0,
            vec![repo(
                &c.clone,
                Some(record(&gone, &c.exclude, &[".penpal"])),
            )],
        );
        rolled_back.status = backups::ROLLED_BACK.to_string();
        write_manifest(&transfers.join("rolled-back"), &rolled_back).unwrap();
        // From before the record existed.
        let mut legacy =
            serde_json::to_value(manifest("legacy", 0, vec![repo(&c.clone, None)])).unwrap();
        assert!(legacy["repos"][0].get("addedExcludes").is_none());
        legacy["repos"][0]
            .as_object_mut()
            .unwrap()
            .remove("ignored");
        let legacy_dir = transfers.join("legacy");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(
            legacy_dir.join(MANIFEST_FILE),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        // Not a transfer's directory, whatever it holds.
        let stray = transfers.join("not a transfer");
        write_manifest(&stray, &ours).unwrap();

        assert!(release_for_worktree(&transfers, &gone).is_empty());

        assert_eq!(
            c.exclude_text(),
            "# git ls-files --others --exclude-from=.git/info/exclude\n.penpal\n/kept.txt\n\
             mine-later\n"
        );
        let after = read_manifest(&transfers.join("ours")).unwrap();
        assert_eq!(after.repos[0].added_excludes, None);
        assert_eq!(after.repos[1].added_excludes, ours.repos[1].added_excludes);
        assert_eq!(
            read_manifest(&transfers.join("rolled-back")).unwrap().repos[0].added_excludes,
            rolled_back.repos[0].added_excludes
        );
        assert_eq!(
            std::fs::read(legacy_dir.join(MANIFEST_FILE)).unwrap(),
            serde_json::to_vec(&legacy).unwrap(),
            "a manifest without records isn't rewritten"
        );

        // Cleared: a second removal (or someone adding the line again by
        // hand) leaves the file be.
        std::fs::write(&c.exclude, "/a.txt\n/kept.txt\n").unwrap();
        assert!(release_for_worktree(&transfers, &gone).is_empty());
        assert_eq!(c.exclude_text(), "/a.txt\n/kept.txt\n");

        // A file the transfer made goes once nothing else is in it.
        let made = c.root.path().join("made/info/exclude");
        std::fs::create_dir_all(made.parent().unwrap()).unwrap();
        std::fs::write(&made, "/x\n").unwrap();
        let made_record = WorktreeExcludes {
            created: true,
            ..record(&other, &made, &["/x"])
        };
        let second = manifest("second", 0, vec![repo(&c.clone, Some(made_record))]);
        write_manifest(&transfers.join("second"), &second).unwrap();
        c.remove_worktree(&other);
        assert!(release_for_worktree(&transfers, &other).is_empty());
        assert!(!made.exists());
        // And every transfer's record for that worktree is released: the
        // first one's `/kept.txt` went too.
        assert_eq!(c.exclude_text(), "/a.txt\n");
        assert_eq!(
            read_manifest(&transfers.join("ours")).unwrap().repos[1].added_excludes,
            None
        );
        assert_eq!(
            read_manifest(&transfers.join("second")).unwrap().repos[0].added_excludes,
            None
        );
    }

    /// One that can't be taken out is said, and its record stays for the
    /// next start's sweep.
    #[test]
    fn a_record_that_cannot_be_released_stays() {
        let c = Clone::new("transfer-excludes-release-fails");
        let gone = c.root.path().join("worktrees/feature");
        // A directory where the file was: unreadable as a file.
        std::fs::remove_file(&c.exclude).unwrap();
        std::fs::create_dir_all(c.exclude.join("x")).unwrap();
        let transfers = c.transfers();
        let m = manifest(
            "blocked",
            0,
            vec![repo(&c.clone, Some(record(&gone, &c.exclude, &["/a"])))],
        );
        write_manifest(&transfers.join("blocked"), &m).unwrap();

        let problems = release_for_worktree(&transfers, &gone);

        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(
            read_manifest(&transfers.join("blocked")).unwrap().repos[0].added_excludes,
            m.repos[0].added_excludes
        );
    }

    /// The record round-trips through the manifest, a manifest from before
    /// it still reads, and prune never drops one whose worktree is still
    /// there: past the retention such a backup keeps just its manifest. One
    /// whose worktree went without Staged (removed by hand) has its entries
    /// taken out at the prune, and one whose worktree moved follows it.
    #[test]
    fn prune_keeps_a_live_worktrees_record_and_releases_a_gone_ones() {
        let c = Clone::new("transfer-excludes-prune");
        let live = c.add_worktree("live", "live");
        let moved_from = c.root.path().join("moved-from");
        let moved_to = c.add_worktree("moved-to", "moved");
        let gone = c.add_worktree("gone", "gone");
        crate::git::cli_run(
            &c.clone,
            &["worktree", "remove", "--force", gone.to_str().unwrap()],
        )
        .unwrap();
        std::fs::write(&c.exclude, "/live\n/moved\n/gone\nmine\n").unwrap();
        let transfers = c.transfers();
        let now = SystemTime::now();
        let old_ms = now_ms(now) - 30 * DAY_MS;
        let with = |worktree: &Path, line: &str, branch: &str| {
            let mut r = repo(&c.clone, Some(record(worktree, &c.exclude, &[line])));
            r.dest_branch = branch.to_string();
            r
        };
        let cases = [
            ("live", vec![with(&live, "/live", "live")]),
            ("moved", vec![with(&moved_from, "/moved", "moved")]),
            ("gone", vec![with(&gone, "/gone", "gone")]),
        ];
        for (id, repos) in &cases {
            let m = manifest(id, old_ms, repos.clone());
            let dir = transfers.join(id);
            write_manifest(&dir, &m).unwrap();
            for name in ["0-widgets.bundle", "0-widgets.info-exclude.added"] {
                std::fs::write(dir.join(name), b"data").unwrap();
            }
            let read = read_manifest(&dir).unwrap();
            assert_eq!(
                read.repos[0].added_excludes, m.repos[0].added_excludes,
                "{id}"
            );
            assert!(
                std::fs::read_to_string(dir.join(MANIFEST_FILE))
                    .unwrap()
                    .contains("\"addedExcludes\""),
                "{id}"
            );
        }
        // Without the field at all: reads, and prunes by its age as before.
        let mut legacy =
            serde_json::to_value(manifest("legacy", old_ms, vec![repo(&c.clone, None)])).unwrap();
        legacy["repos"][0]
            .as_object_mut()
            .unwrap()
            .remove("ignored");
        let legacy_dir = transfers.join("legacy");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(
            legacy_dir.join(MANIFEST_FILE),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        assert_eq!(
            read_manifest(&legacy_dir).unwrap().repos[0].added_excludes,
            None
        );

        let mut pruned = prune(&transfers, RETENTION, now, &[]);
        pruned.removed.sort();
        pruned.trimmed.sort();

        assert_eq!(
            pruned.removed,
            vec![transfers.join("gone"), legacy_dir.clone()]
        );
        assert_eq!(
            pruned.trimmed,
            vec![transfers.join("live"), transfers.join("moved")]
        );
        assert_eq!(c.exclude_text(), "/live\n/moved\nmine\n");
        for id in ["live", "moved"] {
            let dir = transfers.join(id);
            let names: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(names, vec![MANIFEST_FILE.to_string()], "{id}");
            assert!(read_manifest(&dir).unwrap().bundles_pruned_at.is_some());
        }
        let moved = read_manifest(&transfers.join("moved")).unwrap();
        let followed = &moved.repos[0].added_excludes.as_ref().unwrap().worktree;
        assert_eq!(
            followed.canonicalize().unwrap(),
            moved_to.canonicalize().unwrap()
        );

        // Kept, it is left alone at the next prune...
        let later = now + Duration::from_secs(60);
        assert_eq!(prune(&transfers, RETENTION, later, &[]), Default::default());
        // ...until Staged removes the worktree, which takes its entries out;
        // then the manifest goes too.
        assert!(release_for_worktree(&transfers, &live).is_empty());
        assert_eq!(c.exclude_text(), "/moved\nmine\n");
        assert_eq!(
            prune(&transfers, RETENTION, later, &[]).removed,
            vec![transfers.join("live")]
        );
    }

    /// A later transfer to local placed a file, in another worktree, at the
    /// path an earlier one's entry is for, found it ignored by that entry,
    /// and so added — and recorded — none of its own. Removing the first
    /// worktree leaves that entry, still recorded, so the second worktree's
    /// file stays ignored there; once that one goes too, the entry comes
    /// out, whether Staged removes it or a start's sweep finds it gone.
    #[test]
    fn an_entry_another_worktree_has_a_file_for_stays_until_that_one_goes() {
        for by_staged in [true, false] {
            let c = Clone::new("transfer-excludes-shared");
            let a = c.add_worktree("a", "a");
            let b = c.add_worktree("b", "b");
            // The first transfer ended `mine` before adding its two.
            std::fs::write(&c.exclude, "mine\n/.envrc\n/a\\ b.txt\n").unwrap();
            for (worktree, path) in [(&a, ".envrc"), (&a, "a b.txt"), (&b, ".envrc")] {
                std::fs::write(worktree.join(path), "x\n").unwrap();
            }
            let status = |worktree: &Path| {
                crate::git::cli_run(worktree, &["status", "--porcelain", "-uall"]).unwrap()
            };
            assert_eq!(status(&b), "");
            let transfers = c.transfers();
            let mut first = repo(
                &c.clone,
                Some(WorktreeExcludes {
                    terminated: true,
                    ..record(&a, &c.exclude, &["/.envrc", "/a\\ b.txt"])
                }),
            );
            first.dest_branch = "a".to_string();
            let id = "first";
            let created = now_ms(SystemTime::now());
            write_manifest(&transfers.join(id), &manifest(id, created, vec![first])).unwrap();
            let goes = |worktree: &Path| {
                c.remove_worktree(worktree);
                if by_staged {
                    assert!(release_for_worktree(&transfers, worktree).is_empty());
                } else {
                    prune(&transfers, RETENTION, SystemTime::now(), &[]);
                }
            };

            goes(&a);

            assert_eq!(
                c.exclude_text(),
                "mine\n/.envrc\n",
                "by Staged: {by_staged}"
            );
            assert_eq!(status(&b), "", "by Staged: {by_staged}");
            let held = read_manifest(&transfers.join(id)).unwrap().repos[0]
                .added_excludes
                .clone()
                .expect("still recorded");
            assert_eq!(held.worktree, a);
            assert_eq!(held.lines, lines(&["/.envrc"]));

            goes(&b);

            assert_eq!(c.exclude_text(), "mine", "by Staged: {by_staged}");
            assert_eq!(
                read_manifest(&transfers.join(id)).unwrap().repos[0].added_excludes,
                None,
                "by Staged: {by_staged}"
            );
        }
    }

    /// With `~/.staged` a symlink, the store names a worktree through it and
    /// git by its real path. A record the sweep follows to a moved worktree
    /// takes the store's name for it, when the store has one, and a record
    /// under either name is that worktree's, before it goes and after.
    #[test]
    fn a_worktree_under_a_symlink_is_the_same_by_either_name() {
        let c = Clone::linked("transfer-excludes-linked");
        let moved_to = c.add_worktree("moved-to", "moved");
        let listed = crate::git::list_worktrees(&c.clone)
            .unwrap()
            .into_iter()
            .find(|(_, branch)| branch.as_deref() == Some("moved"))
            .unwrap()
            .0;
        assert_ne!(listed, moved_to, "git lists the real path");
        std::fs::write(&c.exclude, "/x\n/y\nmine\n").unwrap();
        let transfers = c.transfers();
        let moved = |line: &str| {
            let mut r = repo(
                &c.clone,
                Some(record(&c.base.join("moved-from"), &c.exclude, &[line])),
            );
            r.dest_branch = "moved".to_string();
            r
        };
        let worktree_of = |id: &str| {
            read_manifest(&transfers.join(id)).unwrap().repos[0]
                .added_excludes
                .as_ref()
                .map(|r| r.worktree.clone())
        };
        let now = SystemTime::now();
        let created = now_ms(now);

        // Followed without the store knowing it: git's name.
        write_manifest(
            &transfers.join("unknown"),
            &manifest("unknown", created, vec![moved("/y")]),
        )
        .unwrap();
        prune(&transfers, RETENTION, now, &[]);
        assert_eq!(worktree_of("unknown"), Some(listed.clone()));
        // And with it: the store's.
        write_manifest(
            &transfers.join("known"),
            &manifest("known", created, vec![moved("/x")]),
        )
        .unwrap();
        prune(
            &transfers,
            RETENTION,
            now,
            &[c.base.join("elsewhere"), moved_to.clone()],
        );
        assert_eq!(worktree_of("known"), Some(moved_to.clone()));
        assert_eq!(worktree_of("unknown"), Some(listed.clone()));

        // Staged removes it by the store's name: both records are its.
        c.remove_worktree(&moved_to);
        assert!(release_for_worktree(&transfers, &moved_to).is_empty());
        assert_eq!(c.exclude_text(), "mine\n");
        assert_eq!(worktree_of("known"), None);
        assert_eq!(worktree_of("unknown"), None);
    }

    #[test]
    fn an_entry_names_its_path_with_the_escapes_undone() {
        assert_eq!(entry_path("/.envrc"), Some(PathBuf::from(".envrc")));
        assert_eq!(
            entry_path("/a\\ b/\\[x\\]\\#\\!\\*\\?\\\\.txt"),
            Some(PathBuf::from("a b/[x]#!*?\\.txt"))
        );
        assert_eq!(entry_path(".envrc"), None);
        assert_eq!(entry_path("/"), None);
        assert_eq!(entry_path("/dangling\\"), None);
    }
}
