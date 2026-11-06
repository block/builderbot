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
//! Concurrent first-callers for the same root are coalesced through one
//! `InFlight` entry, so only one shell is spawned per (root, miss). At most
//! four shells (`CAPTURE_LIMIT`) run at once across all roots; the rest queue,
//! captures a caller is blocked on ahead of prewarms and bounded reads (see
//! `capture_slots.rs`). `$HOME`'s capture skips the queue, and startup begins
//! it before anything else ([`prewarm_home_env`]), so worktree captures can
//! never hold it up. A shell still running after [`SHELL_CAPTURE_TIMEOUT`] is
//! killed, so a hung rc file can't keep a slot, or its root's entry, forever.
//! What the shell started gets until [`LEFTOVER_GROUP_TIMEOUT`] to finish.
//! Until it has, or for [`TIMED_OUT_RETRY_AFTER`] if nothing was left, the
//! root's lookups fail at once rather than each wait out another timeout.
//!
//! A capture never belongs to a caller's future. Async callers, bounded
//! reads and prewarms run it on a `shell-env-capture` thread; a sync caller
//! runs it on its own thread. A caller that stops waiting, by timeout or by
//! being dropped, leaves the capture running to fill the cache for everyone
//! else.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::process::Command;
use tokio::sync::watch;

mod capture_slots;
mod env_root;
mod leftover_group;

#[cfg(all(test, unix))]
mod capture_limit_tests;

use capture_slots::{CaptureSlots, Urgency, CAPTURE_LIMIT};
use env_root::resolve_env_root;
use leftover_group::{LeftoverState, Leftovers};

/// Default cache TTL — long enough that pipeline runs amortise the capture
/// cost, short enough that edits to `~/.zshrc` or `bin/activate-hermit` are
/// picked up within an hour without an explicit invalidation.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60 * 60);

/// How long a home-env caller waits before going on with the fallback env.
/// A cold first shell measured 23 s, so this covers it with room to spare.
/// Running out doesn't stop the capture; it lands in the cache for the next
/// caller, unless the shell is still going at [`SHELL_CAPTURE_TIMEOUT`].
const HOME_ENV_CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a capture's shell may run before it is killed and the capture
/// fails. Well past the 23 s a cold first shell measured, so it only catches
/// a shell that won't finish soon: an rc file stuck on a keychain prompt or a
/// network call with no timeout, or a first-run step doing real work, such
/// as a nix-direnv dev-shell build or an `nvm install` download.
///
/// Only the shell is killed. What it started keeps going until
/// [`LEFTOVER_GROUP_TIMEOUT`], so a build or download can finish and fill
/// its own cache for the next capture. Failing frees the capture's slot and
/// replaces its entry with a timed-out mark (see [`TimedOutMark`]), so
/// nobody joins the dead capture.
const SHELL_CAPTURE_TIMEOUT: Duration = Duration::from_secs(90);

/// How long, from its shell's start, what a timed-out capture's shell
/// started may keep running before its whole process group is killed. Long
/// enough for a dev-shell build or a download to finish, short enough that
/// a `security` keychain prompt nobody answers doesn't sit there for good.
/// While it runs, the root's timed-out mark holds, so no second rc run
/// starts beside it.
const LEFTOVER_GROUP_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How long after a capture timed out its env root's lookups fail at once,
/// so callers go on with the fallback env, when the shell left nothing
/// running. Another shell started that soon would most likely time out too,
/// and every caller joining it would wait the full [`SHELL_CAPTURE_TIMEOUT`]
/// again. What the shell did leave running decides the mark's lifetime
/// instead (see [`TimedOutMark::recent_timeout`]). A capture after the mark
/// lapses replaces it, as does [`ShellEnvCache::invalidate`].
const TIMED_OUT_RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// [`SHELL_CAPTURE_TIMEOUT`], [`LEFTOVER_GROUP_TIMEOUT`] and
/// [`TIMED_OUT_RETRY_AFTER`] for one cache. Tests shorten them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CaptureTimeouts {
    /// Kill the shell if it is still running after this long.
    pub(crate) shell: Duration,
    /// Kill what the killed shell started if it is still running this long
    /// after the shell started.
    pub(crate) group: Duration,
    /// Fail lookups at once for this long after a capture timed out, unless
    /// the shell left something running.
    pub(crate) retry_after: Duration,
}

impl Default for CaptureTimeouts {
    fn default() -> Self {
        Self {
            shell: SHELL_CAPTURE_TIMEOUT,
            group: LEFTOVER_GROUP_TIMEOUT,
            retry_after: TIMED_OUT_RETRY_AFTER,
        }
    }
}

/// A capture that took this long from start to finish, queue included, gets
/// a warning. One shell measured about 9 s on its own and about 12 s with
/// another running, so a normal capture, and its callers' wait for it, stays
/// well under this. Twice the standalone cost means the shell itself is
/// slow or the queue is backed up. Nobody can wait on a capture longer than
/// it ran, so this covers slow waits too.
const SLOW_CAPTURE: Duration = Duration::from_secs(20);

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

/// Start capturing `$HOME`'s env without waiting for it.
///
/// App setup calls this before anything that captures a worktree's env, and
/// before the ACP tools reconciler and doctor ask for home's env, so they
/// join a capture that is already under way.
pub fn prewarm_home_env(cache: &ShellEnvCache) {
    if let Some(home) = home_dir_from_env() {
        cache.prewarm(&home);
    }
}

fn home_dir_from_env() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

/// Home's env, waiting at most `timeout_duration`. On timeout the caller
/// gets an empty map (callers fall back to the process env), and the capture
/// carries on to fill the cache.
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
                "Timed out after {:?} capturing home shell env for {}; using the fallback env \
                 while the capture finishes in the background",
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
    /// The root's last capture timed out. Lookups fail at once while the
    /// mark is live, then start afresh.
    TimedOut(TimedOutMark),
}

/// What a capture that timed out leaves in its root's entry.
#[derive(Clone)]
struct TimedOutMark {
    /// When the shell was killed.
    at: Instant,
    /// What the shell left running, kept up to date by its watchdog.
    leftovers: Arc<Leftovers>,
}

impl TimedOutMark {
    /// The error the root's lookups get while this mark is live, or `None`
    /// once it has lapsed.
    ///
    /// - While what the killed shell started is still running, the mark
    ///   holds, up to the group deadline. A second rc run beside it would
    ///   repeat its work (a second `nvm install` into the same dir, a second
    ///   nix-direnv evaluation) and most likely time out as well.
    /// - Once all of that has exited on its own, the mark lapses at once.
    ///   What the shell was waiting on is done, so the next capture will
    ///   most likely finish.
    /// - With nothing left running, or once the leftovers were killed at the
    ///   deadline, it holds for `retry_after` from the kill.
    fn recent_timeout(&self, retry_after: Duration) -> Option<RecentTimeout> {
        let now = Instant::now();
        let ago = now.saturating_duration_since(self.at);
        let retry_in = retry_after.saturating_sub(ago);
        match self.leftovers.state() {
            LeftoverState::Finished => None,
            LeftoverState::Running if now < self.leftovers.deadline() => Some(RecentTimeout {
                ago,
                retry_in: retry_in.max(self.leftovers.deadline() - now),
                leftovers_running: true,
            }),
            _ if !retry_in.is_zero() => Some(RecentTimeout {
                ago,
                retry_in,
                leftovers_running: false,
            }),
            _ => None,
        }
    }
}

/// The error a lookup gets while its env root's timed-out mark is live. See
/// [`is_recent_timeout`].
#[derive(Debug)]
struct RecentTimeout {
    ago: Duration,
    /// The longest until the mark lapses.
    retry_in: Duration,
    /// Whether what the shell started is still running. The mark lapses as
    /// soon as it has all finished.
    leftovers_running: bool,
}

impl fmt::Display for RecentTimeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (ago, retry_in) = (self.ago.as_secs(), self.retry_in.as_secs());
        if self.leftovers_running {
            write!(
                f,
                "the last shell env capture here timed out {ago}s ago, and what its shell \
                 started is still running; not trying again until that has finished, or for at \
                 most {retry_in}s"
            )
        } else {
            write!(
                f,
                "the last shell env capture here timed out {ago}s ago; not trying again for \
                 {retry_in}s"
            )
        }
    }
}

impl std::error::Error for RecentTimeout {}

impl From<RecentTimeout> for io::Error {
    fn from(timed_out: RecentTimeout) -> Self {
        io::Error::new(io::ErrorKind::TimedOut, timed_out)
    }
}

/// Whether `error` is from a lookup that failed at once because its env
/// root's last capture timed out recently, rather than from a capture that
/// ran and failed. That capture already warned when it timed out, so
/// callers that look up often can log these quietly.
pub fn is_recent_timeout(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.is::<RecentTimeout>())
}

/// A callback for when a capture lands; see [`ShellEnvCache::after_capture`].
type AfterCapture = Box<dyn FnOnce() + Send>;

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
    /// Later bounded reads of it then don't wait at all (see
    /// [`ShellEnvCache::get_blocking_within`]).
    bound_expired: AtomicBool,
    /// Who waited on this capture, for its one summary line.
    waits: Mutex<WaitStats>,
    /// Set once a caller waits on this capture without a bound, which moves
    /// it ahead of prewarms and bounded reads in the slot queue.
    urgency: Urgency,
    /// Run when the capture lands, dropped if it fails. `None` once it has
    /// finished either way (see [`InFlightPromise::after_landing`]).
    after: Mutex<Option<Vec<AfterCapture>>>,
}

enum InFlightState {
    Pending,
    /// Capturer published a result (success or capture-time failure). Waiters
    /// take this clone and return it directly.
    Done(Result<ShellEnv, String>),
    /// Capturer dropped without publishing (panic, or its thread couldn't
    /// start). Waiters fall through to retry the outer cache lookup.
    Cancelled,
}

/// How a bounded wait on an [`InFlightPromise`] ended.
enum Waited {
    Done(Result<ShellEnv, String>),
    Cancelled,
    TimedOut,
}

/// Callers waiting on one capture. The capture reports them in a single
/// line when it finishes, instead of each waiter logging its own wait.
#[derive(Default)]
struct WaitStats {
    /// Everyone who waited, including those who stopped early.
    waiters: usize,
    /// Waiters that stopped before the capture finished: a bounded read whose
    /// bound ran out, or an async caller that timed out or was dropped.
    gave_up: usize,
    longest_gave_up: Duration,
    /// When each waiter still waiting started.
    waiting_since: Vec<Instant>,
}

/// [`WaitStats`] as of the moment a capture finished.
#[derive(Debug, PartialEq)]
struct WaitSummary {
    waiters: usize,
    gave_up: usize,
    longest: Duration,
}

/// One caller's wait on an [`InFlightPromise`], from
/// [`InFlightPromise::register_waiter`]. Dropped while the capture is still
/// pending, it counts the caller as having given up.
struct Waiter {
    promise: Arc<InFlightPromise>,
    since: Instant,
}

impl Drop for Waiter {
    fn drop(&mut self) {
        // Holding `state` keeps the capture from finishing, and summarising
        // the waits, while this one is moved to `gave_up`.
        let state = self
            .promise
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !matches!(*state, InFlightState::Pending) {
            return;
        }
        let mut waits = self
            .promise
            .waits
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = waits.waiting_since.iter().position(|s| *s == self.since) {
            waits.waiting_since.swap_remove(i);
        }
        waits.gave_up += 1;
        waits.longest_gave_up = waits.longest_gave_up.max(self.since.elapsed());
    }
}

impl InFlightPromise {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(InFlightState::Pending),
            cv: Condvar::new(),
            bound_expired: AtomicBool::new(false),
            waits: Mutex::new(WaitStats::default()),
            urgency: Urgency::default(),
            after: Mutex::new(Some(Vec::new())),
        })
    }

    /// Run `then` when this capture lands, or now if it already has; drop it
    /// if the capture failed or was cancelled. Returns whether `then` ran or
    /// is queued to. It doesn't wait, register as a waiter, or promote the
    /// capture.
    ///
    /// The capture publishes its result before it takes the queue in
    /// [`finish_landing`], so a `then` that finds the queue gone can read the
    /// outcome from `state`, and one queued before that is run or dropped
    /// there. Either way it is never lost.
    ///
    /// [`finish_landing`]: InFlightPromise::finish_landing
    fn after_landing(&self, then: AfterCapture) -> bool {
        let mut after = self.after.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(queued) = after.as_mut() {
            queued.push(then);
            return true;
        }
        drop(after);
        let landed = matches!(
            *self.state.lock().unwrap_or_else(PoisonError::into_inner),
            InFlightState::Done(Ok(_))
        );
        if landed {
            then();
        }
        landed
    }

    /// Run the [`after_landing`] callbacks if the capture `landed`, or drop
    /// them, once its outcome is published.
    ///
    /// [`after_landing`]: InFlightPromise::after_landing
    fn finish_landing(&self, landed: bool) {
        let queued = self
            .after
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .unwrap_or_default();
        if landed {
            for then in queued {
                then();
            }
        }
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
            let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
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

    /// Whether a bounded wait has already run out on this capture.
    fn bound_has_expired(&self) -> bool {
        self.bound_expired.load(Ordering::Relaxed)
    }

    /// [`register_waiter`] for a caller that waits for the result however
    /// long it takes, which moves the capture to the front of `slots`' queue.
    ///
    /// [`register_waiter`]: InFlightPromise::register_waiter
    fn register_unbounded_waiter(self: &Arc<Self>, slots: &CaptureSlots) -> Waiter {
        slots.promote(&self.urgency);
        self.register_waiter()
    }

    /// Count a caller as waiting on this capture until the returned
    /// [`Waiter`] drops.
    fn register_waiter(self: &Arc<Self>) -> Waiter {
        let since = Instant::now();
        let mut waits = self.waits.lock().unwrap();
        waits.waiters += 1;
        waits.waiting_since.push(since);
        Waiter {
            promise: Arc::clone(self),
            since,
        }
    }

    /// Who waited, and the longest wait, for a capture that finished at
    /// `finished`. Waiters still registered then waited until `finished`.
    fn wait_summary(&self, finished: Instant) -> WaitSummary {
        let waits = self.waits.lock().unwrap_or_else(PoisonError::into_inner);
        let longest_still_waiting = waits
            .waiting_since
            .iter()
            .map(|since| finished.saturating_duration_since(*since))
            .max()
            .unwrap_or_default();
        WaitSummary {
            waiters: waits.waiters,
            gave_up: waits.gave_up,
            longest: waits.longest_gave_up.max(longest_still_waiting),
        }
    }
}

/// Result of [`ShellEnvCache::get_blocking_within`].
#[derive(Debug)]
pub enum BoundedEnv {
    /// A fresh snapshot, cached or captured inside the bound.
    Ready(ShellEnv),
    /// The capture was still running, or still queued for a slot, when the
    /// bound ran out. It keeps going and fills the cache for later callers.
    /// `first` is true for exactly one caller per capture, so a slow capture
    /// is reported once rather than once per waiter.
    StillCapturing { first: bool },
}

/// Cache of [`ShellEnv`] snapshots keyed by environment root (see
/// [`ShellEnvCache::env_key`]).
pub struct ShellEnvCache {
    inner: Arc<Mutex<HashMap<PathBuf, CachedEntry>>>,
    /// Limits this cache's captures to [`CAPTURE_LIMIT`] at a time. The app
    /// has a single cache (`session_runner::shell_env_cache`), so this is the
    /// process-wide limit; caches built for tests each get their own.
    slots: Arc<CaptureSlots>,
    /// Working dir → (env root, when resolved), so the walk runs once per
    /// directory per TTL rather than once per git op.
    env_keys: Mutex<HashMap<PathBuf, (PathBuf, Instant)>>,
    /// Where the env-root walk stops (see [`ShellEnvCache::env_key`]), and the
    /// one key whose capture skips the slot queue.
    home: Option<PathBuf>,
    ttl: Duration,
    shell: PathBuf,
    temp_root: PathBuf,
    /// The defaults, shorter in tests.
    timeouts: CaptureTimeouts,
}

/// What [`ShellEnvCache::lookup_or_start`] found for an env root.
enum Lookup {
    Ready(ShellEnv),
    InFlight(InFlightHandle),
    /// Nothing usable was cached, so this caller now owns a new `InFlight`
    /// entry and must run or spawn its capture.
    Start(CaptureJob),
    /// The root's last capture timed out recently; fail without capturing.
    TimedOut(RecentTimeout),
}

/// Where a capture ran, for its summary line.
#[derive(Clone, Copy)]
enum CaptureMode {
    /// On the thread of the sync caller that needed it.
    Inline,
    /// On a dedicated `shell-env-capture` thread.
    Background,
}

impl CaptureMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Background => "background",
        }
    }
}

/// A capture that owns its env root's `InFlight` entry. Run it on the
/// caller's thread with [`CaptureJob::run_inline`], or hand it to a capture
/// thread with [`CaptureJob::spawn`].
///
/// Dropped without publishing a result (its thread couldn't start, or the
/// capture panicked), it evicts its entry and wakes every waiter so they
/// retry instead of waiting forever. `Drop::drop` runs before the fields
/// drop, so sync waiters are woken and the entry is gone before `tx` drops
/// and wakes async waiters.
struct CaptureJob {
    entries: Arc<Mutex<HashMap<PathBuf, CachedEntry>>>,
    slots: Arc<CaptureSlots>,
    key: PathBuf,
    shell: PathBuf,
    temp_root: PathBuf,
    timeouts: CaptureTimeouts,
    /// `$HOME`'s capture runs without waiting for a slot.
    priority: bool,
    handle: InFlightHandle,
    tx: watch::Sender<Option<Result<ShellEnv, String>>>,
    /// When the `InFlight` entry was published; queue time counts from here.
    created: Instant,
    published: bool,
}

impl CaptureJob {
    /// Run the capture on a new `shell-env-capture` thread and return the
    /// handle to wait on. If the thread can't start, the job is dropped,
    /// which evicts the entry and releases anyone who already joined it.
    fn spawn(self) -> io::Result<InFlightHandle> {
        let handle = self.handle.clone();
        std::thread::Builder::new()
            .name("shell-env-capture".to_string())
            .spawn(move || {
                let _ = self.run(CaptureMode::Background);
            })?;
        Ok(handle)
    }

    /// Run the capture on this thread, counting this caller as a waiter
    /// without a bound.
    fn run_inline(self) -> io::Result<ShellEnv> {
        let _waiter = self.handle.promise.register_unbounded_waiter(&self.slots);
        self.run(CaptureMode::Inline)
    }

    fn run(mut self, mode: CaptureMode) -> io::Result<ShellEnv> {
        // The slot is taken before the shell is spawned, and given back as
        // soon as it exits or is killed.
        let slot = (!self.priority).then(|| self.slots.acquire(&self.handle.promise.urgency));
        let started = Instant::now();
        let leftovers = Arc::new(Leftovers::new(started + self.timeouts.group));
        let outcome = capture_shell_env(
            &self.key,
            &self.shell,
            &self.temp_root,
            self.timeouts,
            &leftovers,
        );
        let finished = Instant::now();
        drop(slot);

        let result = outcome.map(ShellEnv::captured);
        {
            let mut map = self.entries.lock().unwrap();
            if self.owns_entry(&map) {
                match &result {
                    Ok(env) => {
                        map.insert(self.key.clone(), CachedEntry::Ready(env.clone()));
                    }
                    // A shell that ran out the timeout would likely do so
                    // again, so the root's lookups fail fast for a while:
                    // until what it left running has finished, or for
                    // `retry_after` if nothing was left.
                    Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                        map.insert(
                            self.key.clone(),
                            CachedEntry::TimedOut(TimedOutMark {
                                at: finished,
                                leftovers,
                            }),
                        );
                    }
                    // A shell that failed quickly is worth trying again, so
                    // the next caller starts afresh.
                    Err(_) => {
                        map.remove(&self.key);
                    }
                }
            }
            // Otherwise the entry was invalidated while this capture ran,
            // and may since hold a newer capture. Either way this result is
            // only for the callers already waiting on it.
        }
        let published = result.as_ref().cloned().map_err(ToString::to_string);
        self.published = true;
        self.handle.promise.set_done(published.clone());
        let _ = self.tx.send(Some(published));
        self.handle.promise.finish_landing(result.is_ok());

        log_capture(
            &self.key,
            mode,
            self.priority,
            started - self.created,
            finished - started,
            self.handle.promise.wait_summary(finished),
            result.as_ref().err(),
        );
        result
    }

    /// Whether the key's entry is still this job's `InFlight` one, rather
    /// than gone or a newer capture started after an `invalidate`.
    fn owns_entry(&self, map: &HashMap<PathBuf, CachedEntry>) -> bool {
        matches!(
            map.get(&self.key),
            Some(CachedEntry::InFlight(handle)) if Arc::ptr_eq(&handle.promise, &self.handle.promise)
        )
    }
}

impl Drop for CaptureJob {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        {
            let mut map = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            if self.owns_entry(&map) {
                map.remove(&self.key);
            }
        }
        self.handle.promise.set_cancelled();
        self.handle.promise.finish_landing(false);
    }
}

/// One line per finished capture, in place of a line per waiter. It warns
/// when the capture took [`SLOW_CAPTURE`] or longer, queue included, or
/// failed, and is a debug line otherwise.
fn log_capture(
    key: &Path,
    mode: CaptureMode,
    priority: bool,
    queued: Duration,
    capture: Duration,
    waits: WaitSummary,
    error: Option<&io::Error>,
) {
    let line = format!(
        "shell-env-capture dir={} mode={} slot={} queued={}ms capture={}ms waiters={} \
         gave_up={} longest_wait={}ms",
        key.display(),
        mode.as_str(),
        if priority { "home" } else { "queue" },
        queued.as_millis(),
        capture.as_millis(),
        waits.waiters,
        waits.gave_up,
        waits.longest.as_millis(),
    );
    if let Some(error) = error {
        log::warn!("{line} failed: {error}");
    } else if queued + capture >= SLOW_CAPTURE {
        log::warn!("{line} (slow)");
    } else {
        log::debug!("{line}");
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
            inner: Arc::new(Mutex::new(HashMap::new())),
            slots: Arc::new(CaptureSlots::new(CAPTURE_LIMIT)),
            env_keys: Mutex::new(HashMap::new()),
            home: home_dir_from_env(),
            ttl,
            shell,
            temp_root,
            timeouts: CaptureTimeouts::default(),
        }
    }

    /// Treat `home` as `$HOME`: its capture skips the slot queue and the
    /// env-root walk stops there. Tests pass their scratch dir so the walk
    /// never leaves it, whatever checkout `TMPDIR` happens to sit in.
    #[cfg(test)]
    pub(crate) fn with_home(mut self, home: &Path) -> Self {
        self.home = Some(home.to_path_buf());
        self
    }

    /// Use `timeouts` in place of the defaults.
    #[cfg(test)]
    pub(crate) fn with_capture_timeouts(mut self, timeouts: CaptureTimeouts) -> Self {
        self.timeouts = timeouts;
        self
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

    /// A fresh snapshot for `key`, the capture already running for it, a
    /// recent timeout, or a new `InFlight` entry for the caller to capture
    /// into.
    fn lookup_or_start(&self, key: &Path) -> Lookup {
        let mut map = self.inner.lock().unwrap();
        match map.get(key) {
            Some(CachedEntry::Ready(env)) if env.captured_at.elapsed() < self.ttl => {
                return Lookup::Ready(env.clone());
            }
            Some(CachedEntry::InFlight(handle)) => return Lookup::InFlight(handle.clone()),
            Some(CachedEntry::TimedOut(mark)) => {
                if let Some(timed_out) = mark.recent_timeout(self.timeouts.retry_after) {
                    return Lookup::TimedOut(timed_out);
                }
            }
            _ => {}
        }
        let (tx, async_rx) = watch::channel(None);
        let handle = InFlightHandle {
            promise: InFlightPromise::new(),
            async_rx,
        };
        map.insert(key.to_path_buf(), CachedEntry::InFlight(handle.clone()));
        Lookup::Start(CaptureJob {
            entries: Arc::clone(&self.inner),
            slots: Arc::clone(&self.slots),
            key: key.to_path_buf(),
            shell: self.shell.clone(),
            temp_root: self.temp_root.clone(),
            timeouts: self.timeouts,
            priority: self.home.as_deref() == Some(key),
            handle,
            tx,
            created: Instant::now(),
            published: false,
        })
    }

    /// Run `then` once `working_dir`'s env root has a fresh snapshot: now, if
    /// one is cached, or when the capture in flight for it lands.
    ///
    /// This doesn't wait on the capture, so it neither moves it up the slot
    /// queue nor counts as one of its waiters. `then` runs on the thread that
    /// finished the capture, which may be a sync caller's, so it must not
    /// block. It is dropped without running if that capture fails, or if
    /// nothing is cached or being captured for the root. Returns whether
    /// `then` ran or is queued to.
    pub fn after_capture(&self, working_dir: &Path, then: impl FnOnce() + Send + 'static) -> bool {
        let key = self.env_key(working_dir);
        let promise = match self.inner.lock().unwrap().get(&key) {
            Some(CachedEntry::Ready(env)) if env.captured_at.elapsed() < self.ttl => None,
            Some(CachedEntry::InFlight(handle)) => Some(Arc::clone(&handle.promise)),
            _ => return false,
        };
        match promise {
            Some(promise) => promise.after_landing(Box::new(then)),
            None => {
                then();
                true
            }
        }
    }

    /// Start capturing `working_dir`'s env on a capture thread, unless it is
    /// cached, already being captured, or timed out recently, without
    /// waiting for it.
    pub fn prewarm(&self, working_dir: &Path) {
        let key = self.env_key(working_dir);
        if let Lookup::Start(job) = self.lookup_or_start(&key) {
            if let Err(error) = job.spawn() {
                log::warn!(
                    "Failed to start a shell env capture for {}: {error}",
                    key.display()
                );
            }
        }
    }

    /// Return a fresh-or-recent snapshot for `working_dir`, captured at its
    /// [`env_key`]. Starts a capture on a capture thread on miss/expiry;
    /// concurrent misses for directories with the same key share one
    /// capture. Dropping the future, or timing it out, stops only this
    /// caller's wait: the capture runs on and fills the cache.
    ///
    /// For a while after the root's last capture timed out, this and the
    /// other lookups fail at once ([`is_recent_timeout`]): until what its
    /// shell left running has finished, or for [`TIMED_OUT_RETRY_AFTER`] if
    /// it left nothing (see [`TimedOutMark::recent_timeout`]).
    ///
    /// [`env_key`]: ShellEnvCache::env_key
    pub async fn get(&self, working_dir: &Path) -> io::Result<ShellEnv> {
        let key = self.env_key(working_dir);
        let env = self.get_at_root(&key).await?;
        Ok(env.seen_from(&key, working_dir))
    }

    async fn get_at_root(&self, key: &Path) -> io::Result<ShellEnv> {
        loop {
            let handle = match self.lookup_or_start(key) {
                Lookup::Ready(env) => return Ok(env),
                Lookup::InFlight(handle) => handle,
                Lookup::Start(job) => job.spawn()?,
                Lookup::TimedOut(timed_out) => return Err(timed_out.into()),
            };
            let _waiter = handle.promise.register_unbounded_waiter(&self.slots);
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
            // The capture died without publishing — retry.
        }
    }

    /// Synchronous variant of [`get`] for sync callers like `git/cli.rs::run`.
    ///
    /// Returns a `Ready` snapshot if one is present and fresh; otherwise
    /// runs a capture on this thread, waiting for a slot first, and stores
    /// the result.
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
            let promise = match self.lookup_or_start(key) {
                Lookup::Ready(env) => return Ok(env),
                Lookup::InFlight(handle) => handle.promise,
                Lookup::Start(job) => return job.run_inline(),
                Lookup::TimedOut(timed_out) => return Err(timed_out.into()),
            };
            let _waiter = promise.register_unbounded_waiter(&self.slots);
            match promise.wait_blocking() {
                Some(Ok(env)) => return Ok(env),
                Some(Err(msg)) => return Err(io::Error::other(msg)),
                None => continue, // Capturer cancelled — retry.
            }
        }
    }

    /// [`get_blocking`] that waits at most `bound` for the snapshot.
    ///
    /// A fresh snapshot comes back at once. Otherwise the caller waits on the
    /// env root's in-flight capture, joining one that is already running or
    /// starting one on a capture thread. Time the capture spends queued for
    /// a slot counts against the bound. If the bound runs out first, the
    /// capture is left alone: it keeps running, caches its result and wakes
    /// every other waiter as usual. Only this caller goes without it.
    ///
    /// The bound is per capture, not per call. Once one bounded wait has run
    /// out on a capture, later bounded reads of it return `StillCapturing`
    /// at once. A state refresh or `has_unpushed_commits` issues its reads
    /// one after another, so otherwise each would sit out its own bound on
    /// the same cold capture. Those reads don't count as waiters in the
    /// capture's summary line, since they never waited.
    ///
    /// A capture that times out while this caller waits on it fails the wait
    /// with the recent-timeout error the reads after it get
    /// ([`is_recent_timeout`]), not with the capture's own message.
    ///
    /// [`get_blocking`]: ShellEnvCache::get_blocking
    pub fn get_blocking_within(
        &self,
        working_dir: &Path,
        bound: Duration,
    ) -> io::Result<BoundedEnv> {
        let key = self.env_key(working_dir);
        Ok(match self.get_blocking_within_at_root(&key, bound)? {
            BoundedEnv::Ready(env) => BoundedEnv::Ready(env.seen_from(&key, working_dir)),
            still_capturing => still_capturing,
        })
    }

    fn get_blocking_within_at_root(&self, key: &Path, bound: Duration) -> io::Result<BoundedEnv> {
        let deadline = Instant::now() + bound;

        loop {
            let promise = match self.lookup_or_start(key) {
                Lookup::Ready(env) => return Ok(BoundedEnv::Ready(env)),
                Lookup::InFlight(handle) if handle.promise.bound_has_expired() => {
                    return Ok(BoundedEnv::StillCapturing { first: false });
                }
                Lookup::InFlight(handle) => handle.promise,
                // The capture has to outlive this caller's wait.
                Lookup::Start(job) => job.spawn()?.promise,
                Lookup::TimedOut(timed_out) => return Err(timed_out.into()),
            };
            let waiter = promise.register_waiter();
            match promise.wait_blocking_until(deadline) {
                Waited::Done(Ok(env)) => return Ok(BoundedEnv::Ready(env)),
                Waited::Done(Err(msg)) => {
                    return Err(match self.live_timeout(key) {
                        Some(timed_out) => timed_out.into(),
                        None => io::Error::other(msg),
                    });
                }
                Waited::Cancelled => continue, // Capturer cancelled — retry.
                Waited::TimedOut => {
                    drop(waiter);
                    return Ok(BoundedEnv::StillCapturing {
                        first: promise.first_bound_expiry(),
                    });
                }
            }
        }
    }

    /// The error `key`'s live timed-out mark gives lookups, if it has one.
    ///
    /// A capture that times out marks its root before it wakes its waiters,
    /// so a bounded read woken by the timeout finds the mark here. It then
    /// fails as the reads after it do ([`is_recent_timeout`]), and its caller
    /// knows the captured env is still to come rather than that the capture
    /// failed for good.
    fn live_timeout(&self, key: &Path) -> Option<RecentTimeout> {
        match self.inner.lock().unwrap().get(key) {
            Some(CachedEntry::TimedOut(mark)) => mark.recent_timeout(self.timeouts.retry_after),
            _ => None,
        }
    }

    /// Drop the cached snapshot `working_dir` uses, which is its env root's
    /// and so shared with every directory under that root, or the root's
    /// timed-out mark, and resolve `working_dir`'s root afresh (next `get`
    /// will recapture).
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
///
/// The per-process sequence number is what makes it unique. macOS rounds
/// `SystemTime` to whole microseconds, so two captures spawned in the same
/// microsecond got one path: the first to finish removed the file under the
/// second, which failed with ENOENT, and with other timing one capture could
/// have read the other directory's env. The pid and time keep names apart
/// across processes, including a later one reusing this pid.
fn dump_path(temp_root: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let pid = std::process::id();
    temp_root.join(format!("staged-shell-env-{pid}-{nanos}-{seq}"))
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

/// Run `$SHELL -ils` in `working_dir` and return the environment it ends up
/// with. Blocks the current thread on the shell's startup; callers run it on
/// a thread of its own (see [`CaptureJob`]). A shell still running after
/// `timeouts.shell` is killed, and the capture fails with
/// [`io::ErrorKind::TimedOut`]. What the shell started may run until
/// `leftovers`' deadline, which reports how it ended (see
/// [`kill_capture_shell`]).
fn capture_shell_env(
    working_dir: &Path,
    shell: &Path,
    temp_root: &Path,
    timeouts: CaptureTimeouts,
    leftovers: &Arc<Leftovers>,
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

    // Detach from the parent's controlling TTY so an interactive `$SHELL -ils`
    // can't `tcsetpgrp` onto the user's terminal or mutate its modes via
    // rcfiles. Without this, the spawned shell inherits the staged binary's
    // ctty (the user's real /dev/tty up the chain), and on exit leaves the
    // user's TTY foreground-PG pointing at a dead PGID — the outer zsh later
    // dies with EIO on read.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid()` is async-signal-safe.
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let started = Instant::now();
    let mut child = cmd.spawn()?;
    let written = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("Failed to open shell stdin for env capture"))
        .and_then(|mut stdin| {
            stdin.write_all(script.as_bytes())?;
            stdin.flush()
            // Drop stdin → shell sees EOF after `exit` and terminates cleanly.
        });
    if let Err(error) = written {
        kill_capture_shell(&mut child, leftovers);
        return Err(error);
    }

    let Some(status) = wait_or_kill(&mut child, started + timeouts.shell, leftovers)? else {
        let _ = std::fs::remove_file(&dump_path);
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "Shell env capture still running after {:?}; killed the shell, leaving what \
                 it started until {:?} after the shell's start",
                timeouts.shell, timeouts.group
            ),
        ));
    };
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

/// Wait for the capture shell to exit, or kill it at `deadline`. `None`
/// means it was killed; what it started is left until `leftovers`'
/// deadline.
///
/// Polls rather than blocking in `wait`, backing off to
/// [`CAPTURE_POLL_MAX`]: std has no wait with a timeout, and a watchdog
/// thread killing by pid could hit a reused pid once the shell is reaped.
/// Here the shell is only ever killed while it is still this process's
/// unreaped child.
fn wait_or_kill(
    child: &mut std::process::Child,
    deadline: Instant,
    leftovers: &Arc<Leftovers>,
) -> io::Result<Option<std::process::ExitStatus>> {
    let mut pause = Duration::from_millis(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status)),
            Ok(None) => {}
            Err(error) => {
                kill_capture_shell(child, leftovers);
                return Err(error);
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            kill_capture_shell(child, leftovers);
            return Ok(None);
        }
        std::thread::sleep(pause.min(left));
        pause = (pause * 2).min(CAPTURE_POLL_MAX);
    }
}

/// Longest pause between checks on a capture shell, and so the most a
/// capture's result can lag the shell's exit.
const CAPTURE_POLL_MAX: Duration = Duration::from_millis(50);

/// Kill a capture shell and reap it, leaving what it started to run until
/// `leftovers`' deadline.
///
/// Killing only the shell frees its slot and fails its capture, while what
/// its rc files started can still finish useful work: a nix-direnv build,
/// an `nvm install`, a Hermit download, each filling a cache that makes the
/// next capture fast. The shell called `setsid()`, so it leads a process
/// group of its own, and an interactive shell without a terminal has no job
/// control, so what it started stays in that group.
/// [`leftover_group::kill_at`] kills whatever is left there at the
/// deadline (a `security` prompt nobody answers, say), and keeps
/// `leftovers` up to date for the timed-out mark.
fn kill_capture_shell(child: &mut std::process::Child, leftovers: &Arc<Leftovers>) {
    let _ = child.kill();
    let _ = child.wait();
    #[cfg(unix)]
    if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
        leftover_group::kill_at(pgid, leftovers);
    }
    #[cfg(not(unix))]
    let _ = leftovers;
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

    /// Dump paths allocated back to back, on one thread or several, never
    /// collide, though many share a microsecond.
    #[test]
    fn dump_paths_are_unique_per_capture() {
        let temp_root = Path::new("/tmp");
        let threads: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(move || {
                    (0..2_000).map(|_| dump_path(temp_root)).collect::<Vec<_>>()
                })
            })
            .collect();
        let paths: Vec<PathBuf> = threads
            .into_iter()
            .flat_map(|thread| thread.join().unwrap())
            .collect();
        let unique: HashSet<&PathBuf> = paths.iter().collect();
        assert_eq!(unique.len(), paths.len());
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

    /// Dropping the async caller that started a capture stops only its wait:
    /// the `InFlight` entry stays, the capture lands in the cache, and the
    /// next caller gets it without a second shell.
    #[cfg(unix)]
    #[tokio::test]
    async fn dropped_async_caller_leaves_the_capture_running() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let counter = scratch.path().join("launches");
        let shell = counting_shell(&counter, "0.5");
        let cache = Arc::new(
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600))
                .with_home(scratch.path()),
        );
        let dir_path = scratch.path().join("wt");
        std::fs::create_dir(&dir_path).expect("mkdir");

        let first = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { cache.get(&dir).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        first.abort();
        let _ = first.await;

        assert!(
            matches!(
                cache.inner.lock().unwrap().get(&dir_path),
                Some(CachedEntry::InFlight(_))
            ),
            "the capture must outlive the caller that started it"
        );

        let second = tokio::time::timeout(Duration::from_secs(30), cache.get(&dir_path))
            .await
            .expect("second caller must not spin")
            .expect("capture");
        assert!(second.vars().iter().any(|(k, _)| k == "PATH"));
        assert_eq!(
            launches(&counter),
            1,
            "the dropped caller's capture is reused"
        );
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

    /// Two sibling dirs under a scratch dir that the cache treats as `$HOME`,
    /// so the env-root walk stops there and each dir is its own key, whatever
    /// checkout `TMPDIR` sits in.
    fn sibling_dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let scratch = tempfile::tempdir().expect("tempdir");
        let (a, b) = (scratch.path().join("a"), scratch.path().join("b"));
        std::fs::create_dir(&a).expect("mkdir a");
        std::fs::create_dir(&b).expect("mkdir b");
        (scratch, a, b)
    }

    /// A3: Two distinct working dirs are cached independently — each second
    /// `get` hits its own `Ready` entry without recapture.
    #[tokio::test]
    async fn different_dirs_cached_independently() {
        let (scratch, dir_a, dir_b) = sibling_dirs();
        let cache = ShellEnvCache::new().with_home(scratch.path());

        let a_first = cache.get(&dir_a).await.expect("capture a");
        let b_first = cache.get(&dir_b).await.expect("capture b");
        // Per-dir cache hits.
        let a_second = cache.get(&dir_a).await.expect("a hit");
        let b_second = cache.get(&dir_b).await.expect("b hit");
        assert_eq!(a_first.captured_at(), a_second.captured_at());
        assert_eq!(b_first.captured_at(), b_second.captured_at());
        // Distinct captures across dirs.
        assert_ne!(a_first.captured_at(), b_first.captured_at());
    }

    /// A4: `invalidate(A)` must not disturb dir B's cached snapshot.
    #[tokio::test]
    async fn invalidate_only_affects_target_dir() {
        let (scratch, dir_a, dir_b) = sibling_dirs();
        let cache = ShellEnvCache::new().with_home(scratch.path());
        let a_first = cache.get(&dir_a).await.expect("capture a");
        let b_first = cache.get(&dir_b).await.expect("capture b");

        cache.invalidate(&dir_a);

        let a_second = cache.get(&dir_a).await.expect("recapture a");
        let b_second = cache.get(&dir_b).await.expect("b should still be cached");

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

    /// B6: An async waiter joined to a capture whose starter is aborted still
    /// gets that capture's snapshot, from the one shell, within a bound.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn waiter_gets_the_capture_after_its_starter_is_dropped() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let counter = scratch.path().join("launches");
        let shell = counting_shell(&counter, "0.5");
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let starter = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { cache.get(&dir).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let waiter = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { tokio::time::timeout(Duration::from_secs(10), cache.get(&dir)).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        starter.abort();
        let _ = starter.await;

        let env = waiter
            .await
            .expect("waiter task")
            .expect("waiter must not spin past timeout")
            .expect("capture");
        assert!(env.vars().iter().any(|(k, _)| k == "PATH"));
        assert_eq!(launches(&counter), 1);
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

    /// B11: A sync `get_blocking` caller parked on a capture whose async
    /// starter is aborted wakes with that capture's snapshot.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sync_waiter_gets_the_capture_after_its_async_starter_is_dropped() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let counter = scratch.path().join("launches");
        let shell = counting_shell(&counter, "0.5");
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let starter = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { cache.get(&dir).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let waiter = tokio::task::spawn_blocking({
            let cache = cache.clone();
            let dir = dir_path.clone();
            move || cache.get_blocking(&dir)
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        starter.abort();
        let _ = starter.await;

        let result = tokio::time::timeout(Duration::from_secs(10), waiter)
            .await
            .expect("sync waiter must not park forever")
            .expect("waiter task")
            .expect("capture");
        assert!(
            result.vars().iter().any(|(k, _)| k == "PATH"),
            "the starter's capture should produce a valid snapshot"
        );
        assert_eq!(launches(&counter), 1);
    }

    /// B12: A capture dropped before it publishes anything (its thread
    /// couldn't start, or it panicked) evicts its entry and wakes its sync and
    /// async waiters, which retry and capture afresh instead of parking.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unpublished_capture_evicts_its_entry_and_wakes_waiters() {
        let shell =
            write_fake_shell("#!/bin/sh\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n");
        let cache = Arc::new(ShellEnvCache::with_shell_and_ttl(
            shell.to_path_buf(),
            Duration::from_secs(3600),
        ));
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let Lookup::Start(job) = cache.lookup_or_start(&dir_path) else {
            panic!("a cold dir should start a capture");
        };
        let sync_waiter = tokio::task::spawn_blocking({
            let cache = cache.clone();
            let dir = dir_path.clone();
            move || cache.get_blocking(&dir)
        });
        let async_waiter = tokio::spawn({
            let cache = cache.clone();
            let dir = dir_path.clone();
            async move { cache.get(&dir).await }
        });
        let promise = job.handle.promise.clone();
        let deadline = Instant::now() + Duration::from_secs(10);
        while promise.waits.lock().unwrap().waiters < 2 {
            assert!(Instant::now() < deadline, "both waiters should join");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        drop(job);

        let within = Duration::from_secs(10);
        let sync_env = tokio::time::timeout(within, sync_waiter)
            .await
            .expect("sync waiter must not park forever")
            .expect("sync task")
            .expect("recapture");
        let async_env = tokio::time::timeout(within, async_waiter)
            .await
            .expect("async waiter must not wait forever")
            .expect("async task")
            .expect("recapture");
        assert!(matches!(
            *promise.state.lock().unwrap(),
            InFlightState::Cancelled
        ));
        assert_eq!(sync_env.captured_at(), async_env.captured_at());
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
        let cache = Arc::new(
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600))
                .with_home(scratch.path()),
        );
        let dir = tempfile::tempdir_in(scratch.path()).expect("tempdir");

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
