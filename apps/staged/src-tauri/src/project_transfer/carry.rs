//! Carrying one repository's branch from one end to the other.
//!
//! [`carry_repo`] snapshots the source, brings the bundle — and the LFS
//! archive beside it, when the snapshot has one, and an archive of the
//! source's ignored files when the plan carries them — to this machine (where
//! they stay as the backup), sends them on to the destination, and lands them
//! there. The steps either side of it — preparing the destination and
//! reporting its [`origin_tips`] first, adding a local worktree and running
//! [`finish`] in it after, [`cleanup`] on failure, and before deleting the
//! source, [`back_up_ignored`] and checking it is [`unchanged`] — are the
//! orchestrator's.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};

use super::endpoint::TransferEndpoint;
use super::scripts::{
    self, ApplyMode, ApplyReport, ApplyRequest, BundleFile, IgnoredAtDest, IgnoredFiles,
    LfsPayload, OriginTips, ScriptReport, Snapshot,
};
use super::{is_valid_transfer_id, TransferError};

/// How the destination takes the branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    /// Check the branch out in the destination's own checkout and restore its
    /// working state there, in one step. A Blox clone.
    Checkout,
    /// Create the branch in the destination repository without touching any
    /// checkout. The shared local clone: the caller then adds a worktree for
    /// the branch and runs [`finish`] in it.
    Branch,
}

/// One repository's part in a transfer.
#[derive(Debug, Clone)]
pub struct CarryPlan {
    pub transfer_id: String,
    /// The source checkout, as the source endpoint sees it.
    pub source_dir: String,
    /// The branch checked out there.
    pub source_branch: String,
    /// The destination checkout ([`Landing::Checkout`]) or repository
    /// ([`Landing::Branch`]), as the destination endpoint sees it.
    pub dest_dir: String,
    /// The branch's name at the destination; differs from `source_branch`
    /// when the destination already used the name.
    pub dest_branch: String,
    pub landing: Landing,
    /// Restore the uncommitted state at the destination. Off, it stays behind
    /// in the backup bundle only.
    pub include_uncommitted: bool,
    /// The destination's [`origin_tips`], left out of the bundle.
    pub exclude_tips: Vec<String>,
    /// The names on origin `exclude_tips` are the tips of. The source's own
    /// remote-tracking refs for them are left out too, so a destination that
    /// fetched origin after the source last did still gets a bundle cut
    /// against what they share.
    pub origin_branches: Vec<String>,
    /// The bundle's file name at each end, unique within the transfer.
    pub bundle_name: String,
    /// Where the bundle is kept on this machine.
    pub local_bundle: PathBuf,
    /// The source repository's object store: what bundling the whole
    /// history takes time in proportion to.
    pub repo_bytes: u64,
    /// The source's untracked files. Snapshotting, applying and verifying
    /// hash and write what changed, so beside the bundle this is what they
    /// take time in proportion to — not the whole repository, which a
    /// stat-cached `add -A` and a bundle cut against origin never read.
    pub untracked_bytes: u64,
    /// The LFS objects the source has for the branch, by the preflight's
    /// count: what the snapshot packs beside the bundle.
    pub lfs_bytes: u64,
    /// Carry the source's ignored files and place them at the destination.
    /// Off, they are only backed up, when the source is removed.
    pub include_ignored: bool,
    /// The most to archive of them; past it they are neither carried nor
    /// backed up.
    pub ignored_cap: u64,
    /// Their size by the preflight's count: what archiving them reads.
    pub ignored_bytes: u64,
}

impl CarryPlan {
    /// Where the snapshot's LFS archive is kept on this machine, beside the
    /// bundle.
    pub fn local_lfs(&self) -> PathBuf {
        let mut path = self.local_bundle.as_os_str().to_owned();
        path.push(".lfs.tar");
        PathBuf::from(path)
    }

    /// The ignored files' archive's name at each end.
    pub fn ignored_name(&self) -> String {
        IgnoredFiles::archive_name(&self.bundle_name)
    }

    /// Where the ignored files' archive is kept on this machine, beside the
    /// bundle.
    pub fn local_ignored(&self) -> PathBuf {
        self.local_bundle.with_file_name(self.ignored_name())
    }

    /// The ignored files' excludes list's name at each end.
    pub fn ignored_excludes_name(&self) -> String {
        IgnoredFiles::excludes_name(&self.bundle_name)
    }

    /// Where the excludes list is kept on this machine, beside the archive.
    pub fn local_ignored_excludes(&self) -> PathBuf {
        self.local_bundle
            .with_file_name(self.ignored_excludes_name())
    }

    /// Where [`finish`], at a local destination, notes each entry the
    /// excludes list adds to the clone's `info/exclude` before adding it:
    /// `0-widgets.bundle`'s go in `0-widgets.info-exclude.added`, beside it.
    pub fn local_excludes_journal(&self) -> PathBuf {
        let stem = self
            .bundle_name
            .strip_suffix(".bundle")
            .unwrap_or(&self.bundle_name);
        self.local_bundle
            .with_file_name(format!("{stem}.info-exclude.added"))
    }
}

/// A repository carried to the destination.
#[derive(Debug, Clone)]
pub struct CarryOutcome {
    pub snapshot: Snapshot,
    /// The bundle that landed: the snapshot's, or a re-bundle without
    /// exclusions.
    pub bundle: BundleFile,
    /// The bundle's path at the destination.
    pub dest_bundle: String,
    /// The LFS archive's, when the snapshot has one.
    pub dest_lfs: Option<String>,
    /// The ignored files archived for the destination, when the plan carries
    /// them.
    pub ignored: Option<IgnoredFiles>,
    /// Their archive's path at the destination, when there was one to send.
    pub dest_ignored: Option<String>,
    /// Its excludes list's path at the destination, when it had one.
    pub dest_ignored_excludes: Option<String>,
    pub applied: ApplyReport,
    /// Whether the destination lacked the commits the first bundle was cut
    /// against, so it was re-bundled whole.
    pub retried_without_exclusions: bool,
}

/// A step of [`carry_repo_with`], reported before it starts.
#[derive(Debug, Clone, Copy)]
pub enum CarryStep<'a> {
    Snapshotting,
    /// Moving a bundle (and any LFS or ignored files' archive) of this many
    /// bytes.
    Moving {
        bytes: u64,
    },
    /// Landing `snapshot`: what the destination is about to gain, for a
    /// caller that records it first.
    Applying {
        snapshot: &'a Snapshot,
    },
}

/// Snapshot `plan`'s source, move the bundle, and land it at the destination.
///
/// The source is only read (and gains the temporary snapshot ref). When the
/// destination turns out to lack the commits the bundle was cut against —
/// origin rewritten between its fetch and this apply, say — the snapshot is
/// re-bundled without exclusions and sent again, once.
pub fn carry_repo(
    source: &dyn TransferEndpoint,
    dest: &dyn TransferEndpoint,
    plan: &CarryPlan,
) -> Result<CarryOutcome, TransferError> {
    carry_repo_with(source, dest, plan, &mut |_| Ok(()))
}

/// [`carry_repo`], calling `on_step` before each step: for progress, and as
/// a checkpoint — an `Err` from it stops the carry there.
pub fn carry_repo_with(
    source: &dyn TransferEndpoint,
    dest: &dyn TransferEndpoint,
    plan: &CarryPlan,
    on_step: &mut dyn FnMut(CarryStep<'_>) -> Result<(), TransferError>,
) -> Result<CarryOutcome, TransferError> {
    check_transfer_id(&plan.transfer_id)?;
    let source_bundle = source.scratch_path(&plan.transfer_id, &plan.bundle_name)?;
    let dest_bundle = dest.scratch_path(&plan.transfer_id, &plan.bundle_name)?;
    let lfs_name = LfsPayload::archive_name(&plan.bundle_name);

    let started = Instant::now();
    on_step(CarryStep::Snapshotting)?;
    let snapshot = snapshot(source, plan, &source_bundle)?;
    log::info!(
        "[project_transfer] snapshot of {} on {}: {} bytes, {} tips excluded, uncommitted={} ({} ms)",
        plan.source_branch,
        source.describe(),
        snapshot.bundle.size,
        snapshot.excluded,
        snapshot.has_uncommitted_changes(),
        started.elapsed().as_millis()
    );

    let ignored = if plan.include_ignored {
        let ignored = archive_ignored(source, plan)?;
        if ignored.over_cap {
            return Err(TransferError::Script {
                step: "ignored files",
                endpoint: source.describe(),
                message: format!(
                    "the ignored files come to {} bytes, more than the {} a transfer carries",
                    ignored.bytes, plan.ignored_cap
                ),
            });
        }
        Some(ignored)
    } else {
        None
    };
    let ignored_archive = ignored.as_ref().and_then(|i| i.archive.as_ref());
    let ignored_excludes = ignored
        .as_ref()
        .filter(|i| i.archive.is_some())
        .and_then(|i| i.excludes.as_ref());

    let mut bundle = snapshot.bundle.clone();
    let lfs_bytes = snapshot.lfs.as_ref().map_or(0, |lfs| lfs.archive.size);
    on_step(CarryStep::Moving {
        bytes: bundle.size
            + lfs_bytes
            + ignored_archive.map_or(0, |a| a.size)
            + ignored_excludes.map_or(0, |e| e.size),
    })?;
    move_file(
        source,
        dest,
        "the bundle",
        &source_bundle,
        &plan.local_bundle,
        &dest_bundle,
        &bundle,
    )?;
    let dest_lfs = match &snapshot.lfs {
        Some(lfs) => {
            let dest_lfs = dest.scratch_path(&plan.transfer_id, &lfs_name)?;
            move_file(
                source,
                dest,
                "the LFS archive",
                &source.scratch_path(&plan.transfer_id, &lfs_name)?,
                &plan.local_lfs(),
                &dest_lfs,
                &lfs.archive,
            )?;
            Some(dest_lfs)
        }
        None => None,
    };
    let dest_ignored = match ignored_archive {
        Some(archive) => {
            let dest_ignored = dest.scratch_path(&plan.transfer_id, &plan.ignored_name())?;
            move_file(
                source,
                dest,
                "the ignored files",
                &source.scratch_path(&plan.transfer_id, &plan.ignored_name())?,
                &plan.local_ignored(),
                &dest_ignored,
                archive,
            )?;
            Some(dest_ignored)
        }
        None => None,
    };
    let dest_ignored_excludes = match ignored_excludes {
        Some(list) => {
            let name = plan.ignored_excludes_name();
            let dest_list = dest.scratch_path(&plan.transfer_id, &name)?;
            move_file(
                source,
                dest,
                "the ignored files to exclude",
                &source.scratch_path(&plan.transfer_id, &name)?,
                &plan.local_ignored_excludes(),
                &dest_list,
                list,
            )?;
            Some(dest_list)
        }
        None => None,
    };
    on_step(CarryStep::Applying {
        snapshot: &snapshot,
    })?;
    let mut retried = false;
    let land_bundle = |bundle: &BundleFile| {
        land(
            dest,
            plan,
            &snapshot,
            &dest_bundle,
            bundle,
            dest_lfs.as_deref(),
            dest_ignored
                .as_deref()
                .zip(ignored_archive)
                .map(|(archive_path, archive)| IgnoredAtDest {
                    archive_path,
                    archive,
                    excludes: dest_ignored_excludes.as_deref().zip(ignored_excludes),
                    // A workspace's clone is fresh, and goes whole on
                    // rollback.
                    excludes_journal: None,
                }),
        )
    };
    let applied = match land_bundle(&bundle) {
        Err(TransferError::MissingPrerequisites { message, .. }) if snapshot.excluded > 0 => {
            log::warn!(
                "[project_transfer] {} lacks the bundle's prerequisites ({message}); re-bundling {} without exclusions",
                dest.describe(),
                plan.source_branch
            );
            bundle = rebundle(source, plan, &source_bundle)?;
            on_step(CarryStep::Moving { bytes: bundle.size })?;
            move_file(
                source,
                dest,
                "the bundle",
                &source_bundle,
                &plan.local_bundle,
                &dest_bundle,
                &bundle,
            )?;
            on_step(CarryStep::Applying {
                snapshot: &snapshot,
            })?;
            retried = true;
            land_bundle(&bundle)?
        }
        other => other?,
    };
    log::info!(
        "[project_transfer] landed {} as {} on {} ({} ms)",
        plan.source_branch,
        plan.dest_branch,
        dest.describe(),
        started.elapsed().as_millis()
    );
    Ok(CarryOutcome {
        snapshot,
        bundle,
        dest_bundle,
        dest_lfs,
        ignored,
        dest_ignored,
        dest_ignored_excludes,
        applied,
        retried_without_exclusions: retried,
    })
}

/// Archive the source's ignored files into the transfer's backup on this
/// machine, `plan.local_ignored()`, right before the source is removed: an
/// archive the source writes, brought here and checked. Over the cap, or with
/// none, nothing is written (see [`IgnoredFiles`]). `carried` is what the
/// carry already brought, which is not fetched again when the source's
/// archive comes out the same.
pub fn back_up_ignored(
    source: &dyn TransferEndpoint,
    plan: &CarryPlan,
    carried: Option<&IgnoredFiles>,
) -> Result<IgnoredFiles, TransferError> {
    check_transfer_id(&plan.transfer_id)?;
    let ignored = archive_ignored(source, plan)?;
    if let Some(archive) = &ignored.archive {
        let local = plan.local_ignored();
        let same = carried.and_then(|c| c.archive.as_ref()) == Some(archive)
            && sha256_file(&local).is_ok_and(|sha| sha == archive.sha256);
        if !same {
            fetch_file(
                source,
                "the ignored files",
                &source.scratch_path(&plan.transfer_id, &plan.ignored_name())?,
                &local,
                archive,
            )?;
        }
    }
    Ok(ignored)
}

/// Run [`scripts::IGNORED_ARCHIVE_SCRIPT`] in the source's scratch directory.
fn archive_ignored(
    source: &dyn TransferEndpoint,
    plan: &CarryPlan,
) -> Result<IgnoredFiles, TransferError> {
    let archive = source.scratch_path(&plan.transfer_id, &plan.ignored_name())?;
    let report = run_step_sized(
        source,
        "ignored files",
        scripts::IGNORED_ARCHIVE_SCRIPT,
        &scripts::ignored_archive_args(
            &plan.source_dir,
            &archive,
            plan.ignored_cap,
            &source.scratch_path(&plan.transfer_id, &plan.ignored_excludes_name())?,
        ),
        plan.ignored_bytes.min(plan.ignored_cap),
    )?;
    IgnoredFiles::parse(&report)
}

/// Fetch `branches` from origin at `endpoint` and report their tips.
pub fn origin_tips(
    endpoint: &dyn TransferEndpoint,
    repo_dir: &str,
    branches: &[&str],
) -> Result<OriginTips, TransferError> {
    let report = run_step(
        endpoint,
        "origin tips",
        scripts::ORIGIN_TIPS_SCRIPT,
        &scripts::origin_tips_args(repo_dir, branches),
    )?;
    Ok(OriginTips::parse(&report))
}

/// Finish a [`Landing::Branch`] carry in `checkout_dir`, a worktree on
/// `plan.dest_branch`: restore the uncommitted state if the plan includes it,
/// place the ignored files it carried, verify, and drop the temporary ref.
/// Each entry the ignored files' excludes list adds to the clone's
/// `info/exclude` is noted in [`CarryPlan::local_excludes_journal`] first,
/// for a rollback to take back out.
pub fn finish(
    dest: &dyn TransferEndpoint,
    plan: &CarryPlan,
    outcome: &CarryOutcome,
    checkout_dir: &str,
) -> Result<ApplyReport, TransferError> {
    let journal = plan.local_excludes_journal();
    let journal = journal.to_str().ok_or_else(|| {
        TransferError::InvalidPlan(format!("not a UTF-8 path: {}", journal.display()))
    })?;
    apply(
        dest,
        &ApplyRequest {
            mode: ApplyMode::Finish,
            repo_dir: checkout_dir,
            bundle_path: &outcome.dest_bundle,
            bundle: &outcome.bundle,
            transfer_id: &plan.transfer_id,
            branch: &plan.dest_branch,
            snapshot: &outcome.snapshot,
            restore: plan.include_uncommitted,
            lfs_path: outcome.dest_lfs.as_deref(),
            ignored: dest_ignored(outcome).map(|ignored| IgnoredAtDest {
                excludes_journal: Some(journal),
                ..ignored
            }),
        },
        // Checking LFS content out writes every object again.
        plan.untracked_bytes
            + outcome.bundle.size
            + lfs_archive_bytes(&outcome.snapshot)
            + placing_bytes(carried_archive(outcome)),
    )
}

fn carried_archive(outcome: &CarryOutcome) -> Option<&BundleFile> {
    outcome.ignored.as_ref().and_then(|i| i.archive.as_ref())
}

/// The ignored files `outcome` sent to the destination, with their excludes
/// list when it went too.
fn dest_ignored(outcome: &CarryOutcome) -> Option<IgnoredAtDest<'_>> {
    let ignored = outcome.ignored.as_ref()?;
    Some(IgnoredAtDest {
        archive_path: outcome.dest_ignored.as_deref()?,
        archive: ignored.archive.as_ref()?,
        excludes: outcome
            .dest_ignored_excludes
            .as_deref()
            .zip(ignored.excludes.as_ref()),
        excludes_journal: None,
    })
}

/// Placing ignored files unpacks their archive and copies each file out.
fn placing_bytes(archive: Option<&BundleFile>) -> u64 {
    archive.map_or(0, |a| a.size * 2)
}

/// Whether `plan`'s source checkout is still what `snapshot` captured: on
/// its branch at the same `HEAD`, with the same index and working tree. Read
/// only. `None` when it is; otherwise what changed.
pub fn unchanged(
    source: &dyn TransferEndpoint,
    plan: &CarryPlan,
    snapshot: &Snapshot,
) -> Result<Option<String>, TransferError> {
    unchanged_all(source, &[(plan, snapshot)])?
        .pop()
        .unwrap_or_else(|| Err(no_answer(source, plan)))
}

/// [`unchanged`] for several checkouts at the same end, in one script run: a
/// workspace's, checked right before it is deleted. `Err` when the script
/// could not run at all; otherwise one answer per checkout, in order.
pub fn unchanged_all(
    source: &dyn TransferEndpoint,
    checks: &[(&CarryPlan, &Snapshot)],
) -> Result<Vec<Result<Option<String>, TransferError>>, TransferError> {
    let mut args = Vec::new();
    let mut payload_bytes = 0;
    for (plan, snapshot) in checks {
        check_transfer_id(&plan.transfer_id)?;
        let scratch_index =
            source.scratch_path(&plan.transfer_id, &format!("{}.index", plan.bundle_name))?;
        args.extend(scripts::verify_args(
            &plan.source_dir,
            &plan.source_branch,
            snapshot,
            &scratch_index,
        ));
        payload_bytes += plan.untracked_bytes + snapshot.bundle.size;
    }
    let output = source.run_script_sized(scripts::VERIFY_SCRIPT, &args, payload_bytes)?;
    let mut sections = scripts::sections(&output.stdout).into_iter();
    Ok(checks
        .iter()
        .map(|(plan, _)| match sections.next() {
            Some(section) => {
                scripts::parse_verify(&section).map_err(|message| TransferError::Script {
                    step: "verify",
                    endpoint: source.describe(),
                    message,
                })
            }
            None if !output.success && !output.stderr.trim().is_empty() => {
                Err(TransferError::Script {
                    step: "verify",
                    endpoint: source.describe(),
                    message: output.stderr.trim().to_string(),
                })
            }
            None => Err(no_answer(source, plan)),
        })
        .collect())
}

fn no_answer(source: &dyn TransferEndpoint, plan: &CarryPlan) -> TransferError {
    TransferError::Protocol {
        step: "verify",
        message: format!("no answer for {} on {}", plan.source_dir, source.describe()),
    }
}

/// Remove the transfer's temporary refs from `repo_dir` at `endpoint`, and
/// with `remove_scratch` its scratch directory there. Best-effort by design:
/// it runs on paths that already failed.
pub fn cleanup(
    endpoint: &dyn TransferEndpoint,
    repo_dir: &str,
    transfer_id: &str,
    remove_scratch: bool,
) -> Result<(), TransferError> {
    check_transfer_id(transfer_id)?;
    let scratch = if remove_scratch {
        Some(endpoint.scratch_dir(transfer_id)?)
    } else {
        None
    };
    run_step(
        endpoint,
        "cleanup",
        scripts::CLEANUP_SCRIPT,
        &scripts::cleanup_args(repo_dir, transfer_id, scratch.as_deref()),
    )?;
    Ok(())
}

/// The id names refs and a directory that [`cleanup`] removes recursively, so
/// it is checked before any of that happens.
fn check_transfer_id(id: &str) -> Result<(), TransferError> {
    if is_valid_transfer_id(id) {
        Ok(())
    } else {
        Err(TransferError::InvalidPlan(format!(
            "transfer id {id:?} must be ASCII letters, digits and '-'"
        )))
    }
}

fn snapshot(
    source: &dyn TransferEndpoint,
    plan: &CarryPlan,
    source_bundle: &str,
) -> Result<Snapshot, TransferError> {
    let mut report = run_step_sized(
        source,
        "snapshot",
        scripts::SNAPSHOT_SCRIPT,
        &scripts::snapshot_args(
            &plan.source_dir,
            &plan.source_branch,
            source_bundle,
            &plan.transfer_id,
            &plan.exclude_tips,
            &plan.origin_branches,
        ),
        plan.untracked_bytes + plan.lfs_bytes,
    )?;
    // Nothing to cut the bundle against: it is the whole history, which the
    // rebundle step writes in a run given time by the repository's size. Its
    // report completes the snapshot's.
    if ScriptReport::parse(&report).get("bundle") == Some("deferred") {
        report.push_str(&rebundle_report(source, plan, source_bundle)?);
    }
    Snapshot::parse(&report, &plan.source_branch)
}

fn rebundle(
    source: &dyn TransferEndpoint,
    plan: &CarryPlan,
    source_bundle: &str,
) -> Result<BundleFile, TransferError> {
    scripts::parse_rebundle(&rebundle_report(source, plan, source_bundle)?)
}

/// Bundle the whole of the snapshot's history, without exclusions.
fn rebundle_report(
    source: &dyn TransferEndpoint,
    plan: &CarryPlan,
    source_bundle: &str,
) -> Result<String, TransferError> {
    run_step_sized(
        source,
        "rebundle",
        scripts::REBUNDLE_SCRIPT,
        &scripts::rebundle_args(&plan.source_dir, source_bundle, &plan.transfer_id),
        plan.repo_bytes,
    )
}

fn land(
    dest: &dyn TransferEndpoint,
    plan: &CarryPlan,
    snapshot: &Snapshot,
    dest_bundle: &str,
    bundle: &BundleFile,
    dest_lfs: Option<&str>,
    dest_ignored: Option<IgnoredAtDest<'_>>,
) -> Result<ApplyReport, TransferError> {
    let mode = match plan.landing {
        Landing::Checkout => ApplyMode::Checkout,
        // The worktree's `finish` places them.
        Landing::Branch => ApplyMode::Branch,
    };
    let dest_ignored = dest_ignored.filter(|_| mode == ApplyMode::Checkout);
    // Unpacking the LFS objects writes them, and fetching what the source
    // lacked downloads the rest.
    let lfs_bytes = snapshot
        .lfs
        .as_ref()
        .map_or(0, |lfs| lfs.archive.size + lfs.counts.missing_bytes);
    apply(
        dest,
        &ApplyRequest {
            mode,
            repo_dir: &plan.dest_dir,
            bundle_path: dest_bundle,
            bundle,
            transfer_id: &plan.transfer_id,
            branch: &plan.dest_branch,
            snapshot,
            restore: plan.include_uncommitted,
            lfs_path: dest_lfs,
            ignored: dest_ignored,
        },
        plan.untracked_bytes
            + bundle.size
            + lfs_bytes
            + placing_bytes(dest_ignored.map(|d| d.archive)),
    )
}

fn lfs_archive_bytes(snapshot: &Snapshot) -> u64 {
    snapshot.lfs.as_ref().map_or(0, |lfs| lfs.archive.size)
}

/// Run [`scripts::APPLY_SCRIPT`] for `request`, whose work (verifying and
/// indexing the bundle, restoring the working tree) grows with
/// `payload_bytes`.
fn apply(
    dest: &dyn TransferEndpoint,
    request: &ApplyRequest<'_>,
    payload_bytes: u64,
) -> Result<ApplyReport, TransferError> {
    let report = run_step_sized(
        dest,
        "apply",
        scripts::APPLY_SCRIPT,
        &request.args(),
        payload_bytes,
    )?;
    let applied = ApplyReport::parse(&report, &request.snapshot.head)?;
    for warning in &applied.warnings {
        log::warn!(
            "[project_transfer] applying {} on {}: {warning}",
            request.branch,
            dest.describe()
        );
    }
    Ok(applied)
}

/// Bring a bundle or archive from the source to `local` on this machine,
/// check it, and send it to the destination. Either leg is a no-op when the
/// file is already there.
fn move_file(
    source: &dyn TransferEndpoint,
    dest: &dyn TransferEndpoint,
    file_name: &'static str,
    source_path: &str,
    local: &Path,
    dest_path: &str,
    file: &BundleFile,
) -> Result<(), TransferError> {
    fetch_file(source, file_name, source_path, local, file)?;
    dest.put_file(local, dest_path)
}

/// Bring a file from the source to `local` on this machine, and check it is
/// what the source described.
fn fetch_file(
    source: &dyn TransferEndpoint,
    file_name: &'static str,
    source_path: &str,
    local: &Path,
    file: &BundleFile,
) -> Result<(), TransferError> {
    source.get_file(source_path, local, file.size)?;
    let actual = sha256_file(local)?;
    if actual != file.sha256 {
        return Err(TransferError::Checksum {
            file: file_name,
            expected: file.sha256.clone(),
            actual,
        });
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, TransferError> {
    let io_error = |e: std::io::Error| TransferError::Exec {
        endpoint: "this machine".to_string(),
        message: format!("cannot read {}: {e}", path.display()),
    };
    let mut file = std::fs::File::open(path).map_err(io_error)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(io_error)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Run a script and turn its report into a result: an `error=` line, or a
/// failed exit without one, is the step's failure.
pub(super) fn run_step(
    endpoint: &dyn TransferEndpoint,
    step: &'static str,
    script: &str,
    args: &[String],
) -> Result<String, TransferError> {
    run_step_sized(endpoint, step, script, args, 0)
}

/// [`run_step`] for a step whose running time grows with `payload_bytes`;
/// see [`TransferEndpoint::run_script_sized`].
pub(super) fn run_step_sized(
    endpoint: &dyn TransferEndpoint,
    step: &'static str,
    script: &str,
    args: &[String],
    payload_bytes: u64,
) -> Result<String, TransferError> {
    let output = endpoint.run_script_sized(script, args, payload_bytes)?;
    let report = ScriptReport::parse(&output.stdout);
    if let Some(message) = report.get("error") {
        let message = message.trim().to_string();
        if report.get("error_kind") == Some("missing_prerequisites") {
            return Err(TransferError::MissingPrerequisites {
                endpoint: endpoint.describe(),
                message,
            });
        }
        return Err(TransferError::Script {
            step,
            endpoint: endpoint.describe(),
            message,
        });
    }
    if !output.success {
        let stderr = output.stderr.trim();
        return Err(TransferError::Script {
            step,
            endpoint: endpoint.describe(),
            message: if stderr.is_empty() {
                "the script exited with an error".to_string()
            } else {
                stderr.to_string()
            },
        });
    }
    Ok(output.stdout)
}
