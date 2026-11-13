//! Tests for the control request builders, against a fake GitHub and an
//! in-memory store.

use super::*;
use crate::control_server::{Access, Actor, Scope};
use crate::project_commands::create::{create_project_impl, CreatedProject};
use crate::project_commands::repo_spec::tests::{pull_request, FakeHost};
use crate::store;

fn store() -> Arc<Store> {
    Arc::new(Store::in_memory().unwrap())
}

/// The grant a session spawned in project `own` holds.
fn session_grant(session_id: &str) -> ControlGrant {
    ControlGrant {
        actor: Actor::Session {
            session_id: session_id.to_string(),
            epoch: 1,
        },
        scope: Scope::Project("own".to_string()),
        branch_id: None,
        access: Access::ReadWrite,
    }
}

/// The grant a connected app approved for every project holds.
fn all_projects_app_grant(grant_id: &str) -> ControlGrant {
    ControlGrant {
        actor: Actor::App {
            grant_id: grant_id.to_string(),
            client: "test".to_string(),
        },
        scope: Scope::AllProjects,
        branch_id: None,
        access: Access::ReadWrite,
    }
}

fn session(session_id: &str) -> ProjectCreator {
    ProjectCreator::for_grant(&session_grant(session_id))
}

fn bare(name: &str) -> ControlCreateProject {
    ControlCreateProject {
        name: name.to_string(),
        ..Default::default()
    }
}

fn repo(github_repo: &str) -> RepoSpecRequest {
    RepoSpecRequest {
        github_repo: github_repo.to_string(),
        ..Default::default()
    }
}

fn with_repo(name: &str, repo: RepoSpecRequest) -> ControlCreateProject {
    ControlCreateProject {
        repo: Some(repo),
        ..bare(name)
    }
}

/// A control create from request to rows: what the `create_project` arm
/// does for a control caller, short of the background setup.
fn create(
    host: &FakeHost,
    store: &Arc<Store>,
    creator: ProjectCreator,
    ask: ControlCreateProject,
) -> Result<CreatedProject, String> {
    let request = create_project_request(host, store, creator, ask, true)?;
    write(store, request)
}

fn write(store: &Arc<Store>, request: CreateProjectRequest) -> Result<CreatedProject, String> {
    tauri::async_runtime::block_on(create_project_impl(Arc::clone(store), request, true))
}

fn created_by(store: &Store, project_id: &str) -> Option<String> {
    store.get_project(project_id).unwrap().unwrap().created_by
}

/// A local project the user made, holding `acme/widgets` on `branch`.
fn project_with_widgets(store: &Store, branch: &str, subpath: Option<&str>) -> String {
    let project = store::Project::named("alpha");
    store.create_project(&project).unwrap();
    let repo = store::ProjectRepo::new(
        &project.id,
        "acme/widgets",
        branch,
        subpath.map(str::to_string),
    );
    store.create_project_repo(&repo).unwrap();
    project.id
}

fn add(repo: RepoSpecRequest) -> ControlAddRepo {
    ControlAddRepo { repo, reason: None }
}

// --- Who created it -----------------------------------------------

#[test]
fn a_session_creates_a_project_stamped_with_its_session_id() {
    let store = store();
    let host = FakeHost::with_widgets();
    let creator = session("s1");
    assert_eq!(creator.actor, "session:s1");
    assert_eq!(creator.limit, Some(CREATED_PROJECT_LIMIT));

    let created = create(&host, &store, creator, bare("Fix login")).unwrap();

    assert_eq!(
        created_by(&store, &created.project.id).as_deref(),
        Some("session:s1")
    );
    assert_eq!(
        store.list_project_ids_created_by("session:s1").unwrap(),
        vec![created.project.id.clone()],
        "the project is what the session's reach is looked up from"
    );
    assert_eq!(created.project.location, store::ProjectLocation::Local);
}

#[test]
fn an_all_projects_app_creates_a_project_stamped_with_its_grant_id() {
    let store = store();
    let host = FakeHost::with_widgets();
    let creator = ProjectCreator::for_grant(&all_projects_app_grant("g1"));
    assert_eq!(creator.actor, "app:g1");
    assert_eq!(creator.limit, None, "an all-projects grant is not limited");

    let created = create(&host, &store, creator, bare("Fix login")).unwrap();

    assert_eq!(
        created_by(&store, &created.project.id).as_deref(),
        Some("app:g1")
    );
}

/// A repo, a pull request and a note change nothing about the stamp.
#[test]
fn a_project_created_with_a_repo_is_stamped_too() {
    let store = store();
    let host = FakeHost::with_widgets();

    let created = create(
        &host,
        &store,
        session("s1"),
        with_repo("Fix login", repo("acme/widgets")),
    )
    .unwrap();

    assert_eq!(
        created_by(&store, &created.project.id).as_deref(),
        Some("session:s1")
    );
    assert!(created.initial_repo.is_some());
}

// --- The creation limit -------------------------------------------

#[test]
fn a_session_is_refused_its_eleventh_project_and_admitted_again_after_a_deletion() {
    let store = store();
    let host = FakeHost::with_widgets();
    let mut ids = Vec::new();
    for n in 0..CREATED_PROJECT_LIMIT {
        let created = create(&host, &store, session("s1"), bare(&format!("Project {n}")))
            .unwrap_or_else(|e| panic!("project {n} is within the limit: {e}"));
        ids.push(created.project.id);
    }

    let err = create(
        &host,
        &store,
        session("s1"),
        with_repo("One too many", repo("acme/widgets")),
    )
    .unwrap_err();
    assert_eq!(
        err,
        "This session has created 10 projects, the limit for one session. \
         Deleting one in Staged frees a slot."
    );
    assert!(
        host.calls().is_empty(),
        "the limit is checked before GitHub is asked anything: {:?}",
        host.calls()
    );
    assert!(store.get_project_by_name("One too many").unwrap().is_none());

    // The count is of the rows, so the user deleting one frees a slot.
    store.delete_project(&ids[3]).unwrap();
    create(&host, &store, session("s1"), bare("One too many")).unwrap();
    assert_eq!(
        store
            .list_project_ids_created_by("session:s1")
            .unwrap()
            .len(),
        CREATED_PROJECT_LIMIT
    );
}

#[test]
fn the_limit_is_each_creators_own() {
    let store = store();
    let host = FakeHost::with_widgets();
    for n in 0..CREATED_PROJECT_LIMIT {
        create(&host, &store, session("s1"), bare(&format!("Project {n}"))).unwrap();
    }
    // Projects the user made count toward nobody's limit.
    store
        .create_project(&store::Project::named("Made in the app"))
        .unwrap();

    create(&host, &store, session("s2"), bare("Another session's")).unwrap();
    let app = ProjectCreator::for_grant(&all_projects_app_grant("g1"));
    for n in 0..=CREATED_PROJECT_LIMIT {
        create(
            &host,
            &store,
            app.clone(),
            bare(&format!("App project {n}")),
        )
        .unwrap_or_else(|e| panic!("an all-projects grant has no limit: {e}"));
    }
}

/// Two creates that both passed the early check cannot both take the last
/// slot: the count is taken again where the row is written.
#[test]
fn the_limit_is_held_where_the_row_is_written() {
    let store = store();
    let host = FakeHost::with_widgets();
    for n in 0..CREATED_PROJECT_LIMIT - 1 {
        create(&host, &store, session("s1"), bare(&format!("Project {n}"))).unwrap();
    }
    let first = create_project_request(&host, &store, session("s1"), bare("First"), true).unwrap();
    let second =
        create_project_request(&host, &store, session("s1"), bare("Second"), true).unwrap();

    write(&store, first).unwrap();
    let err = write(&store, second).unwrap_err();

    assert!(err.contains("the limit for one session"), "{err}");
    assert!(store.get_project_by_name("Second").unwrap().is_none());
}

// --- What is refused before GitHub is asked ------------------------

#[test]
fn a_blank_name_is_refused_before_anything_is_looked_up() {
    let store = store();
    let host = FakeHost::with_widgets();

    let err = create_project_request(
        &host,
        &store,
        session("s1"),
        with_repo("   ", repo("acme/widgets")),
        true,
    )
    .unwrap_err();

    assert_eq!(err, "Project name is required");
    assert!(host.calls().is_empty(), "{:?}", host.calls());
}

/// The forms never offer a remote project without `sq`, and the shared
/// body would create a local one instead. A control caller asked for a
/// remote project by name, so it is told.
#[test]
fn a_remote_project_without_sq_is_refused_and_nothing_is_written() {
    let store = store();
    let host = FakeHost::with_widgets();
    let ask = ControlCreateProject {
        remote: true,
        ..with_repo("Cloud", repo("acme/widgets"))
    };

    let err = create_project_request(&host, &store, session("s1"), ask.clone(), false).unwrap_err();

    assert!(err.contains("`sq`"), "{err}");
    assert!(host.calls().is_empty(), "{:?}", host.calls());
    assert!(store.list_projects().unwrap().is_empty());

    // With `sq` the same request is a remote project.
    let request = create_project_request(&host, &store, session("s1"), ask, true).unwrap();
    assert_eq!(request.location.as_deref(), Some("remote"));
    let created = write(&store, request).unwrap();
    assert_eq!(created.project.location, store::ProjectLocation::Remote);
}

#[test]
fn a_local_project_does_not_care_whether_sq_is_installed() {
    let store = store();
    let host = FakeHost::with_widgets();
    let request =
        create_project_request(&host, &store, session("s1"), bare("Local"), false).unwrap();
    assert_eq!(request.location.as_deref(), Some("local"));
}

#[test]
fn a_project_without_a_repo_asks_github_nothing() {
    let store = store();
    let host = FakeHost::with_widgets();

    let request =
        create_project_request(&host, &store, session("s1"), bare("Fix login"), true).unwrap();

    assert!(host.calls().is_empty(), "{:?}", host.calls());
    assert_eq!(request.name, "Fix login");
    assert!(request.github_repo.is_none());
    assert!(request.pr_note.is_none());
    assert_eq!(request.created_by, Some(session("s1")));
}

// --- The repository -----------------------------------------------

#[test]
fn the_repository_is_resolved_the_way_a_form_would_check_it() {
    let store = store();
    let mut host = FakeHost::with_widgets();
    host.add_directory("acme/widgets", "services/auth");

    let request = create_project_request(
        &host,
        &store,
        session("s1"),
        with_repo(
            "Fix login",
            RepoSpecRequest {
                subpath: Some("/services/auth/".to_string()),
                ..repo("Acme/Widgets")
            },
        ),
        true,
    )
    .unwrap();

    assert_eq!(request.github_repo.as_deref(), Some("acme/widgets"));
    assert_eq!(request.subpath.as_deref(), Some("services/auth"));
    assert_eq!(request.default_branch.as_deref(), Some("main"));
    assert_eq!(request.branch_name, None, "left for the body to infer");
    assert_eq!(request.pr_number, None);
    assert_eq!(request.head_repo, None);
    assert_eq!(request.pr_note, None);
}

#[test]
fn a_repo_github_does_not_know_is_refused_and_nothing_is_written() {
    let store = store();
    let host = FakeHost::with_widgets();

    let err = create(
        &host,
        &store,
        session("s1"),
        with_repo("Fix login", repo("acme/missing")),
    )
    .unwrap_err();

    assert!(err.contains("was not found on GitHub"), "{err}");
    assert!(store.list_projects().unwrap().is_empty());
}

#[test]
fn a_subpath_that_is_not_in_the_repo_is_refused() {
    let store = store();
    let host = FakeHost::with_widgets();

    let err = create(
        &host,
        &store,
        session("s1"),
        with_repo(
            "Fix login",
            RepoSpecRequest {
                subpath: Some("no/such/dir".to_string()),
                ..repo("acme/widgets")
            },
        ),
    )
    .unwrap_err();

    assert!(err.contains("Subpath 'no/such/dir'"), "{err}");
    assert!(store.list_projects().unwrap().is_empty());
}

#[test]
fn creating_at_a_monorepo_root_is_refused_unless_overridden() {
    let store = store();
    let mut host = FakeHost::with_widgets();
    host.module_counts.insert("acme/widgets".to_string(), 20);

    let err = create(
        &host,
        &store,
        session("s1"),
        with_repo("Fix login", repo("acme/widgets")),
    )
    .unwrap_err();
    assert!(err.contains("monorepo"), "{err}");
    assert!(
        err.contains("--allow-monorepo-root"),
        "the refusal names the way past it: {err}"
    );
    assert!(store.list_projects().unwrap().is_empty());

    let created = create(
        &host,
        &store,
        session("s1"),
        with_repo(
            "Fix login",
            RepoSpecRequest {
                allow_monorepo_root: true,
                ..repo("acme/widgets")
            },
        ),
    )
    .unwrap();
    let initial = created.initial_repo.expect("created with the repo");
    assert_eq!(initial.repo.subpath, None);
}

#[test]
fn adding_a_monorepo_root_is_refused_unless_overridden() {
    let store = store();
    let project_id = project_with_widgets(&store, "fix-login", None);
    let mut host = FakeHost::with_widgets();
    host.add_repo("acme/platform", "main");
    host.module_counts.insert("acme/platform".to_string(), 35);

    let err = add_repo_request(&host, &store, &project_id, add(repo("acme/platform"))).unwrap_err();
    assert!(err.contains("--allow-monorepo-root"), "{err}");

    let request = add_repo_request(
        &host,
        &store,
        &project_id,
        add(RepoSpecRequest {
            allow_monorepo_root: true,
            ..repo("acme/platform")
        }),
    )
    .unwrap();
    assert_eq!(request.github_repo, "acme/platform");
    assert_eq!(request.subpath, None);
}

// --- Pull requests ------------------------------------------------

#[test]
fn a_pull_request_brings_its_branch_base_number_and_note() {
    let store = store();
    let mut host = FakeHost::with_widgets();
    host.add_pull_request(
        "acme/widgets",
        pull_request(42, "fix-login", Some("fork/widgets")),
    );

    let created = create(
        &host,
        &store,
        session("s1"),
        with_repo(
            "Fix login",
            RepoSpecRequest {
                pr_number: Some(42),
                ..repo("acme/widgets")
            },
        ),
    )
    .unwrap();

    let initial = created.initial_repo.expect("created with the repo");
    assert_eq!(initial.repo.branch_name, "fix-login");
    assert_eq!(initial.repo.head_repo.as_deref(), Some("fork/widgets"));
    assert_eq!(initial.branch.pr_number, Some(42));
    assert_eq!(initial.branch.base_branch, "origin/develop");
    // The forms write this note with a second command. A control caller
    // makes one call, so the body writes it.
    let notes = store.list_project_notes(&created.project.id).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "PR #42: Change 42");
    assert_eq!(notes[0].content, "Why change 42 is needed.");
}

/// The forms write the note for a branch whose open pull request the
/// picker found, not only for one picked by number.
#[test]
fn a_branch_with_an_open_pull_request_gets_the_note_too() {
    let store = store();
    let mut host = FakeHost::with_widgets();
    host.branch_pull_requests.insert(
        ("acme/widgets".to_string(), "fix-login".to_string()),
        vec![pull_request(7, "fix-login", Some("acme/widgets"))],
    );

    let request = create_project_request(
        &host,
        &store,
        session("s1"),
        with_repo(
            "Fix login",
            RepoSpecRequest {
                branch_name: Some("fix-login".to_string()),
                ..repo("acme/widgets")
            },
        ),
        true,
    )
    .unwrap();

    assert_eq!(request.pr_number, Some(7));
    assert_eq!(
        request.pr_note,
        Some(PrNote {
            title: "PR #7: Change 7".to_string(),
            content: "Why change 7 is needed.".to_string(),
        })
    );
}

#[test]
fn a_project_without_a_pull_request_gets_no_note() {
    let store = store();
    let host = FakeHost::with_widgets();

    let created = create(
        &host,
        &store,
        session("s1"),
        with_repo("Fix login", repo("acme/widgets")),
    )
    .unwrap();

    assert!(store
        .list_project_notes(&created.project.id)
        .unwrap()
        .is_empty());
}

// --- Adding a repo ------------------------------------------------

#[test]
fn adding_to_a_project_that_does_not_exist_is_refused_before_any_lookup() {
    let store = store();
    let host = FakeHost::with_widgets();

    let err = add_repo_request(&host, &store, "nope", add(repo("acme/widgets"))).unwrap_err();

    assert_eq!(err, "Project not found: nope");
    assert!(host.calls().is_empty(), "{:?}", host.calls());
}

#[test]
fn an_added_repo_is_resolved_and_its_reason_passed_through() {
    let store = store();
    let project_id = project_with_widgets(&store, "fix-login", None);
    let mut host = FakeHost::with_widgets();
    host.add_repo("acme/gadgets", "trunk");
    host.add_directory("acme/gadgets", "packages/ui");

    let request = add_repo_request(
        &host,
        &store,
        &project_id,
        ControlAddRepo {
            repo: RepoSpecRequest {
                subpath: Some("packages/ui/".to_string()),
                ..repo("ACME/Gadgets")
            },
            reason: Some("  The buttons live here.  ".to_string()),
        },
    )
    .unwrap();

    assert_eq!(request.github_repo, "acme/gadgets");
    assert_eq!(request.subpath.as_deref(), Some("packages/ui"));
    assert_eq!(request.default_branch.as_deref(), Some("trunk"));
    assert_eq!(request.branch_name, None);
    assert_eq!(request.reason.as_deref(), Some("The buttons live here."));
    assert_eq!(
        request.set_as_primary, None,
        "a control caller never chooses the primary repo"
    );
    assert_eq!(request.pr_note, None);
}

#[test]
fn a_blank_reason_is_no_reason() {
    let store = store();
    let project_id = project_with_widgets(&store, "fix-login", None);
    let mut host = FakeHost::with_widgets();
    host.add_repo("acme/gadgets", "main");

    let request = add_repo_request(
        &host,
        &store,
        &project_id,
        ControlAddRepo {
            repo: repo("acme/gadgets"),
            reason: Some("   ".to_string()),
        },
    )
    .unwrap();

    assert_eq!(request.reason, None);
}

#[test]
fn an_explicit_branch_the_repo_already_has_in_the_project_is_refused() {
    let store = store();
    let project_id = project_with_widgets(&store, "fix-login", None);
    let mut host = FakeHost::with_widgets();
    host.add_directory("acme/widgets", "services/auth");
    host.add_repo("acme/gadgets", "main");
    let in_auth = |github_repo: &str, branch: Option<&str>| {
        add(RepoSpecRequest {
            subpath: Some("services/auth".to_string()),
            branch_name: branch.map(str::to_string),
            ..repo(github_repo)
        })
    };

    // The name is compared as GitHub spells it, whatever was typed.
    let err = add_repo_request(
        &host,
        &store,
        &project_id,
        in_auth("Acme/Widgets", Some("fix-login")),
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Branch 'fix-login' of 'acme/widgets' is already attached to this project at the \
         repository root. A branch can be checked out once per project, so give a different \
         branch name."
    );

    // Another branch of the same repo is fine.
    add_repo_request(
        &host,
        &store,
        &project_id,
        in_auth("acme/widgets", Some("fix-login-auth")),
    )
    .unwrap();
    // So is no branch at all: the body infers a name and keeps it apart
    // from the one already attached.
    let inferred =
        add_repo_request(&host, &store, &project_id, in_auth("acme/widgets", None)).unwrap();
    assert_eq!(inferred.branch_name, None);
    // And the same branch name in another repository, which has its own
    // clone.
    add_repo_request(
        &host,
        &store,
        &project_id,
        add(RepoSpecRequest {
            branch_name: Some("fix-login".to_string()),
            ..repo("acme/gadgets")
        }),
    )
    .unwrap();
}

#[test]
fn a_pull_request_whose_branch_is_already_attached_is_refused() {
    let store = store();
    let project_id = project_with_widgets(&store, "fix-login", Some("services/auth"));
    let mut host = FakeHost::with_widgets();
    host.add_pull_request("acme/widgets", pull_request(42, "fix-login", None));

    let err = add_repo_request(
        &host,
        &store,
        &project_id,
        add(RepoSpecRequest {
            pr_number: Some(42),
            allow_monorepo_root: true,
            ..repo("acme/widgets")
        }),
    )
    .unwrap_err();

    assert!(
        err.contains("Branch 'fix-login' of 'acme/widgets' is already attached")
            && err.contains("at subpath 'services/auth'"),
        "{err}"
    );
}

#[test]
fn an_added_pull_request_carries_its_branch_base_and_note() {
    let store = store();
    let project_id = project_with_widgets(&store, "fix-login", None);
    let mut host = FakeHost::with_widgets();
    host.add_repo("acme/gadgets", "main");
    host.add_pull_request(
        "acme/gadgets",
        pull_request(9, "new-button", Some("fork/gadgets")),
    );

    let request = add_repo_request(
        &host,
        &store,
        &project_id,
        add(RepoSpecRequest {
            pr_number: Some(9),
            ..repo("acme/gadgets")
        }),
    )
    .unwrap();

    assert_eq!(request.branch_name.as_deref(), Some("new-button"));
    assert_eq!(request.default_branch.as_deref(), Some("develop"));
    assert_eq!(request.pr_number, Some(9));
    assert_eq!(request.head_repo.as_deref(), Some("fork/gadgets"));
    assert_eq!(
        request.pr_note,
        Some(PrNote {
            title: "PR #9: Change 9".to_string(),
            content: "Why change 9 is needed.".to_string(),
        })
    );
}
