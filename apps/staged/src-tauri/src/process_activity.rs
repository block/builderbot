//! Keeps macOS from throttling Staged while any session runs.
//!
//! Every session's agent I/O, setup timer and MCP servers run on that
//! session's own thread in this process. When macOS puts the process into App
//! Nap or background throttling, those threads drop to priority 4 and on a
//! loaded machine get almost no CPU: the agent's answer sits unread in the
//! pipe and the setup timer fires late. The ACP startup timeouts were traced
//! to exactly that kind of stall.
//!
//! An `NSProcessInfo` activity tells the system the process is doing work the
//! user asked for, which opts it out of both. One activity is held from the
//! first running session to the last: each [`SessionRegistry`] entry holds a
//! [`SessionActivity`] guard, and the activity begins when the first guard is
//! taken and ends when the last is dropped. Tying it to the registry entry
//! covers every kind of run (a direct start, a pipeline, a reattach, a Pikchr
//! child session), and dropping the entry on any path, a panic's unwinding
//! included, releases it.
//!
//! The activity allows idle system sleep and display sleep: those are not
//! what stalls a session thread, and a session should not keep a laptop
//! awake. Other platforms have no equivalent here and take no action.
//!
//! [`SessionRegistry`]: crate::session_runner::SessionRegistry

use std::sync::{Mutex, MutexGuard};

/// What the refcount begins and ends. A trait so the counting can be tested
/// without the real `NSProcessInfo`.
pub(crate) trait ActivityBackend: Sync {
    /// The handle a begun activity is ended with.
    type Token: Send;

    /// Begin the activity, or `None` when the platform has none (or it could
    /// not be begun). A `None` is retried on the next first holder.
    fn begin(&self) -> Option<Self::Token>;

    /// End an activity [`begin`](Self::begin) returned.
    fn end(&self, token: Self::Token);
}

struct State<T> {
    holders: usize,
    token: Option<T>,
}

/// A process activity held while at least one [`ActivityGuard`] is alive.
pub(crate) struct ActivityRefcount<B: ActivityBackend> {
    backend: B,
    state: Mutex<State<B::Token>>,
}

impl<B: ActivityBackend> ActivityRefcount<B> {
    pub(crate) const fn new(backend: B) -> Self {
        Self {
            backend,
            state: Mutex::new(State {
                holders: 0,
                token: None,
            }),
        }
    }

    /// Take a holder, beginning the activity if this is the first.
    pub(crate) fn acquire(&self) -> ActivityGuard<'_, B> {
        let mut state = self.lock();
        state.holders += 1;
        if state.holders == 1 {
            state.token = self.backend.begin();
            if state.token.is_some() {
                log::info!(
                    "Began the macOS process activity: a session is running, so App Nap \
                     and background throttling are off until the last one ends"
                );
            }
        }
        ActivityGuard { refcount: self }
    }

    fn release(&self) {
        let mut state = self.lock();
        state.holders = state.holders.saturating_sub(1);
        if state.holders == 0 {
            if let Some(token) = state.token.take() {
                self.backend.end(token);
                log::info!(
                    "Ended the macOS process activity: no session is running, so App Nap \
                     and background throttling apply again"
                );
            }
        }
    }

    /// Never panics: a release runs in `Drop`, possibly while unwinding, and
    /// the count is still right after a poisoning panic (it happened in a
    /// backend call, after the count was updated).
    fn lock(&self) -> MutexGuard<'_, State<B::Token>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    fn holders(&self) -> usize {
        self.lock().holders
    }
}

/// One holder of an [`ActivityRefcount`]; dropping it releases the hold.
#[must_use = "the activity is held only while the guard is alive"]
pub(crate) struct ActivityGuard<'a, B: ActivityBackend> {
    refcount: &'a ActivityRefcount<B>,
}

impl<B: ActivityBackend> Drop for ActivityGuard<'_, B> {
    fn drop(&mut self) {
        self.refcount.release();
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::{NSObjectProtocol, ProtocolObject};
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

    /// User-initiated work, which opts out of App Nap and timer/IO
    /// throttling, without the idle-system-sleep assertion the plain
    /// `UserInitiated` adds. Not `LatencyCritical`: Apple reserves that for
    /// media-style timing precision, and the stall it addresses is the
    /// throttled priority band, which `UserInitiated` already leaves.
    const OPTIONS: NSActivityOptions = NSActivityOptions::UserInitiatedAllowingIdleSystemSleep;
    const REASON: &str = "Staged is running agent sessions";

    pub(crate) struct ProcessInfoActivity;

    pub(crate) struct ActivityToken(Retained<ProtocolObject<dyn NSObjectProtocol>>);

    // SAFETY: the token is an opaque object `NSProcessInfo` hands back only so
    // it can be passed to `endActivity:`; nothing here calls into it, and
    // `NSProcessInfo`'s activity methods may be called from any thread.
    unsafe impl Send for ActivityToken {}

    impl super::ActivityBackend for ProcessInfoActivity {
        type Token = ActivityToken;

        fn begin(&self) -> Option<ActivityToken> {
            // Session threads have no autorelease pool of their own.
            autoreleasepool(|_| {
                let reason = NSString::from_str(REASON);
                let activity =
                    NSProcessInfo::processInfo().beginActivityWithOptions_reason(OPTIONS, &reason);
                Some(ActivityToken(activity))
            })
        }

        fn end(&self, token: ActivityToken) {
            autoreleasepool(|_| {
                // SAFETY: `token` is the object `beginActivityWithOptions:reason:`
                // returned, ended once (the refcount takes it out of its state).
                unsafe { NSProcessInfo::processInfo().endActivity(&token.0) };
            });
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub(crate) struct ProcessInfoActivity;

    impl super::ActivityBackend for ProcessInfoActivity {
        type Token = ();

        fn begin(&self) -> Option<()> {
            None
        }

        fn end(&self, _token: ()) {}
    }
}

static SESSIONS: ActivityRefcount<platform::ProcessInfoActivity> =
    ActivityRefcount::new(platform::ProcessInfoActivity);

/// A running session's hold on the process activity.
pub(crate) type SessionActivity = ActivityGuard<'static, platform::ProcessInfoActivity>;

/// Hold the process activity for one running session. Cheap after the first
/// holder: a mutex and a counter.
pub(crate) fn hold_for_session() -> SessionActivity {
    SESSIONS.acquire()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeBackend {
        begun: AtomicUsize,
        ended: AtomicUsize,
        /// Refuse this many `begin`s before succeeding.
        refusals: AtomicUsize,
    }

    impl FakeBackend {
        fn begun(&self) -> usize {
            self.begun.load(Ordering::SeqCst)
        }
        fn ended(&self) -> usize {
            self.ended.load(Ordering::SeqCst)
        }
    }

    impl ActivityBackend for FakeBackend {
        /// Which `begin` this was, so a test can see the right one ended.
        type Token = usize;

        fn begin(&self) -> Option<usize> {
            // Only ever called under the refcount's lock, so no race here.
            if self.refusals.load(Ordering::SeqCst) > 0 {
                self.refusals.fetch_sub(1, Ordering::SeqCst);
                return None;
            }
            Some(self.begun.fetch_add(1, Ordering::SeqCst))
        }

        fn end(&self, token: usize) {
            assert_eq!(token + 1, self.begun(), "ended a stale activity");
            self.ended.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn begins_on_first_holder_and_ends_on_last() {
        let refcount = ActivityRefcount::new(FakeBackend::default());

        let first = refcount.acquire();
        assert_eq!((refcount.backend.begun(), refcount.backend.ended()), (1, 0));

        let second = refcount.acquire();
        let third = refcount.acquire();
        assert_eq!(refcount.holders(), 3);
        assert_eq!(refcount.backend.begun(), 1, "later holders share it");

        drop(first);
        drop(third);
        assert_eq!(refcount.backend.ended(), 0, "a holder is still alive");

        drop(second);
        assert_eq!(refcount.holders(), 0);
        assert_eq!((refcount.backend.begun(), refcount.backend.ended()), (1, 1));
    }

    #[test]
    fn begins_again_after_going_idle() {
        let refcount = ActivityRefcount::new(FakeBackend::default());

        drop(refcount.acquire());
        drop(refcount.acquire());
        assert_eq!((refcount.backend.begun(), refcount.backend.ended()), (2, 2));
    }

    #[test]
    fn handing_over_between_holders_keeps_one_activity() {
        // The pipeline handoff: the replacement entry is registered before
        // the one it replaces is dropped.
        let refcount = ActivityRefcount::new(FakeBackend::default());

        let pipeline = refcount.acquire();
        let agent = refcount.acquire();
        drop(pipeline);
        assert_eq!((refcount.backend.begun(), refcount.backend.ended()), (1, 0));

        drop(agent);
        assert_eq!(refcount.backend.ended(), 1);
    }

    #[test]
    fn a_refused_begin_counts_holders_and_retries_on_the_next_first() {
        let refcount = ActivityRefcount::new(FakeBackend {
            refusals: AtomicUsize::new(1),
            ..FakeBackend::default()
        });

        let refused = refcount.acquire();
        let sharing = refcount.acquire();
        assert_eq!(refcount.holders(), 2);
        assert_eq!(refcount.backend.begun(), 0, "not retried while held");
        drop(refused);
        drop(sharing);
        assert_eq!(refcount.backend.ended(), 0, "nothing to end");

        let retried = refcount.acquire();
        assert_eq!(refcount.backend.begun(), 1);
        drop(retried);
        assert_eq!(refcount.backend.ended(), 1);
    }

    #[test]
    fn a_panicking_holder_still_releases() {
        let refcount = ActivityRefcount::new(FakeBackend::default());

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = refcount.acquire();
            panic!("session thread panicked");
        }));
        assert!(unwound.is_err());
        assert_eq!(refcount.holders(), 0);
        assert_eq!((refcount.backend.begun(), refcount.backend.ended()), (1, 1));
    }

    #[test]
    fn concurrent_holders_begin_and_end_once() {
        let refcount = ActivityRefcount::new(FakeBackend::default());
        let outer = refcount.acquire();

        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..200 {
                        drop(refcount.acquire());
                    }
                });
            }
        });
        assert_eq!(refcount.holders(), 1);
        assert_eq!((refcount.backend.begun(), refcount.backend.ended()), (1, 0));

        drop(outer);
        assert_eq!(refcount.backend.ended(), 1);
    }

    #[test]
    fn the_platform_activity_begins_and_ends() {
        // On macOS this calls the real NSProcessInfo; elsewhere it is a no-op.
        let refcount = ActivityRefcount::new(platform::ProcessInfoActivity);
        let held = refcount.acquire();
        assert_eq!(
            refcount.lock().token.is_some(),
            cfg!(target_os = "macos"),
            "macOS begins a real activity; other platforms have none"
        );
        drop(held);
        assert!(refcount.lock().token.is_none());
    }
}
