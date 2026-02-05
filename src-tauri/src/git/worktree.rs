//! Git worktree operations for branch-based workflow.
//!
//! Manages worktrees in a standard location (~/.staged/worktrees/<repo>/<branch>).

use super::cli::{self, GitError};
use std::path::{Path, PathBuf};

/// Get the standard worktree base directory.
/// Returns ~/.staged/worktrees/
fn worktree_base_dir() -> Result<PathBuf, GitError> {
    let home = dirs::home_dir()
        .ok_or_else(|| GitError::CommandFailed("Cannot find home directory".to_string()))?;
    Ok(home.join(".staged").join("worktrees"))
}

/// Compute the worktree path for a given repo and branch.
/// Format: ~/.staged/worktrees/<repo-name>/<sanitized-branch-name>/
pub fn worktree_path_for(repo: &Path, branch_name: &str) -> Result<PathBuf, GitError> {
    let base = worktree_base_dir()?;

    // Get repo name from path (last component)
    let repo_name = repo
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| GitError::InvalidPath(repo.display().to_string()))?;

    // Sanitize branch name for filesystem (replace / with -)
    let sanitized_branch = branch_name.replace('/', "-");

    Ok(base.join(repo_name).join(sanitized_branch))
}

/// Create a new worktree with a new branch.
///
/// Creates the branch from the specified start point and sets up a worktree
/// at the standard location.
///
/// Returns the path to the created worktree.
pub fn create_worktree(
    repo: &Path,
    branch_name: &str,
    start_point: &str,
) -> Result<PathBuf, GitError> {
    let worktree_path = worktree_path_for(repo, branch_name)?;

    // Ensure parent directory exists
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            GitError::CommandFailed(format!("Failed to create worktree directory: {}", e))
        })?;
    }

    // Check if worktree already exists
    if worktree_path.exists() {
        return Err(GitError::CommandFailed(format!(
            "Worktree already exists at {}",
            worktree_path.display()
        )));
    }

    let worktree_str = worktree_path
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(worktree_path.display().to_string()))?;

    // Create worktree with new branch from start point:
    // git worktree add <path> -b <branch> <start-point>
    cli::run(
        repo,
        &[
            "worktree",
            "add",
            worktree_str,
            "-b",
            branch_name,
            start_point,
        ],
    )?;

    Ok(worktree_path)
}

/// Remove a worktree and its associated branch.
///
/// Removes the worktree directory, git worktree reference, and the local git branch.
/// Handles various edge cases:
/// - Normal case: directory exists and git knows about it
/// - Directory deleted: just prune stale git references
/// - Git references deleted: just remove the orphaned directory
///
/// The branch_name parameter is optional - if provided, the local branch will be deleted.
/// This is important for allowing the branch to be recreated later.
pub fn remove_worktree(repo: &Path, worktree_path: &Path) -> Result<(), GitError> {
    // First, get the branch name from the worktree before removing it
    let branch_name = get_worktree_branch(repo, worktree_path);

    if worktree_path.exists() {
        // Worktree directory exists on disk - try to remove it normally
        let worktree_str = worktree_path
            .to_str()
            .ok_or_else(|| GitError::InvalidPath(worktree_path.display().to_string()))?;

        // Try: git worktree remove <path> --force
        let result = cli::run(repo, &["worktree", "remove", worktree_str, "--force"]);

        if let Err(e) = result {
            // If git doesn't recognize it as a worktree (admin files already deleted),
            // just remove the directory manually
            if e.to_string().contains("is not a working tree") {
                std::fs::remove_dir_all(worktree_path).map_err(|io_err| {
                    GitError::CommandFailed(format!(
                        "Failed to remove orphaned worktree directory: {}",
                        io_err
                    ))
                })?;
                // Prune any remaining stale references
                cli::run(repo, &["worktree", "prune"])?;
            } else {
                return Err(e);
            }
        }
    } else {
        // Worktree was already deleted from disk - prune stale references
        cli::run(repo, &["worktree", "prune"])?;
    }

    // Delete the local branch if we found one
    // Use -D (force delete) since the branch may not be fully merged
    if let Some(branch) = branch_name {
        // Ignore errors - branch may already be deleted or may be checked out elsewhere
        let _ = cli::run(repo, &["branch", "-D", &branch]);
    }

    Ok(())
}

/// Get the branch name associated with a worktree.
/// Returns None if the worktree doesn't exist or has no branch (detached HEAD).
fn get_worktree_branch(repo: &Path, worktree_path: &Path) -> Option<String> {
    let output = cli::run(repo, &["worktree", "list", "--porcelain"]).ok()?;

    let worktree_str = worktree_path.to_str()?;
    let mut in_target_worktree = false;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            in_target_worktree = path == worktree_str;
        } else if in_target_worktree {
            if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                return Some(branch.to_string());
            }
        }
    }

    None
}

/// List all worktrees for a repository.
/// Returns (path, branch_name) pairs.
pub fn list_worktrees(repo: &Path) -> Result<Vec<(PathBuf, Option<String>)>, GitError> {
    let output = cli::run(repo, &["worktree", "list", "--porcelain"])?;

    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in output.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            // Save previous worktree if any
            if let Some(path) = current_path.take() {
                worktrees.push((path, current_branch.take()));
            }
            current_path = Some(PathBuf::from(path_str));
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(branch.to_string());
        }
    }

    // Don't forget the last one
    if let Some(path) = current_path {
        worktrees.push((path, current_branch));
    }

    Ok(worktrees)
}

/// Get the current HEAD commit SHA for a worktree/repo.
pub fn get_head_sha(worktree: &Path) -> Result<String, GitError> {
    let output = cli::run(worktree, &["rev-parse", "HEAD"])?;
    Ok(output.trim().to_string())
}

/// Get commits on a branch since it diverged from base.
/// Returns commits in reverse chronological order (newest first).
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author: String,
    pub timestamp: i64,
}

/// Get commits between base and head.
/// Returns commits in reverse chronological order (newest first).
pub fn get_commits_since_base(worktree: &Path, base: &str) -> Result<Vec<CommitInfo>, GitError> {
    // Format: sha|short_sha|subject|author|timestamp
    let format = "--format=%H|%h|%s|%an|%ct";
    let range = format!("{}..HEAD", base);

    let output = cli::run(worktree, &["log", format, &range])?;

    let mut commits = Vec::new();
    for line in output.lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(5, '|').collect();
        if parts.len() >= 5 {
            commits.push(CommitInfo {
                sha: parts[0].to_string(),
                short_sha: parts[1].to_string(),
                subject: parts[2].to_string(),
                author: parts[3].to_string(),
                timestamp: parts[4].parse().unwrap_or(0),
            });
        }
    }

    Ok(commits)
}

/// Check if a branch exists in the repository.
pub fn branch_exists(repo: &Path, branch_name: &str) -> Result<bool, GitError> {
    let result = cli::run(
        repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{}", branch_name),
        ],
    );
    Ok(result.is_ok())
}

/// Reset HEAD to a specific commit (hard reset).
/// This discards all commits after the specified commit.
pub fn reset_to_commit(worktree: &Path, commit_sha: &str) -> Result<(), GitError> {
    cli::run(worktree, &["reset", "--hard", commit_sha])?;
    Ok(())
}

/// Get the parent commit SHA of a given commit.
/// Returns None if the commit has no parent (initial commit).
pub fn get_parent_commit(worktree: &Path, commit_sha: &str) -> Result<Option<String>, GitError> {
    let result = cli::run(worktree, &["rev-parse", &format!("{}^", commit_sha)]);
    match result {
        Ok(output) => Ok(Some(output.trim().to_string())),
        Err(_) => Ok(None), // No parent (initial commit or invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_path_sanitization() {
        let repo = Path::new("/Users/test/myrepo");
        let path = worktree_path_for(repo, "feature/auth-flow").unwrap();

        // Should sanitize slashes
        assert!(path.to_string_lossy().contains("feature-auth-flow"));
        assert!(!path.to_string_lossy().contains("feature/auth-flow"));
    }
}
