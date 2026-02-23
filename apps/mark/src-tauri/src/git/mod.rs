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
    check_github_auth, check_monorepo_modules, create_pull_request, detect_default_branch_for_repo,
    ensure_local_clone, fetch_for_worktree, fetch_github_repo, fetch_pr, fetch_pr_status,
    fetch_pr_status_for_repo, get_pr_for_branch, invalidate_cache as invalidate_pr_cache,
    list_branches_for_repo, list_github_orgs, list_github_repos, list_issues, list_issues_for_repo,
    list_pull_requests, list_pull_requests_for_repo, list_user_repos, prune_remote_for_repo,
    push_branch, search_github_repos, search_issues, search_pull_requests, sync_review_to_github,
    update_pull_request, ChecksSummary, CreatePrResult, GitHubAuthStatus, GitHubRepo,
    GitHubSyncResult, Issue, PrStatus, PullRequest, PullRequestInfo,
};
pub use refs::{
    detect_default_branch, get_current_branch, get_remote_url, get_repo_root, list_branches,
    list_refs, merge_base, prune_remote, resolve_ref, BranchRef,
};
pub use types::*;
pub use worktree::{
    branch_exists, create_worktree, create_worktree_at_path, create_worktree_for_existing_branch,
    create_worktree_for_existing_branch_at_path, create_worktree_from_pr,
    create_worktree_from_pr_at_path, get_commits_since_base, get_full_commit_log, get_head_sha,
    get_parent_commit, has_unpushed_commits, list_worktrees, project_worktree_path_for,
    project_worktree_root_for, remove_worktree, reset_to_commit, switch_branch,
    update_branch_from_pr, worktree_path_for, CommitInfo, UpdateFromPrResult,
};
