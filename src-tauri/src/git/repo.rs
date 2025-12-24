//! Repository discovery and utilities

use super::GitError;
use git2::Repository;
use std::path::PathBuf;

/// Find a git repository starting from the given path, searching upward
pub fn find_repo(start_path: Option<&str>) -> Result<Repository, GitError> {
    let path = match start_path {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir().map_err(|e| GitError {
            message: format!("Failed to get current directory: {}", e),
        })?,
    };

    Repository::discover(&path).map_err(|e| e.into())
}

/// Get the current branch name
pub fn get_branch_name(repo: &Repository) -> Option<String> {
    repo.head()
        .ok()
        .and_then(|head| head.shorthand().map(|s| s.to_string()))
}
