//! Per-working-directory snapshot cache of an interactive login shell's
//! environment.
//!
//! Spawns one `$SHELL -ils` per working directory (so directory-based PATH
//! managers like Hermit activate during `chpwd`/`precmd`), captures the
//! resulting environment via `env -0` redirected to a tempfile, and caches
//! the result for a TTL. Subsequent callers apply the snapshot to a native
//! [`tokio::process::Command`] via [`ShellEnv::apply_to`] — paying ~zero
//! per-call cost and producing no shell-init banners on stdout.
//!
//! Concurrent first-callers for the same working directory are coalesced
//! through a `watch` channel so only one shell is spawned per (dir, miss).

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::watch;

/// Default cache TTL — long enough that pipeline runs amortise the capture
/// cost, short enough that edits to `~/.zshrc` or `bin/activate-hermit` are
/// picked up within an hour without an explicit invalidation.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60 * 60);

/// Captured environment from a single interactive login shell invocation.
#[derive(Clone, Debug)]
pub struct ShellEnv {
    vars: Arc<Vec<(String, String)>>,
    captured_at: Instant,
}

impl ShellEnv {
    /// Time when the snapshot was captured.
    pub fn captured_at(&self) -> Instant {
        self.captured_at
    }

    /// Captured `KEY=VALUE` pairs.
    pub fn vars(&self) -> &[(String, String)] {
        &self.vars
    }

    /// Clear `cmd`'s environment and replace it with the captured variables.
    ///
    /// Callers should set `current_dir`, args, and any per-call `extra_env`
    /// overrides *after* `apply_to` so they win.
    pub fn apply_to(&self, cmd: &mut Command) {
        cmd.env_clear();
        for (k, v) in self.vars.iter() {
            cmd.env(k, v);
        }
    }

    /// Std-process variant of [`apply_to`] for synchronous callers (notably
    /// `git/cli.rs::run`).
    pub fn apply_to_std(&self, cmd: &mut std::process::Command) {
        cmd.env_clear();
        for (k, v) in self.vars.iter() {
            cmd.env(k, v);
        }
    }
}

#[derive(Clone)]
enum CachedEntry {
    Ready(ShellEnv),
    InFlight(InFlightHandle),
}

/// Coalescing primitive used by both `get` (async) and `get_blocking` (sync)
/// callers waiting on the same in-flight capture. Async waiters subscribe to
/// the watch channel; sync waiters block on the condvar.
#[derive(Clone)]
struct InFlightHandle {
    promise: Arc<InFlightPromise>,
    async_rx: watch::Receiver<Option<Result<ShellEnv, String>>>,
}

struct InFlightPromise {
    state: Mutex<InFlightState>,
    cv: Condvar,
}

enum InFlightState {
    Pending,
    /// Capturer published a result (success or capture-time failure). Waiters
    /// take this clone and return it directly.
    Done(Result<ShellEnv, String>),
    /// Capturer dropped without publishing (panic/cancellation). Waiters fall
    /// through to retry the outer cache lookup.
    Cancelled,
}

impl InFlightPromise {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(InFlightState::Pending),
            cv: Condvar::new(),
        })
    }

    fn set_done(&self, result: Result<ShellEnv, String>) {
        {
            let mut g = self.state.lock().unwrap();
            *g = InFlightState::Done(result);
        }
        self.cv.notify_all();
    }

    fn set_cancelled(&self) {
        {
            let mut g = self.state.lock().unwrap();
            // No-op if a result was already published — preserves Done(Err).
            if matches!(*g, InFlightState::Pending) {
                *g = InFlightState::Cancelled;
            }
        }
        self.cv.notify_all();
    }

    /// Block the current thread until the promise transitions out of
    /// `Pending`. Returns `Some(result)` on `Done`, `None` on `Cancelled`.
    fn wait_blocking(&self) -> Option<Result<ShellEnv, String>> {
        let mut g = self.state.lock().unwrap();
        while matches!(*g, InFlightState::Pending) {
            g = self.cv.wait(g).unwrap();
        }
        match &*g {
            InFlightState::Done(r) => Some(r.clone()),
            InFlightState::Cancelled => None,
            InFlightState::Pending => unreachable!(),
        }
    }
}

/// Cache of [`ShellEnv`] snapshots keyed by working directory.
pub struct ShellEnvCache {
    inner: Mutex<HashMap<PathBuf, CachedEntry>>,
    ttl: Duration,
    shell: PathBuf,
    temp_root: PathBuf,
}

/// Removes an `InFlight` entry from the cache if its capture is dropped
/// (cancellation or panic) before publishing a result, and wakes any
/// waiters so they can retry rather than block forever.
///
/// Without this, a cancelled capture would leave a stale `InFlight` entry in
/// the map whose async sender is gone (causing async waiters to spin on a
/// dead receiver) and whose promise is stuck `Pending` (causing sync waiters
/// to park on the condvar forever).
struct InFlightGuard<'a> {
    cache: &'a ShellEnvCache,
    key: &'a Path,
    promise: Arc<InFlightPromise>,
    promoted: bool,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if self.promoted {
            return;
        }
        {
            let mut map = self.cache.inner.lock().unwrap();
            if matches!(map.get(self.key), Some(CachedEntry::InFlight(_))) {
                map.remove(self.key);
            }
        }
        // Wake sync waiters; async waiters wake when the capturer's `tx`
        // drops after this guard. `set_cancelled` is a no-op if a `Done`
        // result was already published (capture-time failure path).
        self.promise.set_cancelled();
    }
}

impl ShellEnvCache {
    /// Construct a cache with the default 1h TTL.
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_TTL)
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self::with_shell_and_ttl(resolve_shell(), ttl)
    }

    /// Construct a cache that spawns `shell` instead of `$SHELL`. Tests use this
    /// to point at a hermetic script; production code should use [`new`] or
    /// [`with_ttl`].
    ///
    /// [`new`]: ShellEnvCache::new
    /// [`with_ttl`]: ShellEnvCache::with_ttl
    pub fn with_shell_and_ttl(shell: PathBuf, ttl: Duration) -> Self {
        Self::with_shell_ttl_and_temp_root(shell, ttl, std::env::temp_dir())
    }

    /// Same as [`with_shell_and_ttl`] but also redirects the env-dump tempfile
    /// into `temp_root`. Tests pointing at their own per-test tempdir get
    /// deterministic leak checks that don't race other parallel captures
    /// sharing `std::env::temp_dir()`.
    ///
    /// [`with_shell_and_ttl`]: ShellEnvCache::with_shell_and_ttl
    pub fn with_shell_ttl_and_temp_root(shell: PathBuf, ttl: Duration, temp_root: PathBuf) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            ttl,
            shell,
            temp_root,
        }
    }

    /// Return a fresh-or-recent snapshot for `working_dir`. Spawns a shell on
    /// miss/expiry; concurrent misses for the same dir share one capture.
    pub async fn get(&self, working_dir: &Path) -> io::Result<ShellEnv> {
        let key = working_dir.to_path_buf();

        loop {
            enum Action {
                Wait(InFlightHandle),
                Capture {
                    promise: Arc<InFlightPromise>,
                    tx: watch::Sender<Option<Result<ShellEnv, String>>>,
                },
            }

            let action = {
                let mut map = self.inner.lock().unwrap();
                match map.get(&key) {
                    Some(CachedEntry::Ready(env)) if env.captured_at.elapsed() < self.ttl => {
                        return Ok(env.clone());
                    }
                    Some(CachedEntry::InFlight(handle)) => Action::Wait(handle.clone()),
                    _ => {
                        let promise = InFlightPromise::new();
                        let (tx, rx) = watch::channel(None);
                        map.insert(
                            key.clone(),
                            CachedEntry::InFlight(InFlightHandle {
                                promise: promise.clone(),
                                async_rx: rx,
                            }),
                        );
                        Action::Capture { promise, tx }
                    }
                }
            };

            match action {
                Action::Wait(handle) => {
                    let mut rx = handle.async_rx;
                    while rx.borrow().is_none() {
                        if rx.changed().await.is_err() {
                            // Sender dropped without delivering — re-check.
                            break;
                        }
                    }
                    let final_value = rx.borrow().clone();
                    if let Some(result) = final_value {
                        return result.map_err(io::Error::other);
                    }
                    // Fall through to retry.
                }
                Action::Capture { promise, tx } => {
                    // Declared after `tx`/`promise` (match bindings) so it drops
                    // first on cancellation/panic: evict the InFlight entry and
                    // wake sync waiters before `tx` drops and signals async
                    // waiters Err. Waiters then retry, find no entry, and
                    // become the next Capturer.
                    let mut guard = InFlightGuard {
                        cache: self,
                        key: &key,
                        promise: promise.clone(),
                        promoted: false,
                    };
                    let outcome = capture_shell_env(&key, &self.shell, &self.temp_root).await;
                    match outcome {
                        Ok(vars) => {
                            let env = ShellEnv {
                                vars: Arc::new(vars),
                                captured_at: Instant::now(),
                            };
                            self.inner
                                .lock()
                                .unwrap()
                                .insert(key.clone(), CachedEntry::Ready(env.clone()));
                            guard.promoted = true;
                            promise.set_done(Ok(env.clone()));
                            let _ = tx.send(Some(Ok(env.clone())));
                            return Ok(env);
                        }
                        Err(err) => {
                            let msg = err.to_string();
                            promise.set_done(Err(msg.clone()));
                            let _ = tx.send(Some(Err(msg)));
                            return Err(err);
                        }
                    }
                }
            }
        }
    }

    /// Synchronous variant of [`get`] for sync callers like `git/cli.rs::run`.
    ///
    /// Returns a `Ready` snapshot if one is present and fresh; otherwise
    /// spawns a *blocking* `$SHELL -ils` capture and stores the result.
    ///
    /// Coordinates with both sync and async in-flight captures via the shared
    /// `InFlight` entry: a sync caller arriving while another (sync or async)
    /// caller is capturing for the same dir parks on the promise condvar
    /// rather than spawning a redundant shell.
    ///
    /// [`get`]: ShellEnvCache::get
    pub fn get_blocking(&self, working_dir: &Path) -> io::Result<ShellEnv> {
        let key = working_dir.to_path_buf();

        loop {
            enum Action {
                Wait(Arc<InFlightPromise>),
                Capture {
                    promise: Arc<InFlightPromise>,
                    tx: watch::Sender<Option<Result<ShellEnv, String>>>,
                },
            }

            let action = {
                let mut map = self.inner.lock().unwrap();
                match map.get(&key) {
                    Some(CachedEntry::Ready(env)) if env.captured_at.elapsed() < self.ttl => {
                        return Ok(env.clone());
                    }
                    Some(CachedEntry::InFlight(handle)) => Action::Wait(handle.promise.clone()),
                    _ => {
                        let promise = InFlightPromise::new();
                        let (tx, rx) = watch::channel(None);
                        map.insert(
                            key.clone(),
                            CachedEntry::InFlight(InFlightHandle {
                                promise: promise.clone(),
                                async_rx: rx,
                            }),
                        );
                        Action::Capture { promise, tx }
                    }
                }
            };

            match action {
                Action::Wait(promise) => match promise.wait_blocking() {
                    Some(Ok(env)) => return Ok(env),
                    Some(Err(msg)) => return Err(io::Error::other(msg)),
                    None => continue, // Capturer cancelled — retry.
                },
                Action::Capture { promise, tx } => {
                    let mut guard = InFlightGuard {
                        cache: self,
                        key: &key,
                        promise: promise.clone(),
                        promoted: false,
                    };
                    let outcome = capture_shell_env_blocking(&key, &self.shell, &self.temp_root);
                    match outcome {
                        Ok(vars) => {
                            let env = ShellEnv {
                                vars: Arc::new(vars),
                                captured_at: Instant::now(),
                            };
                            self.inner
                                .lock()
                                .unwrap()
                                .insert(key.clone(), CachedEntry::Ready(env.clone()));
                            guard.promoted = true;
                            promise.set_done(Ok(env.clone()));
                            let _ = tx.send(Some(Ok(env.clone())));
                            return Ok(env);
                        }
                        Err(err) => {
                            let msg = err.to_string();
                            promise.set_done(Err(msg.clone()));
                            let _ = tx.send(Some(Err(msg)));
                            return Err(err);
                        }
                    }
                }
            }
        }
    }

    /// Drop the cached snapshot for `working_dir` (next `get` will recapture).
    pub fn invalidate(&self, working_dir: &Path) {
        self.inner.lock().unwrap().remove(working_dir);
    }

    /// Drop all cached snapshots.
    pub fn invalidate_all(&self) {
        self.inner.lock().unwrap().clear();
    }
}

impl Default for ShellEnvCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the shell binary the cache should spawn. Reads `$SHELL`, falling
/// back to `/bin/bash` — matching the canonical interactive-login-shell
/// wrapper used elsewhere in the codebase.
fn resolve_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/bin/bash"))
}

/// Allocate a unique tempfile path for the env dump inside `temp_root`.
fn dump_path(temp_root: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let pid = std::process::id();
    temp_root.join(format!("staged-shell-env-{pid}-{nanos}"))
}

/// Build the shell script that dumps the interactive-login env NUL-delimited
/// to `dump_path` and exits. The tempfile path is single-quoted so values
/// with newlines round-trip and shell-init banners on stdout are ignored.
fn dump_script(dump_path: &Path) -> String {
    let dump_path_str = dump_path.to_string_lossy();
    format!(
        "env -0 > {} 2>/dev/null\nexit\n",
        single_quote(&dump_path_str)
    )
}

fn parse_env_dump(bytes: &[u8]) -> Vec<(String, String)> {
    let mut vars = Vec::new();
    for chunk in bytes.split(|&b| b == 0) {
        if chunk.is_empty() {
            continue;
        }
        let Ok(s) = std::str::from_utf8(chunk) else {
            continue;
        };
        if let Some(eq_pos) = s.find('=') {
            vars.push((s[..eq_pos].to_string(), s[eq_pos + 1..].to_string()));
        }
    }
    vars
}

async fn capture_shell_env(
    working_dir: &Path,
    shell: &Path,
    temp_root: &Path,
) -> io::Result<Vec<(String, String)>> {
    let dump_path = dump_path(temp_root);
    let script = dump_script(&dump_path);

    let mut cmd = Command::new(shell);
    cmd.current_dir(working_dir)
        .env_clear()
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env("USER", std::env::var("USER").unwrap_or_default())
        .env("SHELL", shell)
        .arg("-i")
        .arg("-l")
        .arg("-s")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = cmd.spawn()?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Failed to open shell stdin for env capture"))?;
        stdin.write_all(script.as_bytes()).await?;
        stdin.flush().await?;
        // Drop stdin → shell sees EOF after `exit` and terminates cleanly.
    }

    let status = child.wait().await?;
    if !status.success() {
        let _ = tokio::fs::remove_file(&dump_path).await;
        return Err(io::Error::other(format!(
            "Shell env capture exited with status {status}"
        )));
    }

    let bytes = match tokio::fs::read(&dump_path).await {
        Ok(b) => b,
        Err(e) => {
            let _ = tokio::fs::remove_file(&dump_path).await;
            return Err(e);
        }
    };
    let _ = tokio::fs::remove_file(&dump_path).await;

    Ok(parse_env_dump(&bytes))
}

/// Synchronous counterpart to [`capture_shell_env`].
///
/// Blocks the current thread on the shell's startup. Suitable for sync
/// callers (`git/cli.rs::run`); async callers should use [`capture_shell_env`].
fn capture_shell_env_blocking(
    working_dir: &Path,
    shell: &Path,
    temp_root: &Path,
) -> io::Result<Vec<(String, String)>> {
    use std::io::Write as _;

    let dump_path = dump_path(temp_root);
    let script = dump_script(&dump_path);

    let mut cmd = std::process::Command::new(shell);
    cmd.current_dir(working_dir)
        .env_clear()
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env("USER", std::env::var("USER").unwrap_or_default())
        .env("SHELL", shell)
        .arg("-i")
        .arg("-l")
        .arg("-s")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = cmd.spawn()?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Failed to open shell stdin for env capture"))?;
        stdin.write_all(script.as_bytes())?;
        stdin.flush()?;
        // Drop stdin → shell sees EOF after `exit` and terminates cleanly.
    }

    let status = child.wait()?;
    if !status.success() {
        let _ = std::fs::remove_file(&dump_path);
        return Err(io::Error::other(format!(
            "Shell env capture exited with status {status}"
        )));
    }

    let bytes = match std::fs::read(&dump_path) {
        Ok(b) => b,
        Err(e) => {
            let _ = std::fs::remove_file(&dump_path);
            return Err(e);
        }
    };
    let _ = std::fs::remove_file(&dump_path);

    Ok(parse_env_dump(&bytes))
}

fn single_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------------

    /// Write `content` to a 0755 tempfile suitable for use as `$SHELL`.
    ///
    /// Hold the returned handle for the test's lifetime — dropping it removes
    /// the file. Returns a `TempPath` (not `NamedTempFile`) so the writable fd
    /// is closed before the script gets exec'd: Linux's exec path refuses to
    /// load a binary whose inode has `i_writecount > 0` and returns ETXTBSY.
    /// macOS has no equivalent check, which is why the prior `NamedTempFile`
    /// shape passed locally on Darwin but failed on Linux CI. Unix-only
    /// because we set the executable mode.
    #[cfg(unix)]
    fn write_fake_shell(content: &str) -> tempfile::TempPath {
        use std::os::unix::fs::PermissionsExt;
        let temp_path = tempfile::Builder::new()
            .prefix("staged-fake-shell-")
            .suffix(".sh")
            .tempfile()
            .expect("create fake shell tempfile")
            .into_temp_path();
        std::fs::write(&temp_path, content).expect("write script");
        let mut perms = std::fs::metadata(&temp_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temp_path, perms).expect("chmod 755");
        temp_path
    }

    /// Collect any `staged-shell-env-*` files left in `temp_root`. Tempfile
    /// cleanup tests point the cache at a per-test tempdir and assert this is
    /// empty after a capture — no need to set-difference against the shared
    /// `std::env::temp_dir()`, where parallel tests' in-flight captures would
    /// race the assertion.
    fn orphan_paths_in(temp_root: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(temp_root)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with("staged-shell-env-")
                    })
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default()
    }

    // ---------------------------------------------------------------------
    // Existing tests (preserved)
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn captures_path_for_a_real_dir() {
        let cache = ShellEnvCache::new();
        let env = cache
            .get(&std::env::temp_dir())
            .await
            .expect("snapshot should succeed");
        assert!(
            env.vars().iter().any(|(k, _)| k == "PATH"),
            "captured env should contain PATH"
        );
    }

    #[tokio::test]
    async fn returns_cached_value_within_ttl() {
        let cache = ShellEnvCache::new();
        let dir = std::env::temp_dir();
        let first = cache.get(&dir).await.expect("first capture");
        let second = cache.get(&dir).await.expect("second capture");
        // Same Arc target → cache hit (the second call should not recapture).
        assert_eq!(first.captured_at(), second.captured_at());
    }

    #[tokio::test]
    async fn invalidate_forces_recapture() {
        let cache = ShellEnvCache::new();
        let dir = std::env::temp_dir();
        let first = cache.get(&dir).await.expect("first capture");
        cache.invalidate(&dir);
        let second = cache.get(&dir).await.expect("second capture");
        assert!(second.captured_at() >= first.captured_at());
        assert_ne!(first.captured_at(), second.captured_at());
    }

    #[tokio::test]
    async fn apply_to_replaces_env() {
        let cache = ShellEnvCache::new();
        let env = cache
            .get(&std::env::temp_dir())
            .await
            .expect("snapshot should succeed");
        let mut cmd = Command::new("/usr/bin/env");
        env.apply_to(&mut cmd);
        // Sanity: env_clear was called, so apply_to fully owns the resulting env.
        // We can't directly observe env on tokio::process::Command, but we can
        // run it and confirm the output contains a captured variable.
        let output = cmd.output().await.expect("env should run");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("PATH="));
    }

    #[test]
    fn single_quote_escapes_quotes() {
        assert_eq!(single_quote("plain"), "'plain'");
        assert_eq!(single_quote("with 'quote'"), "'with '\\''quote'\\'''");
    }

    #[test]
    fn get_blocking_captures_and_caches() {
        let cache = ShellEnvCache::new();
        let dir = std::env::temp_dir();

        let first = cache
            .get_blocking(&dir)
            .expect("blocking snapshot should succeed");
        assert!(
            first.vars().iter().any(|(k, _)| k == "PATH"),
            "captured env should contain PATH"
        );

        let second = cache
            .get_blocking(&dir)
            .expect("second blocking snapshot should hit cache");
        assert_eq!(
            first.captured_at(),
            second.captured_at(),
            "second blocking call should return cached snapshot"
        );
    }

    #[tokio::test]
    async fn get_blocking_sees_snapshots_from_async_get() {
        let cache = Arc::new(ShellEnvCache::new());
        let dir = std::env::temp_dir();

        let async_env = cache.get(&dir).await.expect("async capture should succeed");
        let sync_env = cache
            .get_blocking(&dir)
            .expect("blocking call should hit cache populated by async path");
        assert_eq!(
            async_env.captured_at(),
            sync_env.captured_at(),
            "sync caller should observe the async-populated snapshot"
        );
    }

    #[tokio::test]
    async fn cancelled_capture_evicts_stale_inflight() {
        let cache = Arc::new(ShellEnvCache::new());
        let dir = std::env::temp_dir();

        // Drive a Capturer just long enough to insert InFlight, then abort it.
        // On a current_thread runtime, yield_now hands control to the spawned
        // task; it inserts InFlight before its first internal `.await`, then
        // parks somewhere inside `capture_shell_env`.
        let first = tokio::spawn({
            let cache = cache.clone();
            let dir = dir.clone();
            async move { cache.get(&dir).await }
        });
        tokio::task::yield_now().await;
        first.abort();
        let _ = first.await;

        // The guard's Drop must have evicted the InFlight entry — otherwise
        // subsequent callers would clone a dead receiver and spin.
        {
            let map = cache.inner.lock().unwrap();
            assert!(
                !matches!(map.get(&dir), Some(CachedEntry::InFlight(_))),
                "InFlight entry leaked after capture future was cancelled"
            );
        }

        // And a subsequent caller must complete (not spin) within a sane bound.
        let second = tokio::time::timeout(Duration::from_secs(30), cache.get(&dir))
            .await
            .expect("second caller must not spin");
        assert!(second.is_ok());
    }

    // ---------------------------------------------------------------------
    // Wave 1: Pure unit tests (no fake-shell, no concurrency)
    // ---------------------------------------------------------------------

    /// A1: A `Ready` entry whose `captured_at` is older than `ttl` triggers a
    /// fresh capture instead of being returned. Exercises the stale-`Ready`
    /// branch the in-TTL test cannot reach.
    #[tokio::test]
    async fn ttl_expiry_triggers_recapture() {
        let cache = ShellEnvCache::with_ttl(Duration::from_millis(50));
        let dir = std::env::temp_dir();
        let first = cache.get(&dir).await.expect("first capture");
        tokio::time::sleep(Duration::from_millis(75)).await;
        let second = cache.get(&dir).await.expect("recapture after TTL");
        assert_ne!(
            first.captured_at(),
            second.captured_at(),
            "TTL expiry must force a fresh capture"
        );
        assert!(second.captured_at() > first.captured_at());
    }

    /// A2: `invalidate_all` empties the internal map and forces both dirs to
    /// recapture on the next `get`.
    #[tokio::test]
    async fn invalidate_all_clears_every_entry() {
        let cache = ShellEnvCache::new();
        let dir_a = std::env::temp_dir();
        let dir_b = tempfile::tempdir().expect("tempdir b");

        let a_first = cache.get(&dir_a).await.expect("capture a");
        let b_first = cache.get(dir_b.path()).await.expect("capture b");

        cache.invalidate_all();
        assert!(
            cache.inner.lock().unwrap().is_empty(),
            "invalidate_all should empty the map"
        );

        let a_second = cache.get(&dir_a).await.expect("recapture a");
        let b_second = cache.get(dir_b.path()).await.expect("recapture b");
        assert_ne!(a_first.captured_at(), a_second.captured_at());
        assert_ne!(b_first.captured_at(), b_second.captured_at());
    }

    /// A3: Two distinct working dirs are cached independently — each second
    /// `get` hits its own `Ready` entry without recapture.
    #[tokio::test]
    async fn different_dirs_cached_independently() {
        let cache = ShellEnvCache::new();
        let dir_a = std::env::temp_dir();
        let dir_b = tempfile::tempdir().expect("tempdir b");

        let a_first = cache.get(&dir_a).await.expect("capture a");
        let b_first = cache.get(dir_b.path()).await.expect("capture b");
        // Per-dir cache hits.
        let a_second = cache.get(&dir_a).await.expect("a hit");
        let b_second = cache.get(dir_b.path()).await.expect("b hit");
        assert_eq!(a_first.captured_at(), a_second.captured_at());
        assert_eq!(b_first.captured_at(), b_second.captured_at());
        // Distinct captures across dirs.
        assert_ne!(a_first.captured_at(), b_first.captured_at());
    }

    /// A4: `invalidate(A)` must not disturb dir B's cached snapshot.
    #[tokio::test]
    async fn invalidate_only_affects_target_dir() {
        let cache = ShellEnvCache::new();
        let dir_a = std::env::temp_dir();
        let dir_b = tempfile::tempdir().expect("tempdir b");
        let a_first = cache.get(&dir_a).await.expect("capture a");
        let b_first = cache.get(dir_b.path()).await.expect("capture b");

        cache.invalidate(&dir_a);

        let a_second = cache.get(&dir_a).await.expect("recapture a");
        let b_second = cache
            .get(dir_b.path())
            .await
            .expect("b should still be cached");

        assert_ne!(
            a_first.captured_at(),
            a_second.captured_at(),
            "A should have been recaptured"
        );
        assert_eq!(
            b_first.captured_at(),
            b_second.captured_at(),
            "B's snapshot must be unaffected by invalidating A"
        );
    }

    /// D13: `apply_to` must call `env_clear` — i.e. any env vars set on the
    /// `Command` *before* `apply_to` must NOT leak into the child.
    #[tokio::test]
    async fn apply_to_clears_existing_env() {
        let cache = ShellEnvCache::new();
        let env = cache
            .get(&std::env::temp_dir())
            .await
            .expect("snapshot should succeed");
        let mut cmd = Command::new("/usr/bin/env");
        cmd.env("PIPELINE_TEST_SHOULD_NOT_LEAK", "1");
        env.apply_to(&mut cmd);
        let output = cmd.output().await.expect("env should run");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains("PIPELINE_TEST_SHOULD_NOT_LEAK"),
            "apply_to must clear pre-existing env vars (env_clear): stdout = {stdout}"
        );
    }

    /// D14: Env vars set *after* `apply_to` win — locks in the docstring
    /// contract so per-call `extra_env` overrides keep working.
    #[tokio::test]
    async fn apply_to_then_env_lets_caller_override() {
        let cache = ShellEnvCache::new();
        let env = cache
            .get(&std::env::temp_dir())
            .await
            .expect("snapshot should succeed");
        let mut cmd = Command::new("/usr/bin/env");
        env.apply_to(&mut cmd);
        cmd.env("PATH", "/sentinel");
        let output = cmd.output().await.expect("env should run");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("PATH=/sentinel"),
            "post-apply_to env override must win: stdout = {stdout}"
        );
    }

    /// F17: Empty input → empty single-quote pair.
    #[test]
    fn single_quote_empty_input() {
        assert_eq!(single_quote(""), "''");
    }

    /// F18: Backslashes pass through unchanged — sh single-quotes don't
    /// interpret backslashes. Locks in current behavior against a future
    /// "harden the escape" PR that might over-escape.
    #[test]
    fn single_quote_backslash_unchanged() {
        assert_eq!(single_quote(r"a\b"), r"'a\b'");
    }

    // ---------------------------------------------------------------------
    // Wave 2: Single-capture mechanics (no concurrency, no fake shell)
    // ---------------------------------------------------------------------

    /// C11: The `current_dir` is honoured by the spawned shell — `PWD` in the
    /// captured env matches the directory we asked to capture in.
    #[tokio::test]
    async fn working_dir_argument_is_honoured() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Resolve symlinks so the `PWD` the shell reports lines up with what
        // we expect (macOS's TMPDIR is under a symlink-laden path).
        let resolved = std::fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_owned());
        let cache = ShellEnvCache::new();
        let env = cache.get(&resolved).await.expect("capture should succeed");
        let pwd = env
            .vars()
            .iter()
            .find(|(k, _)| k == "PWD")
            .map(|(_, v)| v.clone())
            .expect("captured env should include PWD");
        let pwd_path =
            std::fs::canonicalize(PathBuf::from(&pwd)).unwrap_or_else(|_| PathBuf::from(&pwd));
        assert_eq!(
            pwd_path, resolved,
            "PWD should match the working_dir argument"
        );
    }

    /// C12: `HOME` and `USER` from the parent process are passed through into
    /// the captured snapshot — guards against `env_clear` accidentally
    /// dropping the whitelist.
    #[tokio::test]
    async fn home_and_user_passed_through() {
        let cache = ShellEnvCache::new();
        let env = cache
            .get(&std::env::temp_dir())
            .await
            .expect("snapshot should succeed");
        let find = |k| {
            env.vars()
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
        };

        if let Ok(parent_home) = std::env::var("HOME") {
            if !parent_home.is_empty() {
                let captured = find("HOME").unwrap_or_default();
                assert_eq!(
                    captured, parent_home,
                    "HOME must round-trip into the snapshot"
                );
            }
        }
        if let Ok(parent_user) = std::env::var("USER") {
            if !parent_user.is_empty() {
                let captured = find("USER").unwrap_or_default();
                assert_eq!(
                    captured, parent_user,
                    "USER must round-trip into the snapshot"
                );
            }
        }
    }

    /// E15: A successful capture promotes the entry to `Ready` and leaves it
    /// in the map (not removed, not still `InFlight`). Locks in the
    /// `promoted = true` semantics.
    #[tokio::test]
    async fn promoted_guard_keeps_ready_entry() {
        let cache = ShellEnvCache::new();
        let dir = std::env::temp_dir();
        let _ = cache.get(&dir).await.expect("capture");
        let map = cache.inner.lock().unwrap();
        match map.get(&dir) {
            Some(CachedEntry::Ready(_)) => {}
            other => panic!(
                "expected Ready after successful capture, got {:?}",
                other.is_some()
            ),
        }
    }

    // ---------------------------------------------------------------------
    // Wave 3: Concurrency + failure modes (needs fake-shell seam)
    // ---------------------------------------------------------------------

    /// B5: Concurrent first-callers coalesce through the watch channel — only
    /// one shell is spawned per (dir, miss) and every caller sees the same
    /// `captured_at`.
    ///
    /// The fake shell's leading `sleep 0.5` guarantees all 8 callers arrive
    /// during `InFlight(rx)` rather than racing a sub-ms real-`$SHELL` capture
    /// (which would still pass the `captured_at` assertion via the cache-hit
    /// path, without proving watch-channel coalescing actually fired).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_first_callers_share_one_capture() {
        let shell = write_fake_shell(
            "#!/bin/sh\nsleep 0.5\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n",
        );
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let cache = cache.clone();
            let dir = dir_path.clone();
            handles.push(tokio::spawn(async move { cache.get(&dir).await }));
        }
        let mut captured_ats = Vec::new();
        for h in handles {
            let env = h.await.expect("task").expect("capture");
            captured_ats.push(env.captured_at());
        }
        let first = captured_ats[0];
        assert!(
            captured_ats.iter().all(|c| *c == first),
            "all coalesced callers should observe the same captured_at: {:?}",
            captured_ats
        );
    }

    /// B6: When the active Capturer is aborted, a parallel Waiter must wake
    /// (either becoming the next Capturer and succeeding, or seeing a
    /// recapture-then-Ok) within a bounded timeout — no spin-loop.
    ///
    /// Uses the fake-shell seam so the post-cancellation recapture is fast
    /// and deterministic. The 3s budget is tight enough to catch
    /// slower-but-not-infinite regressions (~600ms is the expected path:
    /// abort → guard evicts InFlight → waiter retries → new capture's
    /// `sleep 0.5` + parse), not just the hard hang from the original
    /// cancellation bug.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn waiter_wakes_when_capturer_cancels() {
        let shell = write_fake_shell(
            "#!/bin/sh\nsleep 0.5\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n",
        );
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        // First task becomes the Capturer; it inserts InFlight before its
        // first internal `.await` inside `capture_shell_env`.
        let capturer = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { cache.get(&dir).await }
        });
        // Give the capturer a chance to insert InFlight.
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Second task should arrive as a Waiter on the same InFlight rx.
        let waiter = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { tokio::time::timeout(Duration::from_secs(3), cache.get(&dir)).await }
        });
        tokio::task::yield_now().await;

        capturer.abort();
        let _ = capturer.await;

        // Waiter must complete within a bounded time (not spin), and ultimately
        // succeed because the cancellation evicts InFlight so it can recapture.
        let waiter_result = waiter.await.expect("waiter task");
        let inner = waiter_result.expect("waiter must not spin past timeout");
        assert!(
            inner.is_ok(),
            "waiter should ultimately succeed after capturer cancellation: {inner:?}"
        );
    }

    /// B7: A failing capture propagates `Err` to all concurrent waiters AND is
    /// not negative-cached — a subsequent `get` must launch a fresh capture.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn capture_err_propagates_and_is_not_cached() {
        let shell = write_fake_shell("#!/bin/sh\nexit 1\n");
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let h1 = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { cache.get(&dir).await }
        });
        let h2 = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { cache.get(&dir).await }
        });
        let (r1, r2) = (h1.await.expect("h1"), h2.await.expect("h2"));
        assert!(r1.is_err(), "concurrent caller 1 should see Err: {r1:?}");
        assert!(r2.is_err(), "concurrent caller 2 should see Err: {r2:?}");

        // No negative caching — failures must not persist.
        {
            let map = cache.inner.lock().unwrap();
            assert!(
                map.get(&dir_path).is_none(),
                "failed capture must not leave an entry in the cache"
            );
        }

        // A third caller spawns a fresh capture (which also fails).
        let r3 = cache.get(&dir_path).await;
        assert!(r3.is_err(), "third caller should still see Err: {r3:?}");
    }

    /// C8: Tempfile is cleaned up after both successful and failing captures.
    /// Points the cache at a per-test tempdir so the leak check sees only this
    /// test's captures — parallel tests dumping into `std::env::temp_dir()`
    /// can't perturb the assertion.
    #[cfg(unix)]
    #[tokio::test]
    async fn tempfile_cleaned_up_on_success() {
        let shell =
            write_fake_shell("#!/bin/sh\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n");
        let temp_root = tempfile::tempdir().expect("temp_root");
        let cache = ShellEnvCache::with_shell_ttl_and_temp_root(
            shell.to_path_buf(),
            Duration::from_secs(3600),
            temp_root.path().to_path_buf(),
        );
        let dir = tempfile::tempdir().expect("tempdir");

        let env = cache.get(dir.path()).await.expect("capture");
        assert!(
            env.vars().iter().any(|(k, _)| k == "PATH"),
            "fake shell should set PATH"
        );
        let leftover = orphan_paths_in(temp_root.path());
        assert!(
            leftover.is_empty(),
            "successful capture must not leak tempfiles: {leftover:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tempfile_cleaned_up_on_failure() {
        let shell = write_fake_shell("#!/bin/sh\nexit 1\n");
        let temp_root = tempfile::tempdir().expect("temp_root");
        let cache = ShellEnvCache::with_shell_ttl_and_temp_root(
            shell.to_path_buf(),
            Duration::from_secs(3600),
            temp_root.path().to_path_buf(),
        );
        let dir = tempfile::tempdir().expect("tempdir");

        let result = cache.get(dir.path()).await;
        assert!(result.is_err(), "failing fake shell should produce Err");
        let leftover = orphan_paths_in(temp_root.path());
        assert!(
            leftover.is_empty(),
            "failed capture must not leak tempfiles: {leftover:?}"
        );
    }

    /// B8: Concurrent sync `get_blocking` callers coalesce through the
    /// promise condvar — only one shell is spawned per (dir, miss) and every
    /// caller sees the same `captured_at`.
    ///
    /// Mirror of B5 for the sync path. The fake shell's leading `sleep 0.5`
    /// guarantees siblings arrive during `InFlight` rather than racing a
    /// sub-ms real-`$SHELL` capture.
    #[cfg(unix)]
    #[test]
    fn concurrent_sync_first_callers_share_one_capture() {
        let shell = write_fake_shell(
            "#!/bin/sh\nsleep 0.5\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n",
        );
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let cache = cache.clone();
            let dir = dir_path.clone();
            handles.push(std::thread::spawn(move || cache.get_blocking(&dir)));
        }
        let mut captured_ats = Vec::new();
        for h in handles {
            let env = h.join().expect("thread").expect("capture");
            captured_ats.push(env.captured_at());
        }
        let first = captured_ats[0];
        assert!(
            captured_ats.iter().all(|c| *c == first),
            "all coalesced sync callers should observe the same captured_at: {:?}",
            captured_ats
        );
    }

    /// B9: A sync `get_blocking` arriving while an async `get` is in flight
    /// for the same dir coalesces on the promise instead of spawning its own
    /// shell. Closes the race that was causing the four-concurrent
    /// `compute_branch_git_state` git invocations to each fire a separate
    /// `$SHELL -ils` on first project visit.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sync_caller_coalesces_with_in_flight_async() {
        let shell = write_fake_shell(
            "#!/bin/sh\nsleep 0.5\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n",
        );
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let async_handle = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { cache.get(&dir).await }
        });
        // Give the async capturer time to insert `InFlight`.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let sync_handle = tokio::task::spawn_blocking({
            let cache = cache.clone();
            let dir = dir_path.clone();
            move || cache.get_blocking(&dir)
        });

        let async_env = async_handle.await.expect("async task").expect("async");
        let sync_env = sync_handle.await.expect("sync task").expect("sync");
        assert_eq!(
            async_env.captured_at(),
            sync_env.captured_at(),
            "sync caller must coalesce with in-flight async capture"
        );
    }

    /// B10: An async `get` arriving while a sync `get_blocking` is in flight
    /// for the same dir coalesces on the promise instead of spawning its own
    /// shell.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn async_caller_coalesces_with_in_flight_sync() {
        let shell = write_fake_shell(
            "#!/bin/sh\nsleep 0.5\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n",
        );
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let sync_handle = tokio::task::spawn_blocking({
            let cache = cache.clone();
            let dir = dir_path.clone();
            move || cache.get_blocking(&dir)
        });
        // Give the sync capturer time to insert `InFlight`.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let async_handle = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { cache.get(&dir).await }
        });

        let sync_env = sync_handle.await.expect("sync task").expect("sync");
        let async_env = async_handle.await.expect("async task").expect("async");
        assert_eq!(
            sync_env.captured_at(),
            async_env.captured_at(),
            "async caller must coalesce with in-flight sync capture"
        );
    }

    /// B11: A sync `get_blocking` caller waiting on the promise wakes and
    /// retries (rather than blocking forever) when the async capturer is
    /// cancelled mid-capture.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sync_waiter_wakes_when_async_capturer_cancels() {
        let shell = write_fake_shell(
            "#!/bin/sh\nsleep 0.5\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n",
        );
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let capturer = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { cache.get(&dir).await }
        });
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let waiter = tokio::task::spawn_blocking({
            let cache = cache.clone();
            let dir = dir_path.clone();
            move || cache.get_blocking(&dir)
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        capturer.abort();
        let _ = capturer.await;

        let result = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("sync waiter must not park forever")
            .expect("waiter task")
            .expect("recapture must ultimately succeed");
        assert!(
            result.vars().iter().any(|(k, _)| k == "PATH"),
            "post-cancellation recapture should produce a valid snapshot"
        );
    }

    /// C9: Values containing newlines round-trip through the NUL-delimited
    /// `env -0` dump — locks in the choice to use NUL framing instead of
    /// line-based parsing.
    #[cfg(unix)]
    #[tokio::test]
    async fn values_with_newlines_round_trip() {
        let shell = write_fake_shell(
            "#!/bin/sh\nPATH=/usr/bin:/bin\nWEIRD='line1\nline2'\nexport PATH WEIRD\nexec /bin/sh -s\n",
        );
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
        let dir = tempfile::tempdir().expect("tempdir");

        let env = cache.get(dir.path()).await.expect("capture");
        let weird = env
            .vars()
            .iter()
            .find(|(k, _)| k == "WEIRD")
            .map(|(_, v)| v.clone())
            .expect("WEIRD must round-trip into the snapshot");
        assert_eq!(weird, "line1\nline2", "newlines must round-trip verbatim");
    }
}
