//! Git commit operations.

use super::cli::{self, GitError};
use std::path::{Path, PathBuf};

/// Create a commit with the specified files.
/// All listed files are fully staged, then committed together.
/// Returns the short SHA of the new commit.
pub fn commit(repo: &Path, paths: &[PathBuf], message: &str) -> Result<String, GitError> {
    // Reset the index to HEAD first to ensure clean state
    cli::run(repo, &["reset", "HEAD"])?;

    // Stage each file
    for path in paths {
        let path_str = path.to_string_lossy();
        cli::run(repo, &["add", "--", &path_str])?;
    }

    // Create the commit
    cli::run(repo, &["commit", "-m", message])?;

    // Get the short SHA of the new commit
    let output = cli::run(repo, &["rev-parse", "--short", "HEAD"])?;
    Ok(output.trim().to_string())
}

#[cfg(test)]
mod tests {
    // Integration tests would require a real git repo
}
