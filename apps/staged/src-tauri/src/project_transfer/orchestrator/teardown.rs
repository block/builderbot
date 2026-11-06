//! The two ends of a transfer's life: tearing the source down once the
//! database has switched, and rolling the destination back when it didn't.
//!
//! The source is only deleted on the strength of its snapshot, so each
//! checkout is first checked to be [`unchanged`] since: on its branch at the
//! same `HEAD`, with the same index and working tree. One that changed — an
//! editor saving, a terminal committing, while the transfer ran — is kept,
//! and so is anything that can't be checked; the transfer still succeeds,
//! with a warning saying what was left and why. A workspace's checkouts are
//! checked together, in one exec, right before the delete.
//!
//! Before that, each checkout's ignored files (`.env`; never an ignored
//! directory such as `node_modules/`) are archived into the backup on this
//! machine with [`back_up_ignored`], whether or not the transfer carried
//! them: a source whose files can't be archived and brought here is kept.
//! Only files over the cap go unbacked, with a warning. As `add -A` never
//! reads ignored files, the unchanged check also compares them with the
//! fingerprint the archive recorded: a file that changed, came or went after
//! it was backed up (or that can't be read to tell) keeps the source too.
//!
//! A worktree an earlier transfer to local made may have entries in its
//! clone's `info/exclude`, which every other worktree of the clone reads:
//! they come out once the worktree is removed (see
//! [`super::super::excludes`]). A worktree that is kept keeps them, so the
//! ignored files carried into it don't turn up as untracked there, where
//! the warning sends the user to save what changed; a start's sweep takes
//! them out once it is gone.

use std::path::Path;

use super::super::backups;
use super::super::carry::{back_up_ignored, cleanup, unchanged, unchanged_all, CarryPlan};
use super::super::excludes;
use super::super::preflight::{Inspection, TransferDirection, TransferIssue};
use super::super::scripts::IgnoredFiles;
use super::super::undo;
use super::{Leg, Run, TransferPhase};

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
                    match self.remove_local_source(leg) {
                        // Only once it is gone: the check that the worktree
                        // is unchanged hashes its untracked files, which
                        // these entries keep out of it.
                        Ok(()) => warnings.extend(self.release_excludes(leg)),
                        Err(issue) => warnings.push(issue),
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
    /// clone, drop the temporary ref, back its ignored files up, and — unless
    /// the worktree changed since the snapshot — remove the worktree (and its
    /// branch).
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
        leg.backup_ref = Some(backup_ref);
        let ignored = back_up_ignored(endpoint.as_ref(), plan, outcome.ignored.as_ref())
            .map_err(|e| failed(format!("couldn't back its ignored files up: {e}")))?;
        leg.warnings.extend(skipped_warning(&ignored, plan));
        leg.ignored_backup = Some(ignored);
        // The ignored files too: the backup holds them as they were.
        let check = unchanged(
            endpoint.as_ref(),
            plan,
            &outcome.snapshot,
            leg.ignored_backup.as_ref(),
        );
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

    /// Take out the entries earlier transfers to local added to the clone's
    /// `info/exclude` for a carried leg's worktree, which has just been
    /// removed. What can't be taken out is said; its record stays, for a
    /// start's prune to try again.
    fn release_excludes(&self, leg: &Leg) -> Option<TransferIssue> {
        let dir = leg.plan.as_ref()?.source_dir.as_str();
        let problems = match self.host.transfers_dir() {
            Ok(transfers) => excludes::release_for_worktree(&transfers, Path::new(dir)),
            Err(e) => vec![e],
        };
        if problems.is_empty() {
            return None;
        }
        Some(TransferIssue::new(
            "excludes_left",
            format!(
                "Transferred, but couldn't take out the entries an earlier transfer added to \
                 the local clone's info/exclude for {dir}, so other worktrees of the clone may \
                 still ignore those paths: {}",
                problems.join("; ")
            ),
        ))
    }

    /// Keep the snapshot of each branch carried out of `workspace` under a
    /// backup ref in its local clone and its ignored files in the backup,
    /// then — unless one of its clones changed since the snapshot — delete
    /// the workspace.
    fn remove_workspace_source(
        &self,
        workspace: &str,
        legs: &mut [Leg],
    ) -> Result<(), TransferIssue> {
        let mut changed = Vec::new();
        let mut failures = Vec::new();
        let from_here = |leg: &Leg| {
            leg.source.workspace.as_deref() == Some(workspace)
                && leg.plan.is_some()
                && leg.outcome.is_some()
        };
        let endpoint = self.host.workspace_endpoint(workspace);
        for leg in legs.iter_mut().filter(|l| from_here(l)) {
            let (Some(plan), Some(outcome)) = (&leg.plan, &leg.outcome) else {
                continue;
            };
            match self.keep_backup_ref(
                Path::new(&plan.dest_dir),
                &leg.row.branch_name,
                &outcome.snapshot.snapshot,
            ) {
                Ok(backup_ref) => leg.backup_ref = Some(backup_ref),
                Err(e) => failures.push(format!("{}: {e}", leg.row.branch_name)),
            }
            match back_up_ignored(endpoint.as_ref(), plan, outcome.ignored.as_ref()) {
                Ok(ignored) => {
                    leg.warnings.extend(skipped_warning(&ignored, plan));
                    leg.ignored_backup = Some(ignored);
                }
                Err(e) => failures.push(format!(
                    "{}: couldn't back its ignored files up: {e}",
                    leg.row.branch_name
                )),
            }
        }
        // Every check in one exec, as close to the delete as it can be, the
        // ignored files each backup holds included: every repository's, as
        // a file can change while a later one's is backed up.
        let checks: Vec<_> = legs
            .iter()
            .filter(|l| from_here(l))
            .filter_map(|l| {
                Some((
                    l.plan.as_ref()?,
                    &l.outcome.as_ref()?.snapshot,
                    l.ignored_backup.as_ref(),
                ))
            })
            .collect();
        if !checks.is_empty() {
            match unchanged_all(endpoint.as_ref(), &checks) {
                Ok(answers) => {
                    for ((plan, _, _), answer) in checks.iter().zip(answers) {
                        let branch = &plan.source_branch;
                        match answer {
                            Ok(None) => {}
                            Ok(Some(what)) => changed.push(format!("{branch}: {what}")),
                            Err(e) => {
                                failures.push(format!("couldn't check {branch} is unchanged: {e}"))
                            }
                        }
                    }
                }
                Err(e) => failures.push(format!("couldn't check it is unchanged: {e}")),
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

    /// Undo everything the transfer recorded, best-effort; see
    /// [`undo::roll_back`]. The bundles on this machine stay.
    pub(super) fn roll_back(&self) {
        self.emit(TransferPhase::RollingBack, None, None, None);
        let recorded = self.journal.borrow().undo.clone();
        let left = undo::roll_back(&recorded, self.host, self.guard.transfer_id());
        {
            let mut journal = self.journal.borrow_mut();
            journal.warnings.extend(left);
            journal.forget_added_excludes();
        }
        self.close_journal(backups::ROLLED_BACK, None, None);
    }
}

/// Said when a source's ignored files were too many to back up, so removing
/// the source deletes them.
fn skipped_warning(ignored: &IgnoredFiles, plan: &CarryPlan) -> Option<TransferIssue> {
    if !ignored.over_cap {
        return None;
    }
    let mb = |bytes: u64| bytes.div_ceil(1024 * 1024);
    let (files, were, they) = match ignored.files {
        1 => ("1 ignored file".to_string(), "wasn't", "it's"),
        n => (format!("{n} ignored files"), "weren't", "they're"),
    };
    Some(TransferIssue::new(
        "ignored_backup_skipped",
        format!(
            "{}'s {files} ({} MB) {were} backed up: {they} over the {} MB a transfer keeps.",
            plan.source_branch,
            mb(ignored.bytes),
            mb(plan.ignored_cap)
        ),
    ))
}
