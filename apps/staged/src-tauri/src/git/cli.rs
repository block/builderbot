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

/// Run a git command and return stdout as a string
pub fn run(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    let repo_str = repo
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(repo.display().to_string()))?;

    let mut command = Command::new("git");
    command.args(["-C", repo_str]).args(args);
    apply_shell_env(&mut command, repo);

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

/// Replace the spawned git's environment with the project's cached
/// interactive-login-shell snapshot so Hermit-managed `git`, LFS filters,
/// credential helpers, and any binaries invoked by git hooks see the same
/// PATH/env that a user's terminal sees.
///
/// On capture failure (e.g. `$SHELL` unset, init script exits non-zero),
/// falls back to the parent process env with `GIT_*` variables stripped —
/// matching the pre-cache behaviour.
///
/// Gated behind `cfg(not(test))` because unit tests run in fresh tempdirs
/// where shell init has no project context and the per-test spawn would
/// add ~hundreds of ms of overhead with no test value.
#[cfg(not(test))]
fn apply_shell_env(command: &mut Command, repo: &Path) {
    match crate::session_runner::shell_env_cache().get_blocking(repo) {
        Ok(env) => env.apply_to_std(command),
        Err(e) => {
            log::warn!(
                "Failed to capture shell env for {}: {e}; falling back to inherited env",
                repo.display()
            );
            strip_git_env(command);
        }
    }
}

#[cfg(test)]
fn apply_shell_env(command: &mut Command, _repo: &Path) {
    strip_git_env(command);
}
