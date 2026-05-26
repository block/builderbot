mod cli;
mod commit;
pub mod config_apply;
mod diff;
mod env;
mod files;
pub mod github;
mod refs;
mod state;
#[cfg(test)]
mod state_tests;
mod status_parse;
mod types;
mod worktree;

pub(crate) use cli::run as cli_run;
pub use cli::GitError;
pub use commit::commit;
pub use diff::{get_file_diff, get_unified_diff, list_diff_files};
pub(crate) use env::strip_git_env;
pub use files::{get_file_at_ref, search_files};
pub use github::{
    check_github_auth, check_monorepo_modules, create_pull_request, detect_default_branch_for_repo,
    ensure_local_clone, ensure_local_clone_with_progress, fetch_for_worktree, fetch_github_repo,
    fetch_pr, fetch_pr_status, fetch_pr_status_for_repo, fetch_pr_url, get_pr_for_branch,
    invalidate_cache as invalidate_pr_cache, list_branches_for_repo, list_github_orgs,
    list_github_repos, list_issues, list_issues_for_repo, list_pull_requests,
    list_pull_requests_for_repo, list_repo_directories, list_user_repos,
    post_single_comment_to_github, prune_remote_for_repo, push_branch, resolve_default_branch,
    search_github_repos, search_issues, search_pull_requests, sync_review_to_github,
    update_clone_to_remote_head, update_comment_on_github, update_pull_request,
    validate_subpath_in_repo, ChecksSummary, CreatePrResult, FailedCheck, GitHubAuthStatus,
    GitHubCommentResult, GitHubRepo, GitHubSyncResult, Issue, PrStatus, PullRequest,
    PullRequestInfo,
};
pub use refs::{
    branch_name_without_origin, detect_default_branch, detect_default_branch_from_remote,
    get_current_branch, get_remote_url, get_repo_root, list_branches, list_refs, merge_base,
    origin_ref_for_branch, prune_remote, resolve_ref, BranchRef,
};
pub use state::{
    complete_local_git_state, compute_branch_git_state, compute_branch_git_state_batched,
    compute_fast_git_state_batched, compute_fast_local_git_state, compute_local_branch_git_state,
    ensure_fast_forward_pullable, fast_forward_to_ref, local_git_state_cache_key, needs_fetch,
    update_repo_fetch_cache, BaseGitState, BranchGitState, FastGitState, FetchGitState, FetchMode,
    FetchStatus, UpstreamGitState, UpstreamRelation, WorktreeGitState, WorktreeStatusScope,
};
pub use types::*;
pub use worktree::{
    branch_exists, create_worktree, create_worktree_at_path, create_worktree_for_existing_branch,
    create_worktree_for_existing_branch_at_path, create_worktree_from_pr,
    create_worktree_from_pr_at_path, discard_worktree_changes, fetch_pr_head_sha,
    get_commits_since_base, get_full_commit_log, get_head_sha, get_parent_commit,
    has_unpushed_commits, list_worktree_change_paths, list_worktrees, parse_worktree_status_paths,
    project_worktree_path_for, project_worktree_root_for, remote_branch_exists, remove_worktree,
    reset_to_commit, set_upstream_to_origin, switch_branch, update_branch_from_pr,
    worktree_path_for, CommitInfo, UpdateFromPrResult, WorktreeChangePaths,
};
