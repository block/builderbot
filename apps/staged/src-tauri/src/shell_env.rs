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
use std::sync::{Arc, Mutex};
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
    InFlight(watch::Receiver<Option<Result<ShellEnv, String>>>),
}

/// Cache of [`ShellEnv`] snapshots keyed by working directory.
pub struct ShellEnvCache {
    inner: Mutex<HashMap<PathBuf, CachedEntry>>,
    ttl: Duration,
    shell: PathBuf,
}

/// Removes an `InFlight` entry from the cache if its capture future is
/// dropped (cancellation or panic) before publishing a `Ready` result.
///
/// Without this, a cancelled capture would leave a stale `InFlight(rx)` in
/// the map whose `tx` is gone; subsequent callers would clone the receiver,
/// wake immediately on `Err`, and spin the outer retry loop forever.
struct InFlightGuard<'a> {
    cache: &'a ShellEnvCache,
    key: &'a Path,
    promoted: bool,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if self.promoted {
            return;
        }
        let mut map = self.cache.inner.lock().unwrap();
        if matches!(map.get(self.key), Some(CachedEntry::InFlight(_))) {
            map.remove(self.key);
        }
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
        Self {
            inner: Mutex::new(HashMap::new()),
            ttl,
            shell,
        }
    }

    /// Return a fresh-or-recent snapshot for `working_dir`. Spawns a shell on
    /// miss/expiry; concurrent misses for the same dir share one capture.
    pub async fn get(&self, working_dir: &Path) -> io::Result<ShellEnv> {
        let key = working_dir.to_path_buf();

        loop {
            enum Action {
                Wait(watch::Receiver<Option<Result<ShellEnv, String>>>),
                Capture(watch::Sender<Option<Result<ShellEnv, String>>>),
            }

            let action = {
                let mut map = self.inner.lock().unwrap();
                match map.get(&key) {
                    Some(CachedEntry::Ready(env)) if env.captured_at.elapsed() < self.ttl => {
                        return Ok(env.clone());
                    }
                    Some(CachedEntry::InFlight(rx)) => Action::Wait(rx.clone()),
                    _ => {
                        let (tx, rx) = watch::channel(None);
                        map.insert(key.clone(), CachedEntry::InFlight(rx));
                        Action::Capture(tx)
                    }
                }
            };

            match action {
                Action::Wait(mut rx) => {
                    while rx.borrow().is_none() {
                        if rx.changed().await.is_err() {
                            // Sender dropped without delivering — re-check.
                            break;
                        }
                    }
                    if let Some(result) = rx.borrow().clone() {
                        return result.map_err(io::Error::other);
                    }
                    // Fall through to retry.
                }
                Action::Capture(tx) => {
                    // Declared after `tx` (a match binding) so it drops first
                    // on cancellation/panic: evict the InFlight entry before
                    // `tx` drops and signals waiters Err. Waiters then retry,
                    // find no entry, and become the next Capturer.
                    let mut guard = InFlightGuard {
                        cache: self,
                        key: &key,
                        promoted: false,
                    };
                    let outcome = capture_shell_env(&key, &self.shell).await;
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
                            let _ = tx.send(Some(Ok(env.clone())));
                            return Ok(env);
                        }
                        Err(err) => {
                            let msg = err.to_string();
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
    /// Does **not** coordinate with `InFlight` async captures — if a sync
    /// caller races an async caller for the same dir, both will capture
    /// independently and the second writer wins. Semantically safe (both
    /// captures produce equivalent env); only cost is duplicate shell-init
    /// work in that narrow first-call window.
    ///
    /// [`get`]: ShellEnvCache::get
    pub fn get_blocking(&self, working_dir: &Path) -> io::Result<ShellEnv> {
        let key = working_dir.to_path_buf();
        {
            let map = self.inner.lock().unwrap();
            if let Some(CachedEntry::Ready(env)) = map.get(&key) {
                if env.captured_at.elapsed() < self.ttl {
                    return Ok(env.clone());
                }
            }
        }

        let vars = capture_shell_env_blocking(&key, &self.shell)?;
        let env = ShellEnv {
            vars: Arc::new(vars),
            captured_at: Instant::now(),
        };
        self.inner
            .lock()
            .unwrap()
            .insert(key, CachedEntry::Ready(env.clone()));
        Ok(env)
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

/// Allocate a unique tempfile path for the env dump.
fn dump_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let pid = std::process::id();
    std::env::temp_dir().join(format!("staged-shell-env-{pid}-{nanos}"))
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

async fn capture_shell_env(working_dir: &Path, shell: &Path) -> io::Result<Vec<(String, String)>> {
    let dump_path = dump_path();
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
) -> io::Result<Vec<(String, String)>> {
    use std::io::Write as _;

    let dump_path = dump_path();
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
}
