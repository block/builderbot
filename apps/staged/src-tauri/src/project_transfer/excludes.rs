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
//! out when that worktree goes — when a transfer back to remote tears it
//! down, and when Staged removes it with its branch
//! ([`release_worktree_excludes`]) — or, for one that went some other way,
//! at the next start's prune ([`sweep`]). Taking them out is
//! [`undo::remove_exclude_lines`]: each line only while it is still there,
//! verbatim, and the file only when the transfer made it and nothing else is
//! left in it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::backups::{self, TransferManifest, MANIFEST_FILE};
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
    /// Each entry added, as written.
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

    /// Take the entries back out.
    pub fn remove(&self) -> Result<(), String> {
        undo::remove_exclude_lines(&self.file, &self.lines, self.created, self.terminated)
    }

    /// Whether these are `worktree`'s.
    fn is_for(&self, worktree: &Path) -> bool {
        self.worktree == worktree
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
/// added for `worktree`, and clear their records so they aren't taken out
/// twice. A record whose entries couldn't be taken out stays, for a start's
/// [`sweep`] to try again once the worktree is gone. Returns what it
/// couldn't do, one sentence each; reads only the manifests that hold any
/// record.
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
        let mut released = false;
        for repo in &mut manifest.repos {
            let Some(record) = repo.added_excludes.as_ref().filter(|r| r.is_for(worktree)) else {
                continue;
            };
            match record.remove() {
                Ok(()) => {
                    repo.added_excludes = None;
                    released = true;
                }
                Err(e) => problems.push(e),
            }
        }
        if released {
            if let Err(e) = backups::write_manifest(&dir, &manifest) {
                problems.push(e);
            }
        }
    }
    problems
}

/// At the start's prune: take out the entries of each of `manifest`'s
/// records whose worktree is gone, and clear the record. One whose worktree
/// moved — a branch moved to another project takes its worktree along —
/// follows it instead: its branch is still checked out in the clone, at a
/// path that exists. Returns whether the manifest changed, to be written.
pub fn sweep(manifest: &mut TransferManifest) -> bool {
    if manifest.status != backups::COMPLETED {
        return false;
    }
    let mut changed = false;
    for repo in &mut manifest.repos {
        let Some(record) = &mut repo.added_excludes else {
            continue;
        };
        if record.worktree.exists() {
            continue;
        }
        let clone = repo.local_clone.as_deref().map(Path::new);
        match clone.map(|clone| checked_out_at(clone, &repo.dest_branch)) {
            Some(Err(e)) => {
                log::warn!(
                    "[project_transfer] cannot tell where {} went: {e}",
                    record.worktree.display()
                );
                continue;
            }
            Some(Ok(Some(moved))) => {
                record.worktree = moved;
                changed = true;
                continue;
            }
            Some(Ok(None)) | None => {}
        }
        match record.remove() {
            Ok(()) => {
                repo.added_excludes = None;
                changed = true;
            }
            Err(e) => log::warn!("[project_transfer] {e}"),
        }
    }
    changed
}

/// The worktree of `clone` that has `branch` checked out, if one does and
/// is there.
fn checked_out_at(clone: &Path, branch: &str) -> Result<Option<PathBuf>, String> {
    if !clone.exists() {
        return Ok(None);
    }
    let worktrees = crate::git::list_worktrees(clone).map_err(|e| e.to_string())?;
    Ok(worktrees
        .into_iter()
        .find(|(path, on)| on.as_deref() == Some(branch) && path.exists())
        .map(|(path, _)| path))
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

    /// A clone with an `info/exclude` and a worktree for `feature`.
    struct Clone {
        root: crate::test_utils::TempDir,
        clone: PathBuf,
        exclude: PathBuf,
    }

    impl Clone {
        fn new(name: &str) -> Self {
            let root = crate::test_utils::TempDir::new(name);
            let clone = root.path().join("clone");
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
                clone,
                exclude,
            }
        }

        fn add_worktree(&self, name: &str, branch: &str) -> PathBuf {
            let path = self.root.path().join(name);
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

        fn transfers(&self) -> PathBuf {
            self.root.path().join("transfers")
        }

        fn exclude_text(&self) -> String {
            std::fs::read_to_string(&self.exclude).unwrap_or_default()
        }
    }

    /// The hook Staged's own worktree removal runs: the entries recorded for
    /// that worktree come out, once, with the lines anyone else wrote — and
    /// another worktree's, a rolled-back transfer's and an unparsed legacy
    /// manifest's records — left as they were.
    #[test]
    fn removing_a_worktree_takes_out_only_the_entries_recorded_for_it() {
        let c = Clone::new("transfer-excludes-release");
        let gone = c.root.path().join("worktrees/feature");
        let other = c.root.path().join("worktrees/other");
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

        let mut pruned = prune(&transfers, RETENTION, now);
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
        assert_eq!(prune(&transfers, RETENTION, later), Default::default());
        // ...until Staged removes the worktree, which takes its entries out;
        // then the manifest goes too.
        assert!(release_for_worktree(&transfers, &live).is_empty());
        assert_eq!(c.exclude_text(), "/moved\nmine\n");
        assert_eq!(
            prune(&transfers, RETENTION, later).removed,
            vec![transfers.join("live")]
        );
    }
}
