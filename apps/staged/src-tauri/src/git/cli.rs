use std::path::Path;
use std::process::Command;
use thiserror::Error;

use super::strip_git_env;

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
}

/// Run a git command and return stdout as a string. Uses the project's
/// captured interactive-login-shell env (see [`apply_env`]).
pub fn run(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    run_with_env(repo, args, EnvSource::Captured)
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
    warm_shell_env_async(repo);

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

/// Which environment to pass to the spawned git process.
///
/// Also surfaces at the `state::compute_local_branch_git_state` boundary as a
/// self-documenting selector between the foreground-optimized cli entry point
/// (`run_smart`, which tries `Lite` and falls back to `Captured` on env-
/// sensitive failure) and the captured-only entry point (`run`).
#[derive(Debug, Clone, Copy)]
pub enum EnvSource {
    /// Parent env with `GIT_*` variables stripped. Skips the per-project
    /// shell-env capture; correct for foreground reads that don't depend on
    /// Hermit-managed git, LFS smudge filters, or credential helpers.
    Lite,
    /// Project's cached interactive-login-shell snapshot — sees Hermit, LFS,
    /// credential helpers, etc. Blocks on the capture if it isn't ready.
    /// Required for mutating ops (push, pull, discard) and for repos whose
    /// foreground reads have shown env-sensitivity.
    Captured,
}

fn run_with_env(repo: &Path, args: &[&str], source: EnvSource) -> Result<String, GitError> {
    let repo_str = repo
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(repo.display().to_string()))?;

    let mut command = Command::new("git");
    command.args(["-C", repo_str]).args(args);
    apply_env(&mut command, repo, source);
    force_non_interactive(&mut command);
    detach_from_ctty(&mut command);

    let output = command.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::GitNotFound
        } else {
            GitError::CommandFailed(e.to_string())
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Err(GitError::NotARepo(repo.display().to_string()));
        }
        return Err(GitError::CommandFailed(stderr.into_owned()));
    }

    String::from_utf8(output.stdout).map_err(|_| GitError::InvalidUtf8)
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
        GitError::GitNotFound => true,
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

/// Fire-and-forget warm-up of the captured shell env for `repo`. Coalesces
/// with any in-flight capture (sync or async) via the [`ShellEnvCache`]
/// `InFlightHandle`, so multiple concurrent warmers for the same repo cost
/// one capture, not N.
///
/// No-op if there's no Tokio runtime available (e.g. unit tests). In that
/// case the retry path falls back to `get_blocking`, which captures inline.
#[cfg(not(test))]
fn warm_shell_env_async(repo: &Path) {
    let repo = repo.to_path_buf();
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            let _ = crate::session_runner::shell_env_cache().get(&repo).await;
        });
    }
}

#[cfg(test)]
fn warm_shell_env_async(_repo: &Path) {}

#[cfg(not(test))]
fn apply_env(command: &mut Command, repo: &Path, source: EnvSource) {
    match source {
        EnvSource::Lite => strip_git_env(command),
        EnvSource::Captured => match crate::session_runner::shell_env_cache().get_blocking(repo) {
            Ok(env) => env.apply_to_std(command),
            Err(e) => {
                log::warn!(
                    "Failed to capture shell env for {}: {e}; falling back to inherited env",
                    repo.display()
                );
                strip_git_env(command);
            }
        },
    }
    pin_c_locale(command);
}

#[cfg(test)]
fn apply_env(command: &mut Command, _repo: &Path, _source: EnvSource) {
    strip_git_env(command);
    pin_c_locale(command);
}

/// Make git/ssh refuse to prompt at the semantic level. Even if a future
/// refactor reattaches a controlling TTY, ssh will fail fast with
/// `Permission denied` instead of stealing the user's terminal to ask for a
/// passphrase. Pairs with [`detach_from_ctty`] for defense in depth.
fn force_non_interactive(command: &mut Command) {
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env(
        "GIT_SSH_COMMAND",
        "ssh -o BatchMode=yes -o ConnectTimeout=10",
    );
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
}
