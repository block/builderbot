//! GitHub commands — thin wrappers around `git::*` for the frontend.

use crate::git;
use crate::paths;
use crate::store::{self, Store};
use std::sync::{Arc, Mutex};

/// List the authenticated user's GitHub organization memberships.
#[tauri::command]
pub async fn list_github_orgs() -> Result<Vec<String>, String> {
    git::list_github_orgs().map_err(|e| e.to_string())
}

/// List GitHub repositories for the authenticated user or a specific owner.
#[tauri::command]
pub async fn list_github_repos(owner: Option<String>) -> Result<Vec<git::GitHubRepo>, String> {
    git::list_github_repos(owner.as_deref()).map_err(|e| e.to_string())
}

/// List repositories the authenticated user has recently pushed to.
/// Returns repos across all orgs, sorted by most recently pushed.
#[tauri::command]
pub async fn list_user_repos(limit: Option<u32>) -> Result<Vec<git::GitHubRepo>, String> {
    git::list_user_repos(limit.unwrap_or(30)).map_err(|e| e.to_string())
}

/// Fetch a single GitHub repository by owner/repo.
/// Returns None if the repo doesn't exist or user lacks access.
#[tauri::command]
pub async fn get_github_repo(
    owner: String,
    repo: String,
) -> Result<Option<git::GitHubRepo>, String> {
    git::fetch_github_repo(&owner, &repo).map_err(|e| e.to_string())
}

/// Search GitHub repositories for the authenticated user or a specific owner.
#[tauri::command]
pub async fn search_github_repos(
    query: String,
    owner: Option<String>,
) -> Result<Vec<git::GitHubRepo>, String> {
    git::search_github_repos(&query, owner.as_deref()).map_err(|e| e.to_string())
}

/// Check if a repository is likely a monorepo by counting modules in MODULES.yaml.
/// Returns the module count (0 if file doesn't exist).
#[tauri::command]
pub async fn check_monorepo_modules(github_repo: String) -> Result<u32, String> {
    git::check_monorepo_modules(&github_repo).map_err(|e| e.to_string())
}

/// Validate that a subpath exists as a directory in a GitHub repository.
#[tauri::command]
pub async fn validate_subpath(github_repo: String, subpath: String) -> Result<(), String> {
    git::validate_subpath_in_repo(&github_repo, &subpath).map_err(|e| e.to_string())
}

/// List directories at a given path in a GitHub repository.
/// Returns directory names (not files) at the specified path.
#[tauri::command]
pub async fn list_repo_directories(
    github_repo: String,
    path: String,
) -> Result<Vec<String>, String> {
    git::list_repo_directories(&github_repo, &path).map_err(|e| e.to_string())
}

/// List branches for a repo via GitHub API (no local clone needed).
#[tauri::command(rename_all = "camelCase")]
pub async fn list_git_branches(github_repo: String) -> Result<Vec<git::BranchRef>, String> {
    git::list_branches_for_repo(&github_repo).map_err(|e| e.to_string())
}

/// Detect default branch via GitHub API (no local clone needed).
#[tauri::command(rename_all = "camelCase")]
pub async fn detect_default_branch_cmd(github_repo: String) -> Result<String, String> {
    git::detect_default_branch_for_repo(&github_repo).map_err(|e| e.to_string())
}

/// Prune stale remote-tracking refs. With GitHub-repo-based projects,
/// branch listing uses the API directly, so this is a no-op.
#[tauri::command(rename_all = "camelCase")]
pub async fn prune_remote_refs(github_repo: String) -> Result<(), String> {
    git::prune_remote_for_repo(&github_repo).map_err(|e| e.to_string())
}

/// Check if a local branch already exists in the project's local clone.
///
/// Used for "new branch" modal copy so users can intentionally attach to
/// existing local branches.
#[tauri::command(rename_all = "camelCase")]
pub async fn check_existing_local_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    branch_name: String,
) -> Result<bool, String> {
    let store = crate::get_store(&store)?;
    let branch_name = branch_name.trim();
    if branch_name.is_empty() {
        return Ok(false);
    }

    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    let Some(repo_path) = project.primary_repo().and_then(paths::clone_path_for) else {
        return Ok(false);
    };

    if !repo_path.exists() {
        return Ok(false);
    }

    match git::branch_exists(&repo_path, branch_name) {
        Ok(exists) => Ok(exists),
        Err(e) => {
            log::debug!(
                "check_existing_local_branch failed for '{}': {e}",
                crate::branches::project_primary_repo(&project).unwrap_or("<no-primary-repo>")
            );
            Ok(false)
        }
    }
}

/// Fetch a single pull request by number (via `-R owner/repo`).
#[tauri::command(rename_all = "camelCase")]
pub async fn get_pr_for_repo(
    github_repo: String,
    pr_number: u64,
) -> Result<git::github::PullRequest, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::github::get_pr_for_repo(&github_repo, pr_number).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Find the open PR (if any) whose head branch matches `branch_name`.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_pr_for_branch(
    github_repo: String,
    branch_name: String,
) -> Result<Option<git::github::PullRequest>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::github::get_pr_for_branch_for_repo(&github_repo, &branch_name)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// List open pull requests for a repository (via `-R owner/repo`).
#[tauri::command(rename_all = "camelCase")]
pub async fn list_pull_requests(
    github_repo: String,
) -> Result<Vec<git::github::PullRequest>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::list_pull_requests_for_repo(&github_repo).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// If `github_repo` is a fork, return the parent repo slug (e.g. `"base-owner/repo"`).
/// Returns `null` when the repo is not a fork.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_parent_repo(github_repo: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::github::get_parent_repo(&github_repo).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// List open issues for a repository (via `-R owner/repo`).
#[tauri::command(rename_all = "camelCase")]
pub async fn list_issues(github_repo: String) -> Result<Vec<git::github::Issue>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::list_issues_for_repo(&github_repo).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Post or update a single review comment on a GitHub PR.
///
/// If the comment already has a `github_comment_id` (i.e. it was previously
/// posted), the existing GitHub comment is updated. Otherwise a new comment
/// is created. The resulting GitHub comment ID is persisted in the local DB
/// so subsequent edits can update in-place.
#[tauri::command(rename_all = "camelCase")]
pub async fn post_comment_to_github(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    pr_number: u64,
    comment: store::Comment,
    local_head_sha: String,
) -> Result<git::GitHubCommentResult, String> {
    let store = crate::get_store(&store)?;
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let repo_slug = if let Some(repo_id) = &branch.project_repo_id {
        store
            .get_project_repo(repo_id)
            .map_err(|e| e.to_string())?
            .map(|r| r.github_repo)
            .unwrap_or_else(|| project.primary_repo().unwrap_or_default().to_string())
    } else {
        project
            .primary_repo()
            .ok_or_else(|| format!("Project '{}' has no repository attached", project.name))?
            .to_string()
    };

    let repo_path = paths::clone_path_for(&repo_slug)
        .ok_or_else(|| "Cannot determine clone path".to_string())?;

    let comment_id = comment.id.clone();

    let result = if let (Some(gh_id), Some(gh_type)) = (
        comment.github_comment_id,
        comment.github_comment_type.as_deref(),
    ) {
        let body = git::github::github_single_comment_body(&comment);
        let body = if gh_type == "issue" {
            git::github::github_issue_comment_body(&comment, &body)
        } else {
            body
        };

        git::update_comment_on_github(&repo_path, pr_number, gh_id, gh_type, &body)
            .await
            .map_err(|e| e.to_string())?
    } else {
        git::post_single_comment_to_github(&repo_path, pr_number, &comment, &local_head_sha)
            .await
            .map_err(|e| e.to_string())?
    };

    // Persist the GitHub comment ID so future edits can update in-place
    store
        .set_github_comment(&comment_id, result.comment_id, &result.comment_type)
        .map_err(|e| e.to_string())?;

    Ok(result)
}
