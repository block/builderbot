use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::Emitter;

use crate::git;
use crate::session_runner;
use crate::store::{self, Store};

fn get_store(store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized — please reset from the startup prompt".into())
}

fn resolve_branch_repo_and_subpath(
    store: &Arc<Store>,
    project: &store::Project,
    branch: &store::Branch,
) -> Result<(String, Option<String>), String> {
    if let Some(repo_id) = &branch.project_repo_id {
        if let Some(repo) = store.get_project_repo(repo_id).map_err(|e| e.to_string())? {
            return Ok((repo.github_repo, repo.subpath));
        }
    }

    let repo_slug = project
        .primary_repo()
        .ok_or_else(|| format!("Project '{}' has no repository attached", project.name))?;
    Ok((repo_slug.to_string(), project.subpath.clone()))
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PrStatusEvent {
    branch_id: String,
    pr_state: String,
    pr_checks_status: String,
    pr_review_decision: Option<String>,
    pr_mergeable: bool,
    pr_draft: bool,
}

/// Create a pull request for a branch by kicking off an agent session.
#[tauri::command(rename_all = "camelCase")]
pub fn create_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    draft: Option<bool>,
) -> Result<String, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let (repo_slug, repo_subpath) = resolve_branch_repo_and_subpath(&store, &project, &branch)?;

    let is_remote = branch.branch_type == store::BranchType::Remote;

    let (working_dir, workspace_name) = if is_remote {
        let clone_path = crate::paths::repos_dir()
            .map(|d| d.join(&repo_slug))
            .ok_or_else(|| "Cannot determine clone path for remote branch".to_string())?;
        (clone_path, branch.workspace_name.clone())
    } else {
        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut working_dir = PathBuf::from(&workdir.path);
        if let Some(subpath) = repo_subpath {
            working_dir = working_dir.join(subpath);
        }
        (working_dir, None)
    };

    let base_branch = branch
        .base_branch
        .strip_prefix("origin/")
        .unwrap_or(&branch.base_branch);

    // Use origin/{base_branch} in git commands to ensure we're comparing against the remote
    // branch, not the local tracking branch. Local branches can be stale if not kept in sync,
    // which causes incorrect PR diffs that include already-merged commits. Remote-tracking
    // refs are always up-to-date after fetch.
    let is_draft = draft.unwrap_or(false);
    let draft_flag = if is_draft { " --draft" } else { "" };
    let pr_type = if is_draft {
        "draft pull request"
    } else {
        "pull request"
    };

    let prompt = format!(
        r#"<action>
Create a {pr_type} for the current branch.

Steps:
1. First, look at the diff between the current branch and when it branched off of the base branch `{base_branch}` to understand all changes. Use `git log --oneline origin/{base_branch}..HEAD` and `git diff origin/{base_branch}...HEAD --stat` to see what changed.
2. Push the current branch to the remote: `git push -u origin {branch_name}`
3. Create a PR using the GitHub CLI: `gh pr create --base {base_branch} --fill-first{draft_flag}`
   - The title MUST use conventional commit style (e.g., "feat: add user authentication", "fix: resolve null pointer in parser", "refactor: extract validation logic")
   - Choose the most appropriate conventional commit type (feat, fix, refactor, docs, style, test, chore, perf, ci, build) based on the actual changes
   - The body should be a concise summary of the changes

IMPORTANT: After creating the PR, you MUST output the PR URL on its own line in this exact format:
PR_URL: https://github.com/...

This is critical - the application parses this to link the PR.
</action>"#,
        pr_type = pr_type,
        base_branch = base_branch,
        branch_name = branch.branch_name,
        draft_flag = draft_flag,
    );

    let mut session = store::Session::new_running(&prompt, &working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    // Resolve the actual workspace path for remote branches so the remote
    // agent starts in the correct repo directory.
    let remote_working_dir = if is_remote {
        branch
            .workspace_name
            .as_deref()
            .and_then(|ws| {
                crate::branches::resolve_branch_workspace_subpath(&store, &branch)
                    .ok()
                    .flatten()
                    .and_then(|subpath| {
                        crate::branches::resolve_workspace_repo_path(ws, &subpath).ok()
                    })
            })
            .map(PathBuf::from)
    } else {
        None
    };

    session_runner::start_session(
        session_runner::SessionConfig {
            session_id: session.id.clone(),
            prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha: None,
            provider,
            workspace_name,
            extra_env: vec![],
            mcp_project_id: None,
            action_executor: None,
            action_registry: None,
            remote_working_dir,
            image_ids: vec![],
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(session.id)
}

/// Build the GitHub PR URL for a branch from its repo slug and PR number.
#[tauri::command(rename_all = "camelCase")]
pub fn get_pr_url(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    pr_number: u64,
) -> Result<String, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let (repo_slug, _) = resolve_branch_repo_and_subpath(&store, &project, &branch)?;
    let parts: Vec<&str> = repo_slug.splitn(2, '/').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid github_repo format: {}", repo_slug));
    }
    let (owner, repo_name) = (parts[0], parts[1]);

    Ok(format!(
        "https://github.com/{owner}/{repo_name}/pull/{pr_number}"
    ))
}

/// Update the PR number for a branch.
#[tauri::command(rename_all = "camelCase")]
pub fn update_branch_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    pr_number: Option<u64>,
) -> Result<(), String> {
    get_store(&store)?
        .update_branch_pr_number(&branch_id, pr_number)
        .map_err(|e| e.to_string())
}

/// Refresh PR status for a single branch.
#[tauri::command(rename_all = "camelCase")]
pub async fn refresh_pr_status(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;
    let pr_number = branch
        .pr_number
        .ok_or_else(|| "Branch does not have an associated PR".to_string())?;
    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;
    let (github_repo, _) = resolve_branch_repo_and_subpath(&store, &project, &branch)?;

    let pr_status =
        git::fetch_pr_status_for_repo(&github_repo, pr_number).map_err(|e| e.to_string())?;
    let mergeable = pr_status.mergeable == "MERGEABLE";

    store
        .update_branch_pr_status(
            &branch_id,
            Some(pr_status.state.clone()),
            Some(pr_status.checks_summary.state.clone()),
            pr_status.review_decision.clone(),
            Some(mergeable),
            Some(pr_status.is_draft),
            None,
            None,
        )
        .map_err(|e| e.to_string())?;

    app_handle
        .emit(
            "pr-status-changed",
            PrStatusEvent {
                branch_id: branch_id.clone(),
                pr_state: pr_status.state,
                pr_checks_status: pr_status.checks_summary.state,
                pr_review_decision: pr_status.review_decision,
                pr_mergeable: mergeable,
                pr_draft: pr_status.is_draft,
            },
        )
        .map_err(|e| format!("Failed to emit event: {}", e))?;

    Ok(())
}

/// Refresh PR status for all branches in a project.
#[tauri::command(rename_all = "camelCase")]
pub async fn refresh_all_pr_statuses(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    project_id: String,
) -> Result<u32, String> {
    let store = get_store(&store)?;
    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let branches = store
        .list_branches_for_project(&project_id)
        .map_err(|e| e.to_string())?;
    let branches_with_prs: Vec<_> = branches
        .into_iter()
        .filter(|b| b.pr_number.is_some())
        .collect();

    let mut refreshed_count = 0u32;

    for branch in branches_with_prs {
        let pr_number = branch.pr_number.unwrap();
        let github_repo = match resolve_branch_repo_and_subpath(&store, &project, &branch) {
            Ok((repo, _)) => repo,
            Err(e) => {
                log::warn!(
                    "Failed to resolve repo for branch {} (PR #{}): {}",
                    branch.id,
                    pr_number,
                    e
                );
                continue;
            }
        };

        match git::fetch_pr_status_for_repo(&github_repo, pr_number) {
            Ok(pr_status) => {
                let mergeable = pr_status.mergeable == "MERGEABLE";

                if let Err(e) = store.update_branch_pr_status(
                    &branch.id,
                    Some(pr_status.state.clone()),
                    Some(pr_status.checks_summary.state.clone()),
                    pr_status.review_decision.clone(),
                    Some(mergeable),
                    Some(pr_status.is_draft),
                    None,
                    None,
                ) {
                    log::warn!("Failed to update PR status for branch {}: {}", branch.id, e);
                    continue;
                }

                refreshed_count += 1;

                if let Err(e) = app_handle.emit(
                    "pr-status-changed",
                    PrStatusEvent {
                        branch_id: branch.id.clone(),
                        pr_state: pr_status.state,
                        pr_checks_status: pr_status.checks_summary.state,
                        pr_review_decision: pr_status.review_decision,
                        pr_mergeable: mergeable,
                        pr_draft: pr_status.is_draft,
                    },
                ) {
                    log::warn!("Failed to emit pr-status-changed event: {}", e);
                }
            }
            Err(e) => {
                log::warn!(
                    "Failed to fetch PR status for branch {} (PR #{}): {}",
                    branch.id,
                    pr_number,
                    e
                );
            }
        }
    }

    app_handle
        .emit("pr-statuses-refreshed", &project_id)
        .map_err(|e| format!("Failed to emit event: {}", e))?;

    Ok(refreshed_count)
}

/// Clear stale PR status fields for a branch (e.g. after a push invalidates them).
///
/// This nulls out checks, mergeable, review-decision, etc. in the DB and emits
/// a `pr-status-cleared` event so the frontend can drop the stale indicators
/// immediately instead of waiting for the next GitHub refresh.
#[tauri::command(rename_all = "camelCase")]
pub fn clear_branch_pr_status(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;

    store
        .update_branch_pr_status(&branch_id, None, None, None, None, None, None, None)
        .map_err(|e| e.to_string())?;

    app_handle
        .emit("pr-status-cleared", &branch_id)
        .map_err(|e| format!("Failed to emit event: {}", e))?;

    Ok(())
}

/// Check if a branch has commits that haven't been pushed to the remote.
#[tauri::command(rename_all = "camelCase")]
pub fn has_unpushed_commits(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<bool, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if branch.branch_type == store::BranchType::Remote {
        let workspace_name = branch
            .workspace_name
            .as_deref()
            .ok_or_else(|| format!("Branch has no workspace name: {branch_id}"))?;
        let repo_subpath = crate::branches::resolve_branch_workspace_subpath(&store, &branch)?;

        let remote_ref = format!("origin/{}", branch.branch_name);
        // Check that the remote tracking branch exists
        if crate::branches::run_workspace_git(
            workspace_name,
            repo_subpath.as_deref(),
            &["rev-parse", "--verify", &remote_ref],
        )
        .is_err()
        {
            return Ok(false);
        }
        let rev_range = format!("{remote_ref}..HEAD");
        let output = crate::branches::run_workspace_git(
            workspace_name,
            repo_subpath.as_deref(),
            &["rev-list", &rev_range],
        )
        .map_err(|e| e.to_string())?;
        return Ok(!output.trim().is_empty());
    }

    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

    git::has_unpushed_commits(Path::new(&workdir.path), &branch.branch_name)
        .map_err(|e| e.to_string())
}

/// Push a branch to its remote by kicking off an agent session.
#[tauri::command(rename_all = "camelCase")]
pub fn push_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    registry: tauri::State<'_, Arc<session_runner::SessionRegistry>>,
    app_handle: tauri::AppHandle,
    branch_id: String,
    provider: Option<String>,
    force: Option<bool>,
) -> Result<String, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;
    let (repo_slug, repo_subpath) = resolve_branch_repo_and_subpath(&store, &project, &branch)?;

    let is_remote = branch.branch_type == store::BranchType::Remote;

    let (working_dir, workspace_name) = if is_remote {
        let clone_path = crate::paths::repos_dir()
            .map(|d| d.join(repo_slug))
            .ok_or_else(|| "Cannot determine clone path for remote branch".to_string())?;
        (clone_path, branch.workspace_name.clone())
    } else {
        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut working_dir = PathBuf::from(&workdir.path);
        if let Some(subpath) = repo_subpath {
            working_dir = working_dir.join(subpath);
        }
        (working_dir, None)
    };

    let force = force.unwrap_or(false);

    let prompt = if force {
        format!(
            r#"<action>
Push the current branch to the remote using force-with-lease.

Run: `git push -u origin {branch_name} --force-with-lease`

If the push fails due to pre-push hook errors, read the error output, fix the underlying issue, and retry the push.

The push must succeed before you finish.
</action>"#,
            branch_name = branch.branch_name,
        )
    } else {
        format!(
            r#"<action>
Push the current branch to the remote.

Run: `git push -u origin {branch_name}`

IMPORTANT: You MUST NOT use --force, --force-with-lease, or any force-push variant. Only a normal push is allowed.

If the push fails due to pre-push hook errors, read the error output, fix the underlying issue, and retry the push.

If the push is rejected because the remote has commits that would be lost (non-fast-forward rejection), do NOT attempt to fix it. Instead, output the following marker on its own line and stop:
PUSH_REJECTED: NON_FAST_FORWARD

For any other failure, diagnose the problem and fix it, then retry the push.

The push must succeed before you finish (unless you output the non-fast-forward marker above).
</action>"#,
            branch_name = branch.branch_name,
        )
    };

    let mut session = store::Session::new_running(&prompt, &working_dir);
    if let Some(ref p) = provider {
        session = session.with_provider(p);
    }
    store.create_session(&session).map_err(|e| e.to_string())?;

    // Resolve the actual workspace path for remote branches so the remote
    // agent starts in the correct repo directory.
    let remote_working_dir = if is_remote {
        branch
            .workspace_name
            .as_deref()
            .and_then(|ws| {
                crate::branches::resolve_branch_workspace_subpath(&store, &branch)
                    .ok()
                    .flatten()
                    .and_then(|subpath| {
                        crate::branches::resolve_workspace_repo_path(ws, &subpath).ok()
                    })
            })
            .map(PathBuf::from)
    } else {
        None
    };

    session_runner::start_session(
        session_runner::SessionConfig {
            session_id: session.id.clone(),
            prompt,
            working_dir,
            agent_session_id: None,
            pre_head_sha: None,
            provider,
            workspace_name,
            extra_env: vec![],
            mcp_project_id: None,
            action_executor: None,
            action_registry: None,
            remote_working_dir,
            image_ids: vec![],
        },
        store,
        app_handle,
        Arc::clone(&registry),
    )?;

    Ok(session.id)
}
