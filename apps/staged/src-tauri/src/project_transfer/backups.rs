//! What a transfer leaves behind so it can be undone by hand.
//!
//! Each transfer has `~/.staged/transfers/<transfer id>/`: every repository's
//! bundle (the branch tip plus the index and working-tree snapshots) and a
//! `manifest.json` saying what they are. Each carried branch also leaves
//! `refs/staged-transfer-backup/<transfer id>/<branch>` (the source's name)
//! in the shared local clone, pointing at the working-tree snapshot, whose
//! history holds the index snapshot and the branch tip. [`prune`] removes
//! both after [`RETENTION`] — except a transfer that left uncommitted changes
//! behind, for which this backup is the only copy: that one is kept whole
//! until it is removed by hand. One that left only ignored files behind
//! (not carried, or carried and left out at the destination) loses its
//! bundles, LFS archives and refs after [`RETENTION`] like any other, but
//! keeps its ignored files' archives and manifest until they are removed by
//! hand. And one whose repo still has entries in its clone's `info/exclude`
//! for a worktree that is still there (see [`super::excludes`]) keeps its
//! manifest, which records them, until that worktree goes.
//!
//! The manifest is also the transfer's journal: it is written before the
//! transfer makes anything, with what to [`Undo`], and kept up to date as it
//! goes, so one Staged never finished can be dealt with at the next start
//! (see [`super::recovery`]).
//!
//! Recovering is plain git: `git fetch <bundle> refs/staged-transfer/<id>/snapshot`
//! (or check out the backup ref), then `read-tree` the snapshot or its parent.
//! A repository that uses Git LFS also has `<bundle>.lfs.tar` beside its
//! bundle: the LFS objects the snapshot refers to, in the object store's
//! `objects/xx/yy/<oid>` layout. `tar -xf <bundle>.lfs.tar -C .git/lfs objects`
//! then `git lfs checkout` restores their content. And one with ignored files
//! (`.env`, but no ignored directory such as `node_modules/`) has them in
//! `<bundle stem>.ignored.tar`, archived from the source right before it was
//! removed, or when they were carried: `tar -xf` it in a checkout to put them
//! back. It goes with the rest of the directory, or outlives it as above.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::excludes::{self, WorktreeExcludes};
use super::is_valid_transfer_id;
use super::scripts::IgnoredFiles;
use super::undo::Undo;

pub const MANIFEST_FILE: &str = "manifest.json";

/// How long a transfer's backups are kept.
pub const RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// The ref a local source's working-tree snapshot is kept under.
pub fn backup_ref(transfer_id: &str, branch: &str) -> String {
    format!("refs/staged-transfer-backup/{transfer_id}/{branch}")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferManifest {
    pub transfer_id: String,
    pub project_id: String,
    /// `toRemote` or `toLocal`.
    pub direction: String,
    /// Milliseconds since the epoch.
    pub created_at: i64,
    pub include_uncommitted: bool,
    /// Whether the transfer carried the ignored files.
    #[serde(default)]
    pub include_ignored: bool,
    /// `carrying` until the database switches, `committed` while the source
    /// comes down, then `completed`; or `rolledBack`. The first two are a
    /// transfer still running, or one Staged never finished.
    pub status: String,
    /// The Staged process running the transfer.
    #[serde(default)]
    pub owner_pid: Option<u32>,
    /// The workspace on the remote side.
    pub workspace_name: Option<String>,
    /// Some repo's uncommitted changes stayed behind and exist only in this
    /// backup, so [`prune`] never removes any of it: delete the directory
    /// (and its backup refs) by hand once they are no longer wanted. Ignored
    /// files left behind don't set it; see [`Self::bundles_pruned_at`].
    #[serde(default)]
    pub keep_until_removed: bool,
    /// When [`prune`], past [`RETENTION`], removed the bundles, LFS archives
    /// and refs of a transfer that left only ignored files behind, keeping
    /// each repo's ignored files' archive (and its excludes list) and this
    /// manifest: delete the directory by hand once they are no longer
    /// wanted. Milliseconds since the epoch. The repos' bundles and backup
    /// refs are gone from then on.
    #[serde(default)]
    pub bundles_pruned_at: Option<i64>,
    /// The repos carried so far.
    pub repos: Vec<ManifestRepo>,
    /// What the transfer made before the database switched, recorded before
    /// it was made.
    #[serde(default)]
    pub undo: Undo,
    /// What the user should know: what an interrupted transfer left.
    #[serde(default)]
    pub warnings: Vec<String>,
}

pub const CARRYING: &str = "carrying";
pub const COMMITTED: &str = "committed";
pub const COMPLETED: &str = "completed";
pub const ROLLED_BACK: &str = "rolledBack";

impl TransferManifest {
    /// Whether the transfer finished, one way or the other.
    pub fn is_finished(&self) -> bool {
        self.status == COMPLETED || self.status == ROLLED_BACK
    }

    /// The shared local clones that may hold the transfer's refs.
    fn local_clones(&self) -> Vec<&Path> {
        let mut clones: Vec<&Path> = self
            .repos
            .iter()
            .filter_map(|r| r.local_clone.as_deref().map(Path::new))
            .chain(self.undo.clones.iter().map(PathBuf::as_path))
            .collect();
        clones.sort();
        clones.dedup();
        clones
    }

    /// Whether this backup holds uncommitted changes the transfer did not
    /// restore at the destination, and a completed one may have deleted at
    /// the source: [`prune`] keeps all of it, whatever its age.
    pub fn holds_left_behind_changes(&self) -> bool {
        self.status != ROLLED_BACK
            && !self.include_uncommitted
            && self.repos.iter().any(|r| r.has_uncommitted_changes)
    }

    /// Whether it holds ignored files the transfer archived and didn't place
    /// (not carried, or carried and left out there), which a completed one
    /// may have deleted at the source: [`prune`] keeps their archives,
    /// whatever their age, and lets the rest go after [`RETENTION`].
    pub fn holds_left_behind_ignored(&self) -> bool {
        self.status != ROLLED_BACK
            && self
                .repos
                .iter()
                .any(|r| r.ignored.as_ref().is_some_and(ManifestIgnored::left_behind))
    }

    /// Whether a repo's entries in its clone's `info/exclude` are still
    /// recorded here, for a worktree that hasn't gone yet as far as anyone
    /// noticed: [`prune`] keeps the manifest while they are.
    pub fn holds_added_excludes(&self) -> bool {
        self.status == COMPLETED && self.repos.iter().any(|r| r.added_excludes.is_some())
    }

    /// Whether [`prune`] keeps the whole directory, whatever its age.
    fn kept_whole(&self) -> bool {
        self.keep_until_removed || self.holds_left_behind_changes()
    }

    /// Whether [`prune`], past [`RETENTION`], keeps part of the directory
    /// (see [`Self::kept_after_pruning`]) rather than removing it.
    fn outlives_retention(&self) -> bool {
        self.holds_left_behind_ignored() || self.holds_added_excludes()
    }

    /// The files [`prune`] keeps once the rest has gone: the manifest, and
    /// while it holds ignored files left behind each repo's ignored files'
    /// archive and its excludes list.
    fn kept_after_pruning(&self) -> HashSet<OsString> {
        let mut kept = HashSet::from([OsString::from(MANIFEST_FILE)]);
        if !self.holds_left_behind_ignored() {
            return kept;
        }
        for repo in &self.repos {
            if let Some(archive) = repo.ignored.as_ref().and_then(|i| i.archive.as_deref()) {
                kept.insert(archive.into());
                kept.insert(IgnoredFiles::excludes_name(&repo.bundle).into());
            }
        }
        kept
    }

    /// After a rollback, which took every entry the transfer added to an
    /// `info/exclude` back out with the rest of its undo list.
    pub fn forget_added_excludes(&mut self) {
        for repo in &mut self.repos {
            repo.added_excludes = None;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestRepo {
    pub github_repo: String,
    pub branch: String,
    pub dest_branch: String,
    /// The bundle's file name in this directory, until
    /// [`TransferManifest::bundles_pruned_at`].
    pub bundle: String,
    pub head: String,
    /// The working-tree snapshot commit: the bundle's one ref.
    pub snapshot: String,
    pub has_uncommitted_changes: bool,
    /// The shared local clone of the repo, which holds [`Self::backup_ref`]
    /// once the source comes down.
    pub local_clone: Option<String>,
    /// Cleared when [`prune`] removes it but keeps the manifest.
    pub backup_ref: Option<String>,
    /// The source's ignored files, when it had any.
    #[serde(default)]
    pub ignored: Option<ManifestIgnored>,
    /// The entries a transfer to local added to the clone's `info/exclude`
    /// for the worktree it made, until that worktree goes (see
    /// [`super::excludes`]). Left out of the JSON when there are none, so
    /// that a manifest without any can be told apart unparsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_excludes: Option<WorktreeExcludes>,
}

/// A repo's ignored files, as the transfer archived them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestIgnored {
    /// The archive's file name in this directory; `None` when there were too
    /// many to archive.
    pub archive: Option<String>,
    pub files: u64,
    pub bytes: u64,
    /// More than a transfer archives, so they weren't backed up.
    pub over_cap: bool,
    /// How many were put in the destination checkout.
    pub placed: u64,
}

impl ManifestIgnored {
    /// Whether the archive holds files the destination checkout didn't get.
    pub fn left_behind(&self) -> bool {
        self.archive.is_some() && self.placed < self.files
    }
}

/// Write `manifest` into `dir`, whole or not at all.
pub fn write_manifest(dir: &Path, manifest: &TransferManifest) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let json = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    let path = dir.join(MANIFEST_FILE);
    let partial = dir.join(format!("{MANIFEST_FILE}.part"));
    std::fs::write(&partial, json)
        .and_then(|_| std::fs::rename(&partial, &path))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn read_manifest(dir: &Path) -> Option<TransferManifest> {
    let bytes = std::fs::read(dir.join(MANIFEST_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// What [`prune`] did, by transfer directory.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    /// Removed whole.
    pub removed: Vec<PathBuf>,
    /// Left with only the manifest, and any ignored files' archives left
    /// behind.
    pub trimmed: Vec<PathBuf>,
}

/// Prune every transfer directory under `transfers_dir` older than
/// `retention` as of `now`: remove every ref the transfer has in the local
/// clones its manifest names — the backup refs, and any temporary ref a
/// transfer that stopped part way left — and the directory. Age comes from
/// the manifest, or the directory's mtime without one; only directories
/// named like a transfer id are touched. Never one that holds uncommitted
/// changes left behind (or whose manifest keeps it until it is removed by
/// hand); one that holds ignored files left behind keeps their archives and
/// the manifest, which records when the rest went and is then left alone
/// while it still holds them; and so does one recording `info/exclude`
/// entries for a worktree, with just its manifest. First, whatever their
/// age, the entries of a worktree that went without Staged noticing are
/// taken out (see [`excludes::sweep`]).
pub fn prune(transfers_dir: &Path, retention: Duration, now: SystemTime) -> Pruned {
    let mut pruned = Pruned::default();
    let Ok(entries) = std::fs::read_dir(transfers_dir) else {
        return pruned;
    };
    let cutoff = now.checked_sub(retention).unwrap_or(SystemTime::UNIX_EPOCH);
    for entry in entries.flatten() {
        let dir = entry.path();
        let name = entry.file_name();
        if !dir.is_dir() || !name.to_str().is_some_and(is_valid_transfer_id) {
            continue;
        }
        let mut manifest = read_manifest(&dir);
        if let Some(m) = manifest.as_mut().filter(|m| m.holds_added_excludes()) {
            if excludes::sweep(m) {
                if let Err(e) = write_manifest(&dir, m) {
                    log::warn!("[project_transfer] {e}");
                }
            }
        }
        if manifest.as_ref().is_some_and(|m| {
            m.kept_whole() || (m.bundles_pruned_at.is_some() && m.outlives_retention())
        }) {
            continue;
        }
        let created = match &manifest {
            Some(m) => SystemTime::UNIX_EPOCH + Duration::from_millis(m.created_at.max(0) as u64),
            None => match entry.metadata().and_then(|m| m.modified()) {
                Ok(modified) => modified,
                Err(_) => continue,
            },
        };
        if created >= cutoff {
            continue;
        }
        if let Some(manifest) = &manifest {
            for clone in manifest.local_clones() {
                if clone.exists() {
                    remove_transfer_refs(clone, &manifest.transfer_id);
                }
            }
        }
        match manifest {
            Some(mut manifest) if manifest.outlives_retention() => {
                match trim(&dir, &mut manifest, now) {
                    Ok(()) => pruned.trimmed.push(dir),
                    Err(e) => log::warn!("[project_transfer] {e}"),
                }
            }
            _ => match std::fs::remove_dir_all(&dir) {
                Ok(()) => pruned.removed.push(dir),
                Err(e) => log::warn!("[project_transfer] cannot remove {}: {e}", dir.display()),
            },
        }
    }
    pruned
}

/// Remove everything in `dir` but what `manifest` keeps once pruned (see
/// [`TransferManifest::kept_after_pruning`]), then say so in the manifest.
/// The manifest is only written once every other file is gone, so one that
/// could not be removed is tried again at the next prune.
fn trim(dir: &Path, manifest: &mut TransferManifest, now: SystemTime) -> Result<(), String> {
    let kept = manifest.kept_after_pruning();
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut failed = Vec::new();
    for entry in entries.flatten() {
        if kept.contains(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let removed = match entry.file_type() {
            Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
        if let Err(e) = removed {
            failed.push(format!("{}: {e}", path.display()));
        }
    }
    if !failed.is_empty() {
        return Err(format!("cannot prune {}", failed.join("; ")));
    }
    let now_ms = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    manifest.bundles_pruned_at = Some(now_ms);
    for repo in &mut manifest.repos {
        repo.backup_ref = None;
    }
    write_manifest(dir, manifest)
}

/// Delete `refs/staged-transfer-backup/<id>/*` and `refs/staged-transfer/<id>/*`
/// in `clone`.
fn remove_transfer_refs(clone: &Path, transfer_id: &str) {
    let namespaces = [
        format!("refs/staged-transfer-backup/{transfer_id}/"),
        format!("refs/staged-transfer/{transfer_id}/"),
    ];
    let mut args = vec!["for-each-ref", "--format=%(refname)"];
    args.extend(namespaces.iter().map(String::as_str));
    let refs = match crate::git::cli_run(clone, &args) {
        Ok(refs) => refs,
        Err(e) => {
            log::warn!(
                "[project_transfer] cannot list {transfer_id}'s refs in {}: {e}",
                clone.display()
            );
            return;
        }
    };
    for name in refs.lines().filter(|l| !l.is_empty()) {
        if let Err(e) = crate::git::cli_run(clone, &["update-ref", "-d", name]) {
            log::warn!(
                "[project_transfer] cannot remove {name} from {}: {e}",
                clone.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, created_at: i64, clone: &Path, backup_ref: &str) -> TransferManifest {
        TransferManifest {
            transfer_id: id.to_string(),
            project_id: "p".to_string(),
            direction: "toRemote".to_string(),
            created_at,
            include_uncommitted: true,
            status: COMPLETED.to_string(),
            workspace_name: Some("stg-x".to_string()),
            repos: vec![ManifestRepo {
                github_repo: "acme/widgets".to_string(),
                branch: "feature".to_string(),
                dest_branch: "feature".to_string(),
                bundle: "0-widgets.bundle".to_string(),
                head: "a".repeat(40),
                snapshot: "b".repeat(40),
                has_uncommitted_changes: true,
                local_clone: Some(clone.to_string_lossy().into_owned()),
                backup_ref: Some(backup_ref.to_string()),
                ignored: None,
                added_excludes: None,
            }],
            ..TransferManifest::default()
        }
    }

    #[test]
    fn prunes_old_transfers_and_their_backup_refs_only() {
        let root = crate::test_utils::TempDir::new("transfer-prune");
        let clone = root.path().join("clone");
        std::fs::create_dir_all(&clone).unwrap();
        let git = |args: &[&str]| crate::git::cli_run(&clone, args).unwrap();
        git(&["init", "-q"]);
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
        let head = git(&["rev-parse", "HEAD"]).trim().to_string();
        let old_ref = backup_ref("old", "feature");
        let new_ref = backup_ref("new", "feature");
        git(&["update-ref", &old_ref, &head]);
        git(&["update-ref", &new_ref, &head]);

        let transfers = root.path().join("transfers");
        let now = SystemTime::now();
        let day_ms = 24 * 60 * 60 * 1000;
        let now_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        write_manifest(
            &transfers.join("old"),
            &manifest("old", now_ms - 15 * day_ms, &clone, &old_ref),
        )
        .unwrap();
        write_manifest(
            &transfers.join("new"),
            &manifest("new", now_ms - day_ms, &clone, &new_ref),
        )
        .unwrap();
        std::fs::create_dir_all(transfers.join("not a transfer")).unwrap();

        let pruned = prune(&transfers, RETENTION, now);

        assert_eq!(
            pruned,
            Pruned {
                removed: vec![transfers.join("old")],
                trimmed: vec![],
            }
        );
        assert!(transfers.join("new").join(MANIFEST_FILE).exists());
        assert!(transfers.join("not a transfer").exists());
        let refs = git(&[
            "for-each-ref",
            "--format=%(refname)",
            "refs/staged-transfer-backup/",
        ]);
        assert_eq!(refs.trim(), new_ref);
    }

    /// A transfer that stopped part way may have kept a backup ref it never
    /// wrote down, or left a temporary one: pruning it takes every ref under
    /// its id, in every clone it names.
    #[test]
    fn prunes_every_ref_an_interrupted_transfer_left() {
        let root = crate::test_utils::TempDir::new("transfer-prune-interrupted");
        let clone = root.path().join("clone");
        std::fs::create_dir_all(&clone).unwrap();
        let git = |args: &[&str]| crate::git::cli_run(&clone, args).unwrap();
        git(&["init", "-q"]);
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
        let head = git(&["rev-parse", "HEAD"]).trim().to_string();
        let left = [
            backup_ref("cut", "feature"),
            "refs/staged-transfer/cut/snapshot".to_string(),
            backup_ref("rolled", "feature"),
        ];
        for name in &left {
            git(&["update-ref", name, &head]);
        }
        let transfers = root.path().join("transfers");
        let now = SystemTime::now();
        let old_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            - 15 * 24 * 60 * 60 * 1000;
        // Committed, then cut off while the source came down: the backup ref
        // was kept, the manifest never said so.
        let mut cut = manifest("cut", old_ms, &clone, "unused");
        cut.status = CARRYING.to_string();
        cut.repos[0].backup_ref = None;
        write_manifest(&transfers.join("cut"), &cut).unwrap();
        // Rolled back before anything was carried: only the undo list knows
        // the clone.
        let mut rolled = manifest("rolled", old_ms, &clone, "unused");
        rolled.status = ROLLED_BACK.to_string();
        rolled.repos.clear();
        rolled.undo.clones.push(clone.clone());
        write_manifest(&transfers.join("rolled"), &rolled).unwrap();

        let mut removed = prune(&transfers, RETENTION, now).removed;
        removed.sort();

        assert_eq!(
            removed,
            vec![transfers.join("cut"), transfers.join("rolled")]
        );
        let refs = git(&[
            "for-each-ref",
            "--format=%(refname)",
            "refs/staged-transfer-backup/",
            "refs/staged-transfer/",
        ]);
        assert_eq!(refs.trim(), "");
    }

    #[test]
    fn keeps_a_backup_holding_changes_left_behind_whatever_its_age() {
        let root = crate::test_utils::TempDir::new("transfer-prune-keep");
        let clone = root.path().join("clone");
        let transfers = root.path().join("transfers");
        let now = SystemTime::now();
        let old_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            - 30 * 24 * 60 * 60 * 1000;
        let left_behind = |id: &str, status: &str, flagged: bool| {
            let mut m = manifest(id, old_ms, &clone, &backup_ref(id, "feature"));
            m.include_uncommitted = false;
            m.status = status.to_string();
            m.keep_until_removed = flagged;
            write_manifest(&transfers.join(id), &m).unwrap();
        };
        left_behind("flagged", "completed", true);
        // Without the flag (it was never written, say), the rule still holds.
        left_behind("unflagged", "completed", false);
        // A rolled-back transfer deleted nothing, so its backup can go.
        left_behind("rolled-back", "rolledBack", false);
        let mut carried = manifest("carried", old_ms, &clone, &backup_ref("carried", "f"));
        carried.include_uncommitted = true;
        write_manifest(&transfers.join("carried"), &carried).unwrap();

        let mut removed = prune(&transfers, RETENTION, now).removed;
        removed.sort();

        assert_eq!(
            removed,
            vec![transfers.join("carried"), transfers.join("rolled-back")]
        );
        assert!(transfers.join("flagged").join(MANIFEST_FILE).exists());
        assert!(transfers.join("unflagged").join(MANIFEST_FILE).exists());
    }

    /// An ignored-files archive is one more file in the transfer's directory:
    /// it goes with an old transfer that placed every file, stays with one
    /// kept until removed, and a manifest from before it existed still reads
    /// (and prunes by its age).
    #[test]
    fn prunes_ignored_files_backups_with_their_transfer() {
        let root = crate::test_utils::TempDir::new("transfer-prune-ignored");
        let clone = root.path().join("clone");
        let transfers = root.path().join("transfers");
        let now = SystemTime::now();
        let old_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            - 15 * 24 * 60 * 60 * 1000;
        let archive = "0-widgets.ignored.tar";
        let with_archive = |id: &str, kept: bool| {
            let mut m = manifest(id, old_ms, &clone, &backup_ref(id, "feature"));
            m.keep_until_removed = kept;
            m.include_ignored = true;
            m.repos[0].ignored = Some(ManifestIgnored {
                archive: Some(archive.to_string()),
                files: 2,
                bytes: 20,
                over_cap: false,
                placed: 2,
            });
            let dir = transfers.join(id);
            write_manifest(&dir, &m).unwrap();
            std::fs::write(dir.join(archive), b"tar").unwrap();
            assert_eq!(
                read_manifest(&dir).unwrap().repos[0].ignored,
                m.repos[0].ignored
            );
        };
        with_archive("old", false);
        with_archive("kept", true);
        // Written before ignored files were archived: neither key.
        let mut legacy = serde_json::to_value(manifest(
            "legacy",
            old_ms,
            &clone,
            &backup_ref("legacy", "feature"),
        ))
        .unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("includeIgnored")
            .unwrap();
        legacy["repos"][0]
            .as_object_mut()
            .unwrap()
            .remove("ignored")
            .unwrap();
        let legacy_dir = transfers.join("legacy");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(
            legacy_dir.join(MANIFEST_FILE),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let read = read_manifest(&legacy_dir).expect("an old manifest still reads");
        assert!(!read.include_ignored);
        assert_eq!(read.repos[0].ignored, None);

        let mut removed = prune(&transfers, RETENTION, now).removed;
        removed.sort();

        // The legacy one went by its manifest's age, not its fresh mtime.
        assert_eq!(removed, vec![legacy_dir, transfers.join("old")]);
        assert!(transfers.join("kept").join(archive).exists());
    }

    /// The files a transfer directory gets, as each repo's leg would leave
    /// them: the bundle, its LFS archive, the ignored files' archive and
    /// excludes list, and the scratch a local end leaves beside them.
    fn populate(dir: &Path, with_ignored: bool) {
        let stem = "0-widgets";
        for name in [
            format!("{stem}.bundle"),
            format!("{stem}.bundle.lfs.tar"),
            format!("{stem}.bundle.index"),
            format!("{stem}.info-exclude.added"),
        ] {
            std::fs::write(dir.join(name), b"data").unwrap();
        }
        std::fs::create_dir_all(dir.join(format!("{stem}.ignored.tar.d/files"))).unwrap();
        std::fs::write(dir.join(format!("{stem}.ignored.tar.d/files/x")), b"x").unwrap();
        if with_ignored {
            std::fs::write(dir.join(format!("{stem}.ignored.tar")), b"tar").unwrap();
            std::fs::write(dir.join(format!("{stem}.ignored.excludes")), b"./x\n").unwrap();
        }
    }

    /// The names in `dir`, sorted.
    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Ignored files the destination didn't get — not carried, or carried
    /// and left out there — exist only in their archive once the source is
    /// gone, so it stays, whatever its age. The rest of the backup (bundles,
    /// LFS archives, refs) goes after the retention like any other's; only
    /// uncommitted changes left behind keep all of it.
    #[test]
    fn a_backup_holding_only_ignored_files_left_behind_keeps_just_their_archives() {
        let root = crate::test_utils::TempDir::new("transfer-prune-ignored-left");
        let clone = root.path().join("clone");
        std::fs::create_dir_all(&clone).unwrap();
        let git = |args: &[&str]| crate::git::cli_run(&clone, args).unwrap();
        git(&["init", "-q"]);
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
        let head = git(&["rev-parse", "HEAD"]).trim().to_string();
        let transfers = root.path().join("transfers");
        let now = SystemTime::now();
        let day_ms = 24 * 60 * 60 * 1000;
        let now_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let old_ms = now_ms - 30 * day_ms;
        let ignored = |archive: bool, files: u64, placed: u64| ManifestIgnored {
            archive: archive.then(|| "0-widgets.ignored.tar".to_string()),
            files,
            bytes: files * 10,
            over_cap: !archive,
            placed,
        };
        struct Case {
            id: &'static str,
            created_at: i64,
            status: &'static str,
            include_uncommitted: bool,
            ignored: Option<ManifestIgnored>,
            holds_changes: bool,
            holds_ignored: bool,
        }
        let case = |id, created_at, ignored: Option<ManifestIgnored>| Case {
            id,
            created_at,
            status: COMPLETED,
            include_uncommitted: true,
            holds_changes: false,
            holds_ignored: ignored.as_ref().is_some_and(ManifestIgnored::left_behind),
            ignored,
        };
        let cases = [
            // Not carried: the archive is the only copy.
            case("not-carried", old_ms, Some(ignored(true, 2, 0))),
            // Carried, but one was left out at the destination.
            case("partly-placed", old_ms, Some(ignored(true, 3, 2))),
            // Still within the retention: nothing goes yet.
            case("recent", now_ms - day_ms, Some(ignored(true, 2, 0))),
            // Uncommitted changes left behind as well: all of it stays.
            Case {
                include_uncommitted: false,
                holds_changes: true,
                ..case("changes-too", old_ms, Some(ignored(true, 2, 0)))
            },
            // Every one placed: the directory goes whole.
            case("all-placed", old_ms, Some(ignored(true, 2, 2))),
            // Over the cap: nothing was archived to keep.
            case("over-cap", old_ms, Some(ignored(false, 2, 0))),
            // Rolled back: the source is still there.
            Case {
                status: ROLLED_BACK,
                holds_ignored: false,
                ..case("rolled-back", old_ms, Some(ignored(true, 2, 0)))
            },
        ];
        for c in &cases {
            let backup = backup_ref(c.id, "feature");
            git(&["update-ref", &backup, &head]);
            let mut m = manifest(c.id, c.created_at, &clone, &backup);
            m.status = c.status.to_string();
            m.include_uncommitted = c.include_uncommitted;
            m.repos[0].ignored = c.ignored.clone();
            assert_eq!(m.holds_left_behind_changes(), c.holds_changes, "{}", c.id);
            assert_eq!(m.holds_left_behind_ignored(), c.holds_ignored, "{}", c.id);
            m.keep_until_removed = m.holds_left_behind_changes();
            let dir = transfers.join(c.id);
            write_manifest(&dir, &m).unwrap();
            populate(
                &dir,
                c.ignored.as_ref().is_some_and(|i| i.archive.is_some()),
            );
        }
        let everything = names_in(&transfers.join("recent"));

        let mut pruned = prune(&transfers, RETENTION, now);
        pruned.removed.sort();
        pruned.trimmed.sort();

        assert_eq!(
            pruned,
            Pruned {
                removed: vec![
                    transfers.join("all-placed"),
                    transfers.join("over-cap"),
                    transfers.join("rolled-back"),
                ],
                trimmed: vec![
                    transfers.join("not-carried"),
                    transfers.join("partly-placed")
                ],
            }
        );
        let kept = vec![
            "0-widgets.ignored.excludes".to_string(),
            "0-widgets.ignored.tar".to_string(),
            MANIFEST_FILE.to_string(),
        ];
        for id in ["not-carried", "partly-placed"] {
            let dir = transfers.join(id);
            assert_eq!(names_in(&dir), kept, "{id}");
            let m = read_manifest(&dir).unwrap();
            assert_eq!(m.bundles_pruned_at, Some(now_ms), "{id}");
            assert_eq!(m.repos[0].backup_ref, None, "{id}");
            assert!(!m.keep_until_removed, "{id}");
            assert!(m.holds_left_behind_ignored(), "{id}");
        }
        for id in ["recent", "changes-too"] {
            let dir = transfers.join(id);
            assert_eq!(names_in(&dir), everything, "{id}");
            assert_eq!(read_manifest(&dir).unwrap().bundles_pruned_at, None, "{id}");
        }
        let refs = git(&[
            "for-each-ref",
            "--format=%(refname)",
            "refs/staged-transfer-backup/",
        ]);
        let mut refs: Vec<&str> = refs.lines().collect();
        refs.sort();
        assert_eq!(
            refs,
            vec![
                backup_ref("changes-too", "feature"),
                backup_ref("recent", "feature")
            ]
        );

        // Pruned once, the rest is left until it is removed by hand; the
        // recent one's turn comes.
        let later = now + RETENTION;
        assert_eq!(
            prune(&transfers, RETENTION, later),
            Pruned {
                removed: vec![],
                trimmed: vec![transfers.join("recent")],
            }
        );
        assert_eq!(names_in(&transfers.join("recent")), kept);
        assert_eq!(names_in(&transfers.join("not-carried")), kept);
        assert_eq!(
            read_manifest(&transfers.join("not-carried"))
                .unwrap()
                .bundles_pruned_at,
            Some(now_ms)
        );
    }

    /// A manifest written before uncommitted changes and ignored files were
    /// told apart may say to keep the backup whole: it is kept whole.
    #[test]
    fn a_backup_flagged_to_keep_stays_whole() {
        let root = crate::test_utils::TempDir::new("transfer-prune-flagged");
        let clone = root.path().join("clone");
        let transfers = root.path().join("transfers");
        let now = SystemTime::now();
        let old_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            - 30 * 24 * 60 * 60 * 1000;
        let mut m = manifest("flagged", old_ms, &clone, &backup_ref("flagged", "f"));
        m.keep_until_removed = true;
        m.repos[0].ignored = Some(ManifestIgnored {
            archive: Some("0-widgets.ignored.tar".to_string()),
            files: 1,
            bytes: 1,
            over_cap: false,
            placed: 0,
        });
        let dir = transfers.join("flagged");
        write_manifest(&dir, &m).unwrap();
        populate(&dir, true);
        let before = names_in(&dir);

        assert_eq!(prune(&transfers, RETENTION, now), Pruned::default());

        assert_eq!(names_in(&dir), before);
    }
}
