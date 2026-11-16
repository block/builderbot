//! When the agent last said anything.
//!
//! [`ActivityClock`] is a shared timestamp the [`super::MessageWriter`]
//! bumps on every event the ACP driver hands it (text, tool calls, metadata,
//! permission requests). A remote note session's inactivity watchdog reads
//! it to decide when the connection has been silent long enough to be worth
//! a look at the workstation outbox — see
//! `session_runner::remote_note_watchdog`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// tokio's `Instant` so a watchdog under tokio's paused test clock sees the
// same time the sleeps do; outside a runtime it is the std clock.
use tokio::time::Instant;

/// Monotonic "last heard from the agent" stamp, shared between the writer
/// (which bumps it) and a watchdog (which reads it).
#[derive(Debug)]
pub struct ActivityClock {
    origin: Instant,
    /// Milliseconds since `origin` of the last `touch`.
    last_ms: AtomicU64,
}

impl Default for ActivityClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityClock {
    /// A clock that counts as touched right now.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            last_ms: AtomicU64::new(0),
        }
    }

    /// Record that the agent was heard from now.
    pub fn touch(&self) {
        let now_ms = self.origin.elapsed().as_millis() as u64;
        self.last_ms.fetch_max(now_ms, Ordering::Relaxed);
    }

    /// How long since the last `touch` (or construction).
    pub fn idle_for(&self) -> Duration {
        let last = Duration::from_millis(self.last_ms.load(Ordering::Relaxed));
        self.origin.elapsed().saturating_sub(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_touch_resets_the_idle_time() {
        let clock = ActivityClock::new();
        std::thread::sleep(Duration::from_millis(20));
        assert!(clock.idle_for() >= Duration::from_millis(20));
        clock.touch();
        assert!(clock.idle_for() < Duration::from_millis(20));
    }
}
