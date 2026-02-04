use super::cli::{self, GitError};
use std::path::Path;

/// Get the absolute path to the repository root.
pub fn get_repo_root(repo: &Path) -> Result<String, GitError> {
    let output = cli::run(repo, &["rev-parse", "--show-toplevel"])?;
    Ok(output.trim().to_string())
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

/// List branches (local and remote) for base branch selection.
/// Returns branches sorted with local first, then remote.
/// Filters out HEAD references.
pub fn list_branches(repo: &Path) -> Result<Vec<BranchRef>, GitError> {
    let output = cli::run(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;

    let mut branches: Vec<BranchRef> = output
        .lines()
        .filter(|s| !s.is_empty() && !s.ends_with("/HEAD"))
        .map(|name| {
            let is_remote = name.contains('/');
            let remote = if is_remote {
                name.split('/').next().map(String::from)
            } else {
                None
            };
            BranchRef {
                name: name.to_string(),
                is_remote,
                remote,
            }
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

/// Resolve a ref to its full SHA
pub fn resolve_ref(repo: &Path, reference: &str) -> Result<String, GitError> {
    let output = cli::run(repo, &["rev-parse", reference])?;
    Ok(output.trim().to_string())
}

/// Detect the default branch for this repository.
/// Checks for common default branch names in order of preference.
/// Returns the remote-tracking branch (e.g., "origin/main") if available,
/// otherwise falls back to local branch name.
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

    // Fall back to local branches
    let local_candidates = ["main", "master", "develop", "trunk"];
    for candidate in local_candidates {
        if refs.iter().any(|r| r == candidate) {
            return Ok(candidate.to_string());
        }
    }

    // Last resort: use "main"
    Ok("main".to_string())
}
