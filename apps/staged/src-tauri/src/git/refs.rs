use super::cli::{self, GitError};
use std::path::Path;

/// Get the absolute path to the repository root.
/// For worktrees, this returns the main repository path (not the worktree path).
pub fn get_repo_root(repo: &Path) -> Result<String, GitError> {
    // First, get the common git directory (works for both regular repos and worktrees)
    // For a regular repo: /path/to/repo/.git
    // For a worktree: /path/to/main-repo/.git (the main repo's .git)
    let git_common_dir = cli::run(repo, &["rev-parse", "--git-common-dir"])?;
    let git_common_dir = git_common_dir.trim();

    // The main repo path is the parent of the .git directory
    // Handle both "/path/to/repo/.git" and ".git" (relative path)
    let main_repo_path = if git_common_dir == ".git" {
        // We're in the main repo, use --show-toplevel
        cli::run(repo, &["rev-parse", "--show-toplevel"])?
            .trim()
            .to_string()
    } else {
        // We're in a worktree or got an absolute path
        // Strip the "/.git" suffix to get the repo root
        Path::new(git_common_dir)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                // Fallback to --show-toplevel if we can't parse
                cli::run(repo, &["rev-parse", "--show-toplevel"])
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default()
            })
    };

    Ok(main_repo_path)
}

/// List refs (branches, tags, remotes) for autocomplete
pub fn list_refs(repo: &Path) -> Result<Vec<String>, GitError> {
    // Get all refs with a consistent format
    let output = cli::run(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;

    let refs: Vec<String> = output.lines().map(|s| s.to_string()).collect();

    Ok(refs)
}

/// A branch reference with metadata for display
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchRef {
    /// Short name (e.g., "main", "origin/main")
    pub name: String,
    /// Whether this is a remote-tracking branch
    pub is_remote: bool,
    /// The remote name if this is a remote branch (e.g., "origin")
    pub remote: Option<String>,
}

/// Return the branch name without Staged's preferred `origin/` prefix.
///
/// Stored base branches can be either bare names (`main`) or remote-tracking
/// refs (`origin/main`) depending on when the branch was created. GitHub APIs
/// want the bare base name, while local diff/log commands should use
/// [`origin_ref_for_branch`] so a stale local base branch is never consulted.
pub fn branch_name_without_origin(branch: &str) -> &str {
    branch.strip_prefix("origin/").unwrap_or(branch)
}

/// Return the `origin/<branch>` remote-tracking ref for a stored branch name.
///
/// This intentionally prefers `origin/` refs for comparisons because Staged
/// does not keep local base branches up to date. Callers should fetch before
/// relying on freshness when the exact remote tip matters.
pub fn origin_ref_for_branch(branch: &str) -> String {
    format!("origin/{}", branch_name_without_origin(branch.trim()))
}

/// Prune stale remote-tracking refs (branches deleted on the remote).
///
/// This is a network operation that can be slow, so callers should run it
/// in the background rather than blocking the UI.
pub fn prune_remote(repo: &Path) -> Result<(), GitError> {
    cli::run(repo, &["remote", "prune", "origin"])?;
    Ok(())
}

/// List branches (local and remote) for base branch selection.
/// Returns branches sorted with local first, then remote.
/// Filters out HEAD references.
///
/// Note: this no longer prunes stale remote-tracking refs automatically.
/// Call `prune_remote` separately (in the background) to clean up stale refs.
pub fn list_branches(repo: &Path) -> Result<Vec<BranchRef>, GitError> {
    let output = cli::run(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)\t%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;

    let mut branches: Vec<BranchRef> = output
        .lines()
        .filter(|s| !s.is_empty() && !s.ends_with("/HEAD"))
        .filter_map(|line| {
            let (short, full) = line.split_once('\t')?;
            let is_remote = full.starts_with("refs/remotes/");
            let remote = if is_remote {
                short.split('/').next().map(String::from)
            } else {
                None
            };
            Some(BranchRef {
                name: short.to_string(),
                is_remote,
                remote,
            })
        })
        .collect();

    // Sort: local branches first, then remote (alphabetically within each group)
    branches.sort_by(|a, b| match (a.is_remote, b.is_remote) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    Ok(branches)
}

/// Compute the merge-base between two refs
pub fn merge_base(repo: &Path, ref1: &str, ref2: &str) -> Result<String, GitError> {
    let output = cli::run(repo, &["merge-base", ref1, ref2])?;
    Ok(output.trim().to_string())
}

/// Get the URL of a named remote (e.g. "origin").
pub fn get_remote_url(repo: &Path, remote: &str) -> Result<String, GitError> {
    let output = cli::run(repo, &["remote", "get-url", remote])?;
    Ok(output.trim().to_string())
}

/// Resolve a ref to its full SHA
pub fn resolve_ref(repo: &Path, reference: &str) -> Result<String, GitError> {
    let output = cli::run(repo, &["rev-parse", reference])?;
    Ok(output.trim().to_string())
}

/// Get the current branch name.
/// Returns an error if in detached HEAD state.
pub fn get_current_branch(repo: &Path) -> Result<String, GitError> {
    let output = cli::run(repo, &["branch", "--show-current"])?;
    let branch_name = output.trim();

    if branch_name.is_empty() {
        return Err(GitError::CommandFailed(
            "Not on a branch (detached HEAD)".to_string(),
        ));
    }

    Ok(branch_name.to_string())
}

/// Detect the default branch for this repository.
/// Checks for common default branch names in order of preference.
/// Always returns the remote-tracking form (e.g., "origin/main") so that
/// new branches start from the remote tip rather than the local HEAD.
pub fn detect_default_branch(repo: &Path) -> Result<String, GitError> {
    let refs = list_refs(repo)?;

    // Check for remote-tracking branches first (preferred for merge-base)
    let remote_candidates = [
        "origin/main",
        "origin/master",
        "origin/develop",
        "origin/trunk",
    ];
    for candidate in remote_candidates {
        if refs.iter().any(|r| r == candidate) {
            return Ok(candidate.to_string());
        }
    }

    // If a local default branch exists, prefer its remote-tracking counterpart
    // so that new worktrees start from the remote tip. The remote ref may not
    // be in the local ref list yet (e.g. never fetched), but create_worktree
    // will fetch before branching, so returning "origin/<branch>" is safe.
    let local_candidates = ["main", "master", "develop", "trunk"];
    for candidate in local_candidates {
        if refs.iter().any(|r| r == candidate) {
            return Ok(format!("origin/{candidate}"));
        }
    }

    // Last resort: use "origin/main" so we always branch from the remote
    Ok("origin/main".to_string())
}

#[cfg(test)]
mod tests {
    use super::{branch_name_without_origin, origin_ref_for_branch};

    #[test]
    fn branch_name_without_origin_strips_origin_prefix() {
        assert_eq!(branch_name_without_origin("origin/main"), "main");
        assert_eq!(
            branch_name_without_origin("origin/release/2026-05"),
            "release/2026-05"
        );
        assert_eq!(branch_name_without_origin("main"), "main");
    }

    #[test]
    fn origin_ref_for_branch_normalizes_stored_base_branch() {
        assert_eq!(origin_ref_for_branch("main"), "origin/main");
        assert_eq!(origin_ref_for_branch("origin/main"), "origin/main");
        assert_eq!(
            origin_ref_for_branch("origin/release/2026-05"),
            "origin/release/2026-05"
        );
    }
}
