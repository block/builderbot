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
    log::info!(
        "create_worktree: repo={}, branch={}, start_point={}",
        repo.display(),
        branch_name,
        start_point
    );

    let worktree_path = worktree_path_for(repo, branch_name)?;
    log::debug!(
        "create_worktree: resolved worktree path: {}",
        worktree_path.display()
    );

    // Ensure parent directory exists
    if let Some(parent) = worktree_path.parent() {
        log::debug!(
            "create_worktree: ensuring parent directory exists: {}",
            parent.display()
        );
        std::fs::create_dir_all(parent).map_err(|e| {
            log::error!(
                "create_worktree: failed to create parent directory {}: {e}",
                parent.display()
            );
            GitError::CommandFailed(format!("Failed to create worktree directory: {e}"))
        })?;
    }

    // Check if worktree already exists
    if worktree_path.exists() {
        log::error!(
            "create_worktree: worktree path already exists: {}",
            worktree_path.display()
        );
        return Err(GitError::CommandFailed(format!(
            "Worktree already exists at {}",
            worktree_path.display()
        )));
    }

    // Log existing worktrees for context (helps diagnose "branch already checked out" errors)
    match cli::run(repo, &["worktree", "list"]) {
        Ok(output) => log::debug!("create_worktree: existing worktrees:\n{output}"),
        Err(e) => log::warn!("create_worktree: failed to list existing worktrees: {e}"),
    }

    let worktree_str = worktree_path
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(worktree_path.display().to_string()))?;

    // Create worktree with new branch from start point:
    // git worktree add <path> -b <branch> <start-point>
    log::info!(
        "create_worktree: running `git worktree add {} -b {} {}`",
        worktree_str,
        branch_name,
        start_point
    );
    let result = cli::run(
        repo,
        &[
            "worktree",
            "add",
            worktree_str,
            "-b",
            branch_name,
            start_point,
        ],
    );

    match &result {
        Ok(output) => {
            log::info!(
                "create_worktree: successfully created worktree at {}",
                worktree_path.display()
            );
            if !output.trim().is_empty() {
                log::debug!("create_worktree: git output: {output}");
            }
        }
        Err(e) => {
            log::error!(
                "create_worktree: `git worktree add` failed for branch '{}' at '{}': {e}",
                branch_name,
                worktree_path.display()
            );
            // Log whether the directory was partially created (helps diagnose cleanup issues)
            if worktree_path.exists() {
                log::warn!(
                    "create_worktree: worktree directory still exists after failure (partial creation): {}",
                    worktree_path.display()
                );
            } else {
                log::debug!(
                    "create_worktree: worktree directory does not exist after failure (git cleaned up): {}",
                    worktree_path.display()
                );
            }
        }
    }

    result?;
    Ok(worktree_path)
}

/// Create a new worktree for an existing local branch.
///
/// Uses the standard worktree location and checks out `branch_name` there.
/// Fails if the worktree path already exists.
pub fn create_worktree_for_existing_branch(
    repo: &Path,
    branch_name: &str,
) -> Result<PathBuf, GitError> {
    log::info!(
        "create_worktree_for_existing_branch: repo={}, branch={}",
        repo.display(),
        branch_name
    );

    let worktree_path = worktree_path_for(repo, branch_name)?;
    log::debug!(
        "create_worktree_for_existing_branch: resolved worktree path: {}",
        worktree_path.display()
    );

    // Ensure parent directory exists
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            log::error!(
                "create_worktree_for_existing_branch: failed to create parent directory {}: {e}",
                parent.display()
            );
            GitError::CommandFailed(format!("Failed to create worktree directory: {e}"))
        })?;
    }

    // Check if worktree already exists
    if worktree_path.exists() {
        log::error!(
            "create_worktree_for_existing_branch: worktree path already exists: {}",
            worktree_path.display()
        );
        return Err(GitError::CommandFailed(format!(
            "Worktree already exists at {}",
            worktree_path.display()
        )));
    }

    // Log existing worktrees for context
    match cli::run(repo, &["worktree", "list"]) {
        Ok(output) => {
            log::debug!("create_worktree_for_existing_branch: existing worktrees:\n{output}")
        }
        Err(e) => log::warn!(
            "create_worktree_for_existing_branch: failed to list existing worktrees: {e}"
        ),
    }

    let worktree_str = worktree_path
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(worktree_path.display().to_string()))?;

    // Create worktree for existing branch:
    // git worktree add <path> <branch>
    log::info!(
        "create_worktree_for_existing_branch: running `git worktree add {} {}`",
        worktree_str,
        branch_name
    );
    let result = cli::run(repo, &["worktree", "add", worktree_str, branch_name]);

    match &result {
        Ok(output) => {
            log::info!(
                "create_worktree_for_existing_branch: successfully created worktree at {}",
                worktree_path.display()
            );
            if !output.trim().is_empty() {
                log::debug!("create_worktree_for_existing_branch: git output: {output}");
            }
        }
        Err(e) => {
            log::error!(
                "create_worktree_for_existing_branch: `git worktree add` failed for branch '{}' at '{}': {e}",
                branch_name,
                worktree_path.display()
            );
            if worktree_path.exists() {
                log::warn!(
                    "create_worktree_for_existing_branch: worktree directory still exists after failure: {}",
                    worktree_path.display()
                );
            } else {
                log::debug!(
                    "create_worktree_for_existing_branch: worktree directory does not exist after failure (git cleaned up): {}",
                    worktree_path.display()
                );
            }
        }
    }

    result?;
    Ok(worktree_path)
}

/// Remove a worktree and its associated branch.
///
/// Removes the worktree directory, git worktree reference, and the local git branch.
/// Handles various edge cases:
/// - Normal case: directory exists and git knows about it
/// - Directory deleted: just prune stale git references
/// - Git references deleted: just remove the orphaned directory
/// - Directory not empty: git can't remove due to untracked files (node_modules, etc.)
///
/// The branch_name parameter is optional - if provided, the local branch will be deleted.
/// This is important for allowing the branch to be recreated later.
pub fn remove_worktree(repo: &Path, worktree_path: &Path) -> Result<(), GitError> {
    log::info!(
        "remove_worktree: repo={}, worktree_path={}",
        repo.display(),
        worktree_path.display()
    );

    // First, get the branch name from the worktree before removing it
    let branch_name = get_worktree_branch(repo, worktree_path);
    log::debug!("remove_worktree: associated branch: {:?}", branch_name);

    if worktree_path.exists() {
        log::debug!("remove_worktree: worktree directory exists on disk, attempting git removal");
        // Worktree directory exists on disk - try to remove it normally
        let worktree_str = worktree_path
            .to_str()
            .ok_or_else(|| GitError::InvalidPath(worktree_path.display().to_string()))?;

        // Try: git worktree remove <path> --force
        let result = cli::run(repo, &["worktree", "remove", worktree_str, "--force"]);

        if let Err(e) = result {
            let error_msg = e.to_string();
            log::warn!("remove_worktree: `git worktree remove --force` failed: {error_msg}");

            // If git doesn't recognize it as a worktree (admin files already deleted),
            // or if directory is not empty (untracked files like node_modules),
            // remove the directory manually
            if error_msg.contains("is not a working tree")
                || error_msg.contains("Directory not empty")
            {
                log::info!(
                    "remove_worktree: falling back to manual directory removal for {}",
                    worktree_path.display()
                );
                std::fs::remove_dir_all(worktree_path).map_err(|io_err| {
                    log::error!(
                        "remove_worktree: failed to remove directory {}: {io_err}",
                        worktree_path.display()
                    );
                    GitError::CommandFailed(format!(
                        "Failed to remove worktree directory: {io_err}"
                    ))
                })?;
                // Prune any remaining stale references
                cli::run(repo, &["worktree", "prune"])?;
            } else {
                return Err(e);
            }
        } else {
            log::info!(
                "remove_worktree: successfully removed worktree at {}",
                worktree_path.display()
            );
        }
    } else {
        // Worktree was already deleted from disk - prune stale references
        log::info!(
            "remove_worktree: worktree directory already gone, pruning stale git references"
        );
        cli::run(repo, &["worktree", "prune"])?;
    }

    // Delete the local branch if we found one
    // Use -D (force delete) since the branch may not be fully merged
    if let Some(branch) = branch_name {
        log::info!("remove_worktree: deleting local branch '{branch}'");
        // Ignore errors - branch may already be deleted or may be checked out elsewhere
        if let Err(e) = cli::run(repo, &["branch", "-D", &branch]) {
            log::warn!(
                "remove_worktree: failed to delete branch '{branch}' (may already be gone): {e}"
            );
        }
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
    let range = format!("{base}..HEAD");

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

/// Get the full commit log (with bodies) between base and HEAD.
///
/// Returns a formatted string suitable for inclusion in a prompt, with
/// commits listed oldest-first. Each entry includes SHA, author, date,
/// and the full commit message (subject + body).
pub fn get_full_commit_log(worktree: &Path, base: &str) -> Result<String, GitError> {
    let range = format!("{base}..HEAD");

    // --reverse gives oldest-first ordering
    // %B is the full commit message (subject + body)
    let output = cli::run(
        worktree,
        &[
            "log",
            "--reverse",
            "--format=commit %H%nAuthor: %an%nDate: %ci%n%n%B",
            &range,
        ],
    )?;

    Ok(output)
}

/// Check if a branch exists in the repository.
pub fn branch_exists(repo: &Path, branch_name: &str) -> Result<bool, GitError> {
    let result = cli::run(
        repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch_name}"),
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
    let result = cli::run(worktree, &["rev-parse", &format!("{commit_sha}^")]);
    match result {
        Ok(output) => Ok(Some(output.trim().to_string())),
        Err(_) => Ok(None), // No parent (initial commit or invalid)
    }
}

/// Create a worktree from a GitHub PR.
///
/// This fetches the PR's head ref and creates a local branch + worktree at that commit.
/// The branch name will be the PR's head_ref (e.g., "feature-x").
///
/// Returns (worktree_path, branch_name, base_branch) where base_branch is the PR's target.
pub fn create_worktree_from_pr(
    repo: &Path,
    pr_number: u64,
    head_ref: &str,
    base_ref: &str,
) -> Result<(PathBuf, String, String), GitError> {
    log::info!(
        "create_worktree_from_pr: repo={}, pr_number={}, head_ref={}, base_ref={}",
        repo.display(),
        pr_number,
        head_ref,
        base_ref
    );

    // Use the PR's head_ref as the local branch name
    let branch_name = head_ref.to_string();

    // Check if branch already exists locally
    if branch_exists(repo, &branch_name)? {
        log::error!(
            "create_worktree_from_pr: branch '{}' already exists locally",
            branch_name
        );
        return Err(GitError::CommandFailed(format!(
            "Branch '{branch_name}' already exists locally"
        )));
    }

    let worktree_path = worktree_path_for(repo, &branch_name)?;
    log::debug!(
        "create_worktree_from_pr: resolved worktree path: {}",
        worktree_path.display()
    );

    // Check if worktree already exists
    if worktree_path.exists() {
        log::error!(
            "create_worktree_from_pr: worktree path already exists: {}",
            worktree_path.display()
        );
        return Err(GitError::CommandFailed(format!(
            "Worktree already exists at {}",
            worktree_path.display()
        )));
    }

    // Ensure parent directory exists
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            log::error!(
                "create_worktree_from_pr: failed to create parent directory {}: {e}",
                parent.display()
            );
            GitError::CommandFailed(format!("Failed to create worktree directory: {e}"))
        })?;
    }

    // Fetch the PR head ref
    let pr_ref = format!("refs/pull/{pr_number}/head");
    log::info!("create_worktree_from_pr: fetching {pr_ref}");
    cli::run(repo, &["fetch", "origin", &pr_ref])?;

    // Get the SHA of the fetched PR head
    let head_sha = cli::run(repo, &["rev-parse", "FETCH_HEAD"])?
        .trim()
        .to_string();
    log::debug!("create_worktree_from_pr: PR head SHA: {head_sha}");

    let worktree_str = worktree_path
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(worktree_path.display().to_string()))?;

    // Log existing worktrees for context
    match cli::run(repo, &["worktree", "list"]) {
        Ok(output) => log::debug!("create_worktree_from_pr: existing worktrees:\n{output}"),
        Err(e) => log::warn!("create_worktree_from_pr: failed to list existing worktrees: {e}"),
    }

    // Create worktree with new branch at the PR's head commit
    // git worktree add <path> -b <branch> <commit>
    log::info!(
        "create_worktree_from_pr: running `git worktree add {} -b {} {}`",
        worktree_str,
        branch_name,
        head_sha
    );
    let result = cli::run(
        repo,
        &[
            "worktree",
            "add",
            worktree_str,
            "-b",
            &branch_name,
            &head_sha,
        ],
    );

    match &result {
        Ok(output) => {
            log::info!(
                "create_worktree_from_pr: successfully created worktree at {}",
                worktree_path.display()
            );
            if !output.trim().is_empty() {
                log::debug!("create_worktree_from_pr: git output: {output}");
            }
        }
        Err(e) => {
            log::error!(
                "create_worktree_from_pr: `git worktree add` failed for PR #{} (branch '{}') at '{}': {e}",
                pr_number,
                branch_name,
                worktree_path.display()
            );
            if worktree_path.exists() {
                log::warn!(
                    "create_worktree_from_pr: worktree directory still exists after failure: {}",
                    worktree_path.display()
                );
            } else {
                log::debug!(
                    "create_worktree_from_pr: worktree directory does not exist after failure (git cleaned up): {}",
                    worktree_path.display()
                );
            }
        }
    }

    result?;

    // The base branch for diffs should be the PR's target (e.g., "origin/main")
    let base_branch = format!("origin/{base_ref}");

    Ok((worktree_path, branch_name, base_branch))
}

/// Result of updating a branch from a PR.
#[derive(Debug)]
pub struct UpdateFromPrResult {
    /// The commit SHA before the update
    pub old_sha: String,
    /// The commit SHA after the update (new PR head)
    pub new_sha: String,
    /// Number of new commits pulled in
    pub commits_added: usize,
}

/// Update a local branch's worktree to match the latest PR head.
///
/// This fetches the latest PR head and fast-forwards (or resets) the local branch
/// to match. Works for both clean fast-forwards and force-pushed PRs.
///
/// **Warning**: This will discard any local uncommitted changes and any local
/// commits that are not in the PR. Use with caution.
///
/// Returns information about what changed.
pub fn update_branch_from_pr(
    worktree: &Path,
    pr_number: u64,
) -> Result<UpdateFromPrResult, GitError> {
    // Get the current HEAD before update
    let old_sha = get_head_sha(worktree)?;

    // Fetch the PR head ref
    let pr_ref = format!("refs/pull/{pr_number}/head");
    cli::run(worktree, &["fetch", "origin", &pr_ref])?;

    // Get the SHA of the fetched PR head
    let new_sha = cli::run(worktree, &["rev-parse", "FETCH_HEAD"])?
        .trim()
        .to_string();

    // If already up to date, return early
    if old_sha == new_sha {
        return Ok(UpdateFromPrResult {
            old_sha,
            new_sha,
            commits_added: 0,
        });
    }

    // Check if this is a fast-forward (new_sha is descendant of old_sha)
    let is_fast_forward = cli::run(
        worktree,
        &["merge-base", "--is-ancestor", &old_sha, &new_sha],
    )
    .is_ok();

    if is_fast_forward {
        // Fast-forward: just move HEAD to the new commit
        cli::run(worktree, &["merge", "--ff-only", "FETCH_HEAD"])?;
    } else {
        // Not a fast-forward (PR was force-pushed or rebased)
        // Hard reset to the new PR head
        cli::run(worktree, &["reset", "--hard", "FETCH_HEAD"])?;
    }

    // Count how many commits were added
    // This counts commits between old and new (may be negative for force-push, but we report 0)
    let commits_added = if is_fast_forward {
        let log_output = cli::run(
            worktree,
            &["log", "--oneline", &format!("{old_sha}..{new_sha}")],
        )?;
        log_output.lines().count()
    } else {
        // For force-push/rebase, just count commits from merge-base to new
        let merge_base = cli::run(worktree, &["merge-base", &old_sha, &new_sha])
            .unwrap_or_default()
            .trim()
            .to_string();
        if merge_base.is_empty() {
            0
        } else {
            let log_output = cli::run(
                worktree,
                &["log", "--oneline", &format!("{merge_base}..{new_sha}")],
            )?;
            log_output.lines().count()
        }
    };

    Ok(UpdateFromPrResult {
        old_sha,
        new_sha,
        commits_added,
    })
}

/// Switch a worktree to a different branch.
///
/// This checks out the specified branch in the worktree.
/// The branch must already exist (locally or as a remote tracking branch).
///
/// Returns an error if the checkout fails (e.g., uncommitted changes, branch doesn't exist).
pub fn switch_branch(worktree: &Path, branch_name: &str) -> Result<(), GitError> {
    cli::run(worktree, &["checkout", branch_name])?;
    Ok(())
}

/// Check if the local branch has commits not yet pushed to the remote.
///
/// Compares the local HEAD with `origin/<branch>`. Returns `true` if there
/// are commits in the local branch that are not in the remote tracking branch.
/// Returns `false` if the remote tracking branch doesn't exist (e.g., never pushed).
pub fn has_unpushed_commits(worktree: &Path, branch: &str) -> Result<bool, GitError> {
    let remote_ref = format!("origin/{branch}");
    // Check that the remote ref exists first
    if cli::run(worktree, &["rev-parse", "--verify", &remote_ref]).is_err() {
        // Remote tracking branch doesn't exist — treat as "all commits are unpushed"
        // but only if there are local commits at all
        return Ok(false);
    }
    let output = cli::run(worktree, &["rev-list", &format!("{remote_ref}..HEAD")])?;
    Ok(!output.trim().is_empty())
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
