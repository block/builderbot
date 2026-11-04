//! What a transfer leaves behind so it can be undone by hand.
//!
//! Each transfer has `~/.staged/transfers/<transfer id>/`: every repository's
//! bundle (the branch tip plus the index and working-tree snapshots) and a
//! `manifest.json` saying what they are. A transfer away from this machine
//! also leaves `refs/staged-transfer-backup/<transfer id>/<branch>` in the
//! shared clone, pointing at the working-tree snapshot, whose history holds
//! the index snapshot and the branch tip. [`prune`] removes both after
//! [`RETENTION`].
//!
//! Recovering is plain git: `git fetch <bundle> refs/staged-transfer/<id>/snapshot`
//! (or check out the backup ref), then `read-tree` the snapshot or its parent.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::is_valid_transfer_id;

pub const MANIFEST_FILE: &str = "manifest.json";

/// How long a transfer's backups are kept.
pub const RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// The ref a local source's working-tree snapshot is kept under.
pub fn backup_ref(transfer_id: &str, branch: &str) -> String {
    format!("refs/staged-transfer-backup/{transfer_id}/{branch}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferManifest {
    pub transfer_id: String,
    pub project_id: String,
    /// `toRemote` or `toLocal`.
    pub direction: String,
    /// Milliseconds since the epoch.
    pub created_at: i64,
    pub include_uncommitted: bool,
    /// `carrying`, `completed`, or `rolledBack`.
    pub status: String,
    /// The workspace on the remote side.
    pub workspace_name: Option<String>,
    pub repos: Vec<ManifestRepo>,
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
    /// The shared clone holding [`Self::backup_ref`], for a local source.
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
/// `retention` as of `now`, and the backup refs its manifest names. Returns
/// the directories removed. Age comes from the manifest, or the directory's
/// mtime without one; only directories named like a transfer id are touched.
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
        for repo in manifest.iter().flat_map(|m| &m.repos) {
            let (Some(clone), Some(backup_ref)) = (&repo.local_clone, &repo.backup_ref) else {
                continue;
            };
            let clone = Path::new(clone);
            if clone.exists() {
                if let Err(e) = crate::git::cli_run(clone, &["update-ref", "-d", backup_ref]) {
                    log::warn!(
                        "[project_transfer] cannot remove {backup_ref} from {}: {e}",
                        clone.display()
                    );
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
            status: "completed".to_string(),
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
}
