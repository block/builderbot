//! One more git-state refresh for a branch whose refresh declined while its
//! worktree's shell env was still being captured.
//!
//! A display refresh's reads wait at most a couple of seconds for the
//! captured env (`git::compute_local_branch_git_state_bounded`). If one of
//! them ran with the inherited env and failed in a way the captured env might
//! have avoided, the refresh declines and the card keeps the state it has.
//! Nothing on the frontend refreshes again when the capture lands, so
//! [`FollowUps::schedule`] queues one refresh for then.
//!
//! The follow-up rides on the capture (`ShellEnvCache::after_capture`):
//! nothing waits on it, so it isn't moved up the slot queue or counted as a
//! waiter. A branch has at most one follow-up queued, however many of its
//! refreshes decline before the capture lands. A capture that fails drops the
//! follow-up, so captures that keep failing can't chain refreshes; the card's
//! next refresh (window focus, the layer becoming active, a timeline reload)
//! runs with whatever env there is by then.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::shell_env::ShellEnvCache;
use crate::store::Store;

/// Queue one more [`super::refresh_branch_git_state_impl`] for `branch_id`,
/// with the fetch TTL, for when `worktree`'s shell env capture lands. Returns
/// whether one is queued, by this call or an earlier one.
pub(super) fn refresh_after_capture(
    app: &tauri::AppHandle,
    store: &Arc<Store>,
    worktree: &Path,
    branch_id: &str,
) -> bool {
    static FOLLOW_UPS: OnceLock<FollowUps> = OnceLock::new();
    let (app, store, id) = (app.clone(), Arc::clone(store), branch_id.to_owned());
    FOLLOW_UPS.get_or_init(FollowUps::default).schedule(
        crate::session_runner::shell_env_cache(),
        worktree,
        branch_id,
        move || {
            if let Err(error) = super::refresh_branch_git_state_impl(&app, &store, &id, None) {
                log::warn!("Follow-up git state refresh for branch {id} failed: {error}");
            }
        },
    )
}

/// Branches with a follow-up refresh queued on their worktree's capture.
#[derive(Default)]
pub(super) struct FollowUps {
    pending: Arc<Mutex<HashSet<String>>>,
}

impl FollowUps {
    /// Run `refresh` on a thread of its own once `worktree`'s shell env
    /// capture lands, or now if it already has, unless `branch_id` has a
    /// follow-up queued already. Returns whether one is queued, by this call
    /// or an earlier one. Nothing is queued if the capture has failed, or
    /// nothing is cached or capturing for `worktree`.
    pub(super) fn schedule(
        &self,
        cache: &ShellEnvCache,
        worktree: &Path,
        branch_id: &str,
        refresh: impl FnOnce() + Send + 'static,
    ) -> bool {
        if !self.lock().insert(branch_id.to_owned()) {
            return true;
        }
        let place = Place {
            pending: Arc::clone(&self.pending),
            branch_id: branch_id.to_owned(),
        };
        cache.after_capture(worktree, move || {
            // Give up the place first, so a follow-up that declines as well
            // can queue another.
            drop(place);
            let spawned = std::thread::Builder::new()
                .name("git-state-follow-up".to_string())
                .spawn(refresh);
            if let Err(error) = spawned {
                log::warn!("Couldn't start a follow-up git state refresh: {error}");
            }
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A branch's place in [`FollowUps::pending`], held by its queued follow-up
/// and given up when the follow-up runs or is dropped.
struct Place {
    pending: Arc<Mutex<HashSet<String>>>,
    branch_id: String,
}

impl Drop for Place {
    fn drop(&mut self) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.branch_id);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::shell_env::BoundedEnv;
    use crate::test_utils::write_fake_shell;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A cache whose fake shell takes `delay` seconds, logs each launch to
    /// `launches`, and then exits with `status`. The scratch dir is its
    /// `$HOME`, so the env-root walk never leaves it.
    fn cache_with_shell(
        scratch: &Path,
        delay: &str,
        status: u8,
    ) -> (Arc<ShellEnvCache>, tempfile::TempPath, PathBuf) {
        let launches = scratch.join("launches");
        let shell = write_fake_shell(&format!(
            "#!/bin/sh\necho launch >> '{}'\nsleep {delay}\n[ {status} = 0 ] || exit {status}\n\
             PATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n",
            launches.display()
        ));
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600))
                .with_home(scratch);
        (Arc::new(cache), shell, launches)
    }

    fn worktree(scratch: &Path) -> PathBuf {
        let dir = scratch.join("wt");
        std::fs::create_dir_all(dir.join(".git")).expect("mkdir wt/.git");
        dir
    }

    /// Stand-in for a refresh that declined: a bounded read of a cold
    /// worktree that gave up while the capture is still running.
    fn decline(cache: &ShellEnvCache, worktree: &Path) {
        let read = cache
            .get_blocking_within(worktree, Duration::from_millis(50))
            .expect("bounded read");
        assert!(
            matches!(read, BoundedEnv::StillCapturing { .. }),
            "{read:?}"
        );
    }

    /// A refresh that counts itself on `runs`.
    fn counted(runs: &mpsc::Sender<&'static str>, name: &'static str) -> impl FnOnce() + Send {
        let runs = runs.clone();
        move || runs.send(name).expect("send")
    }

    fn launches(path: &Path) -> usize {
        std::fs::read_to_string(path).map_or(0, |log| log.lines().count())
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Repeated declines before the capture lands queue one follow-up per
    /// branch, which runs once the capture lands. Once it has run, a later
    /// decline queues afresh, and with the capture landed it runs at once.
    #[test]
    fn declined_refresh_runs_once_when_the_capture_lands() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let (cache, _shell, launch_log) = cache_with_shell(scratch.path(), "1", 0);
        let worktree = worktree(scratch.path());
        let follow_ups = FollowUps::default();
        let (runs, ran) = mpsc::channel();

        for _ in 0..3 {
            decline(&cache, &worktree);
            assert!(follow_ups.schedule(&cache, &worktree, "a", counted(&runs, "a")));
        }
        decline(&cache, &worktree);
        assert!(follow_ups.schedule(&cache, &worktree, "b", counted(&runs, "b")));
        assert!(
            ran.try_recv().is_err(),
            "a follow-up ran before the capture landed"
        );

        let mut first = [
            ran.recv_timeout(Duration::from_secs(30))
                .expect("first follow-up"),
            ran.recv_timeout(Duration::from_secs(30))
                .expect("second follow-up"),
        ];
        first.sort_unstable();
        assert_eq!(first, ["a", "b"], "one follow-up per branch");
        assert!(
            ran.recv_timeout(Duration::from_millis(500)).is_err(),
            "a branch's follow-up ran more than once"
        );
        assert!(follow_ups.lock().is_empty());

        assert!(follow_ups.schedule(&cache, &worktree, "a", counted(&runs, "a")));
        assert_eq!(ran.recv_timeout(Duration::from_secs(30)), Ok("a"));
        assert_eq!(launches(&launch_log), 1, "follow-ups never capture");
    }

    /// A capture that fails drops the follow-up without running it, and frees
    /// the branch's place.
    #[test]
    fn follow_up_is_dropped_when_the_capture_fails() {
        let scratch = tempfile::tempdir().expect("tempdir");
        // Slow enough that the decline, the schedule and the `get_blocking`
        // below all find this one capture still running on a loaded machine.
        let (cache, _shell, launch_log) = cache_with_shell(scratch.path(), "2", 3);
        let worktree = worktree(scratch.path());
        let follow_ups = FollowUps::default();
        let (runs, ran) = mpsc::channel();

        decline(&cache, &worktree);
        assert!(follow_ups.schedule(&cache, &worktree, "a", counted(&runs, "a")));
        // The queued follow-up now holds the only sender.
        drop(runs);
        // The capture fails at about 2 s, and a failure isn't cached.
        let failed = cache.get_blocking(&worktree).map(|_| ());
        assert!(failed.is_err(), "{failed:?}");

        // The capture thread drops the follow-up after it has woken its
        // waiters, so wait for that rather than allow it a fixed time.
        wait_until("the branch to lose its place", || {
            follow_ups.lock().is_empty()
        });
        assert_eq!(
            ran.recv_timeout(Duration::from_secs(30)),
            Err(mpsc::RecvTimeoutError::Disconnected),
            "the follow-up ran"
        );
        assert!(
            !follow_ups.schedule(&cache, &worktree, "a", || panic!("nothing will land")),
            "nothing is capturing, so nothing is queued"
        );
        assert!(follow_ups.lock().is_empty());
        assert_eq!(launches(&launch_log), 1);
    }
}
