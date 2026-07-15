//! Shared project command implementations used by both Tauri commands and MCP tools.

use std::sync::Arc;

use crate::store::repo_affinities::repo_affinity_key;
use crate::store::{self, Store};
use crate::{blox, branches, git};

/// Core logic for adding a GitHub repository to a project.
///
/// Called by both the `add_project_repo` Tauri command and the MCP tool.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn add_project_repo_impl(
    store: Arc<Store>,
    project_id: String,
    github_repo: String,
    branch_name: Option<String>,
    subpath: Option<String>,
    set_as_primary: Option<bool>,
    reason: Option<String>,
    pr_number: Option<u64>,
    default_branch: Option<String>,
    head_repo: Option<String>,
) -> Result<store::ProjectRepo, String> {
    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let mut resolved_branch_name = branch_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| branches::infer_prefixed_branch_name(&project.name));
    // If this github_repo is already attached to the project (i.e. being added
    // again with a different subpath), make the branch name unique by appending
    // a subpath-derived suffix.  Without this, `git worktree add -b <branch>`
    // would fail because the branch is already checked out in the first worktree.
    let existing_repos = store
        .list_project_repos(&project_id)
        .map_err(|e| e.to_string())?;
    let repo_already_attached = existing_repos.iter().any(|r| r.github_repo == github_repo);
    if repo_already_attached && branch_name.is_none() {
        let suffix = match &subpath {
            Some(sub) => sub.trim_matches('/').replace('/', "-"),
            None => "root".to_owned(),
        };
        resolved_branch_name = format!("{resolved_branch_name}-{suffix}");
    }
    // Subpath validation is handled by the frontend before submission
    // (SubpathInput.waitForValidation). Skipping the redundant backend
    // re-validation here removes a ~500ms-2s GitHub API round-trip.

    let repo_subpath = if project.location == store::ProjectLocation::Remote {
        // For remote repos, store the user's subpath as-is (relative to repo
        // root), or None when no subpath was provided. The workspace clone
        // directory is derived from `github_repo` at read time.
        subpath
            .as_deref()
            .map(branches::validate_workspace_subpath)
            .transpose()?
    } else {
        subpath.clone()
    };
    let mut repo = store::ProjectRepo::new(
        &project_id,
        &github_repo,
        &resolved_branch_name,
        repo_subpath,
    );
    if set_as_primary.unwrap_or(false) {
        repo = repo.primary();
    }
    repo.reason = reason;
    repo.head_repo = head_repo;
    if project.location == store::ProjectLocation::Remote {
        let ws_name = branches::resolve_project_workspace_name(&store, &project, None)?;
        let ws_info = tauri::async_runtime::spawn_blocking({
            let ws_name = ws_name.clone();
            move || blox::ws_info(&ws_name)
        })
        .await
        .map_err(|e| format!("Failed to query workspace '{ws_name}': {e}"))?
        .map_err(|e| {
            format!("Workspace '{ws_name}' must be running before adding another repo: {e}")
        })?;
        let ws_status = ws_info
            .status
            .as_deref()
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if ws_status != "running" {
            return Err(format!(
                "Workspace '{ws_name}' is not ready (status: {}). Wait until it is running, then retry.",
                if ws_status.is_empty() {
                    "unknown"
                } else {
                    ws_status.as_str()
                }
            ));
        }
    }

    store
        .create_project_repo(&repo)
        .map_err(|e| e.to_string())?;

    store
        .record_recent_repo(&github_repo, subpath.clone())
        .map_err(|e| e.to_string())?;

    // Record repo affinity for each existing repo in the project.
    let new_key = repo_affinity_key(&github_repo, repo.subpath.as_deref());
    for existing in &existing_repos {
        let existing_key = repo_affinity_key(&existing.github_repo, existing.subpath.as_deref());
        if let Err(e) = store.record_repo_affinity(&new_key, &existing_key) {
            log::warn!("Failed to record repo affinity: {e}");
        }
    }

    let should_be_primary = repo.is_primary
        || store
            .get_primary_project_repo(&project_id)
            .map_err(|e| e.to_string())?
            .is_none();
    if should_be_primary {
        store
            .set_primary_project_repo(&project_id, &repo.id)
            .map_err(|e| e.to_string())?;
        store
            .update_project(
                &project_id,
                &project.name,
                Some(&repo.github_repo),
                &project.location,
                subpath.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        repo.is_primary = true;
    }

    // Use the frontend-prefetched default branch when available to avoid
    // a ~500ms-2s GitHub API round-trip.
    let effective_base = git::resolve_default_branch(default_branch, &repo.github_repo);
    let branch = match project.location {
        store::ProjectLocation::Local => {
            let mut b = store::Branch::new(&project_id, &repo.branch_name, &effective_base)
                .with_project_repo(&repo.id);
            if let Some(pr) = pr_number {
                b = b.with_pr(pr);
            }
            b
        }
        store::ProjectLocation::Remote => {
            let ws_name = branches::resolve_project_workspace_name(&store, &project, None)?;
            let mut b = store::Branch::new_remote(
                &project_id,
                &repo.branch_name,
                &effective_base,
                &ws_name,
            )
            .with_project_repo(&repo.id);
            if let Some(pr) = pr_number {
                b = b.with_pr(pr);
            }
            b
        }
    };
    store.create_branch(&branch).map_err(|e| e.to_string())?;
    Ok(repo)
}
