//! Bounded retry of queued starts that fail before their claim.
//!
//! The stretch of a queued start before `QueuedSessionClaim::claim` — launch
//! context, provider resolution, the remote branch context (an SSH round trip
//! to the workspace), `review_tip_sha` / `commit_pre_head_sha` (git reads
//! that fail on a held `index.lock`) — fails transiently as well as for good.
//! Ending the row on the first failure turned a one-off blip into a crashed
//! session the user had to re-create; leaving it `queued` with nothing to
//! re-kick the branch parked it, and every row behind it, indefinitely. So
//! the drain retries a bounded number of times with a growing delay, and ends
//! the row only when the attempts are spent.
//!
//! The count lives here, in memory on the kicker, not on the row: a column
//! would be a schema change for what is a scheduling detail, and a restart is
//! a fresh first attempt anyway. Entries are pruned on every drain to the rows
//! still queued on the branch, so a row a Stop or a forced start took leaves
//! nothing behind.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The delay before each retry of a queued row whose pre-claim start failed,
/// indexed by how many starts have failed so far; the failure after the last
/// entry ends the row. Short enough that an MCP caller polling
/// `wait_for_repo_session` still sees `queued` resolve within its wait.
pub(crate) const QUEUED_START_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_secs(5), Duration::from_secs(30)];

/// How many pre-claim start failures a queued row absorbs before it is ended:
/// one per delay, plus the one that ends it.
pub(crate) const MAX_QUEUED_START_ATTEMPTS: u32 = QUEUED_START_RETRY_DELAYS.len() as u32 + 1;

/// What the drain does with a queued row whose start just failed before its
/// claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueuedStartRetry {
    /// Attempts remain: leave the row queued and kick the branch again after
    /// `delay`. `failures` is how many starts have failed so far.
    After { failures: u32, delay: Duration },
    /// The attempts are spent after `failures` failed starts: end the row.
    GiveUp { failures: u32 },
}

/// The retry for a row whose `failures`th start just failed.
pub(crate) fn queued_start_retry(failures: u32) -> QueuedStartRetry {
    match QUEUED_START_RETRY_DELAYS.get(failures as usize - 1) {
        Some(delay) => QueuedStartRetry::After {
            failures,
            delay: *delay,
        },
        None => QueuedStartRetry::GiveUp { failures },
    }
}

struct Attempt {
    failures: u32,
    /// When the next start may be tried; a kick before then leaves the row
    /// where it is rather than spending an attempt on it.
    retry_at: Instant,
}

/// Per-row pre-claim start failures, by branch.
#[derive(Default)]
pub(crate) struct QueuedStartAttempts {
    by_branch: Mutex<HashMap<String, HashMap<String, Attempt>>>,
}

impl QueuedStartAttempts {
    /// Record that `session_id`'s start failed at `now`. On `GiveUp` the entry
    /// is dropped, since the row is about to be ended.
    pub(crate) fn record_failure(
        &self,
        branch_id: &str,
        session_id: &str,
        now: Instant,
    ) -> QueuedStartRetry {
        let mut by_branch = self.by_branch.lock().unwrap();
        let rows = by_branch.entry(branch_id.to_string()).or_default();
        let failures = rows.get(session_id).map_or(0, |a| a.failures) + 1;
        let retry = queued_start_retry(failures);
        match retry {
            QueuedStartRetry::After { delay, .. } => {
                rows.insert(
                    session_id.to_string(),
                    Attempt {
                        failures,
                        retry_at: now + delay,
                    },
                );
            }
            QueuedStartRetry::GiveUp { .. } => {
                rows.remove(session_id);
                if rows.is_empty() {
                    by_branch.remove(branch_id);
                }
            }
        }
        retry
    }

    /// Whether `session_id` is waiting out the delay from a failed start.
    pub(crate) fn in_backoff(&self, branch_id: &str, session_id: &str, now: Instant) -> bool {
        self.by_branch
            .lock()
            .unwrap()
            .get(branch_id)
            .and_then(|rows| rows.get(session_id))
            .is_some_and(|attempt| now < attempt.retry_at)
    }

    /// Drop the branch's entries for rows that are no longer queued.
    pub(crate) fn retain_queued(&self, branch_id: &str, still_queued: impl Fn(&str) -> bool) {
        let mut by_branch = self.by_branch.lock().unwrap();
        if let Some(rows) = by_branch.get_mut(branch_id) {
            rows.retain(|session_id, _| still_queued(session_id));
            if rows.is_empty() {
                by_branch.remove(branch_id);
            }
        }
    }

    /// Drop every entry for a branch that is gone.
    pub(crate) fn forget_branch(&self, branch_id: &str) {
        self.by_branch.lock().unwrap().remove(branch_id);
    }

    #[cfg(test)]
    pub(crate) fn failures(&self, branch_id: &str, session_id: &str) -> u32 {
        self.by_branch
            .lock()
            .unwrap()
            .get(branch_id)
            .and_then(|rows| rows.get(session_id))
            .map_or(0, |a| a.failures)
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.by_branch.lock().unwrap().is_empty()
    }
}
