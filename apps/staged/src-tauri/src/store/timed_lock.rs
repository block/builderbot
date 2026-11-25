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

use std::ops::{Deref, DerefMut};
use std::panic::Location;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{LockResult, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// A wait or hold at least this long is logged.
const SLOW_LOCK: Duration = Duration::from_millis(500);

/// A [`Mutex`] whose acquisitions are timed and slow ones logged.
pub(crate) struct TimedMutex<T> {
    inner: Mutex<T>,
    /// The `lock()` call site of the current holder; null while free.
    holder: AtomicPtr<Location<'static>>,
}

impl<T> TimedMutex<T> {
    pub(crate) fn new(value: T) -> Self {
        Self {
            inner: Mutex::new(value),
            holder: AtomicPtr::new(ptr::null_mut()),
        }
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
        let acquired = Instant::now();
        self.holder
            .store(ptr::from_ref(caller).cast_mut(), Ordering::Relaxed);
        let guard = TimedGuard {
            guard: Some(guard),
            owner: self,
            caller,
            holder_at_entry,
            waited: acquired.duration_since(began),
            acquired,
        };
        if poisoned {
            Err(PoisonError::new(guard))
        } else {
            Ok(guard)
        }
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
        if let Some(report) = slow_lock_report(
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
        "Slow store lock at {caller}: waited {:.3}s{holder}, held {:.3}s",
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
            caller,
            Some(holder),
            Duration::from_millis(12_300),
            Duration::from_micros(100),
            SLOW_LOCK,
        )
        .expect("a slow wait is reported");
        assert_eq!(
            waited,
            format!("Slow store lock at {caller}: waited 12.300s (held by {holder} when the wait began), held 0.000s")
        );

        let held = slow_lock_report(
            caller,
            Some(holder),
            Duration::ZERO,
            Duration::from_millis(1400),
            SLOW_LOCK,
        )
        .expect("a slow hold is reported");
        assert_eq!(
            held,
            format!("Slow store lock at {caller}: waited 0.000s, held 1.400s")
        );
    }

    #[test]
    fn the_holder_is_recorded_while_held_and_cleared_on_release() {
        let mutex = TimedMutex::new(1);
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
    fn a_poisoned_lock_still_hands_out_the_value() {
        let mutex = std::sync::Arc::new(TimedMutex::new(7));
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
