//! Shared project command implementations used by both Tauri commands and MCP tools.

use std::sync::Arc;

use crate::store::{self, Store};
use crate::{blox, branches, git};

/// Core logic for adding a GitHub repository to a project.
///
/// Called by both the `add_project_repo` Tauri command and the MCP tool.
pub(crate) async fn add_project_repo_impl(
    store: Arc<Store>,
    project_id: String,
    github_repo: String,
    branch_name: Option<String>,
    subpath: Option<String>,
    set_as_primary: Option<bool>,
    reason: Option<String>,
) -> Result<store::ProjectRepo, String> {
    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let resolved_branch_name = branch_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| branches::infer_branch_name(&project.name));
    // Validate that the subpath exists as a directory in the repo before
    // creating anything. This prevents repos being added with invalid
    // subpaths that would fail later during worktree setup.
    if let Some(sub) = &subpath {
        git::validate_subpath_in_repo(&github_repo, sub).map_err(|e| e.to_string())?;
    }

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

    let detected_base = git::detect_default_branch_for_repo(&repo.github_repo)
        .unwrap_or_else(|_| "main".to_string());
    let effective_base = if detected_base.starts_with("origin/") {
        detected_base
    } else {
        format!("origin/{detected_base}")
    };
    let branch = match project.location {
        store::ProjectLocation::Local => {
            store::Branch::new(&project_id, &repo.branch_name, &effective_base)
                .with_project_repo(&repo.id)
        }
        store::ProjectLocation::Remote => {
            let ws_name = branches::resolve_project_workspace_name(&store, &project, None)?;
            store::Branch::new_remote(&project_id, &repo.branch_name, &effective_base, &ws_name)
                .with_project_repo(&repo.id)
        }
    };
    store.create_branch(&branch).map_err(|e| e.to_string())?;
    Ok(repo)
}
