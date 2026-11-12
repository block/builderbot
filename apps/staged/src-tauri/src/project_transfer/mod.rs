//! Moving a project between local worktrees and a Blox workspace.
//!
//! Each repository's branch travels as a git bundle holding the branch tip and
//! two snapshot commits — the index, and the working tree including untracked
//! files — built with plumbing so the source is never written to (see
//! [`scripts`]) — with the Git LFS objects those refer to in an archive beside
//! it. The bundle comes through this machine, where it stays as the backup, is
//! applied at the destination and checked against tree hashes.
//! Only then does [`crate::store::Store::apply_project_transfer`] switch the
//! project's location, in one transaction.
//!
//! The transport is the scripts, the two [`TransferEndpoint`]s, and the
//! per-repository pipeline in [`carry_repo`]. Around it: [`preflight`] says
//! what a transfer would carry and what stops it, [`orchestrator`] moves the
//! whole project (rolling back on failure, with [`undo`] recording what to
//! take back), [`recovery`] finishes off at startup a transfer Staged never
//! did, [`registry`] keeps one transfer per project and refuses conflicting
//! work meanwhile, [`backups`] describes and prunes what is kept,
//! [`excludes`] takes a transfer's `info/exclude` entries out when their
//! worktree goes (under the lock in [`exclude_file`]), and [`commands`] is
//! the Tauri and web surface.

mod backups;
mod carry;
pub(crate) mod commands;
mod endpoint;
mod exclude_file;
mod excludes;
mod host;
mod orchestrator;
#[cfg(test)]
mod orchestrator_tests;
mod preflight;
pub(crate) mod preflight_scan;
mod recovery;
mod registry;
mod scripts;
#[cfg(test)]
mod tests;
mod undo;

pub use carry::{
    carry_repo, carry_repo_with, cleanup, finish, origin_tips, unchanged, unchanged_all,
    CarryOutcome, CarryPlan, CarryStep, Landing,
};
pub(crate) use commands::{
    cancel_project_transfer, get_project_transfer_preflight_impl, list_transferring_projects,
    prune_backups, recover_interrupted_transfers, transfer_project_impl,
};
pub use endpoint::{BloxEndpoint, LocalEndpoint, ScriptOutput, TransferEndpoint};
pub(crate) use excludes::release_worktree_excludes;
pub use orchestrator::{
    ProjectTransferResult, RepoTransferResult, TransferOptions, TransferPhase, TransferProgress,
};
pub use preflight::{
    ProjectTransferPreflight, RepoPreflight, RepoTransferState, TransferDirection, TransferIssue,
};
pub(crate) use preflight_scan::{
    cancel_project_transfer_preflight_impl, start_project_transfer_preflight_impl,
};
#[cfg(test)]
pub(crate) use registry::begin;
pub use registry::{
    ensure_branch_not_transferring, ensure_project_not_transferring,
    ensure_workspace_not_transferring, is_branch_transferring, TRANSFER_IN_PROGRESS,
};
pub use scripts::{
    ApplyReport, BundleFile, ConfigEntry, LfsCounts, LfsPayload, OriginTips, RemoteConfig,
    Snapshot, SnapshotWarning,
};

/// Why a transfer step failed.
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    /// A script ran and reported a failure: a blocker at the source, a
    /// verification mismatch at the destination.
    #[error("{step} failed on {endpoint}: {message}")]
    Script {
        step: &'static str,
        endpoint: String,
        message: String,
    },
    /// The destination lacks commits the bundle was cut against.
    #[error("{endpoint} is missing commits the bundle needs: {message}")]
    MissingPrerequisites { endpoint: String, message: String },
    /// A script could not be run, or a file could not be moved, at all.
    #[error("cannot reach {endpoint}: {message}")]
    Exec { endpoint: String, message: String },
    /// The bundle (or LFS archive) that arrived is not the one that was made.
    #[error("{file} changed in transit (expected sha256 {expected}, got {actual})")]
    Checksum {
        file: &'static str,
        expected: String,
        actual: String,
    },
    /// The caller asked for something the transfer cannot do safely.
    #[error("invalid transfer plan: {0}")]
    InvalidPlan(String),
    /// A script's output did not say what it should have.
    #[error("unexpected output from {step}: {message}")]
    Protocol { step: &'static str, message: String },
    /// The user cancelled the transfer before it switched the project over.
    #[error("the transfer was cancelled")]
    Cancelled,
}

/// Whether `id` can name a transfer: it becomes a ref component and a
/// directory name at both ends, so nothing but ASCII letters, digits and `-`.
pub fn is_valid_transfer_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
