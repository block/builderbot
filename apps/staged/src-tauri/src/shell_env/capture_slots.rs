//! Limit on how many shell captures run at once, with the captures somebody
//! is blocked on let in first.
//!
//! One `$SHELL -ils` costs about 9 s on a dev machine with nvm and Hermit
//! hooks, and eight to ten of them at once stretched each to 23–31 s. With
//! at most [`CAPTURE_LIMIT`] running, the rest wait their turn and every
//! capture stays near its standalone cost.
//!
//! Waiting captures sit in two tiers. A capture moves to the front tier once
//! a caller waits on it without a bound (`get`, `get_blocking`): a session
//! start, a pipeline step, a fetch or a commit. The back tier holds captures
//! nobody is blocked on: prewarms, and captures started by bounded reads,
//! which give up within their bound whatever the queue does. Each tier goes
//! in arrival order, and the front tier always goes first. So the burst of
//! prewarms and bounded reads the home page starts at launch, one per cold
//! worktree, can't hold up the capture a session start needs. A steady
//! stream of front-tier captures could keep the back tier waiting, but there
//! is at most one capture per env root per cache TTL.
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

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

/// Shell captures allowed to run at once, with home's on top (see the module
/// docs).
///
/// A cap keeps a launch with many worktrees from starting dozens of
/// interactive shells at once: the perf audit saw that contention stretch
/// captures to 8–31 s. Four lets a typical launch with four worktrees land
/// every env in one round on a modern multi-core laptop. With two, the third
/// and fourth worktrees waited for a second round, which doubled the time
/// until every worktree had its env. There was headroom for more: with three
/// shells running at once (home and two worktrees) each capture still took
/// 2.6–2.9 s, about what one takes alone.
pub(super) const CAPTURE_LIMIT: usize = 4;

/// Whether a capture has a caller waiting on it without a bound, which puts
/// it in the front tier.
///
/// Shared by the capture's place in the queue and its in-flight entry, where
/// waiters find it. It only ever goes from back to front, and only through
/// [`CaptureSlots::promote`], under the queue's lock, so a waiter deciding
/// whose turn it is never sees it change halfway through.
#[derive(Clone, Default)]
pub(super) struct Urgency(Arc<AtomicBool>);

impl Urgency {
    fn is_urgent(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Counting semaphore with a two-tier ticket queue (see the module docs).
///
/// A ticket is only ever released by being admitted:
/// [`CaptureSlots::acquire`] has no timeout, so no ticket is abandoned and
/// the queue never stalls on a gap.
pub(super) struct CaptureSlots {
    limit: usize,
    state: Mutex<SlotState>,
    changed: Condvar,
}

struct SlotState {
    in_use: usize,
    next_ticket: u64,
    /// Captures waiting for a slot, in arrival order.
    queue: VecDeque<Queued>,
}

struct Queued {
    ticket: u64,
    urgency: Urgency,
}

impl SlotState {
    /// The ticket admitted when a slot is next free: the earliest urgent
    /// one, or the earliest of all when none is urgent.
    fn next_up(&self) -> Option<u64> {
        self.queue
            .iter()
            .find(|queued| queued.urgency.is_urgent())
            .or_else(|| self.queue.front())
            .map(|queued| queued.ticket)
    }
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
                queue: VecDeque::new(),
            }),
            changed: Condvar::new(),
        }
    }

    // Poison-tolerant throughout: a slot is given back while unwinding out
    // of a capture, and nothing here leaves the state half-updated.
    fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Block until a slot is free and it is this capture's turn: no urgent
    /// capture queued ahead of it, and, unless `urgency` is urgent by then,
    /// no capture of either tier queued ahead of it.
    pub(super) fn acquire(&self, urgency: &Urgency) -> CaptureSlot<'_> {
        let mut state = self.lock();
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.queue.push_back(Queued {
            ticket,
            urgency: urgency.clone(),
        });
        while state.in_use >= self.limit || state.next_up() != Some(ticket) {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.queue.retain(|queued| queued.ticket != ticket);
        state.in_use += 1;
        drop(state);
        // The next ticket may fit too if a slot is still free.
        self.changed.notify_all();
        CaptureSlot { slots: self }
    }

    /// Move the capture holding `urgency` to the front tier: ahead of every
    /// back-tier capture now, if it is queued, or when it joins the queue.
    pub(super) fn promote(&self, urgency: &Urgency) {
        let state = self.lock();
        let was_urgent = urgency.0.swap(true, Ordering::Relaxed);
        drop(state);
        if !was_urgent {
            self.changed.notify_all();
        }
    }

    #[cfg(test)]
    pub(super) fn in_use(&self) -> usize {
        self.lock().in_use
    }

    /// Callers queued for a slot, not counting those holding one.
    #[cfg(test)]
    pub(super) fn queued(&self) -> usize {
        self.lock().queue.len()
    }
}

impl Drop for CaptureSlot<'_> {
    fn drop(&mut self) {
        let mut state = self.slots.lock();
        state.in_use -= 1;
        drop(state);
        self.slots.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Queue a waiter for `slots` under `label`, which it reports on
    /// `order_tx` once admitted, and return after it has queued.
    fn queue_waiter(
        slots: &Arc<CaptureSlots>,
        urgency: &Urgency,
        label: &'static str,
        order_tx: &mpsc::Sender<&'static str>,
    ) -> std::thread::JoinHandle<()> {
        let queued_before = slots.queued();
        let (waiter_slots, urgency, order_tx) = (slots.clone(), urgency.clone(), order_tx.clone());
        let thread = std::thread::spawn(move || {
            let _slot = waiter_slots.acquire(&urgency);
            order_tx.send(label).unwrap();
        });
        wait_until("the waiter to queue", || {
            slots.queued() == queued_before + 1
        });
        thread
    }

    #[test]
    fn never_admits_more_than_the_limit() {
        let slots = Arc::new(CaptureSlots::new(2));
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let (slots, running, peak) = (slots.clone(), running.clone(), peak.clone());
                std::thread::spawn(move || {
                    let urgency = Urgency::default();
                    if i % 2 == 0 {
                        slots.promote(&urgency);
                    }
                    let _slot = slots.acquire(&urgency);
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
        let held = slots.acquire(&Urgency::default());
        let (order_tx, order_rx) = mpsc::channel();
        let threads: Vec<_> = ["0", "1", "2", "3"]
            .into_iter()
            .map(|label| queue_waiter(&slots, &Urgency::default(), label, &order_tx))
            .collect();
        drop(held);
        for thread in threads {
            thread.join().unwrap();
        }
        let order: Vec<_> = order_rx.try_iter().collect();
        assert_eq!(order, vec!["0", "1", "2", "3"]);
    }

    /// With both slots held and a back-tier capture queued first, a capture
    /// promoted after it queued gets the next slot and one urgent from the
    /// start gets the one after, each ahead of the back-tier capture.
    #[test]
    fn urgent_captures_go_ahead_of_back_tier_ones() {
        let slots = Arc::new(CaptureSlots::new(2));
        let held = [
            slots.acquire(&Urgency::default()),
            slots.acquire(&Urgency::default()),
        ];
        let (order_tx, order_rx) = mpsc::channel();

        let prewarm = queue_waiter(&slots, &Urgency::default(), "prewarm", &order_tx);
        let joined = Urgency::default();
        let joined_thread = queue_waiter(&slots, &joined, "promoted", &order_tx);
        let urgent = Urgency::default();
        slots.promote(&urgent);
        let urgent_thread = queue_waiter(&slots, &urgent, "urgent", &order_tx);
        // A caller without a bound joins the second capture after it queued.
        slots.promote(&joined);

        // One slot frees up and the waiters take it in turn, so the order
        // they report is whose turn each release gave.
        let [first, second] = held;
        drop(first);
        for thread in [prewarm, joined_thread, urgent_thread] {
            thread.join().unwrap();
        }
        assert_eq!(
            order_rx.try_iter().collect::<Vec<_>>(),
            vec!["promoted", "urgent", "prewarm"]
        );
        drop(second);
        assert_eq!(slots.in_use(), 0);
    }
}
