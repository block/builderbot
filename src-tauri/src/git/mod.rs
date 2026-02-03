mod cli;
mod commit;
mod diff;
mod files;
pub mod github;
mod refs;
mod types;

pub use cli::GitError;
pub use commit::commit;
pub use diff::{get_file_diff, get_unified_diff, list_diff_files};
pub use files::{get_file_at_ref, search_files};
pub use github::{
    check_github_auth, fetch_pr, invalidate_cache as invalidate_pr_cache, list_pull_requests,
    search_pull_requests, sync_review_to_github, GitHubAuthStatus, GitHubSyncResult, PullRequest,
};
pub use refs::{detect_default_branch, get_repo_root, list_refs, merge_base, resolve_ref};
pub use types::*;
