//! Snapshot cache of an interactive login shell's environment, one per
//! environment root.
//!
//! Spawns one `$SHELL -ils` per environment root (so directory-based PATH
//! managers like Hermit activate during `chpwd`/`precmd`), captures the
//! resulting environment via `env -0` redirected to a tempfile, and caches
//! the result for a TTL. Subsequent callers apply the snapshot to a native
//! [`tokio::process::Command`] via [`ShellEnv::apply_to`] — paying ~zero
//! per-call cost and producing no shell-init banners on stdout.
//!
//! A working directory's environment root is its nearest Hermit root or git
//! top level, unless a directory-scoped env tool's file sits in between (see
//! [`ShellEnvCache::env_key`]). Subdirectories of one worktree share its
//! snapshot, with `PWD` set to the caller's directory.
//!
//! Concurrent first-callers for the same root are coalesced through a
//! `watch` channel so only one shell is spawned per (root, miss).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::watch;

mod env_root;

use env_root::resolve_env_root;

/// Default cache TTL — long enough that pipeline runs amortise the capture
/// cost, short enough that edits to `~/.zshrc` or `bin/activate-hermit` are
/// picked up within an hour without an explicit invalidation.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60 * 60);
const HOME_ENV_CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// Captured environment from a single interactive login shell invocation,
/// as seen from the directory it was requested for.
///
/// The snapshot is captured at the directory's environment root and shared,
/// untouched, by every directory that resolves to that root. When the
/// requested directory isn't the root itself, reading or applying the
/// variables sets `PWD` to the requested directory and drops `OLDPWD`, so a
/// child doesn't inherit the root's `PWD` and the `cd` history of its shell.
#[derive(Clone, Debug)]
pub struct ShellEnv {
    vars: Arc<Vec<(String, String)>>,
    captured_at: Instant,
    /// `PWD` for a directory beneath the environment root; `None` when the
    /// snapshot is used in the directory it was captured in.
    pwd: Option<Arc<str>>,
}

impl ShellEnv {
    fn captured(vars: Vec<(String, String)>) -> Self {
        Self {
            vars: Arc::new(vars),
            captured_at: Instant::now(),
            pwd: None,
        }
    }

    /// This snapshot, captured at `env_root`, as seen from `dir`.
    fn seen_from(mut self, env_root: &Path, dir: &Path) -> Self {
        if dir != env_root {
            self.pwd = Some(dir.to_string_lossy().into());
        }
        self
    }

    /// Time when the snapshot was captured.
    pub fn captured_at(&self) -> Instant {
        self.captured_at
    }

    /// Captured `KEY=VALUE` pairs, with `PWD` set for the requested
    /// directory. Borrows the shared snapshot when it was requested for the
    /// directory it was captured in.
    pub fn vars(&self) -> Cow<'_, [(String, String)]> {
        if self.pwd.is_none() {
            return Cow::Borrowed(self.vars.as_slice());
        }
        Cow::Owned(
            self.iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
        )
    }

    fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        let pwd = self.pwd.as_deref();
        self.vars
            .iter()
            .filter(move |(k, _)| pwd.is_none() || !matches!(k.as_str(), "PWD" | "OLDPWD"))
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(pwd.map(|pwd| ("PWD", pwd)))
    }

    /// Clear `cmd`'s environment and replace it with the captured variables.
    ///
    /// Callers should set `current_dir`, args, and any per-call `extra_env`
    /// overrides *after* `apply_to` so they win.
    pub fn apply_to(&self, cmd: &mut Command) {
        cmd.env_clear();
        for (k, v) in self.iter() {
            cmd.env(k, v);
        }
    }

    /// Std-process variant of [`apply_to`] for synchronous callers (notably
    /// `git/cli.rs::run`).
    pub fn apply_to_std(&self, cmd: &mut std::process::Command) {
        cmd.env_clear();
        for (k, v) in self.iter() {
            cmd.env(k, v);
        }
    }
}

/// Capture the user's home/global interactive login environment and return a
/// deterministic snapshot suitable for subprocesses that clear their env.
///
/// This sanitizes only the returned clone. The underlying cache remains raw so
/// project/git callers keep directory-scoped tool-manager state for their
/// environment root. `$HOME` is always its own root.
pub async fn home_env_vars_with_extended_path(cache: &ShellEnvCache) -> Vec<(String, String)> {
    let shell_env = capture_home_interactive_env(cache).await;
    env_vars_with_extended_path(&shell_env)
}

pub async fn capture_home_interactive_env(cache: &ShellEnvCache) -> HashMap<String, String> {
    let Some(home) = home_dir_from_env() else {
        return HashMap::new();
    };
    capture_home_interactive_env_for_dir(cache, &home, HOME_ENV_CAPTURE_TIMEOUT).await
}

fn home_dir_from_env() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

async fn capture_home_interactive_env_for_dir(
    cache: &ShellEnvCache,
    home: &Path,
    timeout_duration: Duration,
) -> HashMap<String, String> {
    let mut env = match tokio::time::timeout(timeout_duration, cache.get(home)).await {
        Ok(Ok(snapshot)) => snapshot.vars().iter().cloned().collect(),
        Ok(Err(error)) => {
            log::warn!(
                "Failed to capture home shell env for {}: {error}",
                home.display()
            );
            HashMap::new()
        }
        Err(_) => {
            log::warn!(
                "Timed out after {:?} capturing home shell env for {}",
                timeout_duration,
                home.display()
            );
            HashMap::new()
        }
    };
    sanitize_shell_env(&mut env);
    env
}

pub fn sanitize_shell_env(env: &mut HashMap<String, String>) {
    env.retain(|key, value| !should_remove_shell_env_var(key, value));
}

fn should_remove_shell_env_var(key: &str, value: &str) -> bool {
    let upper_key = key.to_ascii_uppercase();

    if upper_key.starts_with("HERMIT_") {
        return true;
    }

    if matches!(
        upper_key.as_str(),
        "NPM_CONFIG_PREFIX" | "NPM_CONFIG_CACHE" | "COREPACK_HOME"
    ) {
        return true;
    }

    if upper_key == "PATH" {
        return false;
    }

    value.contains("/.hermit/") || value.ends_with("/.hermit")
}

fn push_existing_path(paths: &mut Vec<PathBuf>, path: &str) {
    paths.extend(std::env::split_paths(path).filter(|p| {
        !p.to_string_lossy().contains(".hermit") && !p.join("activate-hermit").exists()
    }));
}

pub fn build_extended_path_from_path(path: Option<&str>) -> String {
    let mut paths: Vec<PathBuf> = Vec::new();

    if let Some(path) = path {
        push_existing_path(&mut paths, path);
    } else if let Ok(system_path) = std::env::var("PATH") {
        push_existing_path(&mut paths, &system_path);
    }

    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".amp/bin"));
        paths.push(home.join(".local/bin"));
        paths.push(home.join(".npm-global/bin"));
        paths.push(home.join(".local/share/mise/shims"));
        paths.push(home.join(".volta/bin"));
        paths.push(home.join(".asdf/shims"));
    }

    paths.push(PathBuf::from("/usr/local/bin"));

    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/opt/homebrew/bin"));
        paths.push(PathBuf::from("/opt/local/bin"));
    }

    if cfg!(windows) {
        if let Some(appdata) = dirs::data_dir() {
            paths.push(appdata.join("npm"));
        }
    }

    if let Some(home) = dirs::home_dir() {
        let nvm_dir = home.join(".nvm/versions/node");
        if nvm_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&nvm_dir) {
                let mut versions: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .collect();
                versions.sort_by_key(|b| std::cmp::Reverse(b.file_name()));
                if let Some(latest) = versions.first() {
                    paths.push(latest.path().join("bin"));
                }
            }
        }

        let fnm_dir = home.join(".local/share/fnm/node-versions");
        if fnm_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&fnm_dir) {
                let mut versions: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .collect();
                versions.sort_by_key(|b| std::cmp::Reverse(b.file_name()));
                if let Some(latest) = versions.first() {
                    paths.push(latest.path().join("installation/bin"));
                }
            }
        }
    }

    let mut seen = HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));

    join_paths_best_effort(paths)
}

/// Join PATH entries, degrading to a best-effort join when a single entry
/// embeds the platform separator (legal in macOS paths): `join_paths` rejects
/// the whole list for one such entry, and `unwrap_or_default` would then hand
/// subprocesses an empty PATH. Drop the un-joinable entries (logging each)
/// instead of erasing every search path.
pub(crate) fn join_paths_best_effort(mut paths: Vec<PathBuf>) -> String {
    match std::env::join_paths(&paths) {
        Ok(joined) => joined.to_string_lossy().to_string(),
        Err(_) => {
            paths.retain(|path| {
                let joinable = std::env::join_paths(std::iter::once(path)).is_ok();
                if !joinable {
                    log::warn!("Dropping un-joinable PATH entry: {}", path.display());
                }
                joinable
            });
            std::env::join_paths(paths)
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        }
    }
}

/// Build a deterministic environment snapshot with PATH normalized through
/// [`build_extended_path_from_path`].
///
/// If home env capture failed, fall back to the current process environment so
/// callers that clear child environments still preserve essential variables.
pub fn env_vars_with_extended_path(shell_env: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut env = if shell_env.is_empty() {
        std::env::vars().collect()
    } else {
        shell_env.clone()
    };
    sanitize_shell_env(&mut env);
    let extended_path = build_extended_path_from_path(env.get("PATH").map(String::as_str));
    env.insert("PATH".to_string(), extended_path);

    let mut vars: Vec<_> = env.into_iter().collect();
    vars.sort_by(|(left, _), (right, _)| left.cmp(right));
    vars
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
    /// Set by the first bounded waiter whose bound ran out on this capture.
    bound_expired: AtomicBool,
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

/// How a bounded wait on an [`InFlightPromise`] ended.
enum Waited {
    Done(Result<ShellEnv, String>),
    Cancelled,
    TimedOut,
}

impl InFlightPromise {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(InFlightState::Pending),
            cv: Condvar::new(),
            bound_expired: AtomicBool::new(false),
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

    /// [`wait_blocking`] that gives up at `deadline`, leaving the promise
    /// pending for its capturer to fulfil.
    ///
    /// [`wait_blocking`]: InFlightPromise::wait_blocking
    fn wait_blocking_until(&self, deadline: Instant) -> Waited {
        let g = self.state.lock().unwrap();
        let timeout = deadline.saturating_duration_since(Instant::now());
        let (g, _) = self
            .cv
            .wait_timeout_while(g, timeout, |state| matches!(state, InFlightState::Pending))
            .unwrap();
        match &*g {
            InFlightState::Done(r) => Waited::Done(r.clone()),
            InFlightState::Cancelled => Waited::Cancelled,
            InFlightState::Pending => Waited::TimedOut,
        }
    }

    /// True for the first caller only: whether this is the first bounded wait
    /// to run out on this capture.
    fn first_bound_expiry(&self) -> bool {
        !self.bound_expired.swap(true, Ordering::Relaxed)
    }
}

/// Result of [`ShellEnvCache::get_blocking_within`].
#[derive(Debug)]
pub enum BoundedEnv {
    /// A fresh snapshot, cached or captured inside the bound.
    Ready(ShellEnv),
    /// The capture was still running when the bound ran out. It keeps running
    /// and fills the cache for later callers. `first` is true for exactly one
    /// caller per capture, so a slow capture is reported once rather than once
    /// per waiter.
    StillCapturing { first: bool },
}

/// Cache of [`ShellEnv`] snapshots keyed by environment root (see
/// [`ShellEnvCache::env_key`]).
pub struct ShellEnvCache {
    inner: Mutex<HashMap<PathBuf, CachedEntry>>,
    /// Working dir → (env root, when resolved), so the walk runs once per
    /// directory per TTL rather than once per git op.
    env_keys: Mutex<HashMap<PathBuf, (PathBuf, Instant)>>,
    /// Where the env-root walk stops; see [`ShellEnvCache::env_key`].
    home: Option<PathBuf>,
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
            env_keys: Mutex::new(HashMap::new()),
            home: home_dir_from_env(),
            ttl,
            shell,
            temp_root,
        }
    }

    /// The directory `working_dir`'s snapshot is captured in and cached
    /// under: its nearest Hermit root or git top level, or `working_dir`
    /// itself when it has neither below `$HOME`, or when a directory-scoped
    /// env tool's file sits between it and that root (rules and marker list
    /// in `env_root.rs`). Two worktrees of one repo are always distinct keys.
    ///
    /// Resolved with a filesystem walk and remembered for the cache TTL.
    /// Directories that don't exist yet aren't remembered, so a worktree
    /// that is still being created resolves again once it is there.
    pub fn env_key(&self, working_dir: &Path) -> PathBuf {
        if let Some((key, resolved_at)) = self.env_keys.lock().unwrap().get(working_dir) {
            if resolved_at.elapsed() < self.ttl {
                return key.clone();
            }
        }
        let key = resolve_env_root(working_dir, self.home.as_deref());
        if working_dir.is_dir() {
            self.env_keys
                .lock()
                .unwrap()
                .insert(working_dir.to_path_buf(), (key.clone(), Instant::now()));
        }
        key
    }

    /// Return a fresh-or-recent snapshot for `working_dir`, captured at its
    /// [`env_key`]. Spawns a shell on miss/expiry; concurrent misses for
    /// directories with the same key share one capture.
    ///
    /// [`env_key`]: ShellEnvCache::env_key
    pub async fn get(&self, working_dir: &Path) -> io::Result<ShellEnv> {
        let key = self.env_key(working_dir);
        let env = self.get_at_root(&key).await?;
        Ok(env.seen_from(&key, working_dir))
    }

    async fn get_at_root(&self, key: &Path) -> io::Result<ShellEnv> {
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
                match map.get(key) {
                    Some(CachedEntry::Ready(env)) if env.captured_at.elapsed() < self.ttl => {
                        return Ok(env.clone());
                    }
                    Some(CachedEntry::InFlight(handle)) => Action::Wait(handle.clone()),
                    _ => {
                        let promise = InFlightPromise::new();
                        let (tx, rx) = watch::channel(None);
                        map.insert(
                            key.to_path_buf(),
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
                        key,
                        promise: promise.clone(),
                        promoted: false,
                    };
                    let outcome = capture_shell_env(key, &self.shell, &self.temp_root).await;
                    match outcome {
                        Ok(vars) => {
                            let env = ShellEnv::captured(vars);
                            self.inner
                                .lock()
                                .unwrap()
                                .insert(key.to_path_buf(), CachedEntry::Ready(env.clone()));
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
    /// caller is capturing for the same env root parks on the promise condvar
    /// rather than spawning a redundant shell.
    ///
    /// [`get`]: ShellEnvCache::get
    pub fn get_blocking(&self, working_dir: &Path) -> io::Result<ShellEnv> {
        let key = self.env_key(working_dir);
        let env = self.get_blocking_at_root(&key)?;
        Ok(env.seen_from(&key, working_dir))
    }

    fn get_blocking_at_root(&self, key: &Path) -> io::Result<ShellEnv> {
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
                match map.get(key) {
                    Some(CachedEntry::Ready(env)) if env.captured_at.elapsed() < self.ttl => {
                        return Ok(env.clone());
                    }
                    Some(CachedEntry::InFlight(handle)) => Action::Wait(handle.promise.clone()),
                    _ => {
                        let promise = InFlightPromise::new();
                        let (tx, rx) = watch::channel(None);
                        map.insert(
                            key.to_path_buf(),
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
                    return self.capture_blocking_as_owner(key, promise, tx);
                }
            }
        }
    }

    /// [`get_blocking`] that waits at most `bound` for the snapshot.
    ///
    /// A fresh snapshot comes back at once. Otherwise the caller waits on the
    /// env root's in-flight capture, joining one that is already running or
    /// starting one on a background thread. If the bound runs out first, the
    /// capture is left alone: it keeps running, caches its result and wakes
    /// every other waiter as usual. Only this caller goes without it.
    ///
    /// [`get_blocking`]: ShellEnvCache::get_blocking
    pub fn get_blocking_within(
        self: &Arc<Self>,
        working_dir: &Path,
        bound: Duration,
    ) -> io::Result<BoundedEnv> {
        let key = self.env_key(working_dir);
        Ok(match self.get_blocking_within_at_root(&key, bound)? {
            BoundedEnv::Ready(env) => BoundedEnv::Ready(env.seen_from(&key, working_dir)),
            still_capturing => still_capturing,
        })
    }

    fn get_blocking_within_at_root(
        self: &Arc<Self>,
        key: &Path,
        bound: Duration,
    ) -> io::Result<BoundedEnv> {
        let deadline = Instant::now() + bound;

        loop {
            let promise = {
                let mut map = self.inner.lock().unwrap();
                match map.get(key) {
                    Some(CachedEntry::Ready(env)) if env.captured_at.elapsed() < self.ttl => {
                        return Ok(BoundedEnv::Ready(env.clone()));
                    }
                    Some(CachedEntry::InFlight(handle)) => handle.promise.clone(),
                    _ => {
                        let promise = InFlightPromise::new();
                        let (tx, rx) = watch::channel(None);
                        // The capture has to outlive this caller's wait, so it
                        // gets its own thread. The thread starts before the
                        // entry is published, so if it can't be spawned nobody
                        // has seen the promise and there is nothing to unwind.
                        let cache = Arc::clone(self);
                        let capture_key = key.to_path_buf();
                        let capture_promise = promise.clone();
                        std::thread::Builder::new()
                            .name("shell-env-capture".to_string())
                            .spawn(move || {
                                let _ = cache.capture_blocking_as_owner(
                                    &capture_key,
                                    capture_promise,
                                    tx,
                                );
                            })?;
                        map.insert(
                            key.to_path_buf(),
                            CachedEntry::InFlight(InFlightHandle {
                                promise: promise.clone(),
                                async_rx: rx,
                            }),
                        );
                        promise
                    }
                }
            };

            match promise.wait_blocking_until(deadline) {
                Waited::Done(Ok(env)) => return Ok(BoundedEnv::Ready(env)),
                Waited::Done(Err(msg)) => return Err(io::Error::other(msg)),
                Waited::Cancelled => continue, // Capturer cancelled — retry.
                Waited::TimedOut => {
                    return Ok(BoundedEnv::StillCapturing {
                        first: promise.first_bound_expiry(),
                    });
                }
            }
        }
    }

    /// Run a blocking capture for `key` as the owner of its `InFlight` entry,
    /// then cache the result and wake every waiter.
    fn capture_blocking_as_owner(
        &self,
        key: &Path,
        promise: Arc<InFlightPromise>,
        tx: watch::Sender<Option<Result<ShellEnv, String>>>,
    ) -> io::Result<ShellEnv> {
        // Locals drop before parameters, so on panic the guard evicts the
        // entry and wakes sync waiters before `tx` drops, as in `get`.
        let mut guard = InFlightGuard {
            cache: self,
            key,
            promise: promise.clone(),
            promoted: false,
        };
        match capture_shell_env_blocking(key, &self.shell, &self.temp_root) {
            Ok(vars) => {
                let env = ShellEnv::captured(vars);
                self.inner
                    .lock()
                    .unwrap()
                    .insert(key.to_path_buf(), CachedEntry::Ready(env.clone()));
                guard.promoted = true;
                promise.set_done(Ok(env.clone()));
                let _ = tx.send(Some(Ok(env.clone())));
                Ok(env)
            }
            Err(err) => {
                let msg = err.to_string();
                promise.set_done(Err(msg.clone()));
                let _ = tx.send(Some(Err(msg)));
                Err(err)
            }
        }
    }

    /// Drop the cached snapshot `working_dir` uses, which is its env root's
    /// and so shared with every directory under that root, and resolve
    /// `working_dir`'s root afresh (next `get` will recapture).
    pub fn invalidate(&self, working_dir: &Path) {
        let key = self.env_key(working_dir);
        self.env_keys.lock().unwrap().remove(working_dir);
        self.inner.lock().unwrap().remove(&key);
    }

    /// Drop all cached snapshots and resolved env roots.
    pub fn invalidate_all(&self) {
        self.env_keys.lock().unwrap().clear();
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

    // Detach from the parent's controlling TTY so an interactive `$SHELL -ils`
    // can't `tcsetpgrp` onto the user's terminal or mutate its modes via
    // rcfiles. Without this, the spawned shell inherits the staged binary's
    // ctty (the user's real /dev/tty up the chain), and on exit leaves the
    // user's TTY foreground-PG pointing at a dead PGID — the outer zsh later
    // dies with EIO on read.
    #[cfg(unix)]
    unsafe {
        // SAFETY: `setsid()` is async-signal-safe.
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

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

    // See `capture_shell_env` for why this is required.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid()` is async-signal-safe.
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

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
    #[cfg(unix)]
    use crate::test_utils::write_fake_shell;

    // ---------------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------------

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

    #[test]
    fn sanitize_shell_env_removes_repo_tool_manager_state() {
        let mut env = HashMap::from([
            ("HOME".to_string(), "/Users/morganm".to_string()),
            (
                "HERMIT_ENV".to_string(),
                "/Users/morganm/Development/repo".to_string(),
            ),
            (
                "NPM_CONFIG_PREFIX".to_string(),
                "/Users/morganm/Development/repo/.hermit/node".to_string(),
            ),
            (
                "NPM_CONFIG_CACHE".to_string(),
                "/Users/morganm/Development/repo/.hermit/cache".to_string(),
            ),
            (
                "COREPACK_HOME".to_string(),
                "/Users/morganm/Development/repo/.hermit/node".to_string(),
            ),
            (
                "PATH".to_string(),
                "/Users/morganm/Development/repo/.hermit/bin:/usr/bin".to_string(),
            ),
            (
                "CUSTOM_TOOL_HOME".to_string(),
                "/Users/morganm/Development/repo/.hermit/custom".to_string(),
            ),
        ]);

        sanitize_shell_env(&mut env);

        assert_eq!(env.get("HOME"), Some(&"/Users/morganm".to_string()));
        assert_eq!(
            env.get("PATH"),
            Some(&"/Users/morganm/Development/repo/.hermit/bin:/usr/bin".to_string())
        );
        assert!(!env.contains_key("HERMIT_ENV"));
        assert!(!env.contains_key("NPM_CONFIG_PREFIX"));
        assert!(!env.contains_key("NPM_CONFIG_CACHE"));
        assert!(!env.contains_key("COREPACK_HOME"));
        assert!(!env.contains_key("CUSTOM_TOOL_HOME"));
    }

    #[test]
    fn extended_path_starts_with_shell_path_and_filters_hermit_entries() {
        let path = build_extended_path_from_path(Some(
            "/shell/bin:/repo/.hermit/bin:/another/bin:/shell/bin",
        ));
        let paths: Vec<_> = std::env::split_paths(&path).collect();

        assert_eq!(
            paths.first().map(|p| p.as_path()),
            Some(Path::new("/shell/bin"))
        );
        assert!(paths.iter().any(|p| p == Path::new("/another/bin")));
        assert_eq!(
            paths
                .iter()
                .filter(|p| p.as_path() == Path::new("/shell/bin"))
                .count(),
            1
        );
        assert!(!paths.iter().any(|p| p == Path::new("/repo/.hermit/bin")));
        assert!(paths.iter().any(|p| p.ends_with(".local/share/mise/shims")));
        assert!(paths.iter().any(|p| p.ends_with(".amp/bin")));
        assert!(paths.iter().any(|p| p.ends_with(".volta/bin")));
        assert!(paths.iter().any(|p| p.ends_with(".asdf/shims")));
    }

    #[test]
    #[cfg(unix)]
    fn join_paths_best_effort_drops_unjoinable_entries_instead_of_emptying_path() {
        let joined = join_paths_best_effort(vec![
            PathBuf::from("/weird:dir/bin"),
            PathBuf::from("/shell/bin"),
            PathBuf::from("/user/bin"),
        ]);
        let paths: Vec<_> = std::env::split_paths(&joined).collect();

        assert_eq!(
            paths,
            vec![PathBuf::from("/shell/bin"), PathBuf::from("/user/bin")]
        );
    }

    #[test]
    fn env_vars_with_extended_path_sanitizes_and_normalizes_path() {
        let env = HashMap::from([
            (
                "PATH".to_string(),
                "/repo/.hermit/bin:/shell/bin".to_string(),
            ),
            ("HERMIT_ENV".to_string(), "/repo".to_string()),
            ("LANG".to_string(), "en_US.UTF-8".to_string()),
        ]);

        let vars = env_vars_with_extended_path(&env);
        let map: HashMap<_, _> = vars.into_iter().collect();
        let path = map.get("PATH").expect("PATH");
        let paths: Vec<_> = std::env::split_paths(path).collect();

        assert_eq!(map.get("LANG"), Some(&"en_US.UTF-8".to_string()));
        assert!(!map.contains_key("HERMIT_ENV"));
        assert!(paths.iter().any(|p| p == Path::new("/shell/bin")));
        assert!(!paths.iter().any(|p| p == Path::new("/repo/.hermit/bin")));
        assert!(paths.iter().any(|p| p.ends_with(".asdf/shims")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn home_interactive_env_sanitizes_clone_but_keeps_raw_cache() {
        let shell = write_fake_shell(
            "#!/bin/sh\nPATH='/repo/.hermit/bin:/usr/bin'\nHERMIT_ENV=/repo\nNPM_CONFIG_PREFIX=/repo/.hermit/node\nCUSTOM_VAR=present\nexport PATH HERMIT_ENV NPM_CONFIG_PREFIX CUSTOM_VAR\nexec /bin/sh -s\n",
        );
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
        let home = tempfile::tempdir().expect("home");

        let sanitized =
            capture_home_interactive_env_for_dir(&cache, home.path(), Duration::from_secs(1)).await;

        assert_eq!(sanitized.get("CUSTOM_VAR"), Some(&"present".to_string()));
        assert!(!sanitized.contains_key("HERMIT_ENV"));
        assert!(!sanitized.contains_key("NPM_CONFIG_PREFIX"));
        assert_eq!(
            sanitized.get("PATH"),
            Some(&"/repo/.hermit/bin:/usr/bin".to_string())
        );

        let raw = cache.get(home.path()).await.expect("cached raw env");
        let raw_map: HashMap<_, _> = raw.vars().iter().cloned().collect();
        assert_eq!(raw_map.get("HERMIT_ENV"), Some(&"/repo".to_string()));
        assert_eq!(
            raw_map.get("PATH"),
            Some(&"/repo/.hermit/bin:/usr/bin".to_string())
        );
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

    // ---------------------------------------------------------------------
    // Bounded waits (`get_blocking_within`)
    // ---------------------------------------------------------------------

    /// Fake `$SHELL` that takes `delay` seconds to start and appends its
    /// working directory to `counter` on every launch, so a test can count
    /// captures and see where each one ran.
    #[cfg(unix)]
    fn counting_shell(counter: &Path, delay: &str) -> tempfile::TempPath {
        write_fake_shell(&format!(
            "#!/bin/sh\npwd >> {}\nsleep {delay}\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n",
            single_quote(&counter.to_string_lossy())
        ))
    }

    #[cfg(unix)]
    fn launches(counter: &Path) -> usize {
        std::fs::read_to_string(counter)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    /// D1: A bounded wait on a cold dir gives up on time, and the capture it
    /// started keeps going with nobody waiting: it lands in the cache, and the
    /// next caller gets that snapshot instead of launching a second shell.
    /// Only the first waiter to give up on the capture is told it was first.
    #[cfg(unix)]
    #[test]
    fn bounded_wait_times_out_and_leaves_the_capture_running() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let counter = scratch.path().join("launches");
        let shell = counting_shell(&counter, "1");
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");

        let started = Instant::now();
        let first = cache
            .get_blocking_within(dir.path(), Duration::from_millis(100))
            .expect("bounded wait");
        let waited = started.elapsed();
        assert!(
            matches!(first, BoundedEnv::StillCapturing { first: true }),
            "{first:?}"
        );
        assert!(
            waited < Duration::from_millis(800),
            "bounded wait overran its bound: {waited:?}"
        );

        let second = cache
            .get_blocking_within(dir.path(), Duration::from_millis(100))
            .expect("bounded wait");
        assert!(
            matches!(second, BoundedEnv::StillCapturing { first: false }),
            "{second:?}"
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        let landed = loop {
            if let Some(CachedEntry::Ready(env)) = cache.inner.lock().unwrap().get(dir.path()) {
                break env.clone();
            }
            assert!(
                Instant::now() < deadline,
                "capture never landed in the cache"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(landed
            .vars()
            .iter()
            .any(|(k, v)| k == "PATH" && v == "/usr/bin:/bin"));

        match cache
            .get_blocking_within(dir.path(), Duration::ZERO)
            .expect("cache hit")
        {
            BoundedEnv::Ready(env) => assert_eq!(env.captured_at(), landed.captured_at()),
            other => panic!("expected the landed snapshot, got {other:?}"),
        }
        assert_eq!(
            launches(&counter),
            1,
            "giving up must neither evict nor restart the capture"
        );
    }

    /// D2: A capture that finishes inside the bound comes back `Ready` and is
    /// cached like any other.
    #[cfg(unix)]
    #[test]
    fn bounded_wait_returns_a_capture_that_finishes_in_time() {
        let shell =
            write_fake_shell("#!/bin/sh\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n");
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");

        let env = match cache
            .get_blocking_within(dir.path(), Duration::from_secs(30))
            .expect("bounded wait")
        {
            BoundedEnv::Ready(env) => env,
            other => panic!("a fast capture should land inside the bound, got {other:?}"),
        };
        assert!(env
            .vars()
            .iter()
            .any(|(k, v)| k == "PATH" && v == "/usr/bin:/bin"));

        let again = cache.get_blocking(dir.path()).expect("cache hit");
        assert_eq!(again.captured_at(), env.captured_at());
    }

    /// D3: The production shape — `run_smart`'s async warm-up owns the capture
    /// and a bounded git read joins it. Giving up must not disturb the async
    /// capturer: it still gets its snapshot, from the one shell, and later
    /// bounded reads hit the cache.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn bounded_wait_on_an_async_capture_leaves_it_running() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let counter = scratch.path().join("launches");
        let shell = counting_shell(&counter, "1");
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

        let bounded = tokio::task::spawn_blocking({
            let cache = cache.clone();
            let dir = dir_path.clone();
            move || cache.get_blocking_within(&dir, Duration::from_millis(100))
        })
        .await
        .expect("bounded task")
        .expect("bounded wait");
        assert!(
            matches!(bounded, BoundedEnv::StillCapturing { first: true }),
            "{bounded:?}"
        );

        let env = async_handle.await.expect("async task").expect("capture");
        let after = tokio::task::spawn_blocking({
            let cache = cache.clone();
            move || cache.get_blocking_within(&dir_path, Duration::ZERO)
        })
        .await
        .expect("bounded task")
        .expect("cache hit");
        match after {
            BoundedEnv::Ready(cached) => assert_eq!(cached.captured_at(), env.captured_at()),
            other => panic!("expected the async snapshot, got {other:?}"),
        }
        assert_eq!(launches(&counter), 1);
    }

    // ---------------------------------------------------------------------
    // Env-root keys
    // ---------------------------------------------------------------------

    /// A worktree root (`.git` dir) with `apps/staged` and `docs` beneath it.
    /// Returns the scratch dir to hold, the root, and the two subdirs.
    fn worktree_with_subdirs() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let scratch = tempfile::tempdir().expect("tempdir");
        let root = scratch.path().join("wt");
        std::fs::create_dir_all(root.join(".git")).expect("mkdir .git");
        let staged = root.join("apps/staged");
        let docs = root.join("docs");
        std::fs::create_dir_all(&staged).expect("mkdir apps/staged");
        std::fs::create_dir_all(&docs).expect("mkdir docs");
        (scratch, root, staged, docs)
    }

    #[cfg(unix)]
    fn launch_dirs(counter: &Path) -> Vec<PathBuf> {
        std::fs::read_to_string(counter)
            .unwrap_or_default()
            .lines()
            .map(|line| std::fs::canonicalize(line).unwrap_or_else(|_| PathBuf::from(line)))
            .collect()
    }

    #[cfg(unix)]
    fn canonical(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    #[cfg(unix)]
    fn var<'a>(vars: &'a [(String, String)], key: &str) -> Option<&'a str> {
        vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// K1: An async lookup from a subdirectory captures once, at the worktree
    /// root, and every later lookup for the root or another subdirectory —
    /// async, sync or bounded — gets that snapshot without a second shell.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn subdir_lookups_share_the_roots_capture() {
        let (_scratch, root, staged, docs) = worktree_with_subdirs();
        let log = tempfile::tempdir().expect("tempdir");
        let counter = log.path().join("launches");
        let shell = counting_shell(&counter, "0");
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));

        let from_staged = cache.get(&staged).await.expect("capture");
        assert_eq!(launch_dirs(&counter), vec![canonical(&root)]);

        let from_root = cache.get(&root).await.expect("root hit");
        let from_docs = cache.get_blocking(&docs).expect("sync hit");
        let bounded = match cache
            .get_blocking_within(&staged, Duration::ZERO)
            .expect("bounded hit")
        {
            BoundedEnv::Ready(env) => env,
            other => panic!("expected the root's snapshot, got {other:?}"),
        };
        for env in [&from_root, &from_docs, &bounded] {
            assert_eq!(env.captured_at(), from_staged.captured_at());
        }
        assert_eq!(launches(&counter), 1, "subdir lookups must not recapture");

        let map = cache.inner.lock().unwrap();
        assert_eq!(map.len(), 1);
        assert!(matches!(map.get(&root), Some(CachedEntry::Ready(_))));
    }

    /// K2: The sync paths resolve the key too. A cold bounded read from a
    /// subdirectory starts the capture at the root, and a blocking read from
    /// the root or a sibling subdirectory hits it.
    #[cfg(unix)]
    #[test]
    fn sync_subdir_lookups_share_the_roots_capture() {
        let (_scratch, root, staged, docs) = worktree_with_subdirs();
        let log = tempfile::tempdir().expect("tempdir");
        let counter = log.path().join("launches");
        let shell = counting_shell(&counter, "0");
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));

        let bounded = match cache
            .get_blocking_within(&staged, Duration::from_secs(30))
            .expect("bounded wait")
        {
            BoundedEnv::Ready(env) => env,
            other => panic!("a fast capture should land inside the bound, got {other:?}"),
        };
        let from_root = cache.get_blocking(&root).expect("root hit");
        let from_docs = cache.get_blocking(&docs).expect("sibling hit");

        assert_eq!(from_root.captured_at(), bounded.captured_at());
        assert_eq!(from_docs.captured_at(), bounded.captured_at());
        assert_eq!(launch_dirs(&counter), vec![canonical(&root)]);
    }

    /// K3: A directory-scoped env tool's file below the root gives that
    /// subdirectory its own capture, in the subdirectory itself.
    #[cfg(unix)]
    #[tokio::test]
    async fn subdir_with_an_env_marker_gets_its_own_capture() {
        let (_scratch, root, staged, _docs) = worktree_with_subdirs();
        std::fs::write(staged.join(".envrc"), "export FROM_DIRENV=1\n").expect("write .envrc");
        let log = tempfile::tempdir().expect("tempdir");
        let counter = log.path().join("launches");
        let shell = counting_shell(&counter, "0");
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));

        let from_root = cache.get(&root).await.expect("root capture");
        let from_staged = cache.get(&staged).await.expect("own capture");

        assert_ne!(from_root.captured_at(), from_staged.captured_at());
        assert_eq!(
            launch_dirs(&counter),
            vec![canonical(&root), canonical(&staged)]
        );
    }

    /// K4: A snapshot shared from the root is seen from the requested
    /// directory: `PWD` is that directory and `OLDPWD` is gone, both in
    /// `vars()` and in what `apply_to`/`apply_to_std` hand a child. The
    /// cached snapshot keeps the root's own values for the next caller.
    #[cfg(unix)]
    #[tokio::test]
    async fn shared_snapshot_sets_pwd_and_drops_oldpwd_for_a_subdir() {
        let (_scratch, root, staged, _docs) = worktree_with_subdirs();
        // Runs the dump script itself rather than `exec /bin/sh -s`: bash
        // drops an inherited OLDPWD at startup, so a re-exec'd `sh` loses it.
        let shell = write_fake_shell(
            "#!/bin/sh\nPATH=/usr/bin:/bin\nOLDPWD=/usr\nexport PATH PWD OLDPWD\neval \"$(cat)\"\n",
        );
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
        let staged_str = staged.to_string_lossy().into_owned();

        let from_staged = cache.get(&staged).await.expect("capture");
        let vars = from_staged.vars();
        assert_eq!(var(&vars, "PWD"), Some(staged_str.as_str()));
        assert_eq!(var(&vars, "OLDPWD"), None);
        assert_eq!(var(&vars, "PATH"), Some("/usr/bin:/bin"));
        assert_eq!(
            vars.iter().filter(|(k, _)| k == "PWD").count(),
            1,
            "{vars:?}"
        );

        let from_root = cache.get(&root).await.expect("root hit");
        let root_vars = from_root.vars();
        assert!(
            matches!(root_vars, Cow::Borrowed(_)),
            "the root reads the shared snapshot as is"
        );
        assert_eq!(var(&root_vars, "OLDPWD"), Some("/usr"));
        let root_pwd = var(&root_vars, "PWD").expect("root snapshot has PWD");
        assert_eq!(canonical(Path::new(root_pwd)), canonical(&root));

        let mut std_cmd = std::process::Command::new("/usr/bin/env");
        from_staged.apply_to_std(&mut std_cmd);
        std_cmd.current_dir(&staged);
        let mut tokio_cmd = Command::new("/usr/bin/env");
        from_staged.apply_to(&mut tokio_cmd);
        tokio_cmd.current_dir(&staged);
        let outputs = [
            std_cmd.output().expect("run env"),
            tokio_cmd.output().await.expect("run env"),
        ];
        for output in outputs {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let child: Vec<(String, String)> = stdout
                .lines()
                .filter_map(|line| line.split_once('='))
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
            assert_eq!(var(&child, "PWD"), Some(staged_str.as_str()), "{stdout}");
            assert_eq!(var(&child, "OLDPWD"), None, "{stdout}");
        }
    }

    /// K5: `env_key` remembers what it resolved, so a marker added later
    /// isn't seen until the memo is cleared. A directory that doesn't exist
    /// yet isn't remembered.
    #[test]
    fn env_key_remembers_resolved_roots() {
        let (_scratch, root, staged, _docs) = worktree_with_subdirs();
        let cache = ShellEnvCache::new();

        assert_eq!(cache.env_key(&staged), root);
        std::fs::write(staged.join(".envrc"), "").expect("write .envrc");
        assert_eq!(cache.env_key(&staged), root, "memoized");

        let later = root.join("later");
        assert_eq!(cache.env_key(&later), root);
        std::fs::create_dir_all(&later).expect("mkdir");
        std::fs::write(later.join(".nvmrc"), "22\n").expect("write .nvmrc");
        assert_eq!(cache.env_key(&later), later, "missing dirs aren't memoized");

        cache.invalidate_all();
        assert_eq!(cache.env_key(&staged), staged);
    }
}
