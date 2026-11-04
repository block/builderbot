//! What a transfer leaves behind so it can be undone by hand.
//!
//! Each transfer has `~/.staged/transfers/<transfer id>/`: every repository's
//! bundle (the branch tip plus the index and working-tree snapshots) and a
//! `manifest.json` saying what they are. Each carried branch also leaves
//! `refs/staged-transfer-backup/<transfer id>/<branch>` (the source's name)
//! in the shared local clone, pointing at the working-tree snapshot, whose
//! history holds the index snapshot and the branch tip. [`prune`] removes
//! both after [`RETENTION`] — except a transfer that left uncommitted changes
//! behind, for which this backup is the only copy: that one is kept until it
//! is removed by hand.
//!
//! The manifest is also the transfer's journal: it is written before the
//! transfer makes anything, with what to [`Undo`], and kept up to date as it
//! goes, so one Staged never finished can be dealt with at the next start
//! (see [`super::recovery`]).
//!
//! Recovering is plain git: `git fetch <bundle> refs/staged-transfer/<id>/snapshot`
//! (or check out the backup ref), then `read-tree` the snapshot or its parent.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::is_valid_transfer_id;
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
    /// backup, so [`prune`] never removes it: delete the directory (and its
    /// backup refs) by hand once they are no longer wanted.
    #[serde(default)]
    pub keep_until_removed: bool,
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
    /// restore at the destination, and that a completed transfer may have
    /// deleted at the source.
    pub fn holds_left_behind_changes(&self) -> bool {
        self.status != ROLLED_BACK
            && !self.include_uncommitted
            && self.repos.iter().any(|r| r.has_uncommitted_changes)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestRepo {
    pub github_repo: String,
    pub branch: String,
    pub dest_branch: String,
    /// The bundle's file name in this directory.
    pub bundle: String,
    pub head: String,
    /// The working-tree snapshot commit: the bundle's one ref.
    pub snapshot: String,
    pub has_uncommitted_changes: bool,
    /// The shared local clone of the repo, which holds [`Self::backup_ref`]
    /// once the source comes down.
    pub local_clone: Option<String>,
    pub backup_ref: Option<String>,
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

/// Remove every transfer directory under `transfers_dir` older than
/// `retention` as of `now`, and every ref the transfer has in the local
/// clones its manifest names: the backup refs, and any temporary ref a
/// transfer that stopped part way left. Returns the directories removed. Age
/// comes from the manifest, or the directory's mtime without one; only
/// directories named like a transfer id are touched, and never one whose
/// manifest keeps it until it is removed by hand.
pub fn prune(transfers_dir: &Path, retention: Duration, now: SystemTime) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(transfers_dir) else {
        return Vec::new();
    };
    let cutoff = now.checked_sub(retention).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let name = entry.file_name();
        if !dir.is_dir() || !name.to_str().is_some_and(is_valid_transfer_id) {
            continue;
        }
        let manifest = read_manifest(&dir);
        if manifest
            .as_ref()
            .is_some_and(|m| m.keep_until_removed || m.holds_left_behind_changes())
        {
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
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => removed.push(dir),
            Err(e) => log::warn!("[project_transfer] cannot remove {}: {e}", dir.display()),
        }
    }
    removed
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

        let removed = prune(&transfers, RETENTION, now);

        assert_eq!(removed, vec![transfers.join("old")]);
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

        let mut removed = prune(&transfers, RETENTION, now);
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

        let mut removed = prune(&transfers, RETENTION, now);
        removed.sort();

        assert_eq!(
            removed,
            vec![transfers.join("carried"), transfers.join("rolled-back")]
        );
        assert!(transfers.join("flagged").join(MANIFEST_FILE).exists());
        assert!(transfers.join("unflagged").join(MANIFEST_FILE).exists());
    }
}
