//! First-come, first-served limit on how many shell captures run at once.
//!
//! One `$SHELL -ils` costs about 9 s on a dev machine with nvm and Hermit
//! hooks, and eight to ten of them at once stretched each to 23–31 s. With
//! at most [`CAPTURE_LIMIT`] running, the rest wait their turn and every
//! capture stays near its standalone cost.
//!
//! The queue is a plain `std` mutex and condvar rather than a
//! `tokio::sync::Semaphore` because only blocking code ever waits on it.
//! Every capture runs on a thread of its own: the sync caller that started
//! it, or a dedicated `shell-env-capture` thread for async callers, bounded
//! reads and prewarms (async callers only await the result). So no tokio
//! worker parks here, and a thread holding a slot never needs the runtime
//! to give it back. A `tokio::sync::Semaphore` would have needed a blocking
//! acquire bolted on for the sync paths, and would have left each async
//! capture tied to its caller's future, where dropping the future killed
//! the shell (`kill_on_drop`) that other callers were waiting for.

use std::sync::{Condvar, Mutex};

/// Shell captures allowed to run at once. Three concurrent shells measured
/// about 12 s against 9 s for one, so two keeps each close to standalone
/// while letting a second worktree make progress.
pub(super) const CAPTURE_LIMIT: usize = 2;

/// Counting semaphore with a FIFO ticket queue.
///
/// Waiters are served strictly in arrival order, so a steady stream of new
/// directories can't starve one that has been waiting. A ticket is only
/// ever released by being admitted: [`CaptureSlots::acquire`] has no
/// timeout, so no ticket is abandoned and the queue never stalls on a gap.
pub(super) struct CaptureSlots {
    limit: usize,
    state: Mutex<SlotState>,
    changed: Condvar,
}

struct SlotState {
    in_use: usize,
    next_ticket: u64,
    /// Tickets below this have been admitted.
    admitted: u64,
}

/// A held slot; dropping it lets the next waiter in.
pub(super) struct CaptureSlot<'a> {
    slots: &'a CaptureSlots,
}

impl CaptureSlots {
    pub(super) fn new(limit: usize) -> Self {
        assert!(limit > 0, "a capture limit of 0 would never admit anyone");
        Self {
            limit,
            state: Mutex::new(SlotState {
                in_use: 0,
                next_ticket: 0,
                admitted: 0,
            }),
            changed: Condvar::new(),
        }
    }

    /// Block until a slot is free and every earlier caller has had one.
    pub(super) fn acquire(&self) -> CaptureSlot<'_> {
        let mut state = self.state.lock().unwrap();
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        while state.admitted != ticket || state.in_use >= self.limit {
            state = self.changed.wait(state).unwrap();
        }
        state.admitted += 1;
        state.in_use += 1;
        drop(state);
        // The next ticket may fit too if a slot is still free.
        self.changed.notify_all();
        CaptureSlot { slots: self }
    }

    #[cfg(test)]
    pub(super) fn in_use(&self) -> usize {
        self.state.lock().unwrap().in_use
    }

    /// Callers queued for a slot, not counting those holding one.
    #[cfg(test)]
    pub(super) fn queued(&self) -> usize {
        let state = self.state.lock().unwrap();
        (state.next_ticket - state.admitted) as usize
    }
}

impl Drop for CaptureSlot<'_> {
    fn drop(&mut self) {
        // Poison-tolerant: this runs while unwinding out of a capture.
        let mut state = self
            .slots
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.in_use -= 1;
        drop(state);
        self.slots.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn never_admits_more_than_the_limit() {
        let slots = Arc::new(CaptureSlots::new(2));
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (slots, running, peak) = (slots.clone(), running.clone(), peak.clone());
                std::thread::spawn(move || {
                    let _slot = slots.acquire();
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    running.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(slots.in_use(), 0);
        assert_eq!(slots.queued(), 0);
    }

    #[test]
    fn admits_waiters_in_arrival_order() {
        let slots = Arc::new(CaptureSlots::new(1));
        let held = slots.acquire();
        let (order_tx, order_rx) = mpsc::channel();
        let threads: Vec<_> = (0..4)
            .map(|i| {
                let (waiter_slots, order_tx) = (slots.clone(), order_tx.clone());
                let thread = std::thread::spawn(move || {
                    let _slot = waiter_slots.acquire();
                    order_tx.send(i).unwrap();
                });
                // Queue them one at a time so arrival order is known.
                wait_until("the waiter to queue", || slots.queued() == i + 1);
                thread
            })
            .collect();
        drop(held);
        for thread in threads {
            thread.join().unwrap();
        }
        let order: Vec<_> = order_rx.try_iter().collect();
        assert_eq!(order, vec![0, 1, 2, 3]);
    }
}
