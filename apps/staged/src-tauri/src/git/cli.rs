use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

use super::strip_git_env;
use crate::shell_env::{BoundedEnv, ShellEnvCache};

#[derive(Error, Debug)]
pub enum GitError {
    #[error("git not found - is git installed?")]
    GitNotFound,

    #[error("not a git repository: {0}")]
    NotARepo(String),

    #[error("git command failed: {0}")]
    CommandFailed(String),

    #[error("invalid utf-8 in git output")]
    InvalidUtf8,

    #[error("path contains invalid UTF-8: {0}")]
    InvalidPath(String),

    /// A [`run_bounded`] read ran with the inherited env because the
    /// captured one wasn't there yet, and failed in a way the captured env
    /// might have avoided (see [`should_retry_with_captured`]): a `required`
    /// LFS clean filter missing from the inherited PATH fails `git status`,
    /// for one. So the failure says nothing about the repo yet. Holds the
    /// error the read failed with.
    ///
    /// The captured env wasn't there because the bound ran out on a capture
    /// still running, or because the root's last capture timed out recently
    /// and will be tried again ([`crate::shell_env::is_recent_timeout`]).
    #[error("{0} (ran with the inherited env before the shell env was captured)")]
    CapturedEnvPending(Box<GitError>),
}

/// How long [`EnvSource::CapturedBounded`] waits for the captured env before
/// running with the lite env instead. A capture that is nearly done still
/// lands inside it, and it is short next to the 9–30 s an interactive login
/// shell takes to start on a dev machine with nvm and Hermit hooks.
// The test build stubs `apply_env`, so only `run_bounded`'s docs refer to it.
#[cfg_attr(test, allow(dead_code))]
const CAPTURED_ENV_READ_BOUND: Duration = Duration::from_secs(2);

/// Run a git command and return stdout as a string. Uses the project's
/// captured interactive-login-shell env (see [`apply_env`]).
pub fn run(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    run_with_env(repo, args, EnvSource::Captured)
}

/// [`run`] for read-only git that shouldn't stall on a cold capture. Waits at
/// most [`CAPTURED_ENV_READ_BOUND`] for the captured env, then runs with the
/// lite env while the capture finishes in the background for later callers.
/// See [`EnvSource::CapturedBounded`].
///
/// A read that ran with the lite env that way and failed in an env-sensitive
/// way comes back as [`GitError::CapturedEnvPending`], so a caller that
/// would read a failure as an answer (no changes, no commits) can tell it
/// apart.
pub fn run_bounded(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    run_with_env(repo, args, EnvSource::CapturedBounded)
}

/// Run a git command with the lite env (parent env minus `GIT_*`).
///
/// No shell-env warm-up, no captured-env retry. The right primitive for git
/// invocations that are demonstrably env-independent — e.g. `git config
/// --local --get|set`, which only touches `.git/config` and never fires
/// smudge filters, credential helpers, or anything else the captured env
/// exists to satisfy.
pub fn run_lite(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    run_with_env(repo, args, EnvSource::Lite)
}

/// [`run_lite`], falling back to the captured env only when the lite env's
/// git can't do anything in the repository: there is no `git` on its `PATH`
/// ([`GitError::GitNotFound`]), or the one there fails before running the
/// command ([`is_unusable_git_error`]). On macOS the lite `PATH` always
/// finds `/usr/bin/git`, the xcrun shim, which stops working when the
/// Command Line Tools break or the Xcode license is unaccepted. It may also
/// be an older git than the shell's that can't read the repository's format.
///
/// This is for commands that don't need the login shell's env but still
/// have to work where the only usable git is the one the login shell puts
/// on `PATH`: the worktree teardown's `worktree list|remove|prune` and
/// `branch -D`. None of them runs a smudge/clean filter or a credential
/// helper. Any other failure is git's verdict on the repository (a worktree
/// or branch that is already gone is routine in a teardown), so it is
/// returned unchanged: a captured env can't change it, and a retry would
/// wait on a shell-env capture for nothing. Unlike [`run_smart`], there is
/// no warm-up either, so the success path never spawns a capture.
///
/// These commands do run hooks: `branch -D` fires the
/// `reference-transaction` hook, which can abort the delete. If a hook
/// needs a tool that only the login shell's `PATH` provides, it fails under
/// the lite env, and that failure is not retried.
pub fn run_lite_or_captured(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    match run_with_env(repo, args, EnvSource::Lite) {
        Err(e) if is_unusable_git(&e) => {
            log::info!(
                "[git::cli::run_lite_or_captured] lite-env git unusable for {}; retrying with captured env: {e}",
                repo.display()
            );
            run_with_env(repo, args, EnvSource::Captured)
        }
        result => result,
    }
}

/// Whether a lite-env failure means the git that ran (or failed to) can't be
/// used at all, rather than that git ran and refused the command. See
/// [`run_lite_or_captured`].
fn is_unusable_git(err: &GitError) -> bool {
    match err {
        GitError::GitNotFound => true,
        GitError::CommandFailed(stderr) => is_unusable_git_error(stderr),
        GitError::NotARepo(_) | GitError::InvalidUtf8 | GitError::InvalidPath(_) => false,
    }
}

/// Stderr shapes from a git that fails before running the command, so
/// another git (the login shell's) may succeed where it didn't:
///
/// - `xcrun: …` / `xcode-select: …` lines come from macOS's `/usr/bin/git`
///   shim, not git, when the Command Line Tools are missing or invalidated
///   by an OS upgrade ("invalid active developer path").
/// - "xcodebuild -license" is the shim refusing to run until the Xcode
///   license is accepted.
/// - "unknown repository extension" and "Expected git repo version" are an
///   older git refusing a repository that a newer one set up, e.g. with
///   `extensions.relativeWorktrees` from `worktree.useRelativePaths`.
///
/// Matched against the `LC_ALL=C` wording [`apply_env`] pins. The shim
/// prefixes only count at the start of a line, where git's own messages
/// start with `fatal:`, `error:` or similar, so a path that happens to
/// contain one doesn't match.
fn is_unusable_git_error(stderr: &str) -> bool {
    const SHIM_LINE_PREFIXES: &[&str] = &["xcrun:", "xcode-select:"];
    const UNUSABLE_GIT_PATTERNS: &[&str] = &[
        "xcodebuild -license",          // unaccepted Xcode license
        "unknown repository extension", // repo extension newer than this git
        "Expected git repo version",    // repo format newer than this git
    ];
    stderr.lines().any(|line| {
        let line = line.trim_start();
        SHIM_LINE_PREFIXES.iter().any(|p| line.starts_with(p))
    }) || UNUSABLE_GIT_PATTERNS.iter().any(|p| stderr.contains(p))
}

/// What a git run that got as far as exiting left behind, for callers that
/// read meaning into a particular non-zero exit (`git config --get-regexp`
/// exits 1 when nothing matches, which is an answer, not a failure).
///
/// `code` is `None` when the child was killed by a signal. `stderr` is
/// decoded lossily; `stdout` must be UTF-8, as for [`run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// [`run_lite`] without the success check: a non-zero exit is returned as a
/// [`GitOutput`] for the caller to interpret. A git that cannot be spawned, a
/// path that is not a repository, and non-UTF-8 stdout are still errors.
pub fn run_lite_output(repo: &Path, args: &[&str]) -> Result<GitOutput, GitError> {
    let output = spawn_git(repo, args, EnvSource::Lite)?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() && stderr.contains("not a git repository") {
        return Err(GitError::NotARepo(repo.display().to_string()));
    }
    Ok(GitOutput {
        success: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).map_err(|_| GitError::InvalidUtf8)?,
        stderr,
    })
}

/// Fast-path variant of [`run`] for foreground first-paint reads.
///
/// Tries the lite env first (parent env minus `GIT_*`), which avoids blocking
/// on the per-project shell-env capture. If git fails in a way that suggests
/// env-sensitivity (missing binary, possible LFS smudge failure, etc.), kicks
/// off (or joins) a captured-env warm-up and retries.
///
/// The warm-up is fire-and-forget so any later non-foreground op (refresh,
/// pull, discard) using `run` finds a `Ready` snapshot and doesn't pay the
/// capture cost itself.
pub fn run_smart(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    warm_shell_env(repo);

    match run_with_env(repo, args, EnvSource::Lite) {
        Ok(out) => Ok(out),
        Err(e) if should_retry_with_captured(&e) => {
            log::info!(
                "[git::cli::run_smart] retrying with captured env after lite-env failure for {}: {e}",
                repo.display()
            );
            run_with_env(repo, args, EnvSource::Captured)
        }
        Err(e) => Err(e),
    }
}

/// Fire-and-forget warm-up of the captured shell env for `repo`.
///
/// Use this when a caller has already resolved the directory a future
/// captured-env operation will need, but has no git read to run there yet.
pub(crate) fn prewarm_shell_env(repo: &Path) {
    warm_shell_env(repo);
}

/// Which environment to pass to the spawned git process.
///
/// Also surfaces at the `state::compute_local_branch_git_state` boundary as a
/// self-documenting selector between the foreground-optimized cli entry point
/// (`run_smart`, which tries `Lite` and falls back to `Captured` on env-
/// sensitive failure), the bounded entry point (`run_bounded`, with `fetch`
/// still on `run`), and the captured-only entry point (`run`).
#[derive(Debug, Clone, Copy)]
pub enum EnvSource {
    /// Parent env with `GIT_*` variables stripped. Skips the per-project
    /// shell-env capture; correct for foreground reads that don't depend on
    /// Hermit-managed git, LFS smudge filters, or credential helpers.
    Lite,
    /// `Captured` if the snapshot is ready within [`CAPTURED_ENV_READ_BOUND`],
    /// otherwise `Lite`. The capture is never cut short: it keeps running and
    /// fills the cache for later callers. For reads of refs, the index and the
    /// work tree, where the lite env costs at worst a less faithful answer
    /// while the capture is still running.
    CapturedBounded,
    /// Project's cached interactive-login-shell snapshot — sees Hermit, LFS,
    /// credential helpers, etc. Blocks on the capture if it isn't ready.
    /// Required for mutating ops (push, pull, discard) and for repos whose
    /// foreground reads have shown env-sensitivity.
    Captured,
}

/// The env a spawn actually ran with. It differs from the requested
/// [`EnvSource`] when the captured env wasn't ready in time or failed to
/// capture, which belongs next to any measure of the wait: a bounded read
/// that gave up after the full bound and one whose capture landed just inside
/// it took about as long, but ran with different envs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppliedEnv {
    Lite,
    Captured,
    /// Asked for the captured env, ran with the lite one. `capture_pending`
    /// when the captured env is still to come: a bounded read whose bound
    /// ran out while the capture was still going, or that found its root's
    /// last capture timed out recently, so it will be tried again. Otherwise
    /// the capture failed, and the lite env is all a `Captured` run would get
    /// too.
    LiteFallback {
        capture_pending: bool,
    },
}

/// Spawn `git -C <repo> <args>` with `source`'s env, non-interactive and
/// detached from the controlling TTY, and wait for it. Only failing to run
/// git at all is an error here; the exit status is the caller's to read.
fn spawn_git(
    repo: &Path,
    args: &[&str],
    source: EnvSource,
) -> Result<std::process::Output, GitError> {
    spawn_git_with_applied_env(repo, args, source).map(|(output, _)| output)
}

/// [`spawn_git`], also returning the env the spawn actually got.
fn spawn_git_with_applied_env(
    repo: &Path,
    args: &[&str],
    source: EnvSource,
) -> Result<(std::process::Output, AppliedEnv), GitError> {
    let repo_str = repo
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(repo.display().to_string()))?;

    let mut command = Command::new("git");
    command.args(["-C", repo_str]).args(args);
    // Blocks on the shell-env capture: without limit for `Captured`, for at
    // most `CAPTURED_ENV_READ_BOUND` for `CapturedBounded`.
    let env = apply_env(&mut command, repo, source);
    if matches!(env, AppliedEnv::LiteFallback { .. }) {
        log::debug!(
            "git {} in {} ran with the inherited env in place of {source:?} ({env:?})",
            args.iter().take(2).copied().collect::<Vec<_>>().join(" "),
            repo.display()
        );
    }
    force_non_interactive(&mut command);
    detach_from_ctty(&mut command);

    let output = command.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::GitNotFound
        } else {
            GitError::CommandFailed(e.to_string())
        }
    });
    Ok((output.map_err(|e| flag_pending_capture(e, env))?, env))
}

/// Set up a `git` command the way [`run`] does — `repo`'s captured
/// login-shell env, non-interactive, detached from the controlling TTY — for
/// callers that spawn it themselves to stream its output. `repo` only keys
/// the env snapshot; the command's working directory is left alone.
///
/// Blocks on the shell-env capture, so call it from the blocking pool.
pub(crate) fn configure_captured(command: &mut Command, repo: &Path) {
    apply_env(command, repo, EnvSource::Captured);
    force_non_interactive(command);
    detach_from_ctty(command);
}

fn run_with_env(repo: &Path, args: &[&str], source: EnvSource) -> Result<String, GitError> {
    let (output, env) = spawn_git_with_applied_env(repo, args, source)?;
    stdout_of(repo, output).map_err(|e| flag_pending_capture(e, env))
}

/// A successful run's stdout, or the error a failed one stands for.
fn stdout_of(repo: &Path, output: std::process::Output) -> Result<String, GitError> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Err(GitError::NotARepo(repo.display().to_string()));
        }
        return Err(GitError::CommandFailed(stderr.into_owned()));
    }

    String::from_utf8(output.stdout).map_err(|_| GitError::InvalidUtf8)
}

/// Wrap `error` in [`GitError::CapturedEnvPending`] when the run that failed
/// with it used the lite env only because the captured env was still to
/// come, and the captured env might have changed the outcome.
fn flag_pending_capture(error: GitError, env: AppliedEnv) -> GitError {
    match env {
        AppliedEnv::LiteFallback {
            capture_pending: true,
        } if should_retry_with_captured(&error) => GitError::CapturedEnvPending(Box::new(error)),
        _ => error,
    }
}

/// Whether a `GitError` from the lite path is plausibly env-sensitive and
/// worth retrying with the captured env.
///
/// `GitNotFound` and `CommandFailed` cover the LFS-filter / missing-binary
/// case. `NotARepo`, `InvalidUtf8`, and `InvalidPath` are env-independent — a
/// retry would just waste a fork.
///
/// Also skips retry for `rev-parse --verify` failures on missing refs (the
/// "Needed a single revision" shape), since those happen for unpublished
/// branches whose `origin/<branch>` doesn't exist — a captured env can't
/// conjure the ref into existence, and the retry would block on the ~8.5s
/// `$SHELL -ils` capture for no gain.
fn should_retry_with_captured(err: &GitError) -> bool {
    match err {
        GitError::GitNotFound | GitError::CapturedEnvPending(_) => true,
        GitError::CommandFailed(msg) => !is_missing_ref_error(msg),
        GitError::NotARepo(_) | GitError::InvalidUtf8 | GitError::InvalidPath(_) => false,
    }
}

/// Union of stderr shapes git produces when a ref doesn't resolve.
///
/// Spans `rev-parse --verify`, plain `rev-parse`, `merge-base` / `cat-file`,
/// and `rev-list`. All four mean "the named ref doesn't exist" and none can
/// be fixed by swapping envs, so adding them to the skip-retry set is
/// strictly correct. Substrate is normalized by the `LC_ALL=C` / `LANG=C`
/// pin in [`apply_env`], so we're matching git's canonical English wording.
fn is_missing_ref_error(stderr: &str) -> bool {
    const REF_RESOLVE_FAILURE_PATTERNS: &[&str] = &[
        "Needed a single revision", // git rev-parse --verify
        "unknown revision or path", // git rev-parse (ambiguous arg preamble)
        "Not a valid object name",  // git merge-base / cat-file
        "bad revision",             // git rev-list / rev-parse alt path
    ];
    REF_RESOLVE_FAILURE_PATTERNS
        .iter()
        .any(|p| stderr.contains(p))
}

/// [`run_bounded`] as it runs once the bound has run out on a capture still
/// in progress: with the lite env, flagging failures the way that run does.
/// The test build's [`apply_env`] only ever applies the lite env, so tests
/// that need such a read's result use this.
#[cfg(test)]
pub(super) fn run_bounded_while_capturing(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    run_lite(repo, args).map_err(|error| {
        flag_pending_capture(
            error,
            AppliedEnv::LiteFallback {
                capture_pending: true,
            },
        )
    })
}

/// [`run_bounded`] as the app build runs it, but against `cache` and with
/// `bound` in place of the app's cache and [`CAPTURED_ENV_READ_BOUND`], so
/// tests can drive the bounded arm with a hermetic shell.
#[cfg(test)]
pub(super) fn run_bounded_with_cache(
    cache: &Arc<ShellEnvCache>,
    repo: &Path,
    args: &[&str],
    bound: Duration,
) -> Result<String, GitError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(args);
    let env = apply_captured_env_within(&mut command, repo, cache, bound);
    pin_c_locale(&mut command);
    force_non_interactive(&mut command);
    let output = command
        .output()
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;
    stdout_of(repo, output).map_err(|e| flag_pending_capture(e, env))
}

/// Fire-and-forget warm-up of the captured shell env for `repo`: starts the
/// capture on a capture thread unless one is cached or in flight, and
/// returns at once. Concurrent warmers for the same env root cost one
/// capture, not N, and nobody waits on it until a captured-env op needs it.
///
/// A no-op under unit tests, which must not launch the user's real shell.
#[cfg(not(test))]
fn warm_shell_env(repo: &Path) {
    crate::session_runner::shell_env_cache().prewarm(repo);
}

#[cfg(test)]
fn warm_shell_env(_repo: &Path) {}

#[cfg(not(test))]
fn apply_env(command: &mut Command, repo: &Path, source: EnvSource) -> AppliedEnv {
    use crate::session_runner::shell_env_cache;

    let applied = match source {
        EnvSource::Lite => {
            strip_git_env(command);
            AppliedEnv::Lite
        }
        EnvSource::CapturedBounded => {
            apply_captured_env_within(command, repo, shell_env_cache(), CAPTURED_ENV_READ_BOUND)
        }
        EnvSource::Captured => match shell_env_cache().get_blocking(repo) {
            Ok(env) => {
                env.apply_to_std(command);
                AppliedEnv::Captured
            }
            Err(e) => {
                warn_capture_failed(repo, &e);
                strip_git_env(command);
                AppliedEnv::LiteFallback {
                    capture_pending: false,
                }
            }
        },
    };
    pin_c_locale(command);
    applied
}

#[cfg(test)]
fn apply_env(command: &mut Command, _repo: &Path, _source: EnvSource) -> AppliedEnv {
    strip_git_env(command);
    pin_c_locale(command);
    AppliedEnv::Lite
}

/// The [`EnvSource::CapturedBounded`] arm of [`apply_env`], taking the cache
/// and bound explicitly so tests can drive it with a hermetic shell.
///
/// When the bound runs out the inherited env is used and the capture is left
/// running. The same goes, without the wait, for a root whose last capture
/// timed out recently, and for a read whose capture timed out while it
/// waited. Only the first caller to give up on a given capture logs it; the
/// rest of a refresh's reads, and other branches waiting on the same
/// directory, would otherwise repeat the line once each. The warning is the
/// capture's own summary line when it finishes, which counts every caller
/// that gave up (`gave_up=`).
fn apply_captured_env_within(
    command: &mut Command,
    repo: &Path,
    cache: &Arc<ShellEnvCache>,
    bound: Duration,
) -> AppliedEnv {
    match cache.get_blocking_within(repo, bound) {
        Ok(BoundedEnv::Ready(env)) => {
            env.apply_to_std(command);
            AppliedEnv::Captured
        }
        Ok(BoundedEnv::StillCapturing { first }) => {
            if first {
                log::info!(
                    "Shell env capture for {} still running after {bound:?}; read-only git \
                     there uses the inherited env until it finishes",
                    repo.display()
                );
            }
            strip_git_env(command);
            AppliedEnv::LiteFallback {
                capture_pending: true,
            }
        }
        Err(e) => {
            warn_capture_failed(repo, &e);
            strip_git_env(command);
            AppliedEnv::LiteFallback {
                // A root that timed out recently, before this read or while
                // it waited on the capture, is captured again once what its
                // shell left running has finished, or after a few minutes,
                // so its captured env is still to come, as for a capture
                // still running. A failure the captured env might have
                // avoided then makes a state build decline, and the card
                // keeps its state, rather than reading a `git status` that
                // couldn't run a filter as a clean tree.
                capture_pending: crate::shell_env::is_recent_timeout(&e),
            }
        }
    }
}

/// A lookup that failed fast on a recent timeout
/// ([`crate::shell_env::is_recent_timeout`]) is only a debug line: the
/// capture warned when it timed out, and every git op in the directory falls
/// back the same way for a few minutes after.
fn warn_capture_failed(repo: &Path, error: &std::io::Error) {
    let level = if crate::shell_env::is_recent_timeout(error) {
        log::Level::Debug
    } else {
        log::Level::Warn
    };
    log::log!(
        level,
        "Failed to capture shell env for {}: {error}; falling back to inherited env",
        repo.display()
    );
}

/// Make git/ssh refuse to prompt at the semantic level. Even if a future
/// refactor reattaches a controlling TTY, ssh will fail fast with
/// `Permission denied` instead of stealing the user's terminal to ask for a
/// passphrase. Pairs with [`detach_from_ctty`] for defense in depth.
///
/// Respects a user-provided `GIT_SSH_COMMAND` from the captured shell env
/// (e.g. a corporate ssh wrapper that pins identity files or routes through a
/// jump host). When the user has set their own, we trust them not to prompt
/// interactively and rely solely on [`detach_from_ctty`] to keep a passphrase
/// prompt from stealing the user's TTY.
fn force_non_interactive(command: &mut Command) {
    command.env("GIT_TERMINAL_PROMPT", "0");
    if !has_env(command, "GIT_SSH_COMMAND") {
        command.env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=10",
        );
    }
}

/// Whether `command` has `key` explicitly set to a value (i.e. via
/// [`Command::env`], not removed via [`Command::env_remove`]). Used to detect
/// vars carried in from the captured shell env so we don't clobber them.
fn has_env(command: &Command, key: &str) -> bool {
    command
        .get_envs()
        .any(|(k, v)| v.is_some() && k == std::ffi::OsStr::new(key))
}

/// Detach the spawned git from the parent's controlling TTY. Without this,
/// ssh can `open("/dev/tty", O_RDWR)` directly and bypass the piped stdio
/// to prompt on the user's real terminal — and worse, `tcsetpgrp` the user's
/// TTY onto a soon-to-die PGID, which later wedges the outer zsh with EIO.
fn detach_from_ctty(command: &mut Command) {
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid()` is async-signal-safe.
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    #[cfg(not(unix))]
    let _ = command;
}

/// Pin git's locale so stderr stays in English regardless of the captured
/// shell snapshot or the host `LC_*`. The substring checks in
/// [`is_missing_ref_error`] and the `NotARepo` parse depend on git's
/// canonical wording; without this, a user with `LANG=ja_JP.UTF-8` would
/// silently flip every unpublished-branch `compute_upstream_state` call back
/// onto the ~8.5s captured-env retry path. `LANG` is belt-and-suspenders
/// ahead of `LC_ALL` (POSIX says `LC_ALL` wins, but cheap to set both).
fn pin_c_locale(command: &mut Command) {
    command.env("LC_ALL", "C");
    command.env("LANG", "C");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TempGitRepo;

    /// `should_retry_with_captured` keeps env-independent failures (NotARepo,
    /// InvalidUtf8, InvalidPath) off the retry path so we don't double-spawn
    /// git for things a captured env can't fix.
    #[test]
    fn retry_predicate_skips_env_independent_errors() {
        assert!(should_retry_with_captured(&GitError::GitNotFound));
        assert!(should_retry_with_captured(&GitError::CommandFailed(
            "boom".into()
        )));
        assert!(!should_retry_with_captured(&GitError::NotARepo(
            "/repo".into()
        )));
        assert!(!should_retry_with_captured(&GitError::InvalidUtf8));
        assert!(!should_retry_with_captured(&GitError::InvalidPath(
            "/repo".into()
        )));
    }

    /// `rev-parse --verify` on a missing ref (e.g. an unpublished branch's
    /// `origin/<branch>`) emits "Needed a single revision". A captured env
    /// can't fix that, so we must not retry — otherwise every unpublished
    /// branch eats the ~8.5s `$SHELL -ils` capture on first paint.
    #[test]
    fn retry_predicate_skips_missing_ref_error() {
        let err = GitError::CommandFailed(
            "fatal: Needed a single revision\nfatal: unknown revision\n".into(),
        );
        assert!(!should_retry_with_captured(&err));
    }

    #[test]
    fn retry_predicate_skips_unknown_revision_error() {
        let err = GitError::CommandFailed(
            "fatal: ambiguous argument 'origin/foo': unknown revision or path not in the working tree.\n".into(),
        );
        assert!(!should_retry_with_captured(&err));
    }

    #[test]
    fn retry_predicate_skips_not_a_valid_object_name_error() {
        let err = GitError::CommandFailed("fatal: Not a valid object name origin/foo\n".into());
        assert!(!should_retry_with_captured(&err));
    }

    #[test]
    fn retry_predicate_skips_bad_revision_error() {
        let err = GitError::CommandFailed("fatal: bad revision 'origin/foo'\n".into());
        assert!(!should_retry_with_captured(&err));
    }

    /// Guard against the broadened pattern set drifting wide enough to catch
    /// env-sensitive failures. Anything that doesn't look like a ref-resolve
    /// shape must still take the captured-env retry.
    #[test]
    fn retry_predicate_retries_unrecognized_command_failed() {
        let err = GitError::CommandFailed("fatal: unable to access something\n".into());
        assert!(should_retry_with_captured(&err));
    }

    /// A git that can't run the command at all sends `run_lite_or_captured`
    /// to the captured env: no binary, macOS's xcrun shim with broken
    /// Command Line Tools or an unaccepted license, or a git too old for the
    /// repository.
    #[test]
    fn unusable_git_predicate_matches_broken_git_signatures() {
        assert!(is_unusable_git(&GitError::GitNotFound));
        for stderr in [
            "xcrun: error: invalid active developer path (/Library/Developer/CommandLineTools), \
             missing xcrun at: /Library/Developer/CommandLineTools/usr/bin/xcrun\n",
            "xcode-select: note: No developer tools were found, requesting install.\n",
            "You have not agreed to the Xcode license agreements. Please run \
             'sudo xcodebuild -license' from within a Terminal window to review and agree \
             to the Xcode and Apple SDKs license.\n",
            "Agreeing to the Xcode/iOS license requires admin privileges, please run \
             \u{201c}sudo xcodebuild -license\u{201d} and then retry this command.\n",
            "fatal: unknown repository extension found:\n\trelativeworktrees\n",
            "fatal: unknown repository extensions found:\n\trefstorage\n\tobjectformat\n",
            "fatal: Expected git repo version <= 1, found 2\n",
        ] {
            assert!(
                is_unusable_git(&GitError::CommandFailed(stderr.into())),
                "{stderr:?}"
            );
        }
    }

    /// Routine teardown failures are git's answer about the repository, so
    /// they must not wait on a shell-env capture. Nor may a path that only
    /// mentions a shim name mid-line.
    #[test]
    fn unusable_git_predicate_skips_routine_teardown_failures() {
        for stderr in [
            "fatal: '/tmp/worktrees/feature' is not a working tree\n",
            "error: branch 'feature' not found\n",
            "error: Cannot delete branch 'feature' checked out at '/tmp/worktrees/feature'\n",
            "fatal: Unable to create '/repo/.git/worktrees/feature/index.lock': File exists.\n",
            "fatal: '/Users/me/xcrun: tools/feature' is not a working tree\n",
            "error: hook 'reference-transaction' exited with status 1\n",
        ] {
            assert!(
                !is_unusable_git(&GitError::CommandFailed(stderr.into())),
                "{stderr:?}"
            );
        }
        assert!(!is_unusable_git(&GitError::NotARepo("/repo".into())));
        assert!(!is_unusable_git(&GitError::InvalidUtf8));
        assert!(!is_unusable_git(&GitError::InvalidPath("/repo".into())));
    }

    /// The patterns match what git actually prints: an unknown repository
    /// extension or format version does, a missing worktree or branch
    /// doesn't.
    #[test]
    fn unusable_git_predicate_matches_real_git_stderr() {
        let stderr_of = |result: Result<String, GitError>| match result {
            Err(GitError::CommandFailed(stderr)) => stderr,
            other => panic!("expected CommandFailed, got {other:?}"),
        };

        let repo = TempGitRepo::new();
        repo.write_file("file.txt", "hello\n");
        repo.commit("init");
        let missing_worktree = repo.path().join("no-such-worktree");
        for args in [
            vec!["branch", "-D", "no-such-branch"],
            vec![
                "worktree",
                "remove",
                missing_worktree.to_str().unwrap(),
                "--force",
            ],
        ] {
            let stderr = stderr_of(run_lite_or_captured(repo.path(), &args));
            assert!(!is_unusable_git_error(&stderr), "{args:?}: {stderr:?}");
        }

        repo.run_git(&["config", "core.repositoryformatversion", "1"]);
        repo.run_git(&["config", "extensions.stagedNoSuchExtension", "true"]);
        let stderr = stderr_of(run_lite(repo.path(), &["worktree", "list"]));
        assert!(is_unusable_git_error(&stderr), "{stderr:?}");

        let repo = TempGitRepo::new();
        repo.run_git(&["config", "core.repositoryformatversion", "99"]);
        let stderr = stderr_of(run_lite(repo.path(), &["branch", "-D", "feature"]));
        assert!(is_unusable_git_error(&stderr), "{stderr:?}");
    }

    /// Happy-path smoke test: `run_smart` on a real repo should return stdout
    /// from the lite-env path without ever needing the captured-env retry.
    #[test]
    fn run_smart_succeeds_on_real_repo() {
        let repo = TempGitRepo::new();
        repo.write_file("file.txt", "hello\n");
        repo.commit("init");

        let head =
            run_smart(repo.path(), &["rev-parse", "HEAD"]).expect("rev-parse should succeed");
        assert!(!head.trim().is_empty(), "HEAD should be a sha");
    }

    /// `NotARepo` errors must not be retried — the captured env can't turn a
    /// non-repo path into a repo, so a retry would just double the cost.
    #[test]
    fn run_smart_returns_not_a_repo_for_non_repo_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err =
            run_smart(tmp.path(), &["status"]).expect_err("status on a non-repo path must fail");
        assert!(
            matches!(err, GitError::NotARepo(_)),
            "expected NotARepo, got {err:?}"
        );
    }

    /// Without a user-provided `GIT_SSH_COMMAND` (the common case),
    /// `force_non_interactive` injects the BatchMode/ConnectTimeout default so
    /// ssh fails fast instead of prompting.
    #[test]
    fn force_non_interactive_injects_default_ssh_command() {
        let mut cmd = Command::new("git");
        force_non_interactive(&mut cmd);
        let ssh = cmd
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("GIT_SSH_COMMAND"))
            .and_then(|(_, v)| v)
            .expect("GIT_SSH_COMMAND should be set");
        assert_eq!(ssh, "ssh -o BatchMode=yes -o ConnectTimeout=10");
    }

    /// A `GIT_SSH_COMMAND` carried in from the captured shell env (e.g. a
    /// corporate ssh wrapper) must not be clobbered. Users who set this
    /// have accepted responsibility for non-interactive behavior themselves.
    #[test]
    fn force_non_interactive_respects_user_ssh_command() {
        let mut cmd = Command::new("git");
        cmd.env("GIT_SSH_COMMAND", "/usr/local/bin/my-ssh-wrapper");
        force_non_interactive(&mut cmd);
        let ssh = cmd
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("GIT_SSH_COMMAND"))
            .and_then(|(_, v)| v)
            .expect("GIT_SSH_COMMAND should still be set");
        assert_eq!(ssh, "/usr/local/bin/my-ssh-wrapper");
    }

    /// `run_lite_output` hands back the exit status instead of judging it:
    /// a no-match `git config --get-regexp` is exit 1 with nothing on stderr,
    /// a successful one is exit 0 with the entries, and a non-repo path is
    /// still the error every runner reports.
    #[test]
    fn run_lite_output_reports_the_exit_status() {
        let repo = TempGitRepo::new();
        let hit = run_lite_output(
            repo.path(),
            &["config", "--local", "--get-regexp", "^user\\.name$"],
        )
        .expect("git ran");
        assert!(hit.success);
        assert_eq!(hit.code, Some(0));
        assert_eq!(hit.stdout.trim(), "user.name Test");
        assert!(hit.stderr.is_empty());

        let miss = run_lite_output(
            repo.path(),
            &["config", "--get-regexp", "^staged\\.no-such-key\\."],
        )
        .expect("git ran");
        assert!(!miss.success);
        assert_eq!(miss.code, Some(1));
        assert!(miss.stdout.is_empty());
        assert!(miss.stderr.trim().is_empty(), "{:?}", miss.stderr);

        // `git config` reads the global config happily outside any repo, so
        // the non-repo shape needs a command that looks for one.
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = run_lite_output(tmp.path(), &["status"])
            .expect_err("a non-repo path is an error, not an exit status");
        assert!(matches!(err, GitError::NotARepo(_)), "{err:?}");
    }

    /// Hermetic cache whose `$SHELL` takes `delay` seconds to start and puts a
    /// marker in its snapshot, so a test can tell the captured env from the
    /// inherited one. `home` stands in for `$HOME`, where the env-root walk
    /// stops, so it never climbs out of the test's scratch dir into whatever
    /// checkout `TMPDIR` sits in. Hold the returned shell for the test's
    /// lifetime.
    #[cfg(unix)]
    fn marker_cache(delay: &str, home: &Path) -> (Arc<ShellEnvCache>, tempfile::TempPath) {
        let shell = crate::test_utils::write_fake_shell(&format!(
            "#!/bin/sh\nsleep {delay}\nPATH=/usr/bin:/bin\nSTAGED_CAPTURE_MARKER=captured\n\
             export PATH STAGED_CAPTURE_MARKER\nexec /bin/sh -s\n"
        ));
        let cache = Arc::new(
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600))
                .with_home(home),
        );
        (cache, shell)
    }

    /// A directory beneath a scratch dir that stands in for `$HOME`.
    #[cfg(unix)]
    fn scratch_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let home = tempfile::tempdir().expect("tempdir");
        let dir = home.path().join("wt");
        std::fs::create_dir(&dir).expect("mkdir");
        (home, dir)
    }

    #[cfg(unix)]
    fn capture_marker(command: &Command) -> Option<String> {
        command
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("STAGED_CAPTURE_MARKER"))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
    }

    /// A bounded read whose capture is still running gets the inherited env
    /// once the bound runs out, and the capture it left running is what the
    /// next read gets.
    #[cfg(unix)]
    #[test]
    fn bounded_env_falls_back_to_inherited_env_while_capture_runs() {
        let (home, dir) = scratch_dir();
        let (cache, _shell) = marker_cache("1", home.path());

        let mut command = Command::new("git");
        let started = std::time::Instant::now();
        let applied =
            apply_captured_env_within(&mut command, &dir, &cache, Duration::from_millis(100));
        let waited = started.elapsed();
        assert_eq!(
            applied,
            AppliedEnv::LiteFallback {
                capture_pending: true
            }
        );
        assert!(
            waited < Duration::from_millis(800),
            "bounded read overran its bound: {waited:?}"
        );
        assert_eq!(capture_marker(&command), None);

        cache
            .get_blocking(&dir)
            .expect("the capture left running lands");
        let mut command = Command::new("git");
        let applied = apply_captured_env_within(&mut command, &dir, &cache, Duration::ZERO);
        assert_eq!(applied, AppliedEnv::Captured);
        assert_eq!(capture_marker(&command).as_deref(), Some("captured"));
    }

    /// A capture that lands inside the bound is the env the read runs with.
    #[cfg(unix)]
    #[test]
    fn bounded_env_uses_captured_env_that_lands_in_time() {
        let (home, dir) = scratch_dir();
        let (cache, _shell) = marker_cache("0", home.path());

        let mut command = Command::new("git");
        let applied =
            apply_captured_env_within(&mut command, &dir, &cache, Duration::from_secs(30));
        assert_eq!(applied, AppliedEnv::Captured);
        assert_eq!(capture_marker(&command).as_deref(), Some("captured"));
    }

    /// A read in a repo subdirectory uses the snapshot captured at the repo's
    /// top level, with `PWD` set to the subdirectory, rather than waiting on
    /// a capture of its own.
    #[cfg(unix)]
    #[test]
    fn bounded_env_in_a_subdir_uses_the_top_levels_snapshot() {
        let repo = TempGitRepo::new();
        let (cache, _shell) = marker_cache("0", repo.path().parent().expect("repo parent"));
        let sub = repo.path().join("apps/staged");
        std::fs::create_dir_all(&sub).expect("mkdir");
        cache.get_blocking(repo.path()).expect("top-level capture");

        let mut command = Command::new("git");
        let applied = apply_captured_env_within(&mut command, &sub, &cache, Duration::ZERO);
        assert_eq!(applied, AppliedEnv::Captured);
        assert_eq!(capture_marker(&command).as_deref(), Some("captured"));
        let pwd = command
            .get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new("PWD"))
            .and_then(|(_, v)| v);
        assert_eq!(pwd, Some(sub.as_os_str()));
    }

    /// Only a failure the captured env might have avoided, from a run that
    /// fell back because the capture was still going, is flagged as pending.
    /// After a failed capture the lite env is the answer, and missing refs or
    /// a non-repo path fail the same way with any env.
    #[test]
    fn only_env_sensitive_failures_while_capturing_are_flagged() {
        let capturing = AppliedEnv::LiteFallback {
            capture_pending: true,
        };
        let capture_failed = AppliedEnv::LiteFallback {
            capture_pending: false,
        };
        let filter_failed = || GitError::CommandFailed("fatal: clean filter 'lfs' failed".into());

        assert!(matches!(
            flag_pending_capture(filter_failed(), capturing),
            GitError::CapturedEnvPending(inner) if matches!(*inner, GitError::CommandFailed(_))
        ));
        assert!(matches!(
            flag_pending_capture(GitError::GitNotFound, capturing),
            GitError::CapturedEnvPending(_)
        ));
        for env in [capture_failed, AppliedEnv::Captured, AppliedEnv::Lite] {
            assert!(matches!(
                flag_pending_capture(filter_failed(), env),
                GitError::CommandFailed(_)
            ));
        }
        assert!(matches!(
            flag_pending_capture(
                GitError::CommandFailed("fatal: Needed a single revision".into()),
                capturing
            ),
            GitError::CommandFailed(_)
        ));
        assert!(matches!(
            flag_pending_capture(GitError::NotARepo("/repo".into()), capturing),
            GitError::NotARepo(_)
        ));
    }

    /// End to end through the bounded arm: a `git status` that gave up on a
    /// running capture and can't run the checkout's clean filter comes back
    /// flagged as pending, not as a failure the caller would read as clean.
    #[cfg(unix)]
    #[test]
    fn bounded_status_that_cant_run_a_filter_while_capturing_is_flagged() {
        let repo = crate::test_utils::repo_with_unrunnable_clean_filter();
        let (cache, _shell) = marker_cache("1", repo.path().parent().expect("repo parent"));

        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(repo.path())
            .args(["status", "--porcelain=1"]);
        let env = apply_captured_env_within(
            &mut command,
            repo.path(),
            &cache,
            Duration::from_millis(100),
        );
        assert_eq!(
            env,
            AppliedEnv::LiteFallback {
                capture_pending: true
            }
        );
        let output = command.output().expect("git ran");
        let error = stdout_of(repo.path(), output)
            .map_err(|e| flag_pending_capture(e, env))
            .expect_err("status has to run the filter");
        assert!(
            matches!(
                &error,
                GitError::CapturedEnvPending(inner)
                    if matches!(&**inner, GitError::CommandFailed(msg) if msg.contains("clean filter"))
            ),
            "{error:?}"
        );
    }
}
