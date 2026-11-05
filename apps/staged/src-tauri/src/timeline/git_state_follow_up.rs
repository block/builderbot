//! One more git-state refresh for a branch whose refresh declined, or showed
//! a provisional state, while its worktree's shell env was still being
//! captured.
//!
//! A display refresh's reads wait at most a couple of seconds for the
//! captured env (`git::compute_local_branch_git_state_bounded`). If one of
//! them ran with the inherited env and failed in a way the captured env might
//! have avoided, the refresh declines and the card keeps the state it has.
//! If one ran with the inherited env and succeeded, its answer may still not
//! be the captured env's (a clean filter only on the captured `PATH` that
//! git skipped), so the refresh shows the state but it is provisional.
//! Nothing on the frontend refreshes again when the capture lands, so
//! [`FollowUps::schedule`] queues one refresh for then, in both cases
//! ([`settle_bounded_build`]).
//!
//! A provisional state is shown rather than held back. The inherited env
//! gets the same answer as the captured one unless the repo needs a tool
//! only the login shell's `PATH` has, and holding every state back until
//! the capture lands would leave a card without a first state for the
//! 9–30 s a cold capture takes, which the bounded reads exist to avoid. A
//! wrong provisional state is display only: the pull, reset and discard
//! builds read with the captured env before they act, and the follow-up
//! replaces it once the capture lands. A failed read is different: the
//! build would take it for an answer (a failed `git status` for a clean
//! tree), so that build declines instead.
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

use crate::git::{BoundedGitState, BranchGitState, GitError};
use crate::shell_env::ShellEnvCache;
use crate::store::Store;

/// What a display refresh emits for `branch_id`, given its bounded `build`:
/// the state, provisional or not, or nothing when the build declined.
/// `follow_up` queues a refresh for when the capture lands and returns
/// whether one is queued ([`refresh_after_capture`]); it runs when the build
/// declined or is provisional, never for a settled one, so a follow-up that
/// reads with the captured env queues nothing more.
pub(super) fn settle_bounded_build(
    branch_id: &str,
    build: Result<BoundedGitState, GitError>,
    follow_up: impl FnOnce() -> bool,
) -> Option<BranchGitState> {
    let landing = |queued: bool| {
        if queued {
            "refreshing again once it lands"
        } else {
            "no capture left to wait for"
        }
    };
    match build {
        Ok(BoundedGitState {
            state,
            provisional: false,
        }) => Some(state),
        Ok(BoundedGitState {
            state,
            provisional: true,
        }) => {
            let queued = follow_up();
            log::debug!(
                "Branch {branch_id}'s git state was read before its shell env was captured \
                 ({})",
                landing(queued)
            );
            Some(state)
        }
        Err(error) => {
            let queued = follow_up();
            log::info!(
                "Keeping branch {branch_id}'s git state until its shell env is captured \
                 ({}): {error}",
                landing(queued)
            );
            None
        }
    }
}

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
    use crate::test_utils::{wait_for_file_sh, write_fake_shell};
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A cache whose fake shell logs each launch to `launches`, runs `wait`
    /// (a line of `sh`), and then exits with `status`. The scratch dir is its
    /// `$HOME`, so the env-root walk never leaves it.
    fn cache_with_shell(
        scratch: &Path,
        wait: &str,
        status: u8,
    ) -> (Arc<ShellEnvCache>, tempfile::TempPath, PathBuf) {
        let launches = scratch.join("launches");
        let shell = write_fake_shell(&format!(
            "#!/bin/sh\necho launch >> '{}'\n{wait}\n[ {status} = 0 ] || exit {status}\n\
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

    /// Some state, for builds that don't need a repo.
    fn any_state() -> BranchGitState {
        crate::git::compute_branch_git_state(
            "follow-up-tests",
            |_| Err("no repo".to_string()),
            "main",
            "main",
            "main",
            crate::git::FetchMode::Never,
            crate::git::WorktreeStatusScope::Full,
        )
    }

    /// A settled build is emitted with no follow-up. A provisional one is
    /// emitted and queues one, as a declined build queues one and emits
    /// nothing, whether or not a capture is left to queue on.
    #[test]
    fn provisional_and_declined_builds_queue_a_follow_up() {
        let settle = |build, queued| {
            let asked = std::cell::Cell::new(false);
            let shown = settle_bounded_build("a", build, || {
                asked.set(true);
                queued
            });
            (shown, asked.get())
        };
        let build = |provisional| {
            Ok(BoundedGitState {
                state: any_state(),
                provisional,
            })
        };

        assert_eq!(settle(build(false), true), (Some(any_state()), false));
        for queued in [true, false] {
            assert_eq!(settle(build(true), queued), (Some(any_state()), true));
            let declined = Err(GitError::CapturedEnvPending(Box::new(
                GitError::GitNotFound,
            )));
            assert_eq!(settle(declined, queued), (None, true));
        }
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
        let (cache, _shell, launch_log) = cache_with_shell(scratch.path(), "sleep 1", 0);
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
        // The shell fails only once the test releases it, so the decline and
        // the schedule both find this one capture still running.
        let release = scratch.path().join("release");
        let wait = wait_for_file_sh(&format!("'{}'", release.display()));
        let (cache, _shell, launch_log) = cache_with_shell(scratch.path(), &wait, 3);
        let worktree = worktree(scratch.path());
        let follow_ups = FollowUps::default();
        let (runs, ran) = mpsc::channel();

        decline(&cache, &worktree);
        assert!(follow_ups.schedule(&cache, &worktree, "a", counted(&runs, "a")));
        // The queued follow-up now holds the only sender.
        drop(runs);
        std::fs::write(&release, "").expect("write release");

        // The capture fails, and a failure isn't cached: its entry is gone
        // before the capture thread drops the follow-up, which frees the
        // branch's place.
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
