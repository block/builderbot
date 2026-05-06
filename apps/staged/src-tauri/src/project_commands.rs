//! Shared project command implementations used by Tauri commands, the web
//! transport and MCP tools.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::Manager;

use crate::store::repo_affinities::repo_affinity_key;
use crate::store::{self, Store};
use crate::{blox, branches, git};

/// A repo just added to a project.
pub(crate) struct AddedProjectRepo {
    pub repo: store::ProjectRepo,
    /// Whether the repo's branch provisions the project's Blox workspace: the
    /// first repo of a remote project created without one.
    pub provisions_workspace: bool,
}

/// Rename a project, shared by the `rename_project` Tauri command and the
/// web transport.
///
/// A metadata-only write: the `projects.name` column changes and the store
/// publishes `project-changed`. Branch names, worktrees, Blox workspaces and
/// repo branch names keep whatever was inferred from the old name when they
/// were created, so nothing on disk moves. It is not gated on an in-flight
/// transfer: a transfer rewrites `location`, which this write never carries.
pub(crate) fn rename_project_impl(
    store: &Store,
    project_id: &str,
    name: &str,
) -> Result<store::Project, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Project name is required".to_string());
    }
    store
        .rename_project(project_id, trimmed)
        .map_err(|e| e.to_string())?;
    store
        .get_project(project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))
}

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
) -> Result<AddedProjectRepo, String> {
    crate::project_transfer::ensure_project_not_transferring(&project_id)?;
    // Use the frontend-prefetched default branch when available to avoid
    // a ~500ms-2s GitHub API round-trip. The fallback runs `gh`, so it goes
    // to the blocking pool. It is resolved before the project's repos are
    // read: a `gh` wait between that read and the writes below would let a
    // parallel add of the same repo (MCP tool calls, typically) miss this
    // one and pick the same branch name. It also keeps the wait away from
    // the writes, since the web transport drops a request future at its
    // next await when the client goes away.
    let effective_base = {
        let github_repo = github_repo.clone();
        tauri::async_runtime::spawn_blocking(move || {
            git::resolve_default_branch(default_branch, &github_repo)
        })
        .await
        .map_err(|e| e.to_string())?
    };

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
    // A remote project's repos share one Blox workspace, and a repo joining
    // it is cloned into it, so it must be running. A project created without
    // a repo has no workspace yet: this repo's branch provisions it, created
    // `Starting` like `create_project`'s initial branch and started the same
    // way (`start_workspace`, with this repo as the source), and named the
    // way `create_project` names it: after that branch. The claim is held
    // until that branch row is written.
    let (remote_workspace, provisioning_claim) =
        if project.location == store::ProjectLocation::Remote {
            match claim_remote_workspace(&store, &project)? {
                RemoteWorkspace::Provision(claim) => (
                    Some(branches::infer_workspace_name(
                        &project_id,
                        &repo.branch_name,
                    )),
                    Some(claim),
                ),
                RemoteWorkspace::Existing(ws_name) => {
                    ensure_workspace_running(&ws_name).await?;
                    (Some(ws_name), None)
                }
            }
        } else {
            (None, None)
        };

    // Again, now the awaits are behind us: a transfer that began during them
    // reads the project's repos without this one and switches the location
    // read above, and one that already finished switched it.
    crate::project_transfer::ensure_project_not_transferring(&project_id)?;
    let location_now = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .map(|p| p.location);
    if location_now != Some(project.location) {
        return Err(
            "The project moved while the repository was being added; add it again.".to_string(),
        );
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

    let mut branch = match &remote_workspace {
        None => store::Branch::new(&project_id, &repo.branch_name, &effective_base),
        Some(ws_name) => {
            store::Branch::new_remote(&project_id, &repo.branch_name, &effective_base, ws_name)
        }
    }
    .with_project_repo(&repo.id);
    if let Some(pr) = pr_number {
        branch = branch.with_pr(pr);
    }
    store.create_branch(&branch).map_err(|e| e.to_string())?;
    let provisions_workspace = provisioning_claim.is_some();
    drop(provisioning_claim);
    Ok(AddedProjectRepo {
        repo,
        provisions_workspace,
    })
}

/// Remote projects whose first repo is being added right now: from the check
/// that finds them without a workspace until the branch that provisions one
/// is written.
fn provisioning_projects() -> &'static Mutex<HashSet<String>> {
    static PROJECTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    PROJECTS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Held by the add that provisions a remote project's workspace, and released
/// when that add returns.
struct ProvisioningClaim(String);

impl Drop for ProvisioningClaim {
    fn drop(&mut self) {
        provisioning_projects()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.0);
    }
}

enum RemoteWorkspace {
    /// The project's workspace, which the new repo is cloned into.
    Existing(String),
    /// The project has no workspace: the new repo's branch provisions it.
    Provision(ProvisioningClaim),
}

/// Whether a repo added to a remote project joins its workspace or provisions
/// it. The check and the claim happen under one lock, so of two adds racing
/// on a project with no workspace exactly one provisions it. The other is
/// refused, as any add is while the workspace is still starting.
fn claim_remote_workspace(
    store: &Store,
    project: &store::Project,
) -> Result<RemoteWorkspace, String> {
    let mut provisioning = provisioning_projects()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if provisioning.contains(&project.id) {
        return Err(format!(
            "The workspace for '{}' is being provisioned. Wait until it is running, then retry.",
            project.name
        ));
    }
    if let Some(ws_name) = branches::existing_project_workspace_name(store, &project.id)? {
        return Ok(RemoteWorkspace::Existing(ws_name));
    }
    provisioning.insert(project.id.clone());
    Ok(RemoteWorkspace::Provision(ProvisioningClaim(
        project.id.clone(),
    )))
}

/// Refuse to add a repo to a remote project whose workspace isn't running,
/// since the repo is cloned into it.
async fn ensure_workspace_running(ws_name: &str) -> Result<(), String> {
    let ws_info = tauri::async_runtime::spawn_blocking({
        let ws_name = ws_name.to_string();
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
    Ok(())
}

/// Background worktree + prerun-actions setup for a repo that was just added
/// to a project — fire and forget. Shared by the `add_project_repo` command and
/// the note move that adds a repo on the way. `provisions_workspace` is
/// [`AddedProjectRepo::provisions_workspace`].
pub(crate) fn spawn_project_repo_setup(
    app_handle: tauri::AppHandle,
    store: Arc<Store>,
    project_id: String,
    repo_id: String,
    provisions_workspace: bool,
) {
    log::info!(
        "[add_project_repo] spawning background setup for repo {repo_id} in project {project_id}"
    );
    tauri::async_runtime::spawn(async move {
        crate::web_server::emit_to_all(&app_handle, "project-setup-progress", project_id.clone());

        let branch = match store.list_branches_for_project(&project_id) {
            Ok(branches) => branches
                .into_iter()
                .find(|b| b.project_repo_id.as_deref() == Some(repo_id.as_str())),
            Err(e) => {
                log::warn!("[add_project_repo] failed to list branches: {e}");
                return;
            }
        };
        let branch = match branch {
            Some(b) => b,
            None => {
                log::warn!("[add_project_repo] no branch found for repo {repo_id}");
                return;
            }
        };

        if branch.workspace_name.is_none() {
            // Local branch: set up git worktree and run prerun actions.
            let branch_id = branch.id.clone();
            let store_clone = Arc::clone(&store);
            let app_handle_clone = app_handle.clone();
            let worktree_result = tauri::async_runtime::spawn_blocking(move || {
                branches::setup_worktree_sync(&store_clone, &branch_id, &app_handle_clone)
            })
            .await;

            match worktree_result {
                Ok(Ok(path)) => {
                    log::info!("[add_project_repo] worktree ready at {path}");
                    crate::web_server::emit_to_all(
                        &app_handle,
                        "project-setup-progress",
                        project_id.clone(),
                    );
                }
                Ok(Err(e)) => {
                    log::warn!("[add_project_repo] worktree setup failed: {e}");
                    return;
                }
                Err(e) => {
                    log::warn!("[add_project_repo] worktree task panicked: {e}");
                    return;
                }
            }

            let executor = app_handle.state::<Arc<crate::actions::ActionExecutor>>();
            let act_registry = app_handle.state::<Arc<crate::actions::ActionRegistry>>();
            if let branches::PrerunOutcome::Ran(_) = branches::claim_and_run_prerun_actions(
                &store,
                &app_handle,
                &branch.id,
                &executor,
                &act_registry,
                None,
                "add_project_repo",
            )
            .await
            {
                crate::web_server::emit_to_all(
                    &app_handle,
                    "project-setup-progress",
                    project_id.clone(),
                );
            }
        } else if provisions_workspace {
            // Nothing is running to clone into yet. As with a project created
            // with a repo, the workspace lifecycle runs `start_workspace` for
            // the `Starting` branch, which starts the workspace from this repo.
            log::info!(
                "[add_project_repo] branch '{}' provisions workspace '{}'; leaving the clone to start_workspace",
                branch.branch_name,
                branch.workspace_name.as_deref().unwrap_or_default()
            );
        } else {
            // Remote branch: clone the repo into the running workspace,
            // fetch the base branch, and create the feature branch.
            match branches::setup_remote_repo_clone(&store, &app_handle, &branch.id).await {
                Ok(()) => {
                    log::info!(
                        "[add_project_repo] remote repo cloned for branch '{}'",
                        branch.branch_name
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[add_project_repo] remote repo clone failed for branch '{}': {e}",
                        branch.branch_name
                    );
                    crate::web_server::emit_to_all(
                        &app_handle,
                        "project-setup-progress",
                        project_id,
                    );
                    return;
                }
            }
            crate::web_server::emit_to_all(&app_handle, "project-setup-progress", project_id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_project(store: &Store, name: &str) -> store::Project {
        let mut project = store::Project::named(name);
        project.location = store::ProjectLocation::Remote;
        store.create_project(&project).unwrap();
        project
    }

    /// A remote project created without a repo has no workspace, so its first
    /// repo isn't held to a running one: it gets the branch that provisions
    /// the workspace, `Starting` like `create_project`'s initial branch and
    /// named, as there, after that branch rather than the project. With `sq`
    /// asked nothing — a `ws info` on the workspace that doesn't exist yet
    /// would fail the add.
    #[test]
    fn the_first_repo_of_a_remote_project_without_a_workspace_provisions_it() {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = remote_project(&store, "Cloud only");

        let added = tauri::async_runtime::block_on(add_project_repo_impl(
            Arc::clone(&store),
            project.id.clone(),
            "acme/widgets".to_string(),
            Some("fix-login".to_string()),
            None,
            None,
            None,
            None,
            Some("main".to_string()),
            None,
        ))
        .unwrap();

        assert!(added.provisions_workspace);
        let repo = added.repo;
        assert!(repo.is_primary);
        let project = store.get_project(&project.id).unwrap().unwrap();
        assert_eq!(project.github_repo.as_deref(), Some("acme/widgets"));
        let branches = store.list_branches_for_project(&project.id).unwrap();
        assert_eq!(branches.len(), 1);
        let branch = &branches[0];
        assert_eq!(branch.project_repo_id.as_deref(), Some(repo.id.as_str()));
        assert_eq!(branch.branch_type, store::BranchType::Remote);
        assert_eq!(
            branch.workspace_status,
            Some(store::WorkspaceStatus::Starting)
        );
        let workspace_name = branch.workspace_name.as_deref().unwrap();
        assert_eq!(
            workspace_name,
            branches::infer_workspace_name(&project.id, "fix-login")
        );
        assert!(workspace_name.starts_with("fix-login-"), "{workspace_name}");
        assert_eq!(branch.base_branch, "origin/main");
        // The claim went with the add, so the project isn't left looking busy.
        assert!(!provisioning_projects()
            .lock()
            .unwrap()
            .contains(&project.id));
    }

    /// Of two adds racing on a project with no workspace, only the one that
    /// claims it first provisions it; the other is refused until that claim
    /// is released, after which the project's workspace is the one its
    /// branch named.
    #[test]
    fn only_one_add_provisions_a_remote_project_workspace() {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = remote_project(&store, "Racing");

        let claim = match claim_remote_workspace(&store, &project).unwrap() {
            RemoteWorkspace::Provision(claim) => claim,
            RemoteWorkspace::Existing(name) => panic!("unexpected workspace {name}"),
        };
        let err = match claim_remote_workspace(&store, &project) {
            Err(err) => err,
            Ok(_) => panic!("a second add claimed the workspace too"),
        };
        assert!(err.contains("being provisioned"), "unexpected error: {err}");

        // The first add's branch lands, then its claim is released.
        let repo = store::ProjectRepo::new(&project.id, "acme/widgets", "racing", None);
        store.create_project_repo(&repo).unwrap();
        let branch = store::Branch::new_remote(&project.id, "racing", "origin/main", "stg-racing")
            .with_project_repo(&repo.id);
        store.create_branch(&branch).unwrap();
        drop(claim);

        match claim_remote_workspace(&store, &project).unwrap() {
            RemoteWorkspace::Existing(name) => assert_eq!(name, "stg-racing"),
            RemoteWorkspace::Provision(_) => panic!("the workspace was provisioned twice"),
        }
    }

    /// A failed add releases its claim, so a later one can still provision.
    #[test]
    fn a_dropped_claim_lets_the_next_add_provision() {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = remote_project(&store, "Retry");

        drop(claim_remote_workspace(&store, &project).unwrap());

        assert!(matches!(
            claim_remote_workspace(&store, &project).unwrap(),
            RemoteWorkspace::Provision(_)
        ));
    }

    #[test]
    fn rename_project_impl_trims_and_stores_the_name() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();

        let renamed = rename_project_impl(&store, &project.id, "  Renamed  ").unwrap();

        assert_eq!(renamed.name, "Renamed");
        assert_eq!(renamed.id, project.id);
        assert_eq!(
            store.get_project(&project.id).unwrap().unwrap().name,
            "Renamed"
        );
    }

    #[test]
    fn rename_project_impl_rejects_a_blank_name_without_writing() {
        let store = Store::in_memory().unwrap();
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();

        let err = rename_project_impl(&store, &project.id, "   \t\n").unwrap_err();

        assert_eq!(err, "Project name is required");
        assert_eq!(
            store.get_project(&project.id).unwrap().unwrap().name,
            project.name
        );
    }

    #[test]
    fn rename_project_impl_surfaces_a_missing_project() {
        let store = Store::in_memory().unwrap();

        let err = rename_project_impl(&store, "missing", "Renamed").unwrap_err();

        assert!(err.contains("Project not found"), "{err}");
    }
}
