use std::path::Path;
use std::process::Command;
use thiserror::Error;

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

    let output = Command::new("git")
        .args(["-C", repo_str])
        .args(args)
        .output()
        .map_err(|e| {
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
