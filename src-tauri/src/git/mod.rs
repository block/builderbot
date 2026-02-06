mod cli;
mod commit;
mod diff;
mod files;
pub mod github;
mod refs;
mod types;
mod worktree;

pub use cli::GitError;
pub use commit::commit;
pub use diff::{get_file_diff, get_unified_diff, list_diff_files};
pub use files::{get_file_at_ref, search_files};
pub use github::{
    check_github_auth, create_pull_request, fetch_pr, get_pr_for_branch,
    invalidate_cache as invalidate_pr_cache, list_pull_requests, push_branch, search_pull_requests,
    sync_review_to_github, update_pull_request, CreatePrResult, GitHubAuthStatus, GitHubSyncResult,
    PullRequest, PullRequestInfo,
};
pub use refs::{
    detect_default_branch, get_repo_root, list_branches, list_refs, merge_base, resolve_ref,
    BranchRef,
};
pub use types::*;
pub use worktree::{
    branch_exists, create_worktree, create_worktree_from_pr, get_commits_since_base, get_head_sha,
    get_parent_commit, list_worktrees, remove_worktree, reset_to_commit, update_branch_from_pr,
    worktree_path_for, CommitInfo, UpdateFromPrResult,
};
