//! The capture limit, the queue's tiers, home's place ahead of the queue,
//! the capture timeout and what it leaves running, the timed-out mark,
//! per-capture bounds, `after_capture`, and the per-capture wait summary.
//!
//! The fake shells here sleep for whatever a `.delay` file in the capture
//! directory says when they start (none means no sleep), so one shell can
//! serve fast and slow directories in the same test.

use super::*;
use crate::test_utils::write_fake_shell;
use std::sync::mpsc;

/// Fake `$SHELL` that reads its delay, then logs its directory to
/// `log/launches` and, while it sleeps, keeps a marker file
/// `log/inflight.<pid>`. On start it appends how many markers exist, its own
/// included, to `log/peaks`. Its marker lives inside its capture slot, so the
/// largest peak is the most shells that ran at once. A launch logged has
/// already read its delay, so a test can change `.delay` for the next one.
fn tracking_shell(log: &Path) -> tempfile::TempPath {
    let log = single_quote(&log.to_string_lossy());
    write_fake_shell(&format!(
        "#!/bin/sh\n\
         PATH=/usr/bin:/bin\n\
         export PATH\n\
         delay=\"$(cat .delay 2>/dev/null || echo 0)\"\n\
         pwd >> {log}/launches\n\
         : > {log}/inflight.$$\n\
         ls {log} | grep -c '^inflight\\.' >> {log}/peaks\n\
         sleep \"$delay\"\n\
         rm -f {log}/inflight.$$\n\
         exec /bin/sh -s\n"
    ))
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn launch_dirs(log: &Path) -> Vec<PathBuf> {
    lines(&log.join("launches"))
        .iter()
        .map(|line| canonical(Path::new(line)))
        .collect()
}

fn peak(log: &Path) -> usize {
    lines(&log.join("peaks"))
        .iter()
        .map(|line| line.trim().parse::<usize>().expect("peak count"))
        .max()
        .unwrap_or(0)
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A capture directory under `scratch` whose shell sleeps `delay` seconds.
fn dir_with_delay(scratch: &Path, name: &str, delay: &str) -> PathBuf {
    let dir = scratch.join(name);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join(".delay"), delay).expect("write .delay");
    dir
}

/// A cache using [`tracking_shell`], with `home` as its `$HOME`. Passing the
/// scratch dir keeps the env-root walk inside it, so directories beneath it
/// are their own keys whatever checkout `TMPDIR` sits in. Hold the returned
/// shell for the test's lifetime.
fn tracking_cache(log: &Path, home: &Path) -> (Arc<ShellEnvCache>, tempfile::TempPath) {
    tracking_cache_with(log, home, |cache| cache)
}

/// [`tracking_cache`], adjusted by `configure`.
fn tracking_cache_with(
    log: &Path,
    home: &Path,
    configure: impl FnOnce(ShellEnvCache) -> ShellEnvCache,
) -> (Arc<ShellEnvCache>, tempfile::TempPath) {
    let shell = tracking_shell(log);
    let cache = ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600))
        .with_home(home);
    (Arc::new(configure(cache)), shell)
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Take every capture slot, as captures of other directories would.
fn hold_every_slot(cache: &ShellEnvCache) -> Vec<capture_slots::CaptureSlot<'_>> {
    (0..CAPTURE_LIMIT)
        .map(|_| cache.slots.acquire(&Urgency::default()))
        .collect()
}

fn entry_is_ready(cache: &ShellEnvCache, key: &Path) -> bool {
    matches!(
        cache.inner.lock().unwrap().get(key),
        Some(CachedEntry::Ready(_))
    )
}

fn entry_is_in_flight(cache: &ShellEnvCache, key: &Path) -> bool {
    matches!(
        cache.inner.lock().unwrap().get(key),
        Some(CachedEntry::InFlight(_))
    )
}

/// Whether any process is left in process group `group`.
fn group_exists(group: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks whether the group exists.
    unsafe { libc::kill(-group, 0) == 0 }
}

/// [`tracking_cache_with`] with `timeouts`.
fn timing_out_cache(
    log: &Path,
    home: &Path,
    timeouts: CaptureTimeouts,
) -> (Arc<ShellEnvCache>, tempfile::TempPath) {
    tracking_cache_with(log, home, |cache| cache.with_capture_timeouts(timeouts))
}

fn in_flight_promise(cache: &ShellEnvCache, key: &Path) -> Arc<InFlightPromise> {
    match cache.inner.lock().unwrap().get(key) {
        Some(CachedEntry::InFlight(handle)) => handle.promise.clone(),
        _ => panic!("{} has no capture in flight", key.display()),
    }
}

/// L1: Six cold directories asked for through every path at once — inline
/// `get_blocking`, async `get`, a bounded read and a prewarm — never have
/// more than two shells running, and do use both slots.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn captures_never_exceed_the_limit_across_paths() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let (cache, _shell) = tracking_cache(log.path(), scratch.path());
    let dirs: Vec<PathBuf> = (0..6)
        .map(|i| dir_with_delay(scratch.path(), &format!("wt{i}"), "0.6"))
        .collect();

    let mut sync_tasks = Vec::new();
    for dir in &dirs[0..2] {
        let (cache, dir) = (cache.clone(), dir.clone());
        sync_tasks.push(tokio::task::spawn_blocking(move || {
            cache.get_blocking(&dir).map(|_| ())
        }));
    }
    let mut async_tasks = Vec::new();
    for dir in &dirs[2..4] {
        let (cache, dir) = (cache.clone(), dir.clone());
        async_tasks.push(tokio::spawn(
            async move { cache.get(&dir).await.map(|_| ()) },
        ));
    }
    let bounded = tokio::task::spawn_blocking({
        let (cache, dir) = (cache.clone(), dirs[4].clone());
        move || cache.get_blocking_within(&dir, Duration::ZERO).map(|_| ())
    });
    cache.prewarm(&dirs[5]);

    for task in sync_tasks {
        task.await.expect("sync task").expect("capture");
    }
    for task in async_tasks {
        task.await.expect("async task").expect("capture");
    }
    bounded.await.expect("bounded task").expect("bounded read");
    tokio::task::spawn_blocking({
        let (cache, dirs) = (cache.clone(), dirs.clone());
        move || {
            wait_until("every capture to land", || {
                dirs.iter().all(|dir| entry_is_ready(&cache, dir))
            })
        }
    })
    .await
    .expect("wait task");

    assert_eq!(launch_dirs(log.path()).len(), 6, "one shell per directory");
    assert_eq!(
        peak(log.path()),
        CAPTURE_LIMIT,
        "peaks: {:?}",
        lines(&log.path().join("peaks"))
    );
    assert_eq!(cache.slots.in_use(), 0);
    assert_eq!(cache.slots.queued(), 0);
}

/// L2: With two worktree captures holding both slots and a third queued,
/// home's capture starts at once and finishes while they are still going.
/// The worktrees sit under home, where the env-root walk stops, so each is
/// its own key whatever checkout `TMPDIR` sits in.
#[test]
fn home_capture_skips_the_queue() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let home = dir_with_delay(scratch.path(), "home", "0");
    let (cache, _shell) = tracking_cache(log.path(), &home);
    let busy = [
        dir_with_delay(&home, "wt-a", "3"),
        dir_with_delay(&home, "wt-b", "3"),
    ];
    let queued = dir_with_delay(&home, "wt-c", "0");

    for dir in &busy {
        cache.prewarm(dir);
    }
    wait_until("both worktree shells to start", || {
        launch_dirs(log.path()).len() == 2
    });
    cache.prewarm(&queued);
    wait_until("the third worktree to queue", || cache.slots.queued() == 1);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");
    let home_env = runtime.block_on(capture_home_interactive_env_for_dir(
        &cache,
        &home,
        Duration::from_secs(30),
    ));

    assert_eq!(
        home_env.get("PATH").map(String::as_str),
        Some("/usr/bin:/bin")
    );
    for dir in &busy {
        assert!(
            entry_is_in_flight(&cache, dir),
            "home finished before {}",
            dir.display()
        );
    }
    assert_eq!(cache.slots.in_use(), 2, "home took no slot");
    assert_eq!(
        cache.slots.queued(),
        1,
        "home went ahead of the queued worktree"
    );

    wait_until("the worktree captures to land", || {
        busy.iter()
            .chain([&queued])
            .all(|dir| entry_is_ready(&cache, dir))
    });
    let launched = launch_dirs(log.path());
    assert_eq!(launched.len(), 4);
    assert_eq!(launched[2], canonical(&home), "launch order: {launched:?}");
    assert_eq!(
        launched[3],
        canonical(&queued),
        "launch order: {launched:?}"
    );
}

/// L3: A home-env caller that runs out of time gets the fallback (an empty
/// map) without killing or evicting the capture. The capture lands, and the
/// next caller gets it from the cache without a second shell. The wait
/// summary counts the caller that gave up.
#[tokio::test]
async fn home_timeout_falls_back_but_the_capture_lands() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let home = dir_with_delay(scratch.path(), "home", "1");
    let (cache, _shell) = tracking_cache(log.path(), &home);

    let started = Instant::now();
    let fallback =
        capture_home_interactive_env_for_dir(&cache, &home, Duration::from_millis(100)).await;
    let waited = started.elapsed();
    assert!(fallback.is_empty(), "{fallback:?}");
    assert!(
        waited < Duration::from_millis(900),
        "overran its timeout: {waited:?}"
    );

    assert!(
        entry_is_in_flight(&cache, &home),
        "the timeout must leave the capture in flight"
    );
    let promise = in_flight_promise(&cache, &home);
    {
        let waits = promise.waits.lock().unwrap();
        assert_eq!((waits.waiters, waits.gave_up), (1, 1));
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    while !entry_is_ready(&cache, &home) {
        assert!(Instant::now() < deadline, "the capture never landed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let env = capture_home_interactive_env_for_dir(&cache, &home, Duration::from_secs(30)).await;
    assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin:/bin"));
    assert_eq!(launch_dirs(log.path()), vec![canonical(&home)]);
}

/// L4: A bounded read whose capture is stuck behind a full queue falls back
/// at its bound, not at the bound plus the queue time, and never launched a
/// shell. Once the slots free up the capture runs and lands for later reads.
#[test]
fn bounded_read_falls_back_at_the_bound_while_queued() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let dir = dir_with_delay(scratch.path(), "wt", "0");
    let (cache, _shell) = tracking_cache(log.path(), scratch.path());

    // Two captures elsewhere hold both slots.
    let storm = hold_every_slot(&cache);

    let bound = Duration::from_millis(300);
    let started = Instant::now();
    let read = cache
        .get_blocking_within(&dir, bound)
        .expect("bounded read");
    let waited = started.elapsed();
    assert!(
        matches!(read, BoundedEnv::StillCapturing { first: true }),
        "{read:?}"
    );
    assert!(waited >= bound, "gave up early: {waited:?}");
    assert!(
        waited < bound + Duration::from_millis(700),
        "overran its bound: {waited:?}"
    );
    wait_until("its capture to be queued", || cache.slots.queued() == 1);
    assert!(launch_dirs(log.path()).is_empty(), "no shell yet");

    drop(storm);
    wait_until("the queued capture to land", || {
        entry_is_ready(&cache, &dir)
    });
    match cache
        .get_blocking_within(&dir, Duration::ZERO)
        .expect("cache hit")
    {
        BoundedEnv::Ready(env) => assert!(env.vars().iter().any(|(k, _)| k == "PATH")),
        other => panic!("expected the landed snapshot, got {other:?}"),
    }
    assert_eq!(launch_dirs(log.path()), vec![canonical(&dir)]);
}

/// L5: The summary counts every waiter once, says how many gave up, and
/// takes the longest wait from whichever was longer: a waiter that stayed
/// to the end, or one that gave up.
#[test]
fn wait_summary_counts_every_waiter_once() {
    let promise = InFlightPromise::new();
    let gave_up = promise.register_waiter();
    let stayed = promise.register_waiter();
    std::thread::sleep(Duration::from_millis(30));
    drop(gave_up);
    let joined_late = promise.register_waiter();

    std::thread::sleep(Duration::from_millis(30));
    let finished = Instant::now();
    promise.set_done(Err("capture failed".to_string()));
    // Waking after the capture finished doesn't count as giving up.
    drop(stayed);
    drop(joined_late);

    let summary = promise.wait_summary(finished);
    assert_eq!((summary.waiters, summary.gave_up), (3, 1));
    assert!(
        summary.longest >= Duration::from_millis(60),
        "the waiter that stayed waited longest: {summary:?}"
    );
    let first_wait = promise.waits.lock().unwrap().longest_gave_up;
    assert!(first_wait >= Duration::from_millis(30) && first_wait < summary.longest);
}

/// L6: A shell that never finishes is killed at the capture timeout,
/// whether the capture ran inline for a sync caller or on a capture thread
/// for an async one. Each capture fails, frees its slot and leaves a
/// timed-out mark where its entry was. Only the shells are killed then:
/// the `sleep` each started keeps its process group going until the group
/// deadline, when what is left of the group is killed.
#[test]
fn hung_shell_is_killed_at_the_capture_timeout() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    // Short enough to keep the test quick, long enough that on a loaded
    // machine the hung shells get as far as their markers and the fresh
    // captures at the end finish in time. The marks expire at once, so
    // those fresh captures can start.
    let timeouts = CaptureTimeouts {
        shell: Duration::from_secs(5),
        group: Duration::from_secs(8),
        retry_after: Duration::ZERO,
    };
    let (cache, _shell) = tracking_cache_with(log.path(), scratch.path(), |cache| {
        cache.with_capture_timeouts(timeouts)
    });
    let inline_dir = dir_with_delay(scratch.path(), "wt-inline", "30");
    let async_dir = dir_with_delay(scratch.path(), "wt-async", "30");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let started = Instant::now();
    let inline = std::thread::spawn({
        let (cache, dir) = (cache.clone(), inline_dir.clone());
        move || cache.get_blocking(&dir).map(|_| ())
    });
    let async_err = runtime
        .block_on(cache.get(&async_dir))
        .map(|_| ())
        .expect_err("the hung background capture fails");
    let inline_err = inline
        .join()
        .expect("sync thread")
        .expect_err("the hung inline capture fails");
    let waited = started.elapsed();

    // Waiters get the error as text through the in-flight promise, as for
    // any failed capture, so its message is what both callers share.
    for error in [&inline_err, &async_err] {
        assert!(error.to_string().contains("still running after"), "{error}");
    }
    assert!(
        waited >= timeouts.shell && waited < timeouts.shell * 3,
        "each capture should end at its timeout: {waited:?}"
    );
    assert_eq!(cache.slots.in_use(), 0, "both slots are free again");
    for dir in [&inline_dir, &async_dir] {
        assert!(
            matches!(
                cache.inner.lock().unwrap().get(dir),
                Some(CachedEntry::TimedOut(_))
            ),
            "{} has no timed-out mark",
            dir.display()
        );
    }

    // Each killed shell left its marker behind, named for its pid, which is
    // also its process group. The `sleep` it was running is still in there.
    let groups: Vec<libc::pid_t> = std::fs::read_dir(log.path())
        .expect("read log dir")
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            name.to_str()?.strip_prefix("inflight.")?.parse().ok()
        })
        .collect();
    assert_eq!(groups.len(), 2, "one marker per killed shell");
    for group in &groups {
        assert!(
            group_exists(*group),
            "group {group} was killed with its shell"
        );
    }
    wait_until("the leftover groups to be killed", || {
        groups.iter().all(|group| !group_exists(*group))
    });
    let gone_after = started.elapsed();
    assert!(
        gone_after < timeouts.group + Duration::from_secs(10),
        "the groups outlived their deadline, so their `sleep 30`s ended on their own: \
         {gone_after:?}"
    );

    for dir in [&inline_dir, &async_dir] {
        std::fs::write(dir.join(".delay"), "0").expect("write .delay");
    }
    let inline_env = cache
        .get_blocking(&inline_dir)
        .expect("a fresh inline capture");
    let async_env = runtime
        .block_on(cache.get(&async_dir))
        .expect("a fresh async capture");
    for env in [inline_env, async_env] {
        assert!(env
            .vars()
            .iter()
            .any(|(k, v)| k == "PATH" && v == "/usr/bin:/bin"));
    }
    assert_eq!(
        launch_dirs(log.path()).len(),
        4,
        "a fresh shell per directory once the marks expire"
    );
}

/// L7: The bound is per capture. Once one bounded read has waited out its
/// bound on a capture, the next bounded read of that same capture returns
/// at once rather than waiting out a bound of its own.
#[test]
fn bound_runs_out_once_per_capture_not_once_per_read() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let dir = dir_with_delay(scratch.path(), "wt", "5");
    let (cache, _shell) = tracking_cache(log.path(), scratch.path());
    let bound = Duration::from_secs(1);

    let started = Instant::now();
    let first = cache.get_blocking_within(&dir, bound).expect("first read");
    let first_waited = started.elapsed();
    let started = Instant::now();
    let second = cache.get_blocking_within(&dir, bound).expect("second read");
    let second_waited = started.elapsed();

    assert!(
        matches!(first, BoundedEnv::StillCapturing { first: true }),
        "{first:?}"
    );
    assert!(first_waited >= bound, "gave up early: {first_waited:?}");
    assert!(
        matches!(second, BoundedEnv::StillCapturing { first: false }),
        "{second:?}"
    );
    assert!(
        second_waited < bound / 2,
        "the second read waited out a bound of its own: {second_waited:?}"
    );
    let waits = in_flight_promise(&cache, &dir)
        .waits
        .lock()
        .unwrap()
        .waiters;
    assert_eq!(
        waits, 1,
        "the read that didn't wait isn't counted as a waiter"
    );

    // An unbounded caller still gets the capture, which ran once.
    cache.get_blocking(&dir).expect("capture");
    assert_eq!(launch_dirs(log.path()), vec![canonical(&dir)]);
}

/// L7b: The state build's shape, which the before/after benchmark pinned
/// at about two bounds per refresh. Phase 1's reads wait on a cold capture
/// side by side and give up together at the bound. Phase 2's `rev-list` and
/// `merge-base`, issued one after the other once phase 1 is done, return at
/// once, including from a subdirectory sharing the worktree's capture. So a
/// refresh waits about one bound in all.
#[test]
fn later_phases_of_a_state_build_dont_wait_again() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let worktree = dir_with_delay(scratch.path(), "wt", "6");
    std::fs::create_dir(worktree.join(".git")).expect("mkdir .git");
    let subdir = worktree.join("apps/staged");
    std::fs::create_dir_all(&subdir).expect("mkdir subdir");
    let (cache, _shell) = tracking_cache(log.path(), scratch.path());
    let bound = Duration::from_secs(1);

    let started = Instant::now();
    let phase_1: Vec<_> = ["rev-parse", "status", "for-each-ref", "merge-base"]
        .into_iter()
        .map(|_| {
            let (cache, dir) = (cache.clone(), worktree.clone());
            std::thread::spawn(move || cache.get_blocking_within(&dir, bound))
        })
        .collect();
    for read in phase_1 {
        let read = read.join().expect("phase 1 read").expect("bounded read");
        assert!(
            matches!(read, BoundedEnv::StillCapturing { .. }),
            "{read:?}"
        );
    }
    let phase_1_took = started.elapsed();

    let started = Instant::now();
    for dir in [&worktree, &worktree, &subdir] {
        let read = cache.get_blocking_within(dir, bound).expect("bounded read");
        assert!(
            matches!(read, BoundedEnv::StillCapturing { first: false }),
            "{read:?}"
        );
    }
    let phase_2_took = started.elapsed();

    assert!(
        phase_1_took >= bound && phase_1_took < bound * 3,
        "phase 1 should give up together at the bound: {phase_1_took:?}"
    );
    assert!(
        phase_2_took < bound / 2,
        "phase 2 waited out the bound again: {phase_2_took:?}"
    );
    assert_eq!(launch_dirs(log.path()), vec![canonical(&worktree)]);
}

/// L8: With both slots held and two prewarms queued, a capture with a caller
/// waiting on it without a bound goes ahead of them: one prewarm that an
/// async caller then joins, and a sync caller's capture queued after both.
/// The other prewarm goes last.
#[test]
fn captures_a_caller_is_blocked_on_go_ahead_of_prewarms() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let (cache, _shell) = tracking_cache(log.path(), scratch.path());
    let prewarm_only = dir_with_delay(scratch.path(), "wt-prewarm", "0");
    let joined = dir_with_delay(scratch.path(), "wt-joined", "0");
    let sync_needed = dir_with_delay(scratch.path(), "wt-sync", "0");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime");

    let mut held = hold_every_slot(&cache);
    cache.prewarm(&prewarm_only);
    wait_until("the first prewarm to queue", || cache.slots.queued() == 1);
    cache.prewarm(&joined);
    wait_until("the second prewarm to queue", || cache.slots.queued() == 2);
    let async_caller = runtime.spawn({
        let (cache, dir) = (cache.clone(), joined.clone());
        async move { cache.get(&dir).await.map(|_| ()) }
    });
    wait_until("the async caller to join", || {
        in_flight_promise(&cache, &joined)
            .waits
            .lock()
            .unwrap()
            .waiters
            == 1
    });
    let sync_caller = std::thread::spawn({
        let (cache, dir) = (cache.clone(), sync_needed.clone());
        move || cache.get_blocking(&dir).map(|_| ())
    });
    wait_until("the sync capture to queue", || cache.slots.queued() == 3);
    assert!(launch_dirs(log.path()).is_empty());

    // One slot frees up and the queue drains through it one capture at a
    // time, so the launch order is whose turn each release gave.
    drop(held.pop());
    runtime
        .block_on(async_caller)
        .expect("async task")
        .expect("capture");
    sync_caller.join().expect("sync thread").expect("capture");
    wait_until("the prewarm to land", || {
        entry_is_ready(&cache, &prewarm_only)
    });
    drop(held);

    assert_eq!(
        launch_dirs(log.path()),
        vec![
            canonical(&joined),
            canonical(&sync_needed),
            canonical(&prewarm_only)
        ]
    );
}

/// L9: A capture that finishes after its key was invalidated doesn't
/// overwrite the newer capture started since. Its own waiter still gets its
/// result, and the newer capture is what lands in the cache.
#[test]
fn capture_finishing_after_invalidate_leaves_the_newer_entry() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let dir = dir_with_delay(scratch.path(), "wt", "0.3");
    let (cache, _shell) = tracking_cache(log.path(), scratch.path());

    cache.prewarm(&dir);
    wait_until("the first shell to start", || {
        launch_dirs(log.path()).len() == 1
    });
    let older = in_flight_promise(&cache, &dir);
    cache.invalidate(&dir);
    std::fs::write(dir.join(".delay"), "3").expect("write .delay");
    cache.prewarm(&dir);
    let newer = in_flight_promise(&cache, &dir);
    assert!(!Arc::ptr_eq(&older, &newer));

    let older_env = older
        .wait_blocking()
        .expect("published")
        .expect("older capture");
    match cache.inner.lock().unwrap().get(&dir) {
        Some(CachedEntry::InFlight(handle)) => assert!(
            Arc::ptr_eq(&handle.promise, &newer),
            "the newer capture's entry was replaced"
        ),
        Some(CachedEntry::Ready(_) | CachedEntry::TimedOut(_)) => {
            panic!("the older capture overwrote the newer one")
        }
        None => panic!("the newer capture's entry was removed"),
    }

    let newer_env = newer
        .wait_blocking()
        .expect("published")
        .expect("newer capture");
    match cache.inner.lock().unwrap().get(&dir) {
        Some(CachedEntry::Ready(env)) => {
            assert_eq!(env.captured_at(), newer_env.captured_at());
            assert_ne!(env.captured_at(), older_env.captured_at());
        }
        _ => panic!("the newer capture didn't land"),
    }
    assert_eq!(launch_dirs(log.path()).len(), 2);
}

/// L10: The capture timeout kills the shell but not what it started. A
/// child still at work when the shell is killed, as a dev-shell build or a
/// download would be, carries on after the capture has failed and finishes.
#[test]
fn timeout_kills_the_shell_but_not_what_it_started() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let dir = scratch.path().join("wt");
    std::fs::create_dir_all(&dir).expect("mkdir");
    // The child says it started, waits for the test's go-ahead, and says it
    // finished. The shell waits for the child to start, then hangs.
    let marks = single_quote(&log.path().to_string_lossy());
    let shell = write_fake_shell(&format!(
        "#!/bin/sh\n\
         PATH=/usr/bin:/bin\n\
         export PATH\n\
         ( : > {marks}/started; \
           while [ ! -e {marks}/go ]; do sleep 0.05; done; \
           : > {marks}/finished ) &\n\
         while [ ! -e {marks}/started ]; do sleep 0.05; done\n\
         exec sleep 30\n"
    ));
    let cache = ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600))
        .with_home(scratch.path())
        .with_capture_timeouts(CaptureTimeouts {
            shell: Duration::from_secs(2),
            group: Duration::from_secs(60),
            retry_after: Duration::from_secs(60),
        });

    let error = cache
        .get_blocking(&dir)
        .map(|_| ())
        .expect_err("the hung capture fails");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
    assert_eq!(cache.slots.in_use(), 0);
    wait_until("the child to start", || log.path().join("started").exists());
    assert!(!log.path().join("finished").exists());

    std::fs::write(log.path().join("go"), "").expect("write go");
    wait_until("the child to finish after its shell was killed", || {
        log.path().join("finished").exists()
    });
}

/// L11: For a while after a capture times out, every lookup of its root
/// fails at once with a recent-timeout error, and nothing launches another
/// shell: not a bounded read, a sync or async caller, a prewarm, or
/// `after_capture`. `invalidate` clears the mark, and the next caller
/// captures afresh.
#[test]
fn lookups_fail_fast_after_a_timeout() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let dir = dir_with_delay(scratch.path(), "wt", "30");
    let (cache, _shell) = timing_out_cache(
        log.path(),
        scratch.path(),
        CaptureTimeouts {
            shell: Duration::from_secs(1),
            group: Duration::from_secs(2),
            retry_after: Duration::from_secs(600),
        },
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let timed_out = cache.get_blocking(&dir).map(|_| ()).expect_err("times out");
    assert!(!is_recent_timeout(&timed_out), "{timed_out}");

    let fast = Duration::from_millis(500);
    let started = Instant::now();
    let bounded = cache
        .get_blocking_within(&dir, Duration::from_secs(5))
        .map(|_| ())
        .expect_err("bounded read");
    let sync = cache.get_blocking(&dir).map(|_| ()).expect_err("sync");
    let async_ = runtime
        .block_on(cache.get(&dir))
        .map(|_| ())
        .expect_err("async");
    let took = started.elapsed();
    for error in [&bounded, &sync, &async_] {
        assert!(is_recent_timeout(error), "{error}");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
    assert!(took < fast, "three lookups took {took:?}");
    cache.prewarm(&dir);
    assert!(!cache.after_capture(&dir, || panic!("nothing will land")));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(launch_dirs(log.path()).len(), 1, "no second shell");

    cache.invalidate(&dir);
    std::fs::write(dir.join(".delay"), "0").expect("write .delay");
    cache.get_blocking(&dir).expect("a fresh capture");
    assert_eq!(launch_dirs(log.path()).len(), 2);
}

/// L12: The timed-out mark expires. Inside its window a lookup fails at
/// once; after it the next caller starts a capture, whose snapshot replaces
/// the mark.
#[test]
fn timed_out_mark_expires() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let dir = dir_with_delay(scratch.path(), "wt", "30");
    let retry_after = Duration::from_secs(2);
    let (cache, _shell) = timing_out_cache(
        log.path(),
        scratch.path(),
        CaptureTimeouts {
            shell: Duration::from_secs(1),
            group: Duration::from_secs(2),
            retry_after,
        },
    );

    cache.get_blocking(&dir).map(|_| ()).expect_err("times out");
    let marked = Instant::now();
    std::fs::write(dir.join(".delay"), "0").expect("write .delay");
    let early = cache
        .get_blocking_within(&dir, Duration::from_secs(5))
        .map(|_| ())
        .expect_err("inside the window");
    assert!(is_recent_timeout(&early), "{early}");
    assert_eq!(launch_dirs(log.path()).len(), 1);

    std::thread::sleep(retry_after.saturating_sub(marked.elapsed()) + Duration::from_millis(100));
    let env = cache
        .get_blocking(&dir)
        .expect("a capture once the mark expires");
    assert!(env.vars().iter().any(|(k, _)| k == "PATH"));
    assert!(
        entry_is_ready(&cache, &dir),
        "the snapshot replaced the mark"
    );
    assert_eq!(launch_dirs(log.path()).len(), 2);
}

/// L13: `after_capture` neither waits on a capture nor moves it up the
/// queue. With both slots held and two prewarms queued, a callback on the
/// second leaves it behind the first and isn't counted as a waiter. The
/// callback runs once, after the snapshot is cached. On a root with a fresh
/// snapshot it runs at once, and with nothing cached or capturing it is
/// dropped.
#[test]
fn after_capture_runs_once_the_capture_lands_without_promoting_it() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let (cache, _shell) = tracking_cache(log.path(), scratch.path());
    let first = dir_with_delay(scratch.path(), "wt-first", "0");
    let second = dir_with_delay(scratch.path(), "wt-second", "0");
    let idle = dir_with_delay(scratch.path(), "wt-idle", "0");
    let (ran, landed) = mpsc::channel();

    let mut held = hold_every_slot(&cache);
    cache.prewarm(&first);
    wait_until("the first prewarm to queue", || cache.slots.queued() == 1);
    cache.prewarm(&second);
    wait_until("the second prewarm to queue", || cache.slots.queued() == 2);
    assert!(cache.after_capture(&second, {
        let (cache, dir, ran) = (cache.clone(), second.clone(), ran.clone());
        move || ran.send(entry_is_ready(&cache, &dir)).expect("send")
    }));
    let waiters = in_flight_promise(&cache, &second)
        .waits
        .lock()
        .unwrap()
        .waiters;
    assert_eq!(waiters, 0, "a callback isn't a waiter");
    assert!(landed.try_recv().is_err());

    // One slot frees up and the queue drains through it in turn.
    drop(held.pop());
    let ready = landed
        .recv_timeout(Duration::from_secs(30))
        .expect("the callback ran");
    assert!(ready, "the callback ran before the snapshot was cached");
    assert!(
        landed.recv_timeout(Duration::from_millis(300)).is_err(),
        "the callback ran twice"
    );
    assert_eq!(
        launch_dirs(log.path()),
        vec![canonical(&first), canonical(&second)],
        "the callback moved its capture ahead"
    );

    assert!(cache.after_capture(&second, move || ran.send(true).expect("send")));
    assert_eq!(
        landed.try_recv(),
        Ok(true),
        "a fresh snapshot runs it at once"
    );
    assert!(!cache.after_capture(&idle, || panic!("nothing will land")));
    drop(held);
}
