//! The store's connection lock, timed.
//!
//! Every store method takes one mutex, so under load the time a write waits
//! for it is the write path's delay. This is `std::sync::Mutex` with the
//! wait and the hold recorded, and with the one method the store calls on
//! it, so `store/mod.rs` swaps the type and no call site changes.

use std::ops::{Deref, DerefMut};
use std::sync::{LockResult, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use super::metrics;

pub(crate) struct TimedMutex<T>(Mutex<T>);

pub(crate) struct TimedGuard<'a, T> {
    guard: MutexGuard<'a, T>,
    since: Instant,
}

impl<T> TimedMutex<T> {
    pub(crate) fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    pub(crate) fn lock(&self) -> LockResult<TimedGuard<'_, T>> {
        let asked = Instant::now();
        let result = self.0.lock();
        let since = Instant::now();
        metrics::store_lock_waited(since - asked);
        match result {
            Ok(guard) => Ok(TimedGuard { guard, since }),
            Err(poisoned) => Err(PoisonError::new(TimedGuard {
                guard: poisoned.into_inner(),
                since,
            })),
        }
    }
}

impl<T> Deref for TimedGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for TimedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T> Drop for TimedGuard<'_, T> {
    fn drop(&mut self) {
        metrics::store_lock_held(self.since.elapsed());
    }
}
