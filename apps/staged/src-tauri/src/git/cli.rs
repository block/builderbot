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

const GIT_LOCAL_ENV_VARS: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// Run a git command and return stdout as a string
pub fn run(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    let repo_str = repo
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(repo.display().to_string()))?;

    let mut command = Command::new("git");
    command.args(["-C", repo_str]).args(args);
    for key in GIT_LOCAL_ENV_VARS {
        command.env_remove(key);
    }

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
