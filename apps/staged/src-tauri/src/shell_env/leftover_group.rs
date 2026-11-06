//! What a timed-out capture shell started, after the shell itself is gone.
//!
//! At its capture timeout only the shell is killed (see
//! `kill_capture_shell`), so what its rc files started can finish: a
//! nix-direnv build, an `nvm install`, a Hermit download. Those stay in the
//! shell's process group, whose id is the shell's pid. A watchdog thread
//! looks at the group every [`POLL`] and kills what is left of it at the
//! group deadline, unless it has all exited by then. It reports what it sees
//! in [`Leftovers`], which the capture's timed-out mark reads, so the mark
//! lasts as long as the leftovers do.
//!
//! Signalling a group by id after its leader has been reaped is safe while
//! the group still has members: POSIX doesn't reuse a pid that is still a
//! process group's id. Once the last member exits the id is free, so the
//! watchdog stops as soon as it sees the group gone. The only gap is a group
//! that empties and whose id goes to a new group leader within one poll.
//! Pids are handed out in sequence, so that would take the whole pid space
//! wrapping around inside a second.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;
#[cfg(unix)]
use std::{io, sync::Arc, time::Duration};

/// How often the watchdog checks whether the group has exited on its own.
#[cfg(unix)]
const POLL: Duration = Duration::from_secs(1);

/// What one capture's shell left in its process group, as its watchdog last
/// saw it, and when the watchdog kills what is left. The capture creates it
/// before the shell starts. The watchdog and the capture's timed-out mark
/// share it, so the mark sees the group's end however their updates
/// interleave.
#[derive(Debug)]
pub(super) struct Leftovers {
    state: AtomicU8,
    deadline: Instant,
}

/// See [`Leftovers`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum LeftoverState {
    /// The shell hasn't been killed, or nothing it started was left when it
    /// was.
    Nothing = 0,
    /// Some of the group is still running, and the watchdog is waiting on it.
    Running = 1,
    /// All of the group exited on its own before the deadline.
    Finished = 2,
    /// What was left at the deadline has been killed.
    Killed = 3,
}

impl Leftovers {
    /// Nothing left over yet; the watchdog, if one starts, kills what is left
    /// at `deadline`.
    pub(super) fn new(deadline: Instant) -> Self {
        Self {
            state: AtomicU8::new(LeftoverState::Nothing as u8),
            deadline,
        }
    }

    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(super) fn state(&self) -> LeftoverState {
        match self.state.load(Ordering::Acquire) {
            1 => LeftoverState::Running,
            2 => LeftoverState::Finished,
            3 => LeftoverState::Killed,
            _ => LeftoverState::Nothing,
        }
    }

    #[cfg(unix)]
    fn set(&self, state: LeftoverState) {
        self.state.store(state as u8, Ordering::Release);
    }
}

/// Kill process group `pgid`, a killed and reaped capture shell's, at
/// `leftovers`' deadline, unless all of it has exited by then. Returns at
/// once; a `shell-env-leftovers` thread does the waiting and keeps
/// `leftovers` up to date.
#[cfg(unix)]
pub(super) fn kill_at(pgid: libc::pid_t, leftovers: &Arc<Leftovers>) {
    if !group_exists(pgid) {
        return;
    }
    leftovers.set(LeftoverState::Running);
    let watched = Arc::clone(leftovers);
    let watch = std::thread::Builder::new()
        .name("shell-env-leftovers".to_string())
        .spawn(move || watch_until(pgid, &watched));
    if let Err(error) = watch {
        // Unwatched, the group could run forever. Better to stop it now.
        log::warn!(
            "Couldn't watch what a timed-out shell env capture left running (process group \
             {pgid}), so killing it now: {error}"
        );
        leftovers.set(LeftoverState::Killed);
        kill_group(pgid);
    }
}

#[cfg(unix)]
fn watch_until(pgid: libc::pid_t, leftovers: &Leftovers) {
    while group_exists(pgid) {
        let left = leftovers.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            log::warn!(
                "Killing what a timed-out shell env capture left running (process group {pgid})"
            );
            // Before the kill, so whoever sees the group gone also sees that
            // it was killed rather than finished.
            leftovers.set(LeftoverState::Killed);
            kill_group(pgid);
            return;
        }
        std::thread::sleep(left.min(POLL));
    }
    leftovers.set(LeftoverState::Finished);
    log::info!(
        "What a timed-out shell env capture left running (process group {pgid}) has finished, \
         so its directory's next lookup captures again"
    );
}

/// Whether any process is still in group `pgid`. One this process may not
/// signal still counts.
#[cfg(unix)]
fn group_exists(pgid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks whether the group exists and may be
    // signalled; `kill` has no memory-safety preconditions.
    let result = unsafe { libc::kill(-pgid, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn kill_group(pgid: libc::pid_t) {
    // SAFETY: as for `group_exists`. The group still existed when last
    // checked, so its id is still the capture shell's (see the module docs).
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}
