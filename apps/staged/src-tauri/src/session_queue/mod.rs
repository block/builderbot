//! The backend's queue controller.
//!
//! The branch session queue is a per-branch FIFO of `sessions` rows with
//! `status = 'queued'`; a `running` row is the branch's busy marker. Moving the
//! queue along used to be split between the backend (terminal states) and
//! the frontend (startup, provisioning, deletes), so a branch with no client
//! attached — a closed tab, a headless MCP-driven session — could sit parked
//! behind a row nothing would ever release.
//!
//! This module makes progression a backend concern end to end:
//!
//! - [`kick`] is the single entry point every backend event that can unblock
//!   a branch goes through — a terminal state, a cancel or delete, a worktree
//!   or workspace becoming usable, an immediate pull finishing, app startup.
//!   Kicks are coalesced per branch and run on their own task, so a kick is
//!   never dropped with a request future and overlapping kicks collapse into
//!   "run once more after the current drain".
//! - [`sweep`] is the safety net for the `running` marker itself: a row this
//!   process owns that nothing in the process is registered as owning — no
//!   runner token, no liveness hold — is ended and its branch kicked. No
//!   clock is involved; every writer of a `running` row keeps its registry
//!   entry for the row's whole time in `running`.
//!
//! Every kick logs its source at `info`, every sweep action is a `warn`, and
//! the Doctor's "Session Queue" check reports [`sweep::queue_health`], so a
//! stuck or rescued queue is visible without reading the log.

pub(crate) mod kick;
pub(crate) mod sweep;
#[cfg(test)]
mod tests;

pub(crate) use kick::{kick, kick_all_with_queued_work, BranchQueueKicker, KickSource};
