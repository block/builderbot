use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::blox;
use crate::git;
use crate::store::{self, Store};
use crate::BranchWithWorkdir;

fn get_store(store: &tauri::State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Database not initialized — please reset from the startup prompt".into())
}

fn to_branch_with_workdir(
    branch: store::Branch,
    workdir_path: Option<String>,
) -> BranchWithWorkdir {
    BranchWithWorkdir {
        id: branch.id,
        project_id: branch.project_id,
        project_repo_id: branch.project_repo_id,
        branch_name: branch.branch_name,
        base_branch: branch.base_branch,
        pr_number: branch.pr_number,
        branch_type: branch.branch_type,
        workspace_name: branch.workspace_name,
        workspace_status: branch.workspace_status,
        pr_state: branch.pr_state,
        pr_checks_status: branch.pr_checks_status,
        pr_review_decision: branch.pr_review_decision,
        pr_mergeable: branch.pr_mergeable,
        pr_draft: branch.pr_draft,
        pr_url: branch.pr_url,
        pr_updated_at: branch.pr_updated_at,
        pr_fetched_at: branch.pr_fetched_at,
        worktree_path: workdir_path,
        created_at: branch.created_at,
        updated_at: branch.updated_at,
    }
}

pub(crate) fn project_primary_repo(project: &store::Project) -> Result<&str, String> {
    project
        .primary_repo()
        .ok_or_else(|| format!("Project '{}' has no repository attached", project.name))
}

pub(crate) fn resolve_branch_repo_slug(
    store: &Arc<Store>,
    project: &store::Project,
    branch: &store::Branch,
) -> Result<String, String> {
    if let Some(repo_id) = &branch.project_repo_id {
        if let Some(repo) = store.get_project_repo(repo_id).map_err(|e| e.to_string())? {
            return Ok(repo.github_repo);
        }
    }
    Ok(project_primary_repo(project)?.to_string())
}

/// Resolve the shared workspace name for a remote project.
///
/// If any remote branch already exists for the project, reuse its workspace
/// name so all project repos stay on the same Blox workspace.
pub(crate) fn resolve_project_workspace_name(
    store: &Arc<Store>,
    project: &store::Project,
    fallback_workspace_name: Option<&str>,
) -> Result<String, String> {
    let existing = store
        .list_branches_for_project(&project.id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|b| b.branch_type == store::BranchType::Remote)
        .and_then(|b| b.workspace_name)
        .filter(|name| !name.trim().is_empty());
    if let Some(name) = existing {
        return Ok(name);
    }

    if let Some(name) = fallback_workspace_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(name.to_string());
    }

    Ok(infer_workspace_name(&infer_branch_name(&project.name)))
}

/// Reject unsafe workspace-relative paths.
pub(crate) fn validate_workspace_subpath(subpath: &str) -> Result<String, String> {
    let trimmed = subpath.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Err("Workspace subpath must not be empty".to_string());
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(format!(
            "Workspace subpath '{trimmed}' must be relative, not absolute"
        ));
    }
    if trimmed
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(format!(
            "Workspace subpath '{trimmed}' contains an invalid path segment"
        ));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn infer_remote_repo_subpath(github_repo: &str) -> String {
    let repo_name = github_repo
        .rsplit('/')
        .next()
        .unwrap_or(github_repo)
        .to_string();
    let collapsed = repo_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let suffix = if collapsed.is_empty() {
        "repo".to_string()
    } else {
        collapsed
    };
    // Marker format for workspace repo roots. The actual folder path in the
    // workspace is the value after `repo:` (e.g. `repo:builderbot` -> `~/builderbot`).
    format!("repo:{suffix}")
}

pub(crate) fn run_workspace_git(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    git_args: &[&str],
) -> Result<String, blox::BloxError> {
    let mut owned = Vec::<String>::new();
    owned.push("git".to_string());
    if let Some(subpath) = repo_subpath.map(str::trim).filter(|s| !s.is_empty()) {
        let resolved = resolve_workspace_repo_path(workspace_name, subpath)?;
        owned.push("-C".to_string());
        owned.push(resolved);
    }
    owned.extend(git_args.iter().map(|arg| (*arg).to_string()));
    let borrowed = owned.iter().map(String::as_str).collect::<Vec<_>>();
    blox::ws_exec(workspace_name, &borrowed)
}

async fn run_blox_blocking<T, F>(op: F) -> Result<T, blox::BloxError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, blox::BloxError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(op)
        .await
        .map_err(|e| blox::BloxError::CommandFailed(format!("blox task failed: {e}")))?
}

async fn ws_exec_async(workspace_name: &str, args: &[&str]) -> Result<String, blox::BloxError> {
    let ws_name = workspace_name.to_string();
    let owned_args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    run_blox_blocking(move || {
        let borrowed = owned_args.iter().map(String::as_str).collect::<Vec<_>>();
        blox::ws_exec(&ws_name, &borrowed)
    })
    .await
}

async fn run_workspace_git_async(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    git_args: &[&str],
) -> Result<String, blox::BloxError> {
    let ws_name = workspace_name.to_string();
    let subpath = repo_subpath.map(|s| s.to_string());
    let owned_args = git_args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    run_blox_blocking(move || {
        let borrowed_args = owned_args.iter().map(String::as_str).collect::<Vec<_>>();
        run_workspace_git(&ws_name, subpath.as_deref(), &borrowed_args)
    })
    .await
}

fn workspace_home_dir(workspace_name: &str) -> Result<String, blox::BloxError> {
    let out = blox::ws_exec(workspace_name, &["sh", "-lc", "cd ~ && pwd"])?;
    let home = out.trim();
    if home.is_empty() {
        return Err(blox::BloxError::CommandFailed(
            "Could not resolve workspace home directory".to_string(),
        ));
    }
    Ok(home.to_string())
}

pub(crate) fn resolve_workspace_repo_path(
    workspace_name: &str,
    repo_subpath: &str,
) -> Result<String, blox::BloxError> {
    if let Some(rest) = repo_subpath.strip_prefix("home:") {
        let home = workspace_home_dir(workspace_name)?;
        return Ok(format!("{home}/{rest}"));
    }
    Ok(repo_subpath.to_string())
}

pub(crate) fn resolve_branch_workspace_subpath(
    store: &Arc<Store>,
    branch: &store::Branch,
) -> Result<Option<String>, String> {
    let Some(repo_id) = branch.project_repo_id.as_deref() else {
        return Ok(None);
    };
    let Some(repo) = store.get_project_repo(repo_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let Some(subpath) = repo.subpath else {
        return Ok(None);
    };
    let validated = validate_workspace_subpath(&subpath)?;
    if let Some(repo_dir) = validated.strip_prefix("repo:") {
        let dir = validate_workspace_subpath(repo_dir)?;
        return Ok(Some(format!("home:{dir}")));
    }
    // Backward compatibility for previously created repos under `repos/...`.
    if validated.starts_with("repos/") {
        return Ok(Some(validated));
    }
    // Existing plain subpaths (e.g. monorepo paths like `packages/web`) are
    // project-internal paths, not repo roots in the shared workspace.
    Ok(None)
}

fn normalize_branch_ref(branch: &str) -> String {
    branch.strip_prefix("origin/").unwrap_or(branch).to_string()
}

fn is_blox_onboarding_precondition_error(err: &blox::BloxError) -> bool {
    match err {
        blox::BloxError::CommandFailed(stderr) => {
            let lower = stderr.to_ascii_lowercase();
            lower.contains("failed_precondition")
                && (lower.contains("onboard") || lower.contains("has not completed onboarding"))
        }
        _ => false,
    }
}

pub(crate) fn cleanup_branch_resources(
    store: &Arc<Store>,
    branch: &store::Branch,
) -> Result<(), String> {
    match branch.branch_type {
        store::BranchType::Local => {
            let project = store
                .get_project(&branch.project_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

            let workdir = store
                .get_workdir_for_branch(&branch.id)
                .map_err(|e| e.to_string())?;

            if let Some(ref wd) = workdir {
                let repo_slug = resolve_branch_repo_slug(store, &project, branch)?;
                let repo_path = crate::paths::repos_dir()
                    .map(|d| d.join(repo_slug))
                    .ok_or("Cannot determine clone path")?;
                let worktree_path = Path::new(&wd.path);
                git::remove_worktree(&repo_path, worktree_path).map_err(|e| e.to_string())?;
                store.delete_workdir(&wd.id).map_err(|e| e.to_string())?;
            }
        }
        store::BranchType::Remote => {
            if let Some(ref ws_name) = branch.workspace_name {
                let in_use_elsewhere = store
                    .list_branches_for_project(&branch.project_id)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .any(|b| {
                        b.id != branch.id
                            && b.branch_type == store::BranchType::Remote
                            && b.workspace_name.as_deref() == Some(ws_name.as_str())
                    });
                if in_use_elsewhere {
                    return Ok(());
                }
                match blox::ws_delete(ws_name) {
                    Ok(_) => {}
                    Err(blox::BloxError::CommandFailed(msg))
                        if msg.to_lowercase().contains("not found") => {}
                    Err(e) => {
                        return Err(format!("Failed to delete workspace {ws_name}: {e}"));
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn cleanup_branch_resources_best_effort(store: &Arc<Store>, branch: &store::Branch) {
    if let Err(e) = cleanup_branch_resources(store, branch) {
        log::warn!(
            "project delete cleanup warning for branch '{}': {e}",
            branch.branch_name
        );
        if branch.branch_type == store::BranchType::Local {
            if let Ok(Some(wd)) = store.get_workdir_for_branch(&branch.id) {
                let path = Path::new(&wd.path);
                if path.exists() {
                    if let Err(io_err) = std::fs::remove_dir_all(path) {
                        log::warn!(
                            "failed fallback removal for worktree '{}': {io_err}",
                            wd.path
                        );
                    }
                }
            }
        }
    }
}

pub(crate) fn infer_branch_name(project_name: &str) -> String {
    let branch = project_name
        .to_lowercase()
        .replace([' ', '_'], "-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '/' || *c == '.')
        .collect::<String>()
        .replace(['.', '/'], "-")
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if branch.is_empty() {
        "feature".to_string()
    } else {
        branch
    }
}

pub(crate) fn infer_workspace_name(branch_name: &str) -> String {
    const WORKSPACE_NAME_MAX_LENGTH: usize = 32;
    let safe = branch_name
        .replace('/', "-")
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if safe.is_empty() {
        return "stg-feature".to_string();
    }
    let mut full = format!("stg-{safe}");
    if full.len() > WORKSPACE_NAME_MAX_LENGTH {
        full = full[..WORKSPACE_NAME_MAX_LENGTH]
            .trim_end_matches('-')
            .to_string();
    }
    full
}

#[tauri::command]
pub fn list_branches_for_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
) -> Result<Vec<BranchWithWorkdir>, String> {
    let store = get_store(&store)?;
    let _project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    let branches = store
        .list_branches_for_project(&project_id)
        .map_err(|e| e.to_string())?;

    let mut result = Vec::with_capacity(branches.len());
    for branch in branches {
        let workdir = store
            .get_workdir_for_branch(&branch.id)
            .map_err(|e| e.to_string())?;

        let bw = to_branch_with_workdir(branch, workdir.map(|w| w.path));
        result.push(bw);
    }
    Ok(result)
}

/// Create a local branch record (DB only — no git worktree yet).
///
/// Returns immediately with `worktree_path = None`. Call `setup_worktree`
/// separately to create the git worktree in the background.
#[tauri::command(rename_all = "camelCase")]
pub fn create_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    branch_name: String,
    base_branch: Option<String>,
    project_repo_id: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;

    // Detect default branch if none specified (via GitHub API, no local clone needed)
    let target_repo = match project_repo_id {
        Some(repo_id) => store
            .get_project_repo(&repo_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Project repo not found: {repo_id}"))?,
        None => store
            .get_primary_project_repo(&project_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Project '{project_id}' has no repository attached"))?,
    };
    let effective_base = match base_branch {
        Some(b) => b,
        None => git::detect_default_branch_for_repo(&target_repo.github_repo)
            .map_err(|e| e.to_string())?,
    };

    // Normalise to "origin/<branch>" so diffs and worktree creation always
    // use the remote-tracking ref rather than a stale local branch.
    let effective_base = if effective_base.starts_with("origin/") {
        effective_base
    } else {
        format!("origin/{effective_base}")
    };

    // Create branch record only — no git worktree yet
    let branch = store::Branch::new(&project_id, &branch_name, &effective_base)
        .with_project_repo(&target_repo.id);
    store.create_branch(&branch).map_err(|e| e.to_string())?;

    Ok(to_branch_with_workdir(branch, None))
}

/// Create the git worktree for a local branch and record its workdir.
///
/// Separated from `create_branch` so the frontend can dismiss the modal
/// immediately and show a "Creating worktree…" spinner on the branch card
/// while this runs in the background.
#[tauri::command(rename_all = "camelCase")]
pub async fn setup_worktree(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    // Idempotent fast-path: if this branch already has a workdir, reuse it.
    if let Some(existing) = store
        .get_workdir_for_branch(&branch.id)
        .map_err(|e| e.to_string())?
    {
        return Ok(to_branch_with_workdir(branch, Some(existing.path)));
    }

    // Ensure we have a local clone, then fetch the specific refs we need.
    let repo_slug = resolve_branch_repo_slug(&store, &project, &branch)?;
    let repo_path = git::ensure_local_clone(&repo_slug).map_err(|e| e.to_string())?;
    git::fetch_for_worktree(
        &repo_path,
        &repo_slug,
        &branch.branch_name,
        &branch.base_branch,
    )
    .map_err(|e| e.to_string())?;
    let desired_worktree_path =
        git::project_worktree_path_for(&branch.project_id, &repo_slug, &branch.branch_name)
            .map_err(|e| e.to_string())?;

    // Reuse any existing worktree for this branch; otherwise create one.
    let existing_worktree_path = git::list_worktrees(&repo_path)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find_map(|(path, wt_branch)| match wt_branch.as_deref() {
            Some(name) if name == branch.branch_name => Some(path),
            _ => None,
        });

    let worktree_path = if let Some(path) = existing_worktree_path {
        path
    } else if git::branch_exists(&repo_path, &branch.branch_name).map_err(|e| e.to_string())? {
        git::create_worktree_for_existing_branch_at_path(
            &repo_path,
            &branch.branch_name,
            &desired_worktree_path,
        )
        .map_err(|e| e.to_string())?
    } else {
        match git::create_worktree_at_path(
            &repo_path,
            &branch.branch_name,
            &branch.base_branch,
            &desired_worktree_path,
        ) {
            Ok(path) => path,
            Err(create_err) => {
                // Handle races/stale refs where the branch appears between our
                // pre-check and `git worktree add -b ...`.
                if git::branch_exists(&repo_path, &branch.branch_name).map_err(|e| e.to_string())? {
                    log::warn!(
                        "Branch '{}' already exists after create attempt; retrying with existing branch in repo '{}'",
                        branch.branch_name,
                        repo_slug
                    );
                    git::create_worktree_for_existing_branch_at_path(
                        &repo_path,
                        &branch.branch_name,
                        &desired_worktree_path,
                    )
                    .map_err(|e| e.to_string())?
                } else {
                    return Err(create_err.to_string());
                }
            }
        }
    };

    let worktree_str = worktree_path
        .to_str()
        .ok_or("Invalid worktree path")?
        .to_string();

    // Link this path to the branch in DB (create or assign existing record).
    let tracked_workdir = store
        .list_workdirs_for_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|wd| wd.path == worktree_str);

    match tracked_workdir {
        Some(wd) => match wd.branch_id.as_deref() {
            Some(existing_branch_id) if existing_branch_id != branch.id => {
                return Err(format!(
                    "Worktree '{}' is already assigned to another branch",
                    wd.path
                ));
            }
            Some(_) => {}
            None => {
                store
                    .assign_workdir(&wd.id, &branch.id)
                    .map_err(|e| e.to_string())?;
            }
        },
        None => {
            let workdir =
                store::Workdir::new(&branch.project_id, &worktree_str).with_branch(&branch.id);
            store.create_workdir(&workdir).map_err(|e| e.to_string())?;
        }
    }

    Ok(to_branch_with_workdir(branch, Some(worktree_str)))
}

/// Import a GitHub PR as a local branch with a worktree.
///
/// Fetches the PR's head ref, creates a local branch at that commit, sets up
/// a git worktree, and records everything in the DB. Returns the branch with
/// `worktree_path` already populated so the frontend doesn't need to call
/// `setup_worktree` separately.
#[tauri::command(rename_all = "camelCase")]
pub async fn setup_worktree_from_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    pr_number: u64,
    head_ref: String,
    base_ref: String,
    project_repo_id: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;

    let target_repo = match project_repo_id {
        Some(repo_id) => store
            .get_project_repo(&repo_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Project repo not found: {repo_id}"))?,
        None => store
            .get_primary_project_repo(&project_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Project '{project_id}' has no repository attached"))?,
    };
    // Ensure we have a local clone
    let repo_path = git::ensure_local_clone(&target_repo.github_repo).map_err(|e| e.to_string())?;
    let desired_worktree_path =
        git::project_worktree_path_for(&project_id, &target_repo.github_repo, &head_ref)
            .map_err(|e| e.to_string())?;

    // Fetch PR head and create worktree
    let (worktree_path, branch_name, base_branch) = git::create_worktree_from_pr_at_path(
        &repo_path,
        pr_number,
        &head_ref,
        &base_ref,
        &desired_worktree_path,
    )
    .map_err(|e| e.to_string())?;

    let worktree_str = worktree_path
        .to_str()
        .ok_or("Invalid worktree path")?
        .to_string();

    // Create branch record with PR number
    let branch = store::Branch::new(&project_id, &branch_name, &base_branch)
        .with_project_repo(&target_repo.id)
        .with_pr(pr_number);
    store.create_branch(&branch).map_err(|e| e.to_string())?;

    // Create workdir record
    let workdir = store::Workdir::new(&project_id, &worktree_str).with_branch(&branch.id);
    store.create_workdir(&workdir).map_err(|e| e.to_string())?;

    Ok(to_branch_with_workdir(branch, Some(worktree_str)))
}

/// Create a remote branch record.
///
/// Creates the branch DB record with type=remote and status=Starting.
/// No workspace is started here — call `start_workspace` separately.
#[tauri::command(rename_all = "camelCase")]
pub async fn create_remote_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_id: String,
    branch_name: String,
    base_branch: Option<String>,
    workspace_name: String,
    project_repo_id: Option<String>,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;
    let project = store
        .get_project(&project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let resolved_workspace_name =
        resolve_project_workspace_name(&store, &project, Some(&workspace_name))?;

    // Detect default branch if none specified (via GitHub API, no local clone needed)
    let target_repo = match project_repo_id {
        Some(repo_id) => store
            .get_project_repo(&repo_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Project repo not found: {repo_id}"))?,
        None => store
            .get_primary_project_repo(&project_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Project '{project_id}' has no repository attached"))?,
    };
    let effective_base = match base_branch {
        Some(b) => b,
        None => git::detect_default_branch_for_repo(&target_repo.github_repo)
            .map_err(|e| e.to_string())?,
    };

    // Normalise to "origin/<branch>" so diffs and worktree creation always
    // use the remote-tracking ref rather than a stale local branch.
    let effective_base = if effective_base.starts_with("origin/") {
        effective_base
    } else {
        format!("origin/{effective_base}")
    };

    // Create the branch record (starts in Starting status)
    let branch = store::Branch::new_remote(
        &project_id,
        &branch_name,
        &effective_base,
        &resolved_workspace_name,
    )
    .with_project_repo(&target_repo.id);
    store.create_branch(&branch).map_err(|e| e.to_string())?;

    Ok(to_branch_with_workdir(branch, None))
}

/// Start the Blox workspace for a remote branch.
///
/// Separated from `create_remote_branch` so the frontend can dismiss the
/// dialog immediately and show the card in its "Provisioning…" state while
/// this runs in the background.
#[tauri::command(rename_all = "camelCase")]
pub async fn start_workspace(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let ws_name = branch
        .workspace_name
        .as_deref()
        .ok_or("Branch has no workspace name")?;
    let repo_subpath = resolve_branch_workspace_subpath(&store, &branch)?;
    let ref_name = normalize_branch_ref(&branch.base_branch);
    let repo_slug = resolve_branch_repo_slug(&store, &project, &branch)?;

    // Pre-flight: verify the user is authenticated with Blox before starting
    // a workspace. Without this check, `blox ws start` can hang or fail
    // opaquely, leaving the frontend spinning for minutes (issue #99).
    if let Err(e) = run_blox_blocking(blox::check_auth).await {
        store
            .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
            .ok();
        return Err(e.to_string());
    }

    // Secondary repo setup in an already-running shared workspace.
    if let Some(repo_subpath) = repo_subpath.as_deref() {
        let repo_path = resolve_workspace_repo_path(ws_name, repo_subpath)
            .map_err(|e| format!("Failed to resolve workspace repo path '{repo_subpath}': {e}"))?;
        if let Ok(info) = run_blox_blocking({
            let ws_name = ws_name.to_string();
            move || blox::ws_info(&ws_name)
        })
        .await
        {
            let ws_status = info
                .status
                .as_deref()
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            if ws_status == "running" {
                match ws_exec_async(ws_name, &["test", "-d", &format!("{repo_path}/.git")]).await {
                    Ok(_) => {}
                    Err(blox::BloxError::CommandFailed(_)) => {
                        let repo_url = format!("https://github.com/{repo_slug}.git");
                        ws_exec_async(ws_name, &["git", "clone", &repo_url, &repo_path])
                            .await
                            .map_err(|e| {
                                format!(
                                    "Failed to clone '{repo_slug}' into workspace '{ws_name}': {e}"
                                )
                            })?;
                    }
                    Err(e) => {
                        return Err(format!(
                            "Failed to verify repo path '{repo_subpath}' in workspace '{ws_name}': {e}"
                        ));
                    }
                }
                run_workspace_git_async(ws_name, Some(repo_subpath), &["fetch", "origin", &ref_name])
                    .await
                    .map_err(|e| {
                        format!(
                            "Failed to fetch base branch '{ref_name}' for '{repo_slug}' in workspace '{ws_name}': {e}"
                        )
                    })?;
                run_workspace_git_async(
                    ws_name,
                    Some(repo_subpath),
                    &["checkout", "-B", &branch.branch_name, &format!("origin/{ref_name}")],
                )
                .await
                .map_err(|e| {
                    format!(
                        "Failed to create branch '{}' for '{repo_slug}' in workspace '{ws_name}': {e}",
                        branch.branch_name
                    )
                })?;
                store
                    .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Running)
                    .map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
    }

    // Construct the HTTPS git URL directly from github_repo.
    let resolved_source = Some(format!(
        "https://github.com/{}.git?ref={}",
        repo_slug, ref_name
    ));

    match run_blox_blocking({
        let ws_name = ws_name.to_string();
        let source = resolved_source.clone();
        move || blox::ws_start(&ws_name, source.as_deref())
    })
    .await
    {
        Ok(_) => {
            // Create the feature branch inside the workspace so work happens
            // on `branch_name` rather than the detached base ref.
            if let Err(e) = run_workspace_git_async(
                ws_name,
                repo_subpath.as_deref(),
                &["checkout", "-b", &branch.branch_name],
            )
            .await
            {
                log::warn!(
                    "failed to create branch '{}' in workspace '{}': {e}",
                    branch.branch_name,
                    ws_name
                );
            }
            Ok(())
        }
        Err(blox::BloxError::NotAuthenticated) => {
            // Auth errors are definitive — mark as Error so the frontend
            // stops polling and shows an actionable message.
            store
                .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                .ok();
            Err("Not authenticated with Blox. Run: sq login".to_string())
        }
        Err(e) => {
            if is_blox_onboarding_precondition_error(&e) {
                // Blox onboarding precondition failures are definitive.
                // Keep the exact CLI error text so the user sees the
                // onboarding URL and required action.
                store
                    .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                    .ok();
                return Err(e.to_string());
            }
            // Don't set Error status here — `blox ws start` can fail (e.g.
            // timeout, transient network issue) even though the workspace was
            // created and is still booting. Let the frontend's status polling
            // determine the real state; it will keep polling while the DB says
            // Starting and will eventually converge on the correct status.
            log::warn!(
                "blox ws start failed for '{}', leaving status as Starting for polling to resolve: {e}",
                ws_name
            );
            Ok(())
        }
    }
}

/// Get info about a remote branch's Blox workspace.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_workspace_info(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<blox::WorkspaceInfo, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let ws_name = branch
        .workspace_name
        .ok_or("Branch is not a remote workspace branch")?;

    run_blox_blocking(move || blox::ws_info(&ws_name))
        .await
        .map_err(|e| e.to_string())
}

/// Poll a remote branch's workspace status, update the DB, and return the new status.
///
/// This is the primary mechanism for the frontend to detect when a workspace
/// transitions from `Starting` to `Running` (or `Error`). It queries the
/// Blox CLI, maps the reported status to our `WorkspaceStatus` enum, persists
/// the change, and returns the updated status string.
#[tauri::command(rename_all = "camelCase")]
pub async fn poll_workspace_status(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<String, String> {
    let store = get_store(&store)?;

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let ws_name = branch
        .workspace_name
        .as_deref()
        .ok_or("Branch is not a remote workspace branch")?;

    // Secondary repo setup uses `workspace_status=Starting` as a per-branch
    // loading state while clone/fetch/checkout runs inside an already-running
    // shared workspace. Keep it in Starting until that setup command marks
    // this branch as Running.
    if branch.workspace_status == Some(store::WorkspaceStatus::Starting)
        && resolve_branch_workspace_subpath(&store, &branch)?.is_some()
    {
        return Ok(store::WorkspaceStatus::Starting.as_str().to_string());
    }

    let info = match run_blox_blocking({
        let ws_name = ws_name.to_string();
        move || blox::ws_info(&ws_name)
    })
    .await
    {
        Ok(info) => info,
        Err(blox::BloxError::NotAuthenticated) => {
            // Auth errors are definitive — stop polling and surface the error.
            store
                .update_branch_workspace_status(&branch_id, &store::WorkspaceStatus::Error)
                .ok();
            return Err("Not authenticated with Blox. Run: sq login".to_string());
        }
        Err(e) => {
            // During initial creation, `blox ws start` may still be running
            // when the frontend's first poll fires `blox ws info`. The
            // workspace doesn't exist yet, so the CLI returns "not found".
            // If the DB still says Starting, swallow the error and tell the
            // frontend to keep polling.
            if branch.workspace_status == Some(store::WorkspaceStatus::Starting) {
                log::debug!(
                    "blox ws info failed for '{}' while still Starting, treating as Starting: {e}",
                    ws_name
                );
                return Ok(store::WorkspaceStatus::Starting.as_str().to_string());
            }
            return Err(e.to_string());
        }
    };

    // Map the CLI-reported status to our enum.
    // During initial startup, Blox may briefly report "stopped" before the
    // workspace transitions to "running". If the DB still says Starting,
    // treat a Blox "stopped" as still Starting so we keep polling.
    let new_status = match info.status.as_deref() {
        Some("running") | Some("Running") => store::WorkspaceStatus::Running,
        Some("stopped") | Some("Stopped") => {
            if branch.workspace_status == Some(store::WorkspaceStatus::Starting) {
                store::WorkspaceStatus::Starting
            } else {
                store::WorkspaceStatus::Stopped
            }
        }
        Some("starting") | Some("Starting") | Some("provisioning") | Some("Provisioning") => {
            store::WorkspaceStatus::Starting
        }
        Some("error") | Some("Error") | Some("failed") | Some("Failed") => {
            store::WorkspaceStatus::Error
        }
        // If the CLI returns an unrecognized status, keep it as Starting
        // (optimistic — the workspace may still be booting)
        _ => store::WorkspaceStatus::Starting,
    };

    store
        .update_branch_workspace_status(&branch_id, &new_status)
        .map_err(|e| e.to_string())?;

    Ok(new_status.as_str().to_string())
}

#[tauri::command]
pub async fn delete_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<(), String> {
    let store = get_store(&store)?;

    // Get the branch
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    tauri::async_runtime::spawn_blocking({
        let store = Arc::clone(&store);
        let branch = branch.clone();
        move || cleanup_branch_resources(&store, &branch)
    })
    .await
    .map_err(|e| format!("Failed to clean up branch resources: {e}"))??;

    // Delete the branch record (cascades to commits, notes, reviews)
    store.delete_branch(&branch_id).map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn rename_branch(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    branch_name: String,
) -> Result<BranchWithWorkdir, String> {
    let store = get_store(&store)?;
    let new_name = branch_name.trim();
    if new_name.is_empty() {
        return Err("Branch name is required".to_string());
    }

    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if branch.branch_name == new_name {
        let workdir = store
            .get_workdir_for_branch(&branch.id)
            .map_err(|e| e.to_string())?
            .map(|w| w.path);
        return Ok(to_branch_with_workdir(branch, workdir));
    }

    match branch.branch_type {
        store::BranchType::Local => {
            if let Some(wd) = store
                .get_workdir_for_branch(&branch.id)
                .map_err(|e| e.to_string())?
            {
                let status = std::process::Command::new("git")
                    .args(["-C", &wd.path, "branch", "-m", new_name])
                    .status()
                    .map_err(|e| format!("Failed to run git rename: {e}"))?;
                if !status.success() {
                    return Err("Failed to rename local git branch".to_string());
                }
            }
        }
        store::BranchType::Remote => {
            if let Some(ws_name) = &branch.workspace_name {
                let repo_subpath = resolve_branch_workspace_subpath(&store, &branch)?;
                if let Err(e) = run_workspace_git_async(
                    ws_name,
                    repo_subpath.as_deref(),
                    &["branch", "-m", new_name],
                )
                .await
                {
                    log::warn!("Failed to rename branch in workspace '{ws_name}': {e}");
                }
            }
        }
    }

    store
        .update_branch_name(&branch.id, new_name)
        .map_err(|e| e.to_string())?;
    if let Some(repo_id) = &branch.project_repo_id {
        store
            .update_project_repo_branch_name(&branch.project_id, repo_id, new_name)
            .map_err(|e| e.to_string())?;
    }

    let updated = store
        .get_branch(&branch.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {}", branch.id))?;
    let workdir = store
        .get_workdir_for_branch(&updated.id)
        .map_err(|e| e.to_string())?
        .map(|w| w.path);
    Ok(to_branch_with_workdir(updated, workdir))
}
