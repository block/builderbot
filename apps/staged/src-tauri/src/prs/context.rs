//! Where a branch's pipelines run: the repository they belong to and the
//! working directory (local worktree or remote clone) they execute in.

use std::path::PathBuf;
use std::sync::Arc;

use crate::git;
use crate::store::{self, Store};

pub(crate) fn resolve_branch_repo_and_subpath(
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

/// Resolved context for a branch, ready to start a pipeline session.
pub(crate) struct BranchPipelineContext {
    pub(crate) branch: store::Branch,
    pub(crate) working_dir: PathBuf,
    pub(crate) workspace_name: Option<String>,
    pub(crate) remote_working_dir: Option<PathBuf>,
}

/// Resolve branch, project, and working directory for a pipeline command.
///
/// All pipeline commands (create_pr, push_branch, rebase_branch, squash_commits,
/// revert_commit)
/// share the same setup: look up the branch and project, resolve the working
/// directory (local worktree vs remote clone), and compute the remote working
/// directory for workspace-based branches.
pub(crate) fn resolve_branch_pipeline_context(
    store: &Arc<Store>,
    branch_id: &str,
) -> Result<BranchPipelineContext, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let (repo_slug, repo_subpath) = resolve_branch_repo_and_subpath(store, &project, &branch)?;

    let is_remote = branch.branch_type == store::BranchType::Remote;

    let (working_dir, workspace_name) = if is_remote {
        let clone_path = crate::paths::repos_dir()
            .map(|d| d.join(&repo_slug))
            .ok_or_else(|| "Cannot determine clone path for remote branch".to_string())?;
        (clone_path, branch.workspace_name.clone())
    } else {
        let workdir = store
            .get_workdir_for_branch(branch_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("No worktree for branch: {branch_id}"))?;

        let mut working_dir = PathBuf::from(&workdir.path);
        if let Some(subpath) = repo_subpath {
            working_dir = working_dir.join(subpath);
        }
        (working_dir, None)
    };

    // Resolve the actual workspace path for remote branches so the remote
    // agent starts in the correct repo directory.
    let remote_working_dir = if is_remote {
        branch
            .workspace_name
            .as_deref()
            .and_then(|ws| {
                crate::branches::resolve_branch_workspace_subpath(store, &branch)
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

    Ok(BranchPipelineContext {
        branch,
        working_dir,
        workspace_name,
        remote_working_dir,
    })
}

/// Derive the base branch name, stripping the `origin/` prefix if present.
///
/// GitHub's PR API expects the bare branch name, while local comparison
/// commands should use `git::origin_ref_for_branch` so the stale local base
/// branch is never consulted.
pub(super) fn base_branch_name(branch: &store::Branch) -> &str {
    git::branch_name_without_origin(&branch.base_branch)
}
