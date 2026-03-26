use std::path::Path;
use std::process::{Command, Stdio};
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

/// Run a git command and stream stderr lines (split on `\r` and `\n`) to a callback.
///
/// This is useful for commands like `git clone --progress` where progress
/// output is written to stderr using carriage returns.
#[allow(dead_code)]
pub fn run_with_stderr_callback<F>(
    repo: &Path,
    args: &[&str],
    mut on_line: F,
) -> Result<(), GitError>
where
    F: FnMut(&str),
{
    use std::io::Read;

    let repo_str = repo
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(repo.display().to_string()))?;

    let mut child = Command::new("git")
        .args(["-C", repo_str])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::GitNotFound
            } else {
                GitError::CommandFailed(e.to_string())
            }
        })?;

    if let Some(stderr) = child.stderr.take() {
        let mut buf = Vec::new();
        for byte in stderr.bytes() {
            let byte = byte.map_err(|e| GitError::CommandFailed(e.to_string()))?;
            if byte == b'\r' || byte == b'\n' {
                if !buf.is_empty() {
                    if let Ok(line) = std::str::from_utf8(&buf) {
                        on_line(line);
                    }
                    buf.clear();
                }
            } else {
                buf.push(byte);
            }
        }
        if !buf.is_empty() {
            if let Ok(line) = std::str::from_utf8(&buf) {
                on_line(line);
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;
    if !status.success() {
        return Err(GitError::CommandFailed(format!(
            "git command failed with status {}",
            status
        )));
    }
    Ok(())
}
