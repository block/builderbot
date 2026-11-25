//! The store's connection lock, timed.
//!
//! Every store method takes the one connection lock, runs its SQL and lets
//! go. A hold is normally a fraction of a millisecond, so a slow one stalls
//! every other caller, session threads included: a session thread waiting
//! here reads no agent output and fires no setup timer. [`TimedMutex`] times
//! each acquisition, separately as the wait for the lock and the hold after
//! it, and logs any that is slow with the store method that took it (its
//! `lock()` call site) and, for a slow wait, the call site that held the lock
//! when the wait began. A slow wait with a fast hold points at contention; a
//! slow hold points at the holder being slow (a heavy query, I/O, or its
//! thread not being scheduled).
//!
//! The store has more than one connection (the write connection and a few
//! readers, see [`super::connections`]), each behind its own [`TimedMutex`];
//! each is named, and a report says which one it was.
//!
//! In the load-test harness build the write connection's lock also records
//! every wait and hold in the harness's metrics (see
//! [`TimedMutex::recording_perf_metrics`]): its wait is the write path's
//! delay under load.

use std::ops::{Deref, DerefMut};
use std::panic::Location;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{LockResult, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::{Duration, Instant};

/// A wait or hold at least this long is logged.
const SLOW_LOCK: Duration = Duration::from_millis(500);

/// A [`Mutex`] whose acquisitions are timed and slow ones logged.
pub(crate) struct TimedMutex<T> {
    inner: Mutex<T>,
    /// The `lock()` call site of the current holder; null while free.
    holder: AtomicPtr<Location<'static>>,
    /// Which lock this is, for the report ("the write connection").
    name: String,
    /// Record waits and holds in the load-test harness's metrics.
    #[cfg(feature = "perf-harness")]
    perf_metrics: bool,
}

impl<T> TimedMutex<T> {
    pub(crate) fn new(value: T, name: impl Into<String>) -> Self {
        Self {
            inner: Mutex::new(value),
            holder: AtomicPtr::new(ptr::null_mut()),
            name: name.into(),
            #[cfg(feature = "perf-harness")]
            perf_metrics: false,
        }
    }

    /// Record this lock's waits and holds in the load-test harness's metrics.
    /// Only the write connection's: every write waits on it.
    #[cfg(feature = "perf-harness")]
    pub(crate) fn recording_perf_metrics(mut self) -> Self {
        self.perf_metrics = true;
        self
    }

    /// The lock if it is free right now, without waiting; `None` if another
    /// thread holds it. A poisoned lock is taken like a free one: what it
    /// guards is a connection, which a panic does not leave half-changed.
    #[cfg(test)]
    #[track_caller]
    pub(crate) fn try_lock(&self) -> Option<TimedGuard<'_, T>> {
        self.try_lock_waiting_since(Instant::now())
    }

    /// [`Self::try_lock`] for a caller that has been waiting for this lock
    /// or one like it since `began` (a pool handing out whichever of its
    /// locks frees first): the wait is timed from then, so a slow one is
    /// still reported.
    #[track_caller]
    pub(crate) fn try_lock_waiting_since(&self, began: Instant) -> Option<TimedGuard<'_, T>> {
        let caller = Location::caller();
        let guard = match self.inner.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poison)) => poison.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        Some(self.held(guard, caller, None, began))
    }

    /// [`Mutex::lock`], timed against the caller's location.
    #[track_caller]
    pub(crate) fn lock(&self) -> LockResult<TimedGuard<'_, T>> {
        let caller = Location::caller();
        let holder_at_entry = self.holder_location();
        let began = Instant::now();
        let (guard, poisoned) = match self.inner.lock() {
            Ok(guard) => (guard, false),
            Err(poison) => (poison.into_inner(), true),
        };
        let guard = self.held(guard, caller, holder_at_entry, began);
        if poisoned {
            Err(PoisonError::new(guard))
        } else {
            Ok(guard)
        }
    }

    fn held<'a>(
        &'a self,
        guard: MutexGuard<'a, T>,
        caller: &'static Location<'static>,
        holder_at_entry: Option<&'static Location<'static>>,
        began: Instant,
    ) -> TimedGuard<'a, T> {
        let acquired = Instant::now();
        self.holder
            .store(ptr::from_ref(caller).cast_mut(), Ordering::Relaxed);
        #[cfg(feature = "perf-harness")]
        if self.perf_metrics {
            crate::perf_harness::metrics::store_lock_waited(acquired.duration_since(began));
        }
        TimedGuard {
            guard: Some(guard),
            owner: self,
            caller,
            holder_at_entry,
            waited: acquired.duration_since(began),
            acquired,
        }
    }

    /// Which lock this is.
    #[cfg(test)]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    fn holder_location(&self) -> Option<&'static Location<'static>> {
        let holder = self.holder.load(Ordering::Relaxed);
        // SAFETY: only ever null or a `&'static Location` stored by `lock`.
        unsafe { holder.as_ref() }
    }
}

/// A held [`TimedMutex`]. Derefs to the protected value.
pub(crate) struct TimedGuard<'a, T> {
    /// `Some` until dropped; taken so the lock is released before logging.
    guard: Option<MutexGuard<'a, T>>,
    owner: &'a TimedMutex<T>,
    caller: &'static Location<'static>,
    holder_at_entry: Option<&'static Location<'static>>,
    waited: Duration,
    acquired: Instant,
}

impl<T> Deref for TimedGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.guard.as_ref().expect("guard is held until drop")
    }
}

impl<T> DerefMut for TimedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().expect("guard is held until drop")
    }
}

impl<T> Drop for TimedGuard<'_, T> {
    fn drop(&mut self) {
        let held = self.acquired.elapsed();
        self.owner.holder.store(ptr::null_mut(), Ordering::Relaxed);
        drop(self.guard.take());
        #[cfg(feature = "perf-harness")]
        if self.owner.perf_metrics {
            crate::perf_harness::metrics::store_lock_held(held);
        }
        if let Some(report) = slow_lock_report(
            &self.owner.name,
            self.caller,
            self.holder_at_entry,
            self.waited,
            held,
            SLOW_LOCK,
        ) {
            let thread = std::thread::current();
            log::warn!(
                "{report}, on thread {} ({:?})",
                thread.name().unwrap_or("unnamed"),
                thread.id()
            );
        }
    }
}

/// The log line for an acquisition that waited or held at least `threshold`,
/// or `None` for an ordinary one.
fn slow_lock_report(
    name: &str,
    caller: &Location<'_>,
    holder_at_entry: Option<&Location<'_>>,
    waited: Duration,
    held: Duration,
    threshold: Duration,
) -> Option<String> {
    if waited < threshold && held < threshold {
        return None;
    }
    let holder = match holder_at_entry {
        Some(holder) if waited >= threshold => format!(" (held by {holder} when the wait began)"),
        _ => String::new(),
    };
    Some(format!(
        "Slow store lock on {name} at {caller}: waited {:.3}s{holder}, held {:.3}s",
        waited.as_secs_f64(),
        held.as_secs_f64()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_acquisitions_are_not_reported() {
        let here = Location::caller();
        assert_eq!(
            slow_lock_report(
                "the write connection",
                here,
                None,
                Duration::from_millis(1),
                Duration::from_millis(2),
                SLOW_LOCK
            ),
            None
        );
    }

    #[test]
    fn a_slow_wait_names_the_holder_and_a_slow_hold_does_not() {
        let caller = Location::caller();
        let holder = Location::caller();
        let waited = slow_lock_report(
            "the write connection",
            caller,
            Some(holder),
            Duration::from_millis(12_300),
            Duration::from_micros(100),
            SLOW_LOCK,
        )
        .expect("a slow wait is reported");
        assert_eq!(
            waited,
            format!("Slow store lock on the write connection at {caller}: waited 12.300s (held by {holder} when the wait began), held 0.000s")
        );

        let held = slow_lock_report(
            "store reader 2",
            caller,
            Some(holder),
            Duration::ZERO,
            Duration::from_millis(1400),
            SLOW_LOCK,
        )
        .expect("a slow hold is reported");
        assert_eq!(
            held,
            format!("Slow store lock on store reader 2 at {caller}: waited 0.000s, held 1.400s")
        );
    }

    #[test]
    fn the_holder_is_recorded_while_held_and_cleared_on_release() {
        let mutex = TimedMutex::new(1, "test");
        assert!(mutex.holder_location().is_none());
        {
            let mut guard = mutex.lock().unwrap();
            *guard += 1;
            let holder = mutex.holder_location().expect("held");
            assert_eq!(holder.file(), file!());
        }
        assert!(mutex.holder_location().is_none());
        assert_eq!(*mutex.lock().unwrap(), 2);
    }

    #[test]
    fn try_lock_takes_a_free_lock_and_passes_on_a_held_one() {
        let mutex = TimedMutex::new(1, "test");
        let held = mutex.lock().unwrap();
        assert!(mutex.try_lock().is_none());
        drop(held);
        let mut guard = mutex.try_lock().expect("free");
        *guard += 1;
        assert_eq!(mutex.holder_location().expect("held").file(), file!());
        drop(guard);
        assert_eq!(*mutex.lock().unwrap(), 2);
    }

    #[test]
    fn a_poisoned_lock_still_hands_out_the_value() {
        let mutex = std::sync::Arc::new(TimedMutex::new(7, "test"));
        let poisoner = std::sync::Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        match mutex.lock() {
            Ok(_) => panic!("the lock should be poisoned"),
            Err(poison) => assert_eq!(*poison.into_inner(), 7),
        };
    }
}
