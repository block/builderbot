//! Running the legs at once: a bounded pool of scoped threads over the legs,
//! grouped by repository.

use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use super::super::TransferError;
use super::{Failure, Leg, Run};

/// How many legs go at once. Each is a `git` process or two at each end and
/// a bundle on the wire: enough to overlap the waits, not enough to starve
/// the sessions on the same machine.
pub(super) const MAX_PARALLEL_LEGS: usize = 4;

#[cfg(test)]
thread_local! {
    /// A test's override of [`MAX_PARALLEL_LEGS`], on the thread that runs
    /// the transfer.
    static MAX_LEGS_FOR_TEST: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// While it lives, transfers run from this thread take their legs one after
/// another — for a test whose scenario is one leg finishing before the
/// next starts.
#[cfg(test)]
pub(crate) struct SerialLegs;

#[cfg(test)]
impl SerialLegs {
    pub(crate) fn for_this_thread() -> Self {
        MAX_LEGS_FOR_TEST.with(|m| m.set(Some(1)));
        Self
    }
}

#[cfg(test)]
impl Drop for SerialLegs {
    fn drop(&mut self) {
        MAX_LEGS_FOR_TEST.with(|m| m.set(None));
    }
}

fn max_parallel_legs() -> usize {
    #[cfg(test)]
    if let Some(n) = MAX_LEGS_FOR_TEST.with(|m| m.get()) {
        return n.max(1);
    }
    MAX_PARALLEL_LEGS
}

impl Run<'_> {
    /// Run `work` on every leg `filter` keeps, legs of different
    /// repositories at once — up to [`MAX_PARALLEL_LEGS`] at a time — and
    /// legs of one repository one after another in their order, since they
    /// share its clone and index. A single group runs on this thread.
    ///
    /// The first failure stops the legs still running at their next
    /// checkpoint (they fail as cancelled) and is what comes back, once
    /// every leg has stopped. A panic in a leg is re-raised here, likewise
    /// once every leg has stopped, with its message intact.
    pub(super) fn for_each_leg(
        &self,
        legs: &mut [Leg],
        filter: impl Fn(&Leg) -> bool,
        work: impl Fn(usize, &mut Leg) -> Result<(), Failure> + Sync,
    ) -> Result<(), Failure> {
        let mut groups: Vec<Vec<(usize, &mut Leg)>> = Vec::new();
        let mut group_of: HashMap<String, usize> = HashMap::new();
        for (i, leg) in legs.iter_mut().enumerate() {
            if !filter(leg) {
                continue;
            }
            let g = *group_of
                .entry(leg.source.github_repo.clone())
                .or_insert_with(|| {
                    groups.push(Vec::new());
                    groups.len() - 1
                });
            groups[g].push((i, leg));
        }
        let workers = groups.len().min(max_parallel_legs());
        if workers <= 1 {
            for (i, leg) in groups.into_iter().flatten() {
                work(i, leg)?;
            }
            return Ok(());
        }

        let queue = Mutex::new(VecDeque::from(groups));
        let failures: Mutex<Vec<Failure>> = Mutex::new(Vec::new());
        let panicked: Mutex<Option<Box<dyn std::any::Any + Send>>> = Mutex::new(None);
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(|| {
                    while !self.abort.load(Ordering::SeqCst) {
                        let Some(group) = queue.lock().unwrap().pop_front() else {
                            break;
                        };
                        for (i, leg) in group {
                            match catch_unwind(AssertUnwindSafe(|| work(i, leg))) {
                                Ok(Ok(())) => {}
                                Ok(Err(failure)) => {
                                    self.abort.store(true, Ordering::SeqCst);
                                    failures.lock().unwrap().push(failure);
                                    return;
                                }
                                Err(panic) => {
                                    self.abort.store(true, Ordering::SeqCst);
                                    panicked.lock().unwrap().get_or_insert(panic);
                                    return;
                                }
                            }
                        }
                    }
                });
            }
        });
        if let Some(panic) = panicked.into_inner().unwrap().take() {
            resume_unwind(panic);
        }
        let mut failures = failures.into_inner().unwrap();
        if failures.is_empty() {
            return Ok(());
        }
        // The one that stopped the others, not one of the stops it caused.
        let cancelled = TransferError::Cancelled.to_string();
        let first = failures
            .iter()
            .position(|f| f.message != cancelled)
            .unwrap_or(0);
        Err(failures.swap_remove(first))
    }
}
