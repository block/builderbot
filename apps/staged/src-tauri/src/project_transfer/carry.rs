//! Carrying one repository's branch from one end to the other.
//!
//! [`carry_repo`] snapshots the source, brings the bundle to this machine
//! (where it stays as the backup), sends it on to the destination, and lands
//! it there. The steps either side of it — preparing the destination and
//! reporting its [`origin_tips`] first, adding a local worktree and running
//! [`finish`] in it after, [`cleanup`] on failure, and checking the source is
//! [`unchanged`] before deleting it — are the orchestrator's.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};

use super::endpoint::TransferEndpoint;
use super::scripts::{
    self, ApplyMode, ApplyReport, ApplyRequest, BundleFile, OriginTips, ScriptReport, Snapshot,
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
    /// The bundle's file name at each end, unique within the transfer.
    pub bundle_name: String,
    /// Where the bundle is kept on this machine.
    pub local_bundle: PathBuf,
    /// The source repository's object store plus its untracked files: what
    /// snapshotting and applying take time in proportion to, so a Blox end
    /// waits long enough for them.
    pub source_bytes: u64,
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
    pub applied: ApplyReport,
    /// Whether the destination lacked the commits the first bundle was cut
    /// against, so it was re-bundled whole.
    pub retried_without_exclusions: bool,
}

/// A step of [`carry_repo_with`], reported before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarryStep {
    Snapshotting,
    /// Moving a bundle of this many bytes.
    Moving {
        bytes: u64,
    },
    Applying,
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
    on_step: &mut dyn FnMut(CarryStep) -> Result<(), TransferError>,
) -> Result<CarryOutcome, TransferError> {
    check_transfer_id(&plan.transfer_id)?;
    let source_bundle = source.scratch_path(&plan.transfer_id, &plan.bundle_name)?;
    let dest_bundle = dest.scratch_path(&plan.transfer_id, &plan.bundle_name)?;

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

    let mut bundle = snapshot.bundle.clone();
    on_step(CarryStep::Moving { bytes: bundle.size })?;
    move_bundle(source, dest, &source_bundle, plan, &dest_bundle, &bundle)?;
    on_step(CarryStep::Applying)?;
    let mut retried = false;
    let applied = match land(dest, plan, &snapshot, &dest_bundle, &bundle) {
        Err(TransferError::MissingPrerequisites { message, .. }) if snapshot.excluded > 0 => {
            log::warn!(
                "[project_transfer] {} lacks the bundle's prerequisites ({message}); re-bundling {} without exclusions",
                dest.describe(),
                plan.source_branch
            );
            bundle = rebundle(source, plan, &source_bundle)?;
            on_step(CarryStep::Moving { bytes: bundle.size })?;
            move_bundle(source, dest, &source_bundle, plan, &dest_bundle, &bundle)?;
            on_step(CarryStep::Applying)?;
            retried = true;
            land(dest, plan, &snapshot, &dest_bundle, &bundle)?
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
        applied,
        retried_without_exclusions: retried,
    })
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
/// verify, and drop the temporary ref.
pub fn finish(
    dest: &dyn TransferEndpoint,
    plan: &CarryPlan,
    outcome: &CarryOutcome,
    checkout_dir: &str,
) -> Result<ApplyReport, TransferError> {
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
        },
        plan.source_bytes + outcome.bundle.size,
    )
}

/// Whether `plan`'s source checkout is still what `snapshot` captured: on
/// its branch at the same `HEAD`, with the same index and working tree. Read
/// only. `None` when it is; otherwise what changed.
pub fn unchanged(
    source: &dyn TransferEndpoint,
    plan: &CarryPlan,
    snapshot: &Snapshot,
) -> Result<Option<String>, TransferError> {
    check_transfer_id(&plan.transfer_id)?;
    let scratch_index =
        source.scratch_path(&plan.transfer_id, &format!("{}.index", plan.bundle_name))?;
    let report = run_step_sized(
        source,
        "verify",
        scripts::VERIFY_SCRIPT,
        &scripts::verify_args(
            &plan.source_dir,
            &plan.source_branch,
            snapshot,
            &scratch_index,
        ),
        plan.source_bytes,
    )?;
    scripts::parse_verify(&report)
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
    let report = run_step_sized(
        source,
        "snapshot",
        scripts::SNAPSHOT_SCRIPT,
        &scripts::snapshot_args(
            &plan.source_dir,
            &plan.source_branch,
            source_bundle,
            &plan.transfer_id,
            &plan.exclude_tips,
        ),
        plan.source_bytes,
    )?;
    Snapshot::parse(&report, &plan.source_branch)
}

fn rebundle(
    source: &dyn TransferEndpoint,
    plan: &CarryPlan,
    source_bundle: &str,
) -> Result<BundleFile, TransferError> {
    let report = run_step_sized(
        source,
        "rebundle",
        scripts::REBUNDLE_SCRIPT,
        &scripts::rebundle_args(&plan.source_dir, source_bundle, &plan.transfer_id),
        plan.source_bytes,
    )?;
    scripts::parse_rebundle(&report)
}

fn land(
    dest: &dyn TransferEndpoint,
    plan: &CarryPlan,
    snapshot: &Snapshot,
    dest_bundle: &str,
    bundle: &BundleFile,
) -> Result<ApplyReport, TransferError> {
    let (mode, restore) = match plan.landing {
        Landing::Checkout => (ApplyMode::Checkout, plan.include_uncommitted),
        Landing::Branch => (ApplyMode::Branch, false),
    };
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
            restore,
        },
        plan.source_bytes + bundle.size,
    )
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

/// Bring the bundle from the source to this machine, check it, and send it to
/// the destination. Either leg is a no-op when the file is already there.
fn move_bundle(
    source: &dyn TransferEndpoint,
    dest: &dyn TransferEndpoint,
    source_bundle: &str,
    plan: &CarryPlan,
    dest_bundle: &str,
    bundle: &BundleFile,
) -> Result<(), TransferError> {
    source.get_file(source_bundle, &plan.local_bundle, bundle.size)?;
    let actual = sha256_file(&plan.local_bundle)?;
    if actual != bundle.sha256 {
        return Err(TransferError::Checksum {
            expected: bundle.sha256.clone(),
            actual,
        });
    }
    dest.put_file(&plan.local_bundle, dest_bundle)
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
