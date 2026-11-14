//! Tests for the control request builders, against a fake GitHub and an
//! in-memory store.

use super::*;
use crate::control_server::{Access, Actor, Scope};
use crate::project_commands::add_repo::add_project_repo_impl;
use crate::project_commands::create::{create_project_impl, CreatedProject};
use crate::project_commands::creation_limit::SESSION_LINEAGE_MAX_DEPTH;
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

/// What a chain that has used its allowance is told.
const LIMIT_REACHED: &str = "This session and the sessions in its chain have created 10 \
     projects, the limit they share. A chain is a session and every session started from it, \
     directly or through another. Deleting one of those projects in Staged frees a slot.";

/// A session row started by `starter` (an actor string), and its id.
fn session_started_by(store: &Store, starter: Option<&str>) -> String {
    let mut session = store::Session::new_queued("work");
    if let Some(starter) = starter {
        session = session.with_started_by(starter);
    }
    store.create_session(&session).unwrap();
    session.id
}

/// A session row started by the session `parent`, and its id.
fn child_of(store: &Store, parent: &str) -> String {
    session_started_by(store, Some(&format!("session:{parent}")))
}

/// A session row the session `parent` started through an MCP tool, as a
/// project session starts a repo session with `start_repo_session`: the
/// starter is in `parent_session_id`, and `started_by` is unset.
fn mcp_child_of(store: &Store, parent: &str) -> String {
    let session = store::Session::new_queued("repo work").with_parent_session(parent);
    assert_eq!(session.started_by, None);
    store.create_session(&session).unwrap();
    session.id
}

/// Create `count` projects as `session_id`, named after `label`.
fn create_many(host: &FakeHost, store: &Arc<Store>, session_id: &str, label: &str, count: usize) {
    for n in 0..count {
        create(
            host,
            store,
            session(session_id),
            bare(&format!("{label} {n}")),
        )
        .unwrap_or_else(|e| panic!("{label} {n} is within the limit: {e}"));
    }
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
    assert_eq!(err, LIMIT_REACHED);
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

    assert_eq!(err, LIMIT_REACHED);
    assert!(store.get_project_by_name("Second").unwrap().is_none());
}

// --- The limit is a chain's ---------------------------------------

/// The bypass the review found: a session at its limit starts a session,
/// and that one has a session id of its own. Counted per session id it got
/// a fresh allowance, and so did every session it started in turn. A
/// session and the sessions descended from it now draw on one allowance.
#[test]
fn a_session_started_by_a_session_shares_its_allowance() {
    let store = store();
    let host = FakeHost::with_widgets();
    let root = session_started_by(&store, None);
    let child = child_of(&store, &root);
    let grandchild = child_of(&store, &child);

    create_many(&host, &store, &root, "Root", 6);
    create_many(&host, &store, &child, "Child", 3);
    create_many(&host, &store, &grandchild, "Grandchild", 1);

    // Ten between them: none of the three may create another, and a
    // session started now is in the same chain.
    let newcomer = child_of(&store, &grandchild);
    for (who, session_id) in [
        ("the root", &root),
        ("its child", &child),
        ("its grandchild", &grandchild),
        ("a session started after the limit was reached", &newcomer),
    ] {
        let err = create(
            &host,
            &store,
            session(session_id),
            with_repo("One too many", repo("acme/widgets")),
        )
        .unwrap_err();
        assert_eq!(err, LIMIT_REACHED, "{who}");
    }
    assert!(
        host.calls().is_empty(),
        "the chain's limit is checked before GitHub is asked anything: {:?}",
        host.calls()
    );
    // Each project is still stamped with the session that made it, which is
    // what that session's reach is looked up from.
    assert_eq!(
        store
            .list_project_ids_created_by(&format!("session:{child}"))
            .unwrap()
            .len(),
        3
    );
}

/// The bypass the second review found. A session at its limit runs
/// `stagedctl session start --project` on a project it created, and that
/// project session calls the MCP `start_repo_session` tool. The repo session
/// records its starter in `parent_session_id` and has no `started_by`, so a
/// walk that followed `started_by` alone made it the root of a new chain,
/// with an allowance of its own. This is also how a project session hands
/// out repo work, so it happened without anyone trying.
#[test]
fn a_repo_session_started_through_mcp_shares_its_starters_allowance() {
    let store = store();
    let host = FakeHost::with_widgets();
    let root = session_started_by(&store, None);
    let project_session = child_of(&store, &root);
    let repo_session = mcp_child_of(&store, &project_session);
    // And onward: a session the repo session starts with `stagedctl`, and a
    // second repo session beside the first.
    let below = child_of(&store, &repo_session);
    let sibling = mcp_child_of(&store, &project_session);

    create_many(&host, &store, &root, "Root", CREATED_PROJECT_LIMIT - 2);
    // What the repo session creates counts against the root's allowance.
    let made = create(
        &host,
        &store,
        session(&repo_session),
        bare("Repo session's"),
    )
    .unwrap();
    assert_eq!(
        created_by(&store, &made.project.id),
        Some(format!("session:{repo_session}"))
    );
    create(&host, &store, session(&below), bare("Below's")).unwrap();

    for (who, session_id) in [
        ("the root", &root),
        ("the project session", &project_session),
        ("the repo session", &repo_session),
        ("a session the repo session started", &below),
        ("another repo session of the project session", &sibling),
    ] {
        let err = create(
            &host,
            &store,
            session(session_id),
            with_repo("One too many", repo("acme/widgets")),
        )
        .unwrap_err();
        assert_eq!(err, LIMIT_REACHED, "{who}");
    }
    assert!(host.calls().is_empty(), "{:?}", host.calls());

    // Deleting the repo session's project frees a slot for the whole chain.
    store.delete_project(&made.project.id).unwrap();
    create(&host, &store, session(&sibling), bare("Admitted")).unwrap();
    create(&host, &store, session(&root), bare("Refused")).unwrap_err();
}

/// Repo sessions under a project session the user started share one
/// allowance: the project session is their root.
#[test]
fn repo_sessions_of_a_user_started_project_session_share_one_allowance() {
    let store = store();
    let host = FakeHost::with_widgets();
    let project_session = session_started_by(&store, None);
    let first = mcp_child_of(&store, &project_session);
    let second = mcp_child_of(&store, &project_session);

    create_many(&host, &store, &first, "First", 6);
    create_many(&host, &store, &second, "Second", 4);

    for session_id in [&first, &second, &project_session] {
        assert_eq!(
            create(&host, &store, session(session_id), bare("One too many")).unwrap_err(),
            LIMIT_REACHED
        );
    }
    // Another project session's repo session is another chain.
    let elsewhere = mcp_child_of(&store, &session_started_by(&store, None));
    create(&host, &store, session(&elsewhere), bare("Elsewhere")).unwrap();
}

/// A session running in a project another session created can still create
/// projects. It draws on the allowance of the chain it belongs to.
#[test]
fn a_session_in_a_session_created_project_can_create_within_the_chains_allowance() {
    let store = store();
    let host = FakeHost::with_widgets();
    let root = session_started_by(&store, None);
    let made_by_root = create(&host, &store, session(&root), bare("Made by the root")).unwrap();
    assert_eq!(
        created_by(&store, &made_by_root.project.id),
        Some(format!("session:{root}"))
    );
    // The root starts a session in the project it made.
    let child = child_of(&store, &root);

    let made_by_child = create(&host, &store, session(&child), bare("Made by the child")).unwrap();

    assert_eq!(
        created_by(&store, &made_by_child.project.id),
        Some(format!("session:{child}"))
    );
    // Two of the chain's ten are used, so eight more fit, from either.
    create_many(&host, &store, &child, "Child", 4);
    create_many(&host, &store, &root, "Root", 4);
    let err = create(&host, &store, session(&child), bare("One too many")).unwrap_err();
    assert_eq!(err, LIMIT_REACHED);
}

/// The projects a chain holds are counted wherever in the chain they were
/// made, so deleting any of them frees a slot for every session in it.
#[test]
fn deleting_a_project_anywhere_in_the_chain_frees_a_slot_for_all_of_it() {
    let store = store();
    let host = FakeHost::with_widgets();
    let root = session_started_by(&store, None);
    let child = child_of(&store, &root);
    create_many(&host, &store, &root, "Root", CREATED_PROJECT_LIMIT - 1);
    let last = create(&host, &store, session(&child), bare("The child's")).unwrap();
    create(&host, &store, session(&root), bare("Refused")).unwrap_err();

    // The user deletes the child's project, and the root may create again.
    store.delete_project(&last.project.id).unwrap();

    create(&host, &store, session(&root), bare("Admitted")).unwrap();
    create(&host, &store, session(&child), bare("Refused again")).unwrap_err();
}

/// Chains are counted apart. Sessions the user started are different roots,
/// and so is a session a connected app started: an app is not a session.
#[test]
fn chains_under_different_roots_do_not_share_an_allowance() {
    let store = store();
    let host = FakeHost::with_widgets();
    let root = session_started_by(&store, None);
    let child = child_of(&store, &root);
    create_many(&host, &store, &child, "Full chain", CREATED_PROJECT_LIMIT);

    let another_root = session_started_by(&store, None);
    let under_another = child_of(&store, &another_root);
    let by_app = session_started_by(&store, Some("app:g1"));
    let by_same_app = session_started_by(&store, Some("app:g1"));
    for session_id in [&another_root, &under_another, &by_app, &by_same_app] {
        create(
            &host,
            &store,
            session(session_id),
            bare(&format!("From {session_id}")),
        )
        .unwrap_or_else(|e| panic!("another chain has its own allowance: {e}"));
    }
    // A session the store holds no row for is a chain of its own, as the
    // tests above this section rely on.
    create(&host, &store, session("no-row"), bare("From no-row")).unwrap();
}

/// Two sessions of one chain that both passed the early check cannot both
/// take the chain's last slot: the chain is counted again, under the lock,
/// where the row is written.
#[test]
fn a_chains_limit_is_held_where_the_row_is_written() {
    let store = store();
    let host = FakeHost::with_widgets();
    let root = session_started_by(&store, None);
    let child = child_of(&store, &root);
    create_many(&host, &store, &root, "Root", CREATED_PROJECT_LIMIT - 1);
    let from_root =
        create_project_request(&host, &store, session(&root), bare("Root's"), true).unwrap();
    let from_child =
        create_project_request(&host, &store, session(&child), bare("Child's"), true).unwrap();

    write(&store, from_child).unwrap();
    let err = write(&store, from_root).unwrap_err();

    assert_eq!(err, LIMIT_REACHED);
    assert!(store.get_project_by_name("Root's").unwrap().is_none());
}

/// The same, with the creates racing for real: many sessions of one chain
/// ask at once for more projects than the chain has slots left.
#[test]
fn racing_creates_across_a_chain_never_exceed_its_allowance() {
    let store = store();
    let root = session_started_by(&store, None);
    let mut sessions = vec![root.clone()];
    for _ in 0..7 {
        let parent = sessions.last().unwrap().clone();
        sessions.push(child_of(&store, &parent));
    }
    let host = FakeHost::with_widgets();
    create_many(&host, &store, &root, "Before", CREATED_PROJECT_LIMIT - 3);
    let start = std::sync::Barrier::new(sessions.len());

    let admitted = std::thread::scope(|scope| {
        let creates: Vec<_> = sessions
            .iter()
            .map(|session_id| {
                let (store, start) = (&store, &start);
                scope.spawn(move || {
                    let host = FakeHost::with_widgets();
                    start.wait();
                    create(
                        &host,
                        store,
                        session(session_id),
                        bare(&format!("Racing {session_id}")),
                    )
                    .is_ok()
                })
            })
            .collect();
        creates
            .into_iter()
            .map(|create| create.join().unwrap())
            .filter(|admitted| *admitted)
            .count()
    });

    assert_eq!(admitted, 3, "exactly the free slots are taken");
    let held: usize = store
        .count_projects_by_creator()
        .unwrap()
        .iter()
        .map(|(_, projects)| projects)
        .sum();
    assert_eq!(held, CREATED_PROJECT_LIMIT);
}

/// The walk to the root is bounded. A session further down than the bound
/// is refused, and is not handed an allowance counted from part-way up.
#[test]
fn a_session_too_far_down_its_chain_is_refused() {
    let store = store();
    let host = FakeHost::with_widgets();
    let root = session_started_by(&store, None);
    let mut deepest = root.clone();
    for _ in 0..SESSION_LINEAGE_MAX_DEPTH {
        deepest = child_of(&store, &deepest);
    }

    // As far down as the walk reaches: still in the root's chain.
    create(&host, &store, session(&deepest), bare("At the bound")).unwrap();
    assert_eq!(
        created_by(
            &store,
            &store
                .get_project_by_name("At the bound")
                .unwrap()
                .unwrap()
                .id
        ),
        Some(format!("session:{deepest}"))
    );
    create_many(&host, &store, &root, "Root", CREATED_PROJECT_LIMIT - 1);
    assert_eq!(
        create(&host, &store, session(&deepest), bare("Shared")).unwrap_err(),
        LIMIT_REACHED
    );

    // One further: refused whatever the chain holds.
    let beyond = child_of(&store, &deepest);
    let err = create(
        &host,
        &store,
        session(&beyond),
        with_repo("Too deep", repo("acme/widgets")),
    )
    .unwrap_err();
    assert_eq!(
        err,
        "This session is more than 32 sessions removed from the one that started its chain, \
         and a chain that long may not create projects."
    );
    assert!(host.calls().is_empty(), "{:?}", host.calls());
    assert!(store.get_project_by_name("Too deep").unwrap().is_none());
}

/// The actor string a grant records is what the chain is walked from.
#[test]
fn a_session_grants_actor_names_the_session_the_chain_is_walked_from() {
    let creator = ProjectCreator::for_grant(&session_grant("s1"));
    assert_eq!(store::session_actor_id(&creator.actor), Some("s1"));
    let app = ProjectCreator::for_grant(&all_projects_app_grant("g1"));
    assert_eq!(store::session_actor_id(&app.actor), None);
}

// --- A name already in use ----------------------------------------

/// A taken name used to be refused only by the shared body, after the
/// resolver had asked GitHub about the repository.
#[test]
fn a_taken_name_is_refused_before_github_is_asked_anything() {
    let store = store();
    let host = FakeHost::with_widgets();
    create(&host, &store, session("s1"), bare("Fix login")).unwrap();

    let err = create_project_request(
        &host,
        &store,
        session("s2"),
        with_repo(" Fix login ", repo("acme/widgets")),
        true,
    )
    .unwrap_err();

    assert_eq!(err, "A project named \"Fix login\" already exists.");
    assert!(host.calls().is_empty(), "{:?}", host.calls());
}

/// With a subpath the stored name is not known until the repository is
/// (`Project::with_subpath` extends a project named after its repository),
/// so the early check lets a request through while a candidate is free, and
/// the shared body decides.
#[test]
fn a_name_that_may_be_extended_is_left_to_the_shared_body() {
    let store = store();
    let mut host = FakeHost::with_widgets();
    host.add_directory("acme/widgets", "services/auth");
    create(&host, &store, session("s1"), bare("widgets")).unwrap();
    let in_auth = |name: &str| {
        with_repo(
            name,
            RepoSpecRequest {
                subpath: Some("/services/auth/".to_string()),
                ..repo("Acme/Widgets")
            },
        )
    };

    // Named after the repository as GitHub spells it: stored extended.
    let extended = create(&host, &store, session("s1"), in_auth("widgets")).unwrap();
    assert_eq!(extended.project.name, "widgets (services/auth)");

    // Both candidates are taken now, so the same request is refused early.
    let calls_before = host.calls().len();
    let err =
        create_project_request(&host, &store, session("s1"), in_auth("widgets"), true).unwrap_err();
    assert_eq!(err, "A project named \"widgets\" already exists.");
    assert_eq!(host.calls().len(), calls_before, "nothing more was asked");

    // A name that is not the repository's is stored as given. `Fix login`
    // is taken, but `Fix login (services/auth)` is free and the repository
    // is not known yet, so the request passes the early check.
    create(&host, &store, session("s1"), bare("Fix login")).unwrap();
    let request =
        create_project_request(&host, &store, session("s1"), in_auth("Fix login"), true).unwrap();
    // The shared body knows the stored name and refuses it.
    let err = write(&store, request).unwrap_err();
    assert_eq!(err, "A project named \"Fix login\" already exists.");
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

/// A control caller's request asks the shared body to check the branch
/// again beside its write.
#[test]
fn a_control_add_has_its_branch_checked_again_at_the_write() {
    let store = store();
    let project_id = project_with_widgets(&store, "fix-login", None);
    let mut host = FakeHost::with_widgets();
    host.add_repo("acme/gadgets", "main");

    let request = add_repo_request(&host, &store, &project_id, add(repo("acme/gadgets"))).unwrap();

    assert!(request.refuse_branch_in_use);
}

/// The rows an add writes, for a request that passed the early checks.
fn write_add(
    store: &Arc<Store>,
    project_id: &str,
    request: AddProjectRepoRequest,
) -> Result<store::ProjectRepo, String> {
    tauri::async_runtime::block_on(add_project_repo_impl(
        Arc::clone(store),
        project_id.to_string(),
        request.github_repo,
        request.branch_name,
        request.subpath,
        request.set_as_primary,
        request.reason,
        request.pr_number,
        request.default_branch,
        request.head_repo,
        request.refuse_branch_in_use,
    ))
    .map(|added| added.repo)
}

/// `acme/widgets` with the directories the parallel adds below attach.
fn widgets_with_services() -> FakeHost {
    let mut host = FakeHost::with_widgets();
    host.add_directory("acme/widgets", "services/auth");
    host.add_directory("acme/widgets", "services/billing");
    host
}

fn branch_in(subpath: &str, branch: &str) -> ControlAddRepo {
    add(RepoSpecRequest {
        subpath: Some(subpath.to_string()),
        branch_name: Some(branch.to_string()),
        ..repo("acme/widgets")
    })
}

/// The gap the review found: the branch was checked before the shared
/// body's awaits and not again, so two adds of one branch that overlapped
/// both passed, and the second checkout failed in the background. Both
/// requests here passed the early check before either wrote.
#[test]
fn two_adds_of_one_branch_that_both_passed_the_early_check_cannot_both_be_written() {
    let store = store();
    let project_id = project_with_widgets(&store, "main-work", None);
    let host = widgets_with_services();
    let first = add_repo_request(
        &host,
        &store,
        &project_id,
        branch_in("services/auth", "fix-login"),
    )
    .unwrap();
    let second = add_repo_request(
        &host,
        &store,
        &project_id,
        branch_in("services/billing", "fix-login"),
    )
    .unwrap();

    write_add(&store, &project_id, first).unwrap();
    let err = write_add(&store, &project_id, second).unwrap_err();

    assert_eq!(
        err,
        "Branch 'fix-login' of 'acme/widgets' is already attached to this project at subpath \
         'services/auth'. A branch can be checked out once per project, so give a different \
         branch name."
    );
    let repos = store.list_project_repos(&project_id).unwrap();
    assert_eq!(repos.len(), 2, "the project's own repo and the first add");
    assert_eq!(
        store.list_branches_for_project(&project_id).unwrap().len(),
        1,
        "only the first add wrote a branch"
    );
}

/// The same with the writes racing for real: of several adds of one branch
/// to different subpaths, exactly one is written.
#[test]
fn racing_adds_of_one_branch_write_exactly_one() {
    let store = store();
    let project_id = project_with_widgets(&store, "main-work", None);
    let subpaths: Vec<String> = (0..6).map(|n| format!("services/s{n}")).collect();
    let mut host = FakeHost::with_widgets();
    for subpath in &subpaths {
        host.add_directory("acme/widgets", subpath);
    }
    let requests: Vec<AddProjectRepoRequest> = subpaths
        .iter()
        .map(|subpath| {
            add_repo_request(&host, &store, &project_id, branch_in(subpath, "fix-login")).unwrap()
        })
        .collect();
    let start = std::sync::Barrier::new(requests.len());

    let written = std::thread::scope(|scope| {
        let adds: Vec<_> = requests
            .into_iter()
            .map(|request| {
                let (store, project_id, start) = (&store, &project_id, &start);
                scope.spawn(move || {
                    start.wait();
                    write_add(store, project_id, request).is_ok()
                })
            })
            .collect();
        adds.into_iter()
            .map(|add| add.join().unwrap())
            .filter(|written| *written)
            .count()
    });

    assert_eq!(written, 1);
    let attached = store
        .list_project_repos(&project_id)
        .unwrap()
        .iter()
        .filter(|repo| repo.branch_name == "fix-login")
        .count();
    assert_eq!(attached, 1);
}

/// A request from a form is not held to the check, as before: the body
/// writes the branch the user picked.
#[test]
fn an_add_from_a_form_is_not_refused_at_the_write() {
    let store = store();
    let project_id = project_with_widgets(&store, "fix-login", None);

    let repo = write_add(
        &store,
        &project_id,
        AddProjectRepoRequest {
            github_repo: "acme/widgets".to_string(),
            branch_name: Some("fix-login".to_string()),
            subpath: Some("services/auth".to_string()),
            default_branch: Some("main".to_string()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(repo.branch_name, "fix-login");
}

/// A pull request already attached is refused by its number, whatever the
/// worktree code renamed its local branch to.
#[test]
fn a_pull_request_whose_branch_was_renamed_is_refused_by_its_number() {
    let store = store();
    let project = store::Project::named("alpha");
    store.create_project(&project).unwrap();
    // As the worktree code leaves it when another project's worktree holds
    // `fix-login`: both rows renamed, the pull request number kept.
    let attached = store::ProjectRepo::new(&project.id, "acme/widgets", "fix-login-2", None);
    store.create_project_repo(&attached).unwrap();
    let branch = store::Branch::new(&project.id, "fix-login-2", "origin/main")
        .with_project_repo(&attached.id)
        .with_pr(42);
    store.create_branch(&branch).unwrap();
    let mut host = widgets_with_services();
    host.add_pull_request("acme/widgets", pull_request(42, "fix-login", None));
    let ask = || {
        add(RepoSpecRequest {
            pr_number: Some(42),
            subpath: Some("services/auth".to_string()),
            ..repo("acme/widgets")
        })
    };

    let err = add_repo_request(&host, &store, &project.id, ask()).unwrap_err();

    assert!(
        err.contains("Branch 'fix-login' of 'acme/widgets' is already attached")
            && err.contains("checked out as 'fix-login-2'"),
        "{err}"
    );
    // Another pull request of the repository is free.
    host.add_pull_request("acme/widgets", pull_request(43, "fix-logout", None));
    add_repo_request(
        &host,
        &store,
        &project.id,
        add(RepoSpecRequest {
            pr_number: Some(43),
            subpath: Some("services/auth".to_string()),
            ..repo("acme/widgets")
        }),
    )
    .unwrap();
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
