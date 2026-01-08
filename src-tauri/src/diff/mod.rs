//! Diff module - core data types and operations for the diff viewer.
//!
//! This module provides:
//! - `types`: Core data structures (DiffId, FileDiff, etc.)
//! - `git`: Git operations for computing diffs
//! - `github`: GitHub API integration for PR fetching
//! - `review`: SQLite-backed review storage

pub mod git;
pub mod github;
pub mod review;
pub mod types;

// Re-export types used by lib.rs Tauri commands
pub use git::{
    compute_diff, create_commit, fetch_pr_branch, get_merge_base, get_refs, get_repo_info,
    last_commit_message, open_repo, resolve_ref, GitRef, PRFetchResult, RepoInfo, WORKDIR,
};
pub use github::{
    check_github_auth, get_github_remote, list_pull_requests, GitHubAuthStatus, GitHubRepo,
    PullRequest,
};
pub use review::{
    export_markdown, get_store, init_store, Comment, Edit, NewComment, NewEdit, Review,
};
pub use types::{DiffId, FileDiff};
