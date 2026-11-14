//! Creating a project: the store writes, then the background setup.

use std::sync::{Arc, Mutex};

use super::pr_note::PrNote;
use super::setup::spawn_project_repo_setup;
use crate::store::{self, Store};
use crate::{blox, branches, git};

/// What a caller asks for when creating a project. Not persisted: it is the
/// argument list of the `create_project` command, gathered so the Tauri
/// command and the web/control `dispatch` arm hand the shared body the same
/// thing.
#[derive(Debug, Clone, Default)]
pub(crate) struct CreateProjectRequest {
    /// The project's name. Trimmed; empty is refused.
    pub name: String,
    /// The initial repository, `owner/repo`. It becomes the primary repo.
    pub github_repo: Option<String>,
    /// `"remote"` for a project whose branches run in a Blox workspace;
    /// anything else, or nothing, is local.
    pub location: Option<String>,
    pub subpath: Option<String>,
    /// The initial repo's branch. Blank means the name inferred from the
    /// project name.
    pub branch_name: Option<String>,
    pub pr_number: Option<u64>,
    /// The branch the initial repo's branch starts from, when the caller
    /// already knows it. Detected with `gh` otherwise.
    pub default_branch: Option<String>,
    /// The fork a pull request's head lives in, when it isn't `github_repo`.
    pub head_repo: Option<String>,
    /// The control-server actor creating the project, recorded in
    /// `projects.created_by`. `None` for a project made in the app, which
    /// has no creator.
    pub created_by: Option<ProjectCreator>,
    /// The pull request's description, saved as a project note once the
    /// rows are written. The desktop forms leave it unset and write the
    /// note themselves.
    pub pr_note: Option<PrNote>,
}

impl CreateProjectRequest {
    fn wants_remote(&self) -> bool {
        self.location.as_deref() == Some("remote")
    }
}

/// The control-server actor creating a project, and how many it may hold.
/// Not persisted: only `actor` reaches the store, as `projects.created_by`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectCreator {
    /// What `projects.created_by` records: `session:<session-id>` or
    /// `app:<grant-id>`.
    pub actor: String,
    /// The most projects this actor's chain may hold at once, counted from
    /// the `created_by` rows, so deleting one in the app frees a slot. A
    /// session shares the allowance with the sessions under the same root
    /// (see `creation_limit`). `None` for an actor that is not limited.
    pub limit: Option<usize>,
}

/// A project just created.
#[derive(Debug)]
pub(crate) struct CreatedProject {
    pub project: store::Project,
    /// The repo the project was created with, when it was created with one.
    pub initial_repo: Option<InitialRepo>,
}

/// The repo a project was created with, and the one branch tracked for it.
#[derive(Debug)]
pub(crate) struct InitialRepo {
    pub repo: store::ProjectRepo,
    pub branch: store::Branch,
}

/// Create a project and start its background setup.
///
/// The one body behind the `create_project` Tauri command and the
/// web/control `dispatch` arm, so the two can't drift apart: both get the
/// duplicate-name check, the fallback to a local project when `sq` is
/// missing, and — for a local project created with a repo — the worktree
/// followed by the repo's prerun actions.
///
/// Every await is inside [`create_project_impl`] and ahead of its first
/// write, and the setup is spawned in the same synchronous stretch as the
/// last write, so a request future dropped part-way (see the `web_server`
/// module docs) has either written nothing or written every row and started
/// the setup.
pub(crate) async fn create_project_and_setup(
    app_handle: tauri::AppHandle,
    store: Arc<Store>,
    request: CreateProjectRequest,
) -> Result<store::Project, String> {
    // Only a remote request cares, and the first answer costs a PATH probe.
    let remote_available = request.wants_remote() && blox::is_sq_available();
    let created = create_project_impl(Arc::clone(&store), request, remote_available).await?;
    start_project_setup(app_handle, store, &created);
    Ok(created.project)
}

/// What follows a created project's rows: its directory on disk, and the
/// background setup of the repo it was created with.
fn start_project_setup(app_handle: tauri::AppHandle, store: Arc<Store>, created: &CreatedProject) {
    let project = &created.project;
    // Create the project-scoped worktree root so project sessions always
    // have a real directory to run in, even before any repos are attached.
    if let Ok(project_dir) = git::project_worktree_root_for(&project.id) {
        let _ = std::fs::create_dir_all(&project_dir);
    }

    if let Some(initial) = &created.initial_repo {
        // A remote project's first branch is the one that provisions its
        // workspace, so there is nothing running to clone into: the setup
        // leaves the start to the frontend's workspace lifecycle.
        let provisions_workspace = initial.branch.workspace_name.is_some();
        spawn_project_repo_setup(
            app_handle,
            store,
            project.id.clone(),
            initial.repo.id.clone(),
            provisions_workspace,
            "create_project",
        );
    }
}

/// The store writes for creating a project, without anything on disk or in
/// the background.
///
/// `remote_available` is whether a remote project can be provisioned at all
/// (the `sq` CLI is installed). A remote request without it creates a local
/// project instead, as the desktop form has always done.
///
/// Subpath validation is handled by the frontend before submission
/// (`SubpathInput.waitForValidation`). Skipping the redundant backend
/// re-validation here removes a ~500ms-2s GitHub API round-trip.
pub(crate) async fn create_project_impl(
    store: Arc<Store>,
    request: CreateProjectRequest,
    remote_available: bool,
) -> Result<CreatedProject, String> {
    let wants_remote = request.wants_remote();
    let CreateProjectRequest {
        name,
        github_repo,
        location: _,
        subpath,
        branch_name,
        pr_number,
        default_branch,
        head_repo,
        created_by,
        pr_note,
    } = request;

    let trimmed = required_name(&name)?;

    let location = match (wants_remote, remote_available) {
        (true, true) => store::ProjectLocation::Remote,
        (true, false) => {
            log::warn!(
                "[create_project] remote project requested but sq CLI is unavailable; creating a local project instead"
            );
            store::ProjectLocation::Local
        }
        (false, _) => store::ProjectLocation::Local,
    };
    let branch_name = branch_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| branches::infer_prefixed_branch_name(trimmed));

    let mut project = store::Project::named(trimmed);
    project.location = location;
    if let Some(repo) = &github_repo {
        project = project.with_primary_repo(repo);
    }
    if let Some(sub) = &subpath {
        project = project.with_subpath(sub.clone());
    }
    if let Some(creator) = &created_by {
        project = project.with_created_by(&creator.actor);
    }

    // Checked on the name as it will be stored (`with_subpath` can extend
    // it), and ahead of the `gh` wait below so a taken name fails at once.
    // A project created in the meantime is caught by the insert itself.
    ensure_name_is_free(&store, &project.name)?;

    // Use the frontend-prefetched default branch when available to avoid a
    // ~500ms-2s GitHub API round-trip during project creation. The fallback
    // runs `gh`, so it goes to the blocking pool, and ahead of the first
    // write: the web transport drops a request future at its next await when
    // the client goes away, and no await may fall between the writes below.
    let effective_base = match github_repo.clone() {
        Some(repo) => Some(
            tauri::async_runtime::spawn_blocking(move || {
                git::resolve_default_branch(default_branch, &repo)
            })
            .await
            .map_err(|e| e.to_string())?,
        ),
        None => None,
    };

    let initial_repo = write_project_and(&store, &project, created_by.as_ref(), || {
        write_initial_repo(
            &store,
            &project,
            InitialRepoRequest {
                github_repo,
                effective_base,
                branch_name,
                subpath,
                pr_number,
                head_repo,
            },
        )
    })?;

    if let Some(note) = &pr_note {
        note.write(&store, &project.id, "create_project");
    }

    Ok(CreatedProject {
        project,
        initial_repo,
    })
}

/// Write the project row, then run `rest`, which writes what belongs to the
/// project. Either both happen or neither does: when `rest` fails, the
/// project row is deleted again, and its repo and branch rows go with it
/// (the foreign keys cascade).
///
/// A failed create used to leave the project behind with nothing set up.
/// The caller got an error with no project id, and a project made by a
/// session went on holding one of its chain's slots.
///
/// The store has no transaction that spans these writes: each store method
/// takes the connection for itself. Deleting on failure gives a caller the
/// same result. The row is briefly visible to a reader in between.
fn write_project_and<T>(
    store: &Store,
    project: &store::Project,
    created_by: Option<&ProjectCreator>,
    rest: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    match created_by {
        Some(creator) => insert_project_within_limit(store, project, creator)?,
        None => insert_project(store, project)?,
    }
    rest().inspect_err(|e| {
        log::warn!(
            "[create_project] removing project '{}' again, its creation failed part-way: {e}",
            project.id
        );
        if let Err(delete_err) = store.delete_project(&project.id) {
            log::error!(
                "[create_project] could not remove half-created project '{}': {delete_err}",
                project.id
            );
        }
    })
}

/// What [`write_initial_repo`] needs beyond the project row.
struct InitialRepoRequest {
    github_repo: Option<String>,
    /// The `origin/`-prefixed base, resolved when there is a repo.
    effective_base: Option<String>,
    branch_name: String,
    subpath: Option<String>,
    pr_number: Option<u64>,
    head_repo: Option<String>,
}

/// The rows that follow a new project's own: the action context of its
/// primary repo, and, when it was created with a repo, the repo and the one
/// branch tracked for it.
///
/// Runs inside [`write_project_and`], so an error here takes the project row
/// back out. The action context and the recent-repo entry are keyed by
/// repository, not by project, and stay: they record that the repository
/// was used.
fn write_initial_repo(
    store: &Store,
    project: &store::Project,
    request: InitialRepoRequest,
) -> Result<Option<InitialRepo>, String> {
    let InitialRepoRequest {
        github_repo,
        effective_base,
        branch_name,
        subpath,
        pr_number,
        head_repo,
    } = request;

    if let Some(repo) = project.primary_repo() {
        store
            .get_or_create_action_context(repo, project.subpath.as_deref())
            .map_err(|e| e.to_string())?;
    }

    let (Some(github_repo), Some(effective_base)) = (github_repo, effective_base) else {
        if project.location == store::ProjectLocation::Remote {
            log::info!(
                "[create_project] remote project '{}' created without a repo; workspace start will be deferred until a repo is added",
                project.id
            );
        }
        return Ok(None);
    };

    // Record this repo as recently used
    store
        .record_recent_repo(&github_repo, subpath.clone())
        .map_err(|e| e.to_string())?;

    let mut repo =
        store::ProjectRepo::new(&project.id, &github_repo, &branch_name, subpath).primary();
    repo.head_repo = head_repo;
    store
        .create_project_repo(&repo)
        .map_err(|e| e.to_string())?;

    // Create the initial branch record for the first repo so each new
    // project starts with exactly one branch tracked for that repository.
    let mut branch = match project.location {
        store::ProjectLocation::Local => {
            store::Branch::new(&project.id, &branch_name, &effective_base)
        }
        store::ProjectLocation::Remote => {
            let workspace_name = branches::infer_workspace_name(&project.id, &branch_name);
            store::Branch::new_remote(&project.id, &branch_name, &effective_base, &workspace_name)
        }
    }
    .with_project_repo(&repo.id);
    if let Some(pr) = pr_number {
        branch = branch.with_pr(pr);
    }
    store.create_branch(&branch).map_err(|e| e.to_string())?;
    if let Some(workspace_name) = &branch.workspace_name {
        log::info!(
            "[create_project] created remote branch={} workspace={} status=starting project={}",
            branch.id,
            workspace_name,
            project.id
        );
    }

    Ok(Some(InitialRepo { repo, branch }))
}

/// The project's name without surrounding whitespace. A blank name is
/// refused.
pub(super) fn required_name(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Project name is required".to_string());
    }
    Ok(trimmed)
}

/// What a caller is told when the name is taken. Project names are unique
/// (`idx_projects_name`).
fn duplicate_name_error(name: &str) -> String {
    format!("A project named \"{name}\" already exists.")
}

/// Refuse a name another project is stored under. `stored_name` is the name
/// as it will be written, which `Project::with_subpath` can have extended.
fn ensure_name_is_free(store: &Store, stored_name: &str) -> Result<(), String> {
    if name_is_taken(store, stored_name)? {
        return Err(duplicate_name_error(stored_name));
    }
    Ok(())
}

fn name_is_taken(store: &Store, stored_name: &str) -> Result<bool, String> {
    store
        .get_project_by_name(stored_name)
        .map(|project| project.is_some())
        .map_err(|e| e.to_string())
}

/// [`ensure_name_is_free`] for a caller that does not know the repository
/// yet, and so cannot say which name will be stored: refuse only when every
/// name the project could be stored under is taken.
///
/// A project is stored under its name as given, except that one named after
/// its repository and given a subpath has the subpath appended
/// (`Project::with_subpath`). Which repository that is is GitHub's to say: it
/// spells the name, follows a rename, and a pull request can move the
/// request to a fork's parent. So with a subpath there are two candidates,
/// and without one there is only the name. Passing here never skips the
/// check in [`create_project_impl`], which knows the stored name.
pub(super) fn ensure_a_name_is_free(
    store: &Store,
    name: &str,
    subpath: Option<&str>,
) -> Result<(), String> {
    if !name_is_taken(store, name)? {
        return Ok(());
    }
    if let Some(subpath) = subpath {
        // The model's own rule, applied to a repository named like the
        // project.
        let named_after_repo = store::Project::named(name)
            .with_primary_repo(name)
            .with_subpath(subpath.to_string());
        if !name_is_taken(store, &named_after_repo.name)? {
            return Ok(());
        }
    }
    Err(duplicate_name_error(name))
}

/// Write the project row. A failure that leaves a project of that name in
/// the store is the unique index refusing a name another create took after
/// [`create_project_impl`]'s own check, and reads the way that check does.
fn insert_project(store: &Store, project: &store::Project) -> Result<(), String> {
    store
        .create_project(project)
        .map_err(|e| match store.get_project_by_name(&project.name) {
            Ok(Some(_)) => duplicate_name_error(&project.name),
            _ => e.to_string(),
        })
}

/// [`insert_project`] for a project with a creator, holding the creator to
/// its limit. The count and the insert happen under one lock, so creates
/// racing for a creator's last slot cannot both take it.
fn insert_project_within_limit(
    store: &Store,
    project: &store::Project,
    creator: &ProjectCreator,
) -> Result<(), String> {
    static COUNTING: Mutex<()> = Mutex::new(());
    let _counting = COUNTING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    creator.ensure_within_limit(store)?;
    insert_project(store, project)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(name: &str) -> CreateProjectRequest {
        CreateProjectRequest {
            name: name.to_string(),
            ..Default::default()
        }
    }

    /// A request with a repo. The default branch is given, as the forms give
    /// it, so nothing asks `gh`.
    fn request_with_repo(name: &str) -> CreateProjectRequest {
        CreateProjectRequest {
            github_repo: Some("acme/widgets".to_string()),
            default_branch: Some("main".to_string()),
            ..request(name)
        }
    }

    fn create(
        store: &Arc<Store>,
        request: CreateProjectRequest,
        remote_available: bool,
    ) -> Result<CreatedProject, String> {
        tauri::async_runtime::block_on(create_project_impl(
            Arc::clone(store),
            request,
            remote_available,
        ))
    }

    /// The desktop and browser requests name no creator, so the column stays
    /// NULL: the project is the user's own, no control credential reaches it
    /// by the creator rule, and it counts toward nobody's limit. They write
    /// their pull request note themselves, so the body writes none.
    #[test]
    fn a_project_made_in_the_app_has_no_creator_and_no_note() {
        let store = Arc::new(Store::in_memory().unwrap());

        let bare = create(&store, request("Fix login"), false).unwrap();
        let with_repo = create(
            &store,
            CreateProjectRequest {
                pr_number: Some(42),
                ..request_with_repo("Fix logout")
            },
            false,
        )
        .unwrap();

        for created in [&bare, &with_repo] {
            let project = store.get_project(&created.project.id).unwrap().unwrap();
            assert_eq!(project.created_by, None);
            assert!(store.list_project_notes(&project.id).unwrap().is_empty());
        }
    }

    /// A creator is recorded as given, and its limit is not part of what is
    /// stored.
    #[test]
    fn a_creator_is_recorded_on_the_project_row() {
        let store = Arc::new(Store::in_memory().unwrap());

        let created = create(
            &store,
            CreateProjectRequest {
                created_by: Some(ProjectCreator {
                    actor: "session:s1".to_string(),
                    limit: Some(1),
                }),
                ..request("Fix login")
            },
            false,
        )
        .unwrap();

        let project = store.get_project(&created.project.id).unwrap().unwrap();
        assert_eq!(project.created_by.as_deref(), Some("session:s1"));
        // The one slot is taken now.
        let err = create(
            &store,
            CreateProjectRequest {
                created_by: Some(ProjectCreator {
                    actor: "session:s1".to_string(),
                    limit: Some(1),
                }),
                ..request("Fix logout")
            },
            false,
        )
        .unwrap_err();
        assert!(err.contains("the limit they share"), "{err}");
        assert!(store.get_project_by_name("Fix logout").unwrap().is_none());
    }

    fn limited_creator(limit: usize) -> ProjectCreator {
        ProjectCreator {
            actor: "session:s1".to_string(),
            limit: Some(limit),
        }
    }

    /// A create that fails after the project row is written used to leave
    /// the project behind: no repo set up, no id returned to the caller, and
    /// one of the creator's slots taken. The row now goes again, with
    /// whatever was written under it.
    #[test]
    fn a_create_that_fails_after_the_insert_leaves_nothing_behind() {
        let store = Arc::new(Store::in_memory().unwrap());
        let creator = limited_creator(1);
        let project = store::Project::named("Fix login").with_created_by(&creator.actor);

        let err = write_project_and(&store, &project, Some(&creator), || {
            // As far as a real create gets before its last write fails.
            let repo =
                store::ProjectRepo::new(&project.id, "acme/widgets", "fix-login", None).primary();
            store.create_project_repo(&repo).unwrap();
            let branch = store::Branch::new(&project.id, "fix-login", "origin/main")
                .with_project_repo(&repo.id);
            store.create_branch(&branch).unwrap();
            Err::<(), _>("disk I/O error".to_string())
        })
        .unwrap_err();

        assert_eq!(err, "disk I/O error", "the caller gets the cause");
        assert!(store.get_project(&project.id).unwrap().is_none());
        assert!(store.list_projects().unwrap().is_empty());
        assert!(store.list_project_repos(&project.id).unwrap().is_empty());
        assert!(store
            .list_branches_for_project(&project.id)
            .unwrap()
            .is_empty());
        // The slot is free again, and so is the name.
        assert!(store
            .list_project_ids_created_by(&creator.actor)
            .unwrap()
            .is_empty());
        let retried = create(
            &store,
            CreateProjectRequest {
                created_by: Some(creator),
                ..request_with_repo("Fix login")
            },
            false,
        )
        .unwrap();
        assert!(retried.initial_repo.is_some());
    }

    /// The same for a project made in the app, which has no creator.
    #[test]
    fn a_failed_create_in_the_app_leaves_nothing_behind_either() {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = store::Project::named("Fix login");

        let err = write_project_and(&store, &project, None, || {
            Err::<(), _>("no space left".to_string())
        })
        .unwrap_err();

        assert_eq!(err, "no space left");
        assert!(store.list_projects().unwrap().is_empty());
    }

    /// A create that succeeds keeps its rows, and one refused at the insert
    /// (a taken name, a full limit) never runs what follows the insert or
    /// touches the project that holds the name.
    #[test]
    fn only_a_failure_after_the_insert_removes_the_project() {
        let store = Arc::new(Store::in_memory().unwrap());
        let creator = limited_creator(1);
        let first = store::Project::named("Fix login").with_created_by(&creator.actor);

        let wrote = write_project_and(&store, &first, Some(&creator), || Ok("rows")).unwrap();
        assert_eq!(wrote, "rows");
        assert!(store.get_project(&first.id).unwrap().is_some());

        let ran = std::cell::Cell::new(false);
        let same_name = store::Project::named("Fix login");
        let err = write_project_and(&store, &same_name, None, || {
            ran.set(true);
            Ok(())
        })
        .unwrap_err();
        assert_eq!(err, "A project named \"Fix login\" already exists.");
        let over_limit = store::Project::named("Fix logout").with_created_by(&creator.actor);
        let err = write_project_and(&store, &over_limit, Some(&creator), || {
            ran.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(err.contains("the limit they share"), "{err}");

        assert!(!ran.get(), "nothing follows an insert that was refused");
        assert_eq!(store.list_projects().unwrap().len(), 1);
        assert!(store.get_project(&first.id).unwrap().is_some());
    }

    /// Before the repository is resolved the stored name is not always
    /// known, so the early check refuses only when every candidate is
    /// taken. It must never refuse a request the full check would admit.
    #[test]
    fn the_early_name_check_refuses_only_when_every_candidate_is_taken() {
        let store = Arc::new(Store::in_memory().unwrap());
        create(&store, request("widgets"), false).unwrap();

        // No subpath: the name is stored as given.
        assert_eq!(
            ensure_a_name_is_free(&store, "widgets", None).unwrap_err(),
            "A project named \"widgets\" already exists."
        );
        ensure_a_name_is_free(&store, "gadgets", None).unwrap();
        ensure_a_name_is_free(&store, "gadgets", Some("services/auth")).unwrap();

        // With a subpath, a project named after its repository would be
        // stored as `widgets (services/auth)`, which is free.
        ensure_a_name_is_free(&store, "widgets", Some("services/auth")).unwrap();
        let in_subpath = CreateProjectRequest {
            subpath: Some("services/auth".to_string()),
            ..request_with_repo("widgets")
        };
        let extended = create(&store, in_subpath, false).unwrap();
        assert_eq!(extended.project.name, "widgets (services/auth)");

        // Now both candidates are taken.
        assert_eq!(
            ensure_a_name_is_free(&store, "widgets", Some("services/auth")).unwrap_err(),
            "A project named \"widgets\" already exists."
        );
        // Another subpath leaves a candidate free.
        ensure_a_name_is_free(&store, "widgets", Some("services/billing")).unwrap();
    }

    /// A project needs no repo: it is one row, local, with nothing attached.
    #[test]
    fn a_project_without_a_repo_is_one_row() {
        let store = Arc::new(Store::in_memory().unwrap());

        let created = create(&store, request("  Fix login  "), false).unwrap();

        assert!(created.initial_repo.is_none());
        let project = store.get_project(&created.project.id).unwrap().unwrap();
        assert_eq!(project.name, "Fix login");
        assert_eq!(project.location, store::ProjectLocation::Local);
        assert_eq!(project.github_repo, None);
        assert_eq!(project.subpath, None);
        assert!(store.list_project_repos(&project.id).unwrap().is_empty());
        assert!(store
            .list_branches_for_project(&project.id)
            .unwrap()
            .is_empty());
        assert!(store.list_action_contexts().unwrap().is_empty());
        assert!(store.list_recent_repos(10).unwrap().is_empty());
    }

    /// A local project created with a repo gets that repo as its primary and
    /// one local branch for it, plus the bookkeeping the repo pickers and the
    /// actions read.
    #[test]
    fn a_local_project_with_a_repo_gets_a_primary_repo_and_a_local_branch() {
        let store = Arc::new(Store::in_memory().unwrap());

        let created = create(
            &store,
            CreateProjectRequest {
                subpath: Some("services/auth".to_string()),
                branch_name: Some("  fix-login  ".to_string()),
                head_repo: Some("forker/widgets".to_string()),
                ..request_with_repo("Fix login")
            },
            false,
        )
        .unwrap();

        let project = store.get_project(&created.project.id).unwrap().unwrap();
        assert_eq!(project.location, store::ProjectLocation::Local);
        assert_eq!(project.github_repo.as_deref(), Some("acme/widgets"));
        assert_eq!(project.subpath.as_deref(), Some("services/auth"));

        let initial = created.initial_repo.unwrap();
        let repos = store.list_project_repos(&project.id).unwrap();
        assert_eq!(repos.len(), 1);
        let repo = &repos[0];
        assert_eq!(repo.id, initial.repo.id);
        assert_eq!(repo.github_repo, "acme/widgets");
        assert_eq!(repo.subpath.as_deref(), Some("services/auth"));
        assert_eq!(repo.branch_name, "fix-login");
        assert_eq!(repo.head_repo.as_deref(), Some("forker/widgets"));
        assert!(repo.is_primary);

        let branches = store.list_branches_for_project(&project.id).unwrap();
        assert_eq!(branches.len(), 1);
        let branch = &branches[0];
        assert_eq!(branch.id, initial.branch.id);
        assert_eq!(branch.project_repo_id.as_deref(), Some(repo.id.as_str()));
        assert_eq!(branch.branch_name, "fix-login");
        assert_eq!(branch.base_branch, "origin/main");
        assert_eq!(branch.branch_type, store::BranchType::Local);
        assert_eq!(branch.workspace_name, None);
        assert_eq!(branch.workspace_status, None);
        assert_eq!(branch.pr_number, None);

        let contexts = store.list_action_contexts().unwrap();
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].github_repo, "acme/widgets");
        assert_eq!(contexts[0].subpath.as_deref(), Some("services/auth"));
        let recent = store.list_recent_repos(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].github_repo, "acme/widgets");
        assert_eq!(recent[0].subpath.as_deref(), Some("services/auth"));
    }

    /// With no branch named, the repo's branch is the one inferred from the
    /// project name, as when the form's field is left blank.
    #[test]
    fn a_blank_branch_name_is_inferred_from_the_project_name() {
        let store = Arc::new(Store::in_memory().unwrap());

        let created = create(
            &store,
            CreateProjectRequest {
                branch_name: Some("   ".to_string()),
                ..request_with_repo("Fix login")
            },
            false,
        )
        .unwrap();

        let initial = created.initial_repo.unwrap();
        let inferred = branches::infer_prefixed_branch_name("Fix login");
        assert_eq!(initial.repo.branch_name, inferred);
        assert_eq!(initial.branch.branch_name, inferred);
    }

    /// A remote project created with a repo gets the branch that provisions
    /// its workspace: remote, `Starting`, and naming the workspace after
    /// itself.
    #[test]
    fn a_remote_project_with_a_repo_gets_a_starting_branch_and_a_workspace_name() {
        let store = Arc::new(Store::in_memory().unwrap());

        let created = create(
            &store,
            CreateProjectRequest {
                location: Some("remote".to_string()),
                branch_name: Some("fix-login".to_string()),
                ..request_with_repo("Fix login")
            },
            true,
        )
        .unwrap();

        let project = store.get_project(&created.project.id).unwrap().unwrap();
        assert_eq!(project.location, store::ProjectLocation::Remote);
        let initial = created.initial_repo.unwrap();
        assert!(initial.repo.is_primary);
        let branches = store.list_branches_for_project(&project.id).unwrap();
        assert_eq!(branches.len(), 1);
        let branch = &branches[0];
        assert_eq!(branch.id, initial.branch.id);
        assert_eq!(
            branch.project_repo_id.as_deref(),
            Some(initial.repo.id.as_str())
        );
        assert_eq!(branch.branch_type, store::BranchType::Remote);
        assert_eq!(
            branch.workspace_status,
            Some(store::WorkspaceStatus::Starting)
        );
        assert_eq!(
            branch.workspace_name.as_deref(),
            Some(branches::infer_workspace_name(&project.id, "fix-login").as_str())
        );
        assert_eq!(branch.base_branch, "origin/main");
    }

    /// A remote project created without a repo has no branch and so no
    /// workspace: its first repo provisions one (see `add_repo`).
    #[test]
    fn a_remote_project_without_a_repo_has_no_branch() {
        let store = Arc::new(Store::in_memory().unwrap());

        let created = create(
            &store,
            CreateProjectRequest {
                location: Some("remote".to_string()),
                ..request("Cloud only")
            },
            true,
        )
        .unwrap();

        assert!(created.initial_repo.is_none());
        let project = store.get_project(&created.project.id).unwrap().unwrap();
        assert_eq!(project.location, store::ProjectLocation::Remote);
        assert!(store.list_project_repos(&project.id).unwrap().is_empty());
        assert!(store
            .list_branches_for_project(&project.id)
            .unwrap()
            .is_empty());
    }

    /// A project created from a pull request records its number on the
    /// branch, local or remote.
    #[test]
    fn the_pr_number_is_recorded_on_the_initial_branch() {
        for (location, remote_available) in [(None, false), (Some("remote"), true)] {
            let store = Arc::new(Store::in_memory().unwrap());

            let created = create(
                &store,
                CreateProjectRequest {
                    location: location.map(str::to_string),
                    branch_name: Some("contributor-fix".to_string()),
                    pr_number: Some(1234),
                    ..request_with_repo("PR 1234")
                },
                remote_available,
            )
            .unwrap();

            let branches = store
                .list_branches_for_project(&created.project.id)
                .unwrap();
            assert_eq!(branches.len(), 1, "{location:?}");
            assert_eq!(branches[0].pr_number, Some(1234), "{location:?}");
        }
    }

    /// A name that is empty once trimmed is refused before anything is
    /// written.
    #[test]
    fn an_empty_name_is_refused() {
        let store = Arc::new(Store::in_memory().unwrap());

        for name in ["", "   \t"] {
            let err = create(&store, request_with_repo(name), false).unwrap_err();
            assert_eq!(err, "Project name is required");
        }

        assert!(store.list_projects().unwrap().is_empty());
        assert!(store.list_recent_repos(10).unwrap().is_empty());
    }

    /// Without `sq` a remote request falls back to a local project, branch
    /// included, instead of one whose workspace could never start.
    #[test]
    fn a_remote_request_falls_back_to_local_without_sq() {
        let store = Arc::new(Store::in_memory().unwrap());

        let created = create(
            &store,
            CreateProjectRequest {
                location: Some("remote".to_string()),
                ..request_with_repo("Fix login")
            },
            false,
        )
        .unwrap();

        let project = store.get_project(&created.project.id).unwrap().unwrap();
        assert_eq!(project.location, store::ProjectLocation::Local);
        let branches = store.list_branches_for_project(&project.id).unwrap();
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].branch_type, store::BranchType::Local);
        assert_eq!(branches[0].workspace_name, None);
        assert_eq!(branches[0].workspace_status, None);
    }

    /// A taken name is refused by name, before anything is written, rather
    /// than with the unique index's raw constraint error.
    #[test]
    fn a_duplicate_name_is_refused_with_a_friendly_error() {
        let store = Arc::new(Store::in_memory().unwrap());
        let first = create(&store, request("Fix login"), false).unwrap();

        // Trimmed the same way as the name that was stored.
        let err = create(&store, request_with_repo(" Fix login "), false).unwrap_err();

        assert_eq!(err, "A project named \"Fix login\" already exists.");
        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, first.project.id);
        assert!(store.list_recent_repos(10).unwrap().is_empty());
        assert!(store.list_action_contexts().unwrap().is_empty());
    }

    /// The check runs on the name as stored. A project named after its repo
    /// gets the subpath appended (`Project::with_subpath`), so the same
    /// request twice collides on the extended name.
    #[test]
    fn the_duplicate_check_uses_the_name_as_stored() {
        let store = Arc::new(Store::in_memory().unwrap());
        let in_subpath = CreateProjectRequest {
            subpath: Some("services/auth".to_string()),
            ..request_with_repo("widgets")
        };
        let first = create(&store, in_subpath.clone(), false).unwrap();
        assert_eq!(first.project.name, "widgets (services/auth)");

        let err = create(&store, in_subpath, false).unwrap_err();

        assert_eq!(
            err,
            "A project named \"widgets (services/auth)\" already exists."
        );
        // The bare name is still free.
        create(&store, request("widgets"), false).unwrap();
    }

    /// A name taken between the check and the insert fails on the unique
    /// index, and reads the same as the check; any other insert failure
    /// keeps its own text.
    #[test]
    fn a_unique_index_failure_reads_as_a_duplicate_name() {
        let store = Arc::new(Store::in_memory().unwrap());
        store
            .create_project(&store::Project::named("Fix login"))
            .unwrap();

        // Past the check, as a create that lost the race would be.
        let err = insert_project(&store, &store::Project::named("Fix login")).unwrap_err();
        assert_eq!(err, "A project named \"Fix login\" already exists.");

        // The same id under a new name is a primary-key failure, not a name
        // clash.
        let mut same_id = store::Project::named("Another");
        same_id.id = store.get_project_by_name("Fix login").unwrap().unwrap().id;
        let err = insert_project(&store, &same_id).unwrap_err();
        assert!(!err.contains("already exists"), "unexpected error: {err}");
    }
}
