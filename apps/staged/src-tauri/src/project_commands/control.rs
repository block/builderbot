//! What a control caller (`stagedctl`) asks of project creation and repo
//! attachment, turned into the requests the shared bodies take.
//!
//! A desktop form checks its fields as the user fills them in, then submits
//! arguments the shared bodies trust. A control caller has no form and sends
//! what was typed, so the same checks run here, ahead of any write:
//!
//! - The repository is resolved by [`resolve_repo_spec`]: its name as GitHub
//!   spells it, the subpath, the monorepo rule, the pull request and the
//!   base branch.
//! - A session may hold only so many created projects
//!   ([`CREATED_PROJECT_LIMIT`]).
//! - A remote project is refused when `sq` is missing. The desktop form
//!   never offers one then, and the shared body would quietly create a local
//!   project instead.
//! - A branch the same repo already has in the project is refused: its
//!   checkout would fail in the background, where only the log would say so.
//!
//! The pull request's description rides along as a [`PrNote`], which the
//! shared bodies save once the rows exist. The forms write that note with a
//! second command.
//!
//! Every lookup here blocks on `gh`, so the two entry points run on the
//! blocking pool. They write nothing: a request dropped while they wait
//! leaves no trace.

use std::sync::Arc;

use super::add_repo::AddProjectRepoRequest;
use super::create::{required_name, CreateProjectRequest, ProjectCreator};
use super::pr_note::PrNote;
use super::repo_spec::{resolve_repo_spec, GitHubCli, RepoHost, RepoSpecRequest, ResolvedRepoSpec};
use crate::blox;
use crate::control_server::ControlGrant;
use crate::store::Store;

/// The most projects one confined control actor may hold at once. A runaway
/// session stops here. Deleting a created project in the app frees a slot.
pub(crate) const CREATED_PROJECT_LIMIT: usize = 10;

impl ProjectCreator {
    /// The creator a control grant is. A grant confined to one project is
    /// limited, and in practice that is a session: the scope check refuses
    /// `create_project` to a connected app confined to one project. A
    /// connected app approved for every project is not limited.
    pub(crate) fn for_grant(grant: &ControlGrant) -> Self {
        Self {
            actor: grant.actor.started_by(),
            limit: grant.project_scope().map(|_| CREATED_PROJECT_LIMIT),
        }
    }
}

/// What a control caller asks for when creating a project.
#[derive(Debug, Clone, Default)]
pub(crate) struct ControlCreateProject {
    pub name: String,
    /// Whether the project's branches run in a Blox workspace.
    pub remote: bool,
    /// The repository to create the project with, as the caller typed it.
    pub repo: Option<RepoSpecRequest>,
}

/// What a control caller asks for when adding a repository to a project.
#[derive(Debug, Clone, Default)]
pub(crate) struct ControlAddRepo {
    /// The repository, as the caller typed it.
    pub repo: RepoSpecRequest,
    /// Why the repo is being added, shown on its branch card.
    pub reason: Option<String>,
}

/// Check a control caller's create-project request and fill it in. See
/// [`create_project_request`] for the rules.
pub(crate) async fn control_create_project_request(
    store: Arc<Store>,
    creator: ProjectCreator,
    ask: ControlCreateProject,
) -> Result<CreateProjectRequest, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // Only a remote request cares, and the first answer costs a PATH
        // probe.
        let remote_available = ask.remote && blox::is_sq_available();
        create_project_request(&GitHubCli, &store, creator, ask, remote_available)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Check a control caller's add-repo request and fill it in. See
/// [`add_repo_request`] for the rules.
pub(crate) async fn control_add_repo_request(
    store: Arc<Store>,
    project_id: String,
    ask: ControlAddRepo,
) -> Result<AddProjectRepoRequest, String> {
    tauri::async_runtime::spawn_blocking(move || {
        add_repo_request(&GitHubCli, &store, &project_id, ask)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The create-project request a control caller's ask becomes. The checks run
/// cheapest first, so nothing asks GitHub about a request that is refused
/// anyway: the name, the creator's limit, `sq` for a remote project, then
/// the repository.
///
/// The limit is checked again when the project row is written, under a lock
/// (see `create::insert_project_within_limit`). This check only saves the
/// lookups. A duplicate name is left to the shared body, which knows the
/// name as it will be stored.
fn create_project_request(
    host: &dyn RepoHost,
    store: &Store,
    creator: ProjectCreator,
    ask: ControlCreateProject,
    remote_available: bool,
) -> Result<CreateProjectRequest, String> {
    let ControlCreateProject { name, remote, repo } = ask;
    required_name(&name)?;
    creator.ensure_within_limit(store)?;
    if remote && !remote_available {
        return Err(
            "A remote project needs the `sq` CLI to provision its workspace, and Staged \
             could not find it. Create a local project, or install `sq` and restart Staged."
                .to_string(),
        );
    }
    let location = Some(if remote { "remote" } else { "local" }.to_string());
    let Some(repo) = repo else {
        return Ok(CreateProjectRequest {
            name,
            location,
            created_by: Some(creator),
            ..Default::default()
        });
    };

    let resolved = resolve_repo_spec(host, repo)?;
    let pr_note = pr_note(&resolved);
    Ok(CreateProjectRequest {
        name,
        github_repo: Some(resolved.github_repo),
        location,
        subpath: resolved.subpath,
        branch_name: resolved.branch_name,
        pr_number: resolved.pull_request.map(|pr| pr.number),
        default_branch: resolved.base,
        head_repo: resolved.head_repo,
        created_by: Some(creator),
        pr_note,
    })
}

/// The add-repo request a control caller's ask becomes: the repository
/// resolved, and its branch known to be free in the project.
///
/// A control caller never chooses the primary repo. The app has no way to
/// change it back, and the control surface offers only what the user can
/// undo.
fn add_repo_request(
    host: &dyn RepoHost,
    store: &Store,
    project_id: &str,
    ask: ControlAddRepo,
) -> Result<AddProjectRepoRequest, String> {
    // The shared body refuses a missing project too, but only after the
    // lookups below.
    store
        .get_project(project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;

    let resolved = resolve_repo_spec(host, ask.repo)?;
    ensure_branch_is_free(store, project_id, &resolved)?;
    let pr_note = pr_note(&resolved);
    Ok(AddProjectRepoRequest {
        github_repo: resolved.github_repo,
        branch_name: resolved.branch_name,
        subpath: resolved.subpath,
        set_as_primary: None,
        reason: ask
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map(ToOwned::to_owned),
        pr_number: resolved.pull_request.map(|pr| pr.number),
        default_branch: resolved.base,
        head_repo: resolved.head_repo,
        pr_note,
    })
}

/// Refuse a branch the same repository already has in the project.
///
/// Two attachments of one repository share a clone, and a branch can be
/// checked out in it once. The shared body avoids the clash for a name it
/// infers, by adding a suffix. A name the caller gave, or a pull request's
/// branch, is used as it is, and the second checkout would fail in the
/// background setup, where only the log says so.
fn ensure_branch_is_free(
    store: &Store,
    project_id: &str,
    resolved: &ResolvedRepoSpec,
) -> Result<(), String> {
    let Some(branch) = resolved.branch_name.as_deref() else {
        return Ok(());
    };
    let repos = store
        .list_project_repos(project_id)
        .map_err(|e| e.to_string())?;
    let Some(taken) = repos.iter().find(|repo| {
        repo.github_repo.eq_ignore_ascii_case(&resolved.github_repo) && repo.branch_name == branch
    }) else {
        return Ok(());
    };
    let place = match taken.subpath.as_deref() {
        Some(subpath) => format!("at subpath '{subpath}'"),
        None => "at the repository root".to_string(),
    };
    Err(format!(
        "Branch '{branch}' of '{}' is already attached to this project {place}. \
         A branch can be checked out once per project, so give a different branch name.",
        resolved.github_repo
    ))
}

/// The note for the pull request a resolved repo belongs to: the one asked
/// for by number, or the open one found for the caller's branch. The forms
/// write a note in both cases.
fn pr_note(resolved: &ResolvedRepoSpec) -> Option<PrNote> {
    resolved
        .pull_request
        .as_ref()
        .map(|pr| PrNote::for_pull_request(pr.number, &pr.title, &pr.body))
}

#[cfg(test)]
mod tests;
