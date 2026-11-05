//! What a timed-out capture shell started, after the shell itself is gone.
//!
//! At its capture timeout only the shell is killed (see
//! `kill_capture_shell`), so what its rc files started can finish: a
//! nix-direnv build, an `nvm install`, a Hermit download. Those stay in the
//! shell's process group, whose id is the shell's pid. A watchdog thread
//! looks at the group every [`POLL`] and kills what is left of it at the
//! group deadline, unless it has all exited by then.
//!
//! Signalling a group by id after its leader has been reaped is safe while
//! the group still has members: POSIX doesn't reuse a pid that is still a
//! process group's id. Once the last member exits the id is free, so the
//! watchdog stops as soon as it sees the group gone. The only gap is a group
//! that empties and whose id goes to a new group leader within one poll.
//! Pids are handed out in sequence, so that would take the whole pid space
//! wrapping around inside a second.

use std::io;
use std::time::{Duration, Instant};

/// How often the watchdog checks whether the group has exited on its own.
const POLL: Duration = Duration::from_secs(1);

/// Kill process group `pgid`, a killed and reaped capture shell's, at
/// `deadline`, unless all of it has exited by then. Returns at once; a
/// `shell-env-leftovers` thread does the waiting.
pub(super) fn kill_at(pgid: libc::pid_t, deadline: Instant) {
    if !group_exists(pgid) {
        return;
    }
    let watch = std::thread::Builder::new()
        .name("shell-env-leftovers".to_string())
        .spawn(move || watch_until(pgid, deadline));
    if let Err(error) = watch {
        // Unwatched, the group could run forever. Better to stop it now.
        log::warn!(
            "Couldn't watch what a timed-out shell env capture left running (process group \
             {pgid}), so killing it now: {error}"
        );
        kill_group(pgid);
    }
}

fn watch_until(pgid: libc::pid_t, deadline: Instant) {
    while group_exists(pgid) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            log::warn!(
                "Killing what a timed-out shell env capture left running (process group {pgid})"
            );
            kill_group(pgid);
            return;
        }
        std::thread::sleep(left.min(POLL));
    }
    log::info!(
        "What a timed-out shell env capture left running (process group {pgid}) has finished"
    );
}

/// Whether any process is still in group `pgid`. One this process may not
/// signal still counts.
fn group_exists(pgid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks whether the group exists and may be
    // signalled; `kill` has no memory-safety preconditions.
    let result = unsafe { libc::kill(-pgid, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn kill_group(pgid: libc::pid_t) {
    // SAFETY: as for `group_exists`. The group still existed when last
    // checked, so its id is still the capture shell's (see the module docs).
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}
