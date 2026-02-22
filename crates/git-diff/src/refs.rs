use crate::cli::{self, GitError};
use std::path::Path;

/// List refs (branches, tags, remotes) for autocomplete
fn list_refs(repo: &Path) -> Result<Vec<String>, GitError> {
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

/// Compute the merge-base between two refs
pub fn merge_base(repo: &Path, ref1: &str, ref2: &str) -> Result<String, GitError> {
    let output = cli::run(repo, &["merge-base", ref1, ref2])?;
    Ok(output.trim().to_string())
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
    let local_candidates = ["main", "master", "develop", "trunk"];
    for candidate in local_candidates {
        if refs.iter().any(|r| r == candidate) {
            return Ok(format!("origin/{candidate}"));
        }
    }

    // Last resort
    Ok("origin/main".to_string())
}
