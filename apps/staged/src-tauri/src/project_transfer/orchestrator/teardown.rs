//! The two ends of a transfer's life: tearing the source down once the
//! database has switched, and rolling the destination back when it didn't.
//!
//! The source is only deleted on the strength of its snapshot, so each
//! checkout is first checked to be [`unchanged`] since: on its branch at the
//! same `HEAD`, with the same index and working tree. One that changed — an
//! editor saving, a terminal committing, while the transfer ran — is kept,
//! and so is anything that can't be checked; the transfer still succeeds,
//! with a warning saying what was left and why.

use std::path::{Path, PathBuf};

use super::super::backups;
use super::super::carry::{cleanup, unchanged};
use super::super::preflight::{Inspection, TransferDirection, TransferIssue};
use super::{path_str, Leg, Run, TransferPhase};

impl Run<'_> {
    /// Remove the source: the worktrees or the workspaces, after keeping each
    /// snapshot under a backup ref in the shared local clone. Never fails;
    /// returns what it left.
    pub(super) fn tear_down(
        &self,
        inspection: &Inspection,
        legs: &mut [Leg],
    ) -> Vec<TransferIssue> {
        self.emit(TransferPhase::TearingDown, None, None, None);
        let mut warnings = Vec::new();
        let tid = self.guard.transfer_id();
        match self.direction {
            TransferDirection::ToRemote => {
                for leg in legs.iter_mut() {
                    if let Err(issue) = self.remove_local_source(leg) {
                        warnings.push(issue);
                    }
                }
                // The bundles at the destination have served their purpose.
                if let (Some(workspace), Some(dir)) = (
                    self.workspace_name.as_deref(),
                    legs.iter().find_map(|l| l.dest_dir.as_deref()),
                ) {
                    let endpoint = self.host.workspace_endpoint(workspace);
                    if let Err(e) = cleanup(endpoint.as_ref(), dir, tid, true) {
                        log::warn!("[project_transfer] cannot clear {workspace}'s scratch: {e}");
                    }
                }
            }
            TransferDirection::ToLocal => {
                for workspace in &inspection.running_workspaces {
                    if let Err(issue) = self.remove_workspace_source(workspace, legs) {
                        warnings.push(issue);
                        let endpoint = self.host.workspace_endpoint(workspace);
                        for leg in legs
                            .iter()
                            .filter(|l| l.source.workspace.as_deref() == Some(workspace))
                        {
                            if let Some(dir) = &leg.source.source_dir {
                                let _ = cleanup(endpoint.as_ref(), dir, tid, true);
                            }
                        }
                    }
                }
            }
        }
        warnings
    }

    /// Keep a carried worktree's snapshot under a backup ref in the shared
    /// clone, drop the temporary ref, and — unless the worktree changed since
    /// the snapshot — remove the worktree (and its branch).
    fn remove_local_source(&self, leg: &mut Leg) -> Result<(), TransferIssue> {
        let (Some(plan), Some(outcome)) = (&leg.plan, &leg.outcome) else {
            return Ok(());
        };
        let dir = plan.source_dir.as_str();
        let worktree = Path::new(dir);
        let failed = |e: String| {
            TransferIssue::new(
                "worktree_left",
                format!("Transferred, but couldn't remove the old worktree {dir}: {e}"),
            )
        };
        let clone = self
            .host
            .local_clone_path(&leg.source.github_repo)
            .map_err(failed)?;
        if clone.canonicalize().ok() == worktree.canonicalize().ok() {
            return Err(failed("it is the shared clone itself".to_string()));
        }
        let backup_ref = self
            .keep_backup_ref(&clone, &leg.row.branch_name, &outcome.snapshot.snapshot)
            .map_err(failed)?;
        let endpoint = self.host.local_endpoint(worktree).map_err(failed)?;
        if let Err(e) = cleanup(endpoint.as_ref(), dir, self.guard.transfer_id(), false) {
            log::warn!("[project_transfer] cannot drop the temporary ref: {e}");
        }
        let check = unchanged(endpoint.as_ref(), plan, &outcome.snapshot);
        leg.backup_ref = Some(backup_ref);
        match check {
            Ok(None) => {}
            Ok(Some(what)) => {
                return Err(TransferIssue::new(
                    "worktree_left",
                    format!(
                        "Transferred, but the old worktree {dir} changed after it was \
                         snapshotted ({what}), so it was kept. What changed is only there."
                    ),
                ))
            }
            Err(e) => return Err(failed(format!("couldn't check it is unchanged: {e}"))),
        }
        crate::git::remove_worktree(&clone, worktree).map_err(|e| failed(e.to_string()))
    }

    /// Keep the snapshot of each branch carried out of `workspace` under a
    /// backup ref in its local clone, then — unless one of its clones changed
    /// since the snapshot — delete the workspace.
    fn remove_workspace_source(
        &self,
        workspace: &str,
        legs: &mut [Leg],
    ) -> Result<(), TransferIssue> {
        let endpoint = self.host.workspace_endpoint(workspace);
        let mut changed = Vec::new();
        let mut failures = Vec::new();
        for leg in legs
            .iter_mut()
            .filter(|l| l.source.workspace.as_deref() == Some(workspace))
        {
            let (Some(plan), Some(outcome)) = (&leg.plan, &leg.outcome) else {
                continue;
            };
            let branch = &leg.row.branch_name;
            let backup_ref = self.keep_backup_ref(
                Path::new(&plan.dest_dir),
                branch,
                &outcome.snapshot.snapshot,
            );
            match unchanged(endpoint.as_ref(), plan, &outcome.snapshot) {
                Ok(None) => {}
                Ok(Some(what)) => changed.push(format!("{branch}: {what}")),
                Err(e) => failures.push(format!("couldn't check {branch} is unchanged: {e}")),
            }
            match backup_ref {
                Ok(backup_ref) => leg.backup_ref = Some(backup_ref),
                Err(e) => failures.push(format!("{branch}: {e}")),
            }
        }
        if !changed.is_empty() {
            return Err(TransferIssue::new(
                "workspace_left",
                format!(
                    "Transferred, but workspace {workspace} changed after it was snapshotted \
                     ({}), so it was kept. What changed is only there.",
                    changed.join("; ")
                ),
            ));
        }
        if !failures.is_empty() {
            return Err(TransferIssue::new(
                "workspace_left",
                format!(
                    "Transferred, but couldn't delete workspace {workspace}: {}",
                    failures.join("; ")
                ),
            ));
        }
        self.host.delete_workspace(workspace).map_err(|e| {
            TransferIssue::new(
                "workspace_left",
                format!("Transferred, but couldn't delete workspace {workspace}: {e}"),
            )
        })
    }

    /// Point `refs/staged-transfer-backup/<id>/<branch>` in `clone` at the
    /// working-tree snapshot, so it outlives the source.
    fn keep_backup_ref(
        &self,
        clone: &Path,
        branch: &str,
        snapshot: &str,
    ) -> Result<String, String> {
        let backup_ref = backups::backup_ref(self.guard.transfer_id(), branch);
        crate::git::cli_run(clone, &["update-ref", &backup_ref, snapshot])
            .map_err(|e| format!("cannot keep the backup ref: {e}"))?;
        Ok(backup_ref)
    }

    /// Undo everything in [`super::Undo`], best-effort: destination first,
    /// then the source's temporary refs. The bundles on this machine stay.
    pub(super) fn roll_back(&self) {
        self.emit(TransferPhase::RollingBack, None, None, None);
        let tid = self.guard.transfer_id();
        let undo = &self.undo;
        if let Some(workspace) = &undo.workspace {
            if let Err(e) = self.host.delete_workspace(workspace) {
                log::warn!("[project_transfer] rollback: cannot delete workspace {workspace}: {e}");
            }
        }
        for (clone, worktree) in undo.worktrees.iter().rev() {
            remove_worktree_keeping_branch(clone, worktree);
        }
        // Only a branch the apply created, and only while it is where the
        // transfer put it.
        for (clone, branch, tip) in &undo.branches {
            let branch_ref = format!("refs/heads/{branch}");
            if let Err(e) = crate::git::cli_run(clone, &["update-ref", "-d", &branch_ref, tip]) {
                log::warn!("[project_transfer] rollback: cannot delete {branch}: {e}");
            }
        }
        // Before the branch config goes back: removing a remote also unsets
        // the config of the branches that track it.
        for (clone, remote) in &undo.remotes {
            let _ = crate::git::cli_run(clone, &["remote", "remove", remote]);
        }
        for (clone, branch, entries) in undo.branch_configs.iter().rev() {
            if let Err(e) = restore_branch_config(clone, branch, entries) {
                log::warn!("[project_transfer] rollback: cannot restore {branch}'s config: {e}");
            }
        }
        for clone in &undo.clones {
            let result = self
                .host
                .local_endpoint(clone)
                .map_err(|e| e.to_string())
                .and_then(|endpoint| {
                    cleanup(endpoint.as_ref(), &path_str(clone)?, tid, false)
                        .map_err(|e| e.to_string())
                });
            if let Err(e) = result {
                log::warn!("[project_transfer] rollback: {e}");
            }
        }
        for source in &undo.sources {
            let result = match &source.workspace {
                Some(workspace) => cleanup(
                    self.host.workspace_endpoint(workspace).as_ref(),
                    &source.dir,
                    tid,
                    true,
                )
                .map_err(|e| e.to_string()),
                None => self
                    .host
                    .local_endpoint(Path::new(&source.dir))
                    .and_then(|endpoint| {
                        cleanup(endpoint.as_ref(), &source.dir, tid, false)
                            .map_err(|e| e.to_string())
                    }),
            };
            if let Err(e) = result {
                log::warn!("[project_transfer] rollback: {e}");
            }
        }
        if self.transfer_dir.join(backups::MANIFEST_FILE).exists() {
            if let Some(mut manifest) = backups::read_manifest(&self.transfer_dir) {
                manifest.status = "rolledBack".to_string();
                manifest.keep_until_removed = manifest.holds_left_behind_changes();
                let _ = backups::write_manifest(&self.transfer_dir, &manifest);
            }
        }
    }
}

/// A branch's `branch.<branch>.*` config, as `(key, value)` in order.
pub(super) type BranchConfig = Vec<(String, String)>;

/// `branch.<branch>.*` in `clone`'s own config. A variable set without a
/// value reads as `true`, as git takes it.
pub(super) fn branch_config(clone: &Path, branch: &str) -> Result<BranchConfig, String> {
    let listing = crate::git::cli_run(clone, &["config", "--local", "-z", "--list"])
        .map_err(|e| e.to_string())?;
    let prefix = format!("branch.{branch}.");
    Ok(listing
        .split('\0')
        .filter_map(|entry| {
            let (key, value) = entry.split_once('\n').unwrap_or((entry, "true"));
            let var = key.strip_prefix(&prefix)?;
            (!var.is_empty() && !var.contains('.')).then(|| (key.to_string(), value.to_string()))
        })
        .collect())
}

/// Replace `branch.<branch>.*` in `clone` with `entries`, as
/// [`branch_config`] read them.
fn restore_branch_config(
    clone: &Path,
    branch: &str,
    entries: &[(String, String)],
) -> Result<(), String> {
    // Fails when there is no such section, which is as good as removed.
    let _ = crate::git::cli_run(
        clone,
        &[
            "config",
            "--local",
            "--remove-section",
            &format!("branch.{branch}"),
        ],
    );
    for (key, value) in entries {
        crate::git::cli_run(clone, &["config", "--local", "--add", key, value])
            .map_err(|e| format!("cannot set {key}: {e}"))?;
    }
    Ok(())
}

/// Remove a worktree the transfer added, without `remove_worktree`'s branch
/// deletion: the branch is the rollback's to decide about.
fn remove_worktree_keeping_branch(clone: &Path, worktree: &PathBuf) {
    let Some(path) = worktree.to_str() else {
        return;
    };
    if crate::git::cli_run(clone, &["worktree", "remove", "--force", path]).is_err() {
        if worktree.exists() {
            if let Err(e) = std::fs::remove_dir_all(worktree) {
                log::warn!("[project_transfer] rollback: cannot remove {path}: {e}");
            }
        }
        let _ = crate::git::cli_run(clone, &["worktree", "prune"]);
    }
}
