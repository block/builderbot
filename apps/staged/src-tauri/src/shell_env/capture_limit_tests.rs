//! The capture limit, home's place ahead of the queue, and the per-capture
//! wait summary.
//!
//! The fake shells here sleep for whatever a `.delay` file in the capture
//! directory says (none means no sleep), so one shell can serve fast and
//! slow directories in the same test.

use super::*;
use crate::test_utils::write_fake_shell;

/// Fake `$SHELL` that logs each launch's directory to `log/launches` and,
/// while it sleeps, keeps a marker file in `log/`. On start it appends how
/// many markers exist, its own included, to `log/peaks`. Its marker lives
/// inside its capture slot, so the largest peak is the most shells that ran
/// at once.
fn tracking_shell(log: &Path) -> tempfile::TempPath {
    let log = single_quote(&log.to_string_lossy());
    write_fake_shell(&format!(
        "#!/bin/sh\n\
         PATH=/usr/bin:/bin\n\
         export PATH\n\
         pwd >> {log}/launches\n\
         : > {log}/inflight.$$\n\
         ls {log} | grep -c '^inflight\\.' >> {log}/peaks\n\
         sleep \"$(cat .delay 2>/dev/null || echo 0)\"\n\
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

/// A cache using [`tracking_shell`], with `home` as its `$HOME` if given.
/// Hold the returned shell for the test's lifetime.
fn tracking_cache(log: &Path, home: Option<&Path>) -> (Arc<ShellEnvCache>, tempfile::TempPath) {
    let shell = tracking_shell(log);
    let mut cache =
        ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
    if let Some(home) = home {
        cache = cache.with_home(home);
    }
    (Arc::new(cache), shell)
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
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
    let (cache, _shell) = tracking_cache(log.path(), None);
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
#[test]
fn home_capture_skips_the_queue() {
    let log = tempfile::tempdir().expect("tempdir");
    let scratch = tempfile::tempdir().expect("tempdir");
    let home = dir_with_delay(scratch.path(), "home", "0");
    let (cache, _shell) = tracking_cache(log.path(), Some(&home));
    let busy = [
        dir_with_delay(scratch.path(), "wt-a", "3"),
        dir_with_delay(scratch.path(), "wt-b", "3"),
    ];
    let queued = dir_with_delay(scratch.path(), "wt-c", "0");

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
    let (cache, _shell) = tracking_cache(log.path(), Some(&home));

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
    let (cache, _shell) = tracking_cache(log.path(), None);

    // Two captures elsewhere hold both slots.
    let storm: Vec<_> = (0..CAPTURE_LIMIT).map(|_| cache.slots.acquire()).collect();

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
