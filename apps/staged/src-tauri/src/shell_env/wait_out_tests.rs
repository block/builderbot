//! [`ShellEnvCache::wait_out_and_invalidate`]: what a caller about to delete
//! a directory uses so no capture's shell is still running in it.

use super::*;
use crate::test_utils::{wait_for_file_sh, write_fake_shell};
use std::sync::mpsc;

/// Fake `$SHELL` that logs `started`, waits for `log/release`, runs the
/// capture's script, and logs `exited` as the last thing it does.
fn held_shell(log: &Path) -> tempfile::TempPath {
    let log = single_quote(&log.to_string_lossy());
    write_fake_shell(&format!(
        "#!/bin/sh\n\
         PATH=/usr/bin:/bin\n\
         export PATH\n\
         echo started >> {log}/events\n\
         {wait}\n\
         /bin/sh -s\n\
         echo exited >> {log}/events\n",
        wait = wait_for_file_sh(&format!("{log}/release")),
    ))
}

fn events(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log.join("events"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn wait_for_events(log: &Path, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while events(log).len() < count {
        assert!(Instant::now() < deadline, "{:?}", events(log));
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn cache_with(shell: &Path, home: &Path) -> Arc<ShellEnvCache> {
    Arc::new(
        ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600))
            .with_home(home),
    )
}

/// The shell of a capture in flight is running in the directory, so the
/// call returns only once it has exited, and leaves nothing cached. It
/// waits for that capture rather than starting one of its own.
#[test]
fn waits_for_the_capture_in_flight_then_drops_its_result() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let shell = held_shell(scratch.path());
    let cache = cache_with(&shell, scratch.path());
    let dir = tempfile::tempdir_in(scratch.path()).expect("tempdir");

    cache.prewarm(dir.path());
    wait_for_events(scratch.path(), 1);

    let (done, waited) = mpsc::channel();
    let (waiting, path) = (Arc::clone(&cache), dir.path().to_path_buf());
    let waiter = std::thread::spawn(move || {
        waiting.wait_out_and_invalidate(&path);
        let _ = done.send(());
    });
    assert!(
        waited.recv_timeout(Duration::from_millis(300)).is_err(),
        "returned while the shell was still running"
    );
    assert_eq!(events(scratch.path()), ["started"]);

    std::fs::write(scratch.path().join("release"), "").unwrap();
    waited
        .recv_timeout(Duration::from_secs(30))
        .expect("the capture finished");
    waiter.join().unwrap();

    assert_eq!(
        events(scratch.path()),
        ["started", "exited"],
        "one shell, gone by the time the wait ended"
    );
    assert!(
        cache.inner.lock().unwrap().get(dir.path()).is_none(),
        "the capture's result stayed cached"
    );
}

/// With nothing in flight there is nothing to wait for, and nothing is
/// started: a capture would put a shell in the directory about to go.
#[test]
fn starts_no_capture_where_none_is_in_flight() {
    let scratch = tempfile::tempdir().expect("tempdir");
    std::fs::write(scratch.path().join("release"), "").unwrap();
    let shell = held_shell(scratch.path());
    let cache = cache_with(&shell, scratch.path());
    let dir = tempfile::tempdir_in(scratch.path()).expect("tempdir");

    cache.wait_out_and_invalidate(dir.path());
    assert_eq!(events(scratch.path()), Vec::<String>::new());

    // A snapshot already cached is dropped, as `invalidate` drops it.
    cache.get_blocking(dir.path()).expect("capture");
    assert_eq!(events(scratch.path()), ["started", "exited"]);
    cache.wait_out_and_invalidate(dir.path());
    assert_eq!(events(scratch.path()), ["started", "exited"]);
    assert!(cache.inner.lock().unwrap().get(dir.path()).is_none());

    // And a directory that is already gone is no trouble.
    let gone = dir.path().join("gone");
    cache.wait_out_and_invalidate(&gone);
    assert_eq!(events(scratch.path()).len(), 2);
}
