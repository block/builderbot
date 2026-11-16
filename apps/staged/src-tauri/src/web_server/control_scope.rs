//! Scope enforcement for control-server callers, consulted by `dispatch`
//! ahead of its arm table.
//!
//! A `DispatchCaller::Control(grant)` carries the project its credential is
//! scoped to and whether it may write. Every allowlisted control command
//! has a row in [`CONTROL_SCOPE`] saying how to find the project a call
//! addresses — a `projectId` argument, a branch id, a session id, or the
//! `create_image` pair — and whether the command changes the app. The check
//! runs before any arm, so a confined credential cannot read or touch the
//! rows of a project outside its reach however it spells the request, and a
//! read-only credential never reaches a writing arm. The two commands that
//! take no identifier (`list_projects`, `get_active_sessions`) are filtered
//! inside their arms instead, through [`reach`]. `create_project` addresses
//! no project either, since the project does not exist yet. It has its own
//! rule, below.
//!
//! # Reach
//!
//! A confined credential reaches its own project and the projects its actor
//! created. A project records its creator in `projects.created_by`, as the
//! string [`Actor::started_by`](crate::control_server::Actor::started_by)
//! gives: `session:<session-id>` or `app:<grant-id>`. The rule exists
//! because a session's credential lasts one run. A resume or a follow-up
//! message mints a fresh one under the same session id, scoped to the
//! session's own project again. The session id is all that survives, so the
//! projects a session created are looked up from it on every check, and
//! they stay within reach across runs and app restarts. Reach grows only by
//! creating a project. Projects the user made in the app have no creator,
//! so no credential reaches one this way.
//!
//! The check applies the rule in [`require_in_scope`], with one single-row
//! read of the addressed project, and only when that project is not the
//! credential's own. The result filters apply it through [`reach`], which
//! lists the actor's created projects and so runs in the arm, not here.
//!
//! # Creating a project
//!
//! `create_project` ([`ScopeKey::NewProject`]) names no project to compare,
//! so the decision is the access level and who is asking. A session may
//! create one, and so may a connected app approved for every project. A
//! connected app approved for one project may not: the user's consent named
//! that project. The arm stamps the new project with the caller's actor
//! string, which is what puts it within the caller's reach from then on,
//! and holds a session to a limit on how many it may hold
//! (`project_commands::control`). `add_project_repo` is an ordinary
//! `projectId` command, so a session may add repos to its own project and
//! to the ones it created.
//!
//! # Tags
//!
//! The tag catalog (groups and tags) is shared by every project, so it has
//! its own rules. Any grant may read it (`get_tag_state`; the arm confines
//! the `assignments` to the projects within [`reach`], so a session learns
//! no other project's tags), and any writing grant may set the assignments
//! of a project it reaches (`set_project_group_tag`,
//! `set_project_tag_assigned`, by `projectId`). Editing the catalog itself
//! (`update_tag`, `move_tag` and the group commands) takes an unconfined
//! writing grant, since a rename from one project's session would show on
//! every project. `create_tag` is the one exception: a confined grant may
//! create a tag if the call assigns it to a project it reaches in the same
//! request (`assignProjectId`), which is what the tag picker's "Create tag"
//! row does. The two deletes are not allowlisted at all.
//!
//! # Table shape
//!
//! `open_project` is the one command decided by who holds the grant as well
//! as by its scope. It raises a window and switches the project it shows,
//! which is the user's to ask for: a connected app's grant covering the
//! project may, a session's credential may not, even for its own project.
//!
//! This is a table iterated with `find`, not a `match`, on purpose: the
//! dispatch parity tests locate `dispatch`'s arm table by its `match` over
//! `command` and each arm by its `"<name>" =>` spelling, so neither may
//! appear here (pinned in the tests below).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::{arg, control_create_image_args, get_store, opt_arg, DispatchCaller};
use crate::control_server::{Access, Actor, ControlGrant};
use crate::store::Store;

/// Prefix on a `dispatch` error that denotes a scope refusal, so the control
/// server can answer it as `403 scope_denied` rather than a plain command
/// error. Stripped from the message the CLI shows.
pub(crate) const SCOPE_DENIED_PREFIX: &str = "scope_denied: ";

/// How a command names the project it addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKey {
    /// No identifier: the arm filters its results by [`reach`].
    ResultFilter,
    /// A `projectId` argument.
    ProjectArg,
    /// `create_project`: no project exists to compare. Any session may, and
    /// any connected app approved for every project. A connected app
    /// approved for one project may not.
    NewProject,
    /// A `branchId` argument; the branch's project.
    BranchArg,
    /// A `sessionId` argument; the session's project, through its project
    /// note or branch, or its parent session's for a diagram child.
    SessionArg,
    /// `create_image`'s `branchId` + `projectId`, which must agree.
    CreateImage,
    /// A read of the shared tag catalog: no identifier, any grant. The arm
    /// confines the per-project assignments by [`reach`].
    Catalog,
    /// `create_tag`: unconfined grants may create anything; a confined grant
    /// must assign the new tag to a project it reaches (`assignProjectId`).
    CreateTag,
    /// An edit to the shared tag catalog: only an unconfined grant.
    Unconfined,
    /// `open_project`'s `projectId`, from a connected app only: a session's
    /// credential is refused, so an agent cannot pull the user's window to
    /// its project.
    OpenProject,
}

/// One row per allowlisted control command: how to resolve its project,
/// and the access it needs.
pub(crate) const CONTROL_SCOPE: &[(&str, ScopeKey, Access)] = &[
    ("list_projects", ScopeKey::ResultFilter, Access::ReadOnly),
    ("create_project", ScopeKey::NewProject, Access::ReadWrite),
    ("list_project_repos", ScopeKey::ProjectArg, Access::ReadOnly),
    ("add_project_repo", ScopeKey::ProjectArg, Access::ReadWrite),
    (
        "list_branches_for_project",
        ScopeKey::ProjectArg,
        Access::ReadOnly,
    ),
    ("get_branch_timeline", ScopeKey::BranchArg, Access::ReadOnly),
    (
        "get_project_timeline",
        ScopeKey::ProjectArg,
        Access::ReadOnly,
    ),
    ("list_project_notes", ScopeKey::ProjectArg, Access::ReadOnly),
    // A write: the window marks the project read and persists it as the
    // last one shown.
    ("open_project", ScopeKey::OpenProject, Access::ReadWrite),
    (
        "start_branch_session",
        ScopeKey::BranchArg,
        Access::ReadWrite,
    ),
    (
        "start_project_session",
        ScopeKey::ProjectArg,
        Access::ReadWrite,
    ),
    ("get_session", ScopeKey::SessionArg, Access::ReadOnly),
    (
        "get_active_sessions",
        ScopeKey::ResultFilter,
        Access::ReadOnly,
    ),
    ("cancel_session", ScopeKey::SessionArg, Access::ReadWrite),
    ("create_image", ScopeKey::CreateImage, Access::ReadWrite),
    ("get_tag_state", ScopeKey::Catalog, Access::ReadOnly),
    (
        "set_project_group_tag",
        ScopeKey::ProjectArg,
        Access::ReadWrite,
    ),
    (
        "set_project_tag_assigned",
        ScopeKey::ProjectArg,
        Access::ReadWrite,
    ),
    ("create_tag", ScopeKey::CreateTag, Access::ReadWrite),
    ("update_tag", ScopeKey::Unconfined, Access::ReadWrite),
    ("move_tag", ScopeKey::Unconfined, Access::ReadWrite),
    ("create_tag_group", ScopeKey::Unconfined, Access::ReadWrite),
    ("rename_tag_group", ScopeKey::Unconfined, Access::ReadWrite),
    (
        "set_icon_tag_group",
        ScopeKey::Unconfined,
        Access::ReadWrite,
    ),
    (
        "set_tag_group_sidebar",
        ScopeKey::Unconfined,
        Access::ReadWrite,
    ),
    (
        "reorder_tag_groups",
        ScopeKey::Unconfined,
        Access::ReadWrite,
    ),
];

/// The projects a caller's results are confined to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reach {
    /// Every project: a web caller, or an all-projects grant.
    All,
    /// A confined credential: its own project and the projects its actor
    /// created.
    Projects(HashSet<String>),
}

impl Reach {
    /// Whether a row of this project may be shown to the caller.
    pub(super) fn contains(&self, project_id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Projects(projects) => projects.contains(project_id),
        }
    }

    /// [`Reach::contains`] for a row that may belong to no project. An
    /// unconfined caller sees such a row and a confined one does not.
    pub(super) fn contains_optional(&self, project_id: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Projects(projects) => project_id.is_some_and(|id| projects.contains(id)),
        }
    }
}

/// What a caller's results must be confined to: everything for an
/// unconfined caller (a web caller, or an all-projects grant), else the
/// grant's own project and the projects its actor created. Lists the
/// actor's created projects, so it is for the arms. The check ahead of them
/// does single-row reads only (see [`require_in_scope`]).
pub(super) fn reach(caller: &DispatchCaller, store: &Store) -> Result<Reach, String> {
    let DispatchCaller::Control(grant) = caller else {
        return Ok(Reach::All);
    };
    let Some(own) = grant.project_scope() else {
        return Ok(Reach::All);
    };
    let mut projects: HashSet<String> = store
        .list_project_ids_created_by(&grant.actor.started_by())
        .map_err(|e| e.to_string())?
        .into_iter()
        .collect();
    projects.insert(own.to_string());
    Ok(Reach::Projects(projects))
}

/// Refuse `cmd` for `grant` unless it stays within the grant's scope and
/// access. Resolves the store from `dispatch`'s state; see
/// [`check_with_store`].
pub(super) fn check(
    cmd: &str,
    args: &Value,
    grant: &ControlGrant,
    store_mutex: &Mutex<Option<Arc<Store>>>,
) -> Result<(), String> {
    let store = get_store(store_mutex)?;
    check_with_store(cmd, args, grant, &store)
}

/// The scope decision. Store reads here are single-row lookups only: this
/// runs inline on the runtime, ahead of the arm.
pub(crate) fn check_with_store(
    cmd: &str,
    args: &Value,
    grant: &ControlGrant,
    store: &Store,
) -> Result<(), String> {
    let Some((_, key, required)) = CONTROL_SCOPE.iter().find(|(name, ..)| *name == cmd) else {
        // Unreachable through the control server (it rejects anything off
        // its allowlist first); fail closed for any other caller.
        return Err(denied(format!("{cmd} has no control scope rule")));
    };
    if *required == Access::ReadWrite && grant.access == Access::ReadOnly {
        return Err(denied(format!(
            "{cmd} changes the app and this credential is read-only"
        )));
    }
    let scope = grant.project_scope();
    match key {
        ScopeKey::ResultFilter => Ok(()),
        ScopeKey::ProjectArg => {
            let project_id: String = arg(args, "projectId")?;
            require_in_scope(store, grant, &project_id, "project")?;
            require_images_in_scope(store, args, grant)
        }
        ScopeKey::NewProject => match (&grant.actor, scope) {
            (Actor::App { .. }, Some(allowed)) => Err(denied(format!(
                "{cmd} needs a connected app approved for every project; this one was \
                 approved for project {allowed} only. Reconnect without --project to create \
                 projects"
            ))),
            _ => Ok(()),
        },
        ScopeKey::BranchArg => {
            let branch_id: String = arg(args, "branchId")?;
            if scope.is_some() {
                let project_id = branch_project(store, &branch_id)?;
                require_in_scope(store, grant, &project_id, "branch")?;
            }
            require_images_in_scope(store, args, grant)
        }
        ScopeKey::SessionArg => {
            let session_id: String = arg(args, "sessionId")?;
            if scope.is_some() {
                let project_id = session_project(store, &session_id)?;
                require_in_scope(store, grant, &project_id, "session")?;
            }
            Ok(())
        }
        ScopeKey::CreateImage => {
            let (branch_id, project_id, _) = control_create_image_args(args)?;
            // Nothing downstream checks the pair agrees (the images table
            // has independent foreign keys), so every control caller gets
            // this check, scoped or not.
            let branch_project_id = branch_project(store, &branch_id)?;
            if branch_project_id != project_id {
                return Err(denied(format!(
                    "branchId {branch_id} belongs to project {branch_project_id}, not projectId \
                     {project_id}"
                )));
            }
            require_in_scope(store, grant, &project_id, "project")
        }
        ScopeKey::Catalog => Ok(()),
        ScopeKey::CreateTag => {
            let Some(allowed) = scope else {
                return Ok(());
            };
            // A grouped tag becomes an option in a shared group, which every
            // project's pill for that group then offers: a catalog edit, not
            // what the picker's "Create tag" row does (ungrouped, assigned).
            let group_id: Option<String> = opt_arg(args, "groupId")?;
            if group_id.is_some() {
                return Err(denied(format!(
                    "{cmd} with a groupId adds an option to a shared tag group; {}, and it may \
                     only create an ungrouped tag it assigns to one of them",
                    covers(allowed)
                )));
            }
            let assign_project_id: Option<String> = opt_arg(args, "assignProjectId")?;
            match assign_project_id {
                Some(project_id) => require_in_scope(store, grant, &project_id, "project"),
                None => Err(denied(format!(
                    "{cmd} edits the shared tag catalog; {}, and it may only create a tag it \
                     assigns to one of them (assignProjectId)",
                    covers(allowed)
                ))),
            }
        }
        ScopeKey::Unconfined => match scope {
            Some(allowed) => Err(denied(format!(
                "{cmd} edits the shared tag catalog, which needs a credential for every \
                 project; {}",
                covers(allowed)
            ))),
            None => Ok(()),
        },
        ScopeKey::OpenProject => {
            if let Actor::Session { session_id, .. } = &grant.actor {
                return Err(denied(format!(
                    "{cmd} raises a Staged window and switches the project it shows, which a \
                     spawned session may not do: this credential belongs to Staged session \
                     {session_id}; `stagedctl project open` is for clients the user runs \
                     outside a session"
                )));
            }
            let project_id: String = arg(args, "projectId")?;
            require_in_scope(store, grant, &project_id, "project")
        }
    }
}

fn denied(message: String) -> String {
    format!("{SCOPE_DENIED_PREFIX}{message}")
}

/// What a confined credential reaches, as every refusal says it: its own
/// project and the projects its actor created (see the module docs).
fn covers(allowed: &str) -> String {
    format!("this credential covers project {allowed} and the projects it created")
}

/// `Ok` when the grant is unconfined, `project_id` is its own project, or
/// its actor created `project_id` (see the module docs).
fn require_in_scope(
    store: &Store,
    grant: &ControlGrant,
    project_id: &str,
    what: &str,
) -> Result<(), String> {
    let Some(allowed) = grant.project_scope() else {
        return Ok(());
    };
    if within_reach(store, grant, allowed, project_id)? {
        return Ok(());
    }
    Err(denied(format!(
        "the {what} belongs to another project; {}",
        covers(allowed)
    )))
}

/// Whether a grant confined to `allowed` reaches `project_id`: it is that
/// project, or one whose `created_by` is the grant's actor. The read is one
/// row, and only for a project other than the grant's own. A project that
/// does not exist is outside every reach.
fn within_reach(
    store: &Store,
    grant: &ControlGrant,
    allowed: &str,
    project_id: &str,
) -> Result<bool, String> {
    if allowed == project_id {
        return Ok(true);
    }
    let creator = store
        .get_project(project_id)
        .map_err(|e| e.to_string())?
        .and_then(|project| project.created_by);
    Ok(creator.is_some_and(|creator| creator == grant.actor.started_by()))
}

/// The project of each `imageIds` entry, for the two start commands: an
/// image from a project outside the grant's reach cannot be attached to a
/// prompt.
fn require_images_in_scope(
    store: &Store,
    args: &Value,
    grant: &ControlGrant,
) -> Result<(), String> {
    let Some(allowed) = grant.project_scope() else {
        return Ok(());
    };
    let image_ids: Option<Vec<String>> = opt_arg(args, "imageIds")?;
    for image_id in image_ids.unwrap_or_default() {
        let image = store
            .get_image(&image_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| denied(format!("image {image_id} not found")))?;
        if !within_reach(store, grant, allowed, &image.project_id)? {
            return Err(denied(format!(
                "image {image_id} belongs to another project; {}",
                covers(allowed)
            )));
        }
    }
    Ok(())
}

fn branch_project(store: &Store, branch_id: &str) -> Result<String, String> {
    store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .map(|branch| branch.project_id)
        .ok_or_else(|| denied(format!("branch {branch_id} not found")))
}

/// A session's project: its project note's or its branch's, else — for a
/// diagram child, which has neither — its parent session's, one level up.
fn session_project(store: &Store, session_id: &str) -> Result<String, String> {
    if let Some(project_id) = store
        .get_project_id_for_session(session_id)
        .map_err(|e| e.to_string())?
    {
        return Ok(project_id);
    }
    let parent = store
        .get_session(session_id)
        .map_err(|e| e.to_string())?
        .and_then(|session| session.parent_session_id);
    if let Some(parent_id) = parent {
        if let Some(project_id) = store
            .get_project_id_for_session(&parent_id)
            .map_err(|e| e.to_string())?
        {
            return Ok(project_id);
        }
    }
    Err(denied(format!(
        "session {session_id} not found or belongs to no project"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_server::Scope;
    use crate::store::{Branch, Image, Project, Session};
    use serde_json::json;

    /// Two projects, a branch, a branch session, a diagram child of that
    /// session and an image in each.
    struct Fixture {
        store: Store,
        project_a: String,
        project_b: String,
        branch_a: String,
        branch_b: String,
        session_a: String,
        session_b: String,
        child_of_a: String,
        orphan: String,
        image_a: String,
        image_b: String,
    }

    fn fixture() -> Fixture {
        let store = Store::in_memory().unwrap();
        let project_a = Project::named("alpha");
        let project_b = Project::named("beta");
        store.create_project(&project_a).unwrap();
        store.create_project(&project_b).unwrap();
        let branch_a = Branch::new(&project_a.id, "feat/a", "main");
        let branch_b = Branch::new(&project_b.id, "feat/b", "main");
        store.create_branch(&branch_a).unwrap();
        store.create_branch(&branch_b).unwrap();
        let dir = std::env::temp_dir();
        let session_a = Session::new_running("a", &dir).with_branch(&branch_a.id);
        let session_b = Session::new_running("b", &dir).with_branch(&branch_b.id);
        let child_of_a = Session::new_running("diagram", &dir).with_parent_session(&session_a.id);
        let orphan = Session::new_running("orphan", &dir);
        for session in [&session_a, &session_b, &child_of_a, &orphan] {
            store.create_session(session).unwrap();
        }
        let image_a = Image::new(
            Some(&branch_a.id),
            &project_a.id,
            "a.png",
            "image/png",
            1,
            false,
        );
        let image_b = Image::new(
            Some(&branch_b.id),
            &project_b.id,
            "b.png",
            "image/png",
            1,
            false,
        );
        store.create_image(&image_a).unwrap();
        store.create_image(&image_b).unwrap();
        Fixture {
            store,
            project_a: project_a.id,
            project_b: project_b.id,
            branch_a: branch_a.id,
            branch_b: branch_b.id,
            session_a: session_a.id,
            session_b: session_b.id,
            child_of_a: child_of_a.id,
            orphan: orphan.id,
            image_a: image_a.id,
            image_b: image_b.id,
        }
    }

    fn grant(scope: Scope, access: Access) -> ControlGrant {
        ControlGrant {
            actor: Actor::Session {
                session_id: "caller".to_string(),
                epoch: 1,
            },
            scope,
            branch_id: None,
            access,
        }
    }

    fn scoped(project_id: &str) -> ControlGrant {
        grant(Scope::Project(project_id.to_string()), Access::ReadWrite)
    }

    fn assert_denied(result: Result<(), String>, needle: &str) {
        let err = result.expect_err("should be denied");
        assert!(
            err.starts_with(SCOPE_DENIED_PREFIX),
            "a scope refusal must carry the prefix: {err}"
        );
        assert!(err.contains(needle), "expected {needle:?} in: {err}");
    }

    // -------------------------------------------------------------------
    // Table shape
    // -------------------------------------------------------------------

    #[test]
    fn scope_table_uses_no_match_arm_spellings() {
        // `dispatch`'s parity tests find its arm table by the first match
        // over `command` in a source and an arm by the `"<name>" =>`
        // spelling; this module must never satisfy either search.
        let source = include_str!("control_scope.rs");
        let marker = format!("match {} {{", "command");
        assert!(
            !source.contains(&marker),
            "control_scope.rs must not match over `command`"
        );
        for (name, ..) in CONTROL_SCOPE {
            let arm = format!("\"{name}\" =>");
            assert!(
                !source.contains(&arm),
                "control_scope.rs must not spell an arm for {name}"
            );
        }
    }

    #[test]
    fn dispatch_consults_the_scope_check_before_its_arm_table() {
        let source = include_str!("../web_server.rs");
        let check_at = source
            .find("control_scope::check(command, &args, grant, store_mutex)")
            .expect("dispatch should call control_scope::check");
        let arms_at = source
            .find(&format!("match {} {{", "command"))
            .expect("dispatch should have an arm table");
        assert!(
            check_at < arms_at,
            "the scope check must run before any arm can"
        );
    }

    // -------------------------------------------------------------------
    // Resolvers
    // -------------------------------------------------------------------

    #[test]
    fn project_argument_commands_compare_the_project_id() {
        let f = fixture();
        let g = scoped(&f.project_a);
        for cmd in [
            "list_project_repos",
            "list_branches_for_project",
            "get_project_timeline",
            "list_project_notes",
            "start_project_session",
        ] {
            check_with_store(cmd, &json!({ "projectId": f.project_a }), &g, &f.store)
                .unwrap_or_else(|e| panic!("{cmd} on own project: {e}"));
            assert_denied(
                check_with_store(cmd, &json!({ "projectId": f.project_b }), &g, &f.store),
                "another project",
            );
        }
        let err = check_with_store("list_project_repos", &json!({}), &g, &f.store).unwrap_err();
        assert!(
            err.contains("missing argument: projectId") && !err.starts_with(SCOPE_DENIED_PREFIX),
            "a missing argument is a plain command error: {err}"
        );
    }

    #[test]
    fn branch_argument_commands_resolve_the_branch_project() {
        let f = fixture();
        let g = scoped(&f.project_a);
        for cmd in ["get_branch_timeline", "start_branch_session"] {
            check_with_store(cmd, &json!({ "branchId": f.branch_a }), &g, &f.store)
                .unwrap_or_else(|e| panic!("{cmd} on own branch: {e}"));
            assert_denied(
                check_with_store(cmd, &json!({ "branchId": f.branch_b }), &g, &f.store),
                "another project",
            );
            assert_denied(
                check_with_store(cmd, &json!({ "branchId": "nope" }), &g, &f.store),
                "not found",
            );
        }
    }

    #[test]
    fn session_argument_commands_resolve_through_branch_and_parent() {
        let f = fixture();
        let g = scoped(&f.project_a);
        for cmd in ["get_session", "cancel_session"] {
            check_with_store(cmd, &json!({ "sessionId": f.session_a }), &g, &f.store)
                .unwrap_or_else(|e| panic!("{cmd} on own session: {e}"));
            check_with_store(cmd, &json!({ "sessionId": f.child_of_a }), &g, &f.store)
                .unwrap_or_else(|e| panic!("{cmd} on a diagram child walks to the parent: {e}"));
            assert_denied(
                check_with_store(cmd, &json!({ "sessionId": f.session_b }), &g, &f.store),
                "another project",
            );
            assert_denied(
                check_with_store(cmd, &json!({ "sessionId": f.orphan }), &g, &f.store),
                "no project",
            );
            assert_denied(
                check_with_store(cmd, &json!({ "sessionId": "nope" }), &g, &f.store),
                "not found",
            );
        }
    }

    #[test]
    fn start_commands_check_every_attached_image() {
        let f = fixture();
        let g = scoped(&f.project_a);
        check_with_store(
            "start_branch_session",
            &json!({ "branchId": f.branch_a, "imageIds": [f.image_a] }),
            &g,
            &f.store,
        )
        .unwrap();
        assert_denied(
            check_with_store(
                "start_branch_session",
                &json!({ "branchId": f.branch_a, "imageIds": [f.image_a, f.image_b] }),
                &g,
                &f.store,
            ),
            "image",
        );
        assert_denied(
            check_with_store(
                "start_project_session",
                &json!({ "projectId": f.project_a, "imageIds": [f.image_b] }),
                &g,
                &f.store,
            ),
            "image",
        );
        assert_denied(
            check_with_store(
                "start_project_session",
                &json!({ "projectId": f.project_a, "imageIds": ["nope"] }),
                &g,
                &f.store,
            ),
            "not found",
        );
    }

    #[test]
    fn create_image_requires_branch_and_project_to_agree() {
        let f = fixture();
        let own = json!({
            "branchId": f.branch_a, "projectId": f.project_a, "filePath": "/tmp/x.png"
        });
        check_with_store("create_image", &own, &scoped(&f.project_a), &f.store).unwrap();
        let mismatch = json!({
            "branchId": f.branch_a, "projectId": f.project_b, "filePath": "/tmp/x.png"
        });
        // Even an unconfined caller cannot file a branch's image under
        // another project.
        assert_denied(
            check_with_store(
                "create_image",
                &mismatch,
                &ControlGrant::test_all_projects(),
                &f.store,
            ),
            "belongs to project",
        );
        let other = json!({
            "branchId": f.branch_b, "projectId": f.project_b, "filePath": "/tmp/x.png"
        });
        assert_denied(
            check_with_store("create_image", &other, &scoped(&f.project_a), &f.store),
            "another project",
        );
        // The arg-shape rules still apply ahead of the scope ones.
        let relative = json!({
            "branchId": f.branch_a, "projectId": f.project_a, "filePath": "x.png"
        });
        let err = check_with_store("create_image", &relative, &scoped(&f.project_a), &f.store)
            .unwrap_err();
        assert!(err.contains("absolute filePath"), "got: {err}");
    }

    #[test]
    fn result_filtered_commands_pass_the_check_and_expose_the_filter() {
        let f = fixture();
        let g = scoped(&f.project_a);
        for cmd in ["list_projects", "get_active_sessions"] {
            check_with_store(cmd, &json!({}), &g, &f.store).unwrap();
        }
        assert_eq!(
            reach(&DispatchCaller::Control(g.clone()), &f.store).unwrap(),
            Reach::Projects(HashSet::from([f.project_a.clone()]))
        );
        assert_eq!(
            reach(
                &DispatchCaller::Control(ControlGrant::test_all_projects()),
                &f.store
            )
            .unwrap(),
            Reach::All
        );
        assert_eq!(reach(&DispatchCaller::Web, &f.store).unwrap(), Reach::All);
    }

    // -------------------------------------------------------------------
    // Projects the caller created
    // -------------------------------------------------------------------

    /// A project recorded as created by `creator`, with a branch, a running
    /// branch session, a diagram child of that session and an image.
    struct Created {
        project: String,
        branch: String,
        session: String,
        child: String,
        image: String,
    }

    fn create_project_by(f: &Fixture, name: &str, creator: &str) -> Created {
        let project = Project::named(name).with_created_by(creator);
        f.store.create_project(&project).unwrap();
        let branch = Branch::new(&project.id, &format!("feat/{name}"), "main");
        f.store.create_branch(&branch).unwrap();
        let dir = std::env::temp_dir();
        let session = Session::new_running(name, &dir).with_branch(&branch.id);
        let child = Session::new_running("diagram", &dir).with_parent_session(&session.id);
        f.store.create_session(&session).unwrap();
        f.store.create_session(&child).unwrap();
        let image = Image::new(
            Some(&branch.id),
            &project.id,
            "c.png",
            "image/png",
            1,
            false,
        );
        f.store.create_image(&image).unwrap();
        Created {
            project: project.id,
            branch: branch.id,
            session: session.id,
            child: child.id,
            image: image.id,
        }
    }

    /// The string `grant` and `scoped` callers record as a creator.
    const CALLER: &str = "session:caller";

    fn session_grant(session_id: &str, epoch: u64, project_id: &str) -> ControlGrant {
        ControlGrant {
            actor: Actor::Session {
                session_id: session_id.to_string(),
                epoch,
            },
            scope: Scope::Project(project_id.to_string()),
            branch_id: None,
            access: Access::ReadWrite,
        }
    }

    fn app_grant(grant_id: &str, project_id: &str) -> ControlGrant {
        ControlGrant {
            actor: Actor::App {
                grant_id: grant_id.to_string(),
                client: "test".to_string(),
            },
            scope: Scope::Project(project_id.to_string()),
            branch_id: None,
            access: Access::ReadWrite,
        }
    }

    /// One call per way a command can address `created`: by project, by
    /// branch, by session (a diagram child included), by attached image and
    /// by the `create_image` pair.
    fn calls_addressing(created: &Created, own_project: &str) -> Vec<(&'static str, Value)> {
        let mut calls = Vec::new();
        for cmd in [
            "list_project_repos",
            "list_branches_for_project",
            "get_project_timeline",
            "list_project_notes",
            "start_project_session",
        ] {
            calls.push((cmd, json!({ "projectId": created.project })));
        }
        calls.push((
            "add_project_repo",
            json!({ "projectId": created.project, "githubRepo": "acme/widgets" }),
        ));
        for cmd in ["get_branch_timeline", "start_branch_session"] {
            calls.push((cmd, json!({ "branchId": created.branch })));
        }
        for cmd in ["get_session", "cancel_session"] {
            calls.push((cmd, json!({ "sessionId": created.session })));
            calls.push((cmd, json!({ "sessionId": created.child })));
        }
        // An image of the created project, attached to a prompt in the
        // caller's own project.
        calls.push((
            "start_project_session",
            json!({ "projectId": own_project, "imageIds": [created.image] }),
        ));
        calls.push((
            "create_image",
            json!({
                "branchId": created.branch, "projectId": created.project, "filePath": "/tmp/x.png"
            }),
        ));
        calls.push((
            "set_project_group_tag",
            json!({ "projectId": created.project, "groupId": "g", "tagId": null }),
        ));
        calls.push((
            "set_project_tag_assigned",
            json!({ "projectId": created.project, "tagId": "t", "assigned": true }),
        ));
        calls
    }

    #[test]
    fn a_session_grant_reaches_a_project_it_created_by_every_identifier() {
        let f = fixture();
        let created = create_project_by(&f, "gamma", CALLER);
        let g = scoped(&f.project_a);
        for (cmd, args) in calls_addressing(&created, &f.project_a) {
            check_with_store(cmd, &args, &g, &f.store)
                .unwrap_or_else(|e| panic!("{cmd} {args} on a created project: {e}"));
        }
        // Images of the own and the created project attach together, in
        // either project.
        check_with_store(
            "start_branch_session",
            &json!({ "branchId": created.branch, "imageIds": [f.image_a, created.image] }),
            &g,
            &f.store,
        )
        .unwrap();
        // Creating a project widens the reach by that project only.
        assert_denied(
            check_with_store(
                "list_project_repos",
                &json!({ "projectId": f.project_b }),
                &g,
                &f.store,
            ),
            "another project",
        );
        assert_denied(
            check_with_store(
                "get_branch_timeline",
                &json!({ "branchId": f.branch_b }),
                &g,
                &f.store,
            ),
            "another project",
        );
        assert_denied(
            check_with_store(
                "get_session",
                &json!({ "sessionId": f.session_b }),
                &g,
                &f.store,
            ),
            "another project",
        );
        assert_denied(
            check_with_store(
                "start_branch_session",
                &json!({ "branchId": created.branch, "imageIds": [f.image_b] }),
                &g,
                &f.store,
            ),
            "image",
        );
        // The pair must still agree: a created project's branch is not
        // filed under the own project.
        assert_denied(
            check_with_store(
                "create_image",
                &json!({
                    "branchId": created.branch, "projectId": f.project_a, "filePath": "/tmp/x.png"
                }),
                &g,
                &f.store,
            ),
            "belongs to project",
        );
    }

    #[test]
    fn a_created_project_is_refused_to_every_other_actor() {
        let f = fixture();
        let created = create_project_by(&f, "gamma", CALLER);
        let strangers = [
            ("another session", session_grant("other", 1, &f.project_a)),
            // The creator is compared whole: neither a longer nor a shorter
            // session id matches.
            ("a longer id", session_grant("caller2", 1, &f.project_a)),
            ("a shorter id", session_grant("calle", 1, &f.project_a)),
            ("an app grant", app_grant("g1", &f.project_a)),
            // Nor does an app grant whose id happens to be the session's.
            ("an app named alike", app_grant("caller", &f.project_a)),
        ];
        for (who, g) in &strangers {
            for (cmd, args) in calls_addressing(&created, &f.project_a) {
                let err = check_with_store(cmd, &args, g, &f.store)
                    .expect_err(&format!("{cmd} {args} should be refused to {who}"));
                assert!(
                    err.starts_with(SCOPE_DENIED_PREFIX),
                    "{cmd} for {who} must be a scope refusal: {err}"
                );
            }
            assert_denied(
                check_with_store(
                    "create_tag",
                    &json!({ "name": "q4", "assignProjectId": created.project }),
                    g,
                    &f.store,
                ),
                "another project",
            );
            assert_eq!(
                reach(&DispatchCaller::Control(g.clone()), &f.store).unwrap(),
                Reach::Projects(HashSet::from([f.project_a.clone()])),
                "{who}"
            );
        }
        // A project the user made has no creator, so it stays out of every
        // confined credential's reach, the creating session's included.
        assert_denied(
            check_with_store(
                "list_project_repos",
                &json!({ "projectId": f.project_b }),
                &scoped(&f.project_a),
                &f.store,
            ),
            "another project",
        );
    }

    #[test]
    fn the_creator_rule_follows_the_actor_string_for_app_grants_too() {
        let f = fixture();
        let by_app = create_project_by(&f, "delta", "app:g1");
        let g = app_grant("g1", &f.project_a);
        for (cmd, args) in calls_addressing(&by_app, &f.project_a) {
            check_with_store(cmd, &args, &g, &f.store)
                .unwrap_or_else(|e| panic!("{cmd} {args} for the creating app grant: {e}"));
        }
        // A session is a different actor, whatever its id.
        assert_denied(
            check_with_store(
                "list_project_repos",
                &json!({ "projectId": by_app.project }),
                &session_grant("g1", 1, &f.project_a),
                &f.store,
            ),
            "another project",
        );
    }

    #[test]
    fn a_new_run_of_the_same_session_still_reaches_what_it_created() {
        let f = fixture();
        let created = create_project_by(&f, "gamma", CALLER);
        // A resume or follow-up registers again: a new epoch, the same
        // session id, and the session's own project as the scope.
        let resumed = session_grant("caller", 7, &f.project_a);
        assert_ne!(resumed, scoped(&f.project_a));
        for (cmd, args) in calls_addressing(&created, &f.project_a) {
            check_with_store(cmd, &args, &resumed, &f.store)
                .unwrap_or_else(|e| panic!("{cmd} {args} after a resume: {e}"));
        }
        assert_eq!(
            reach(&DispatchCaller::Control(resumed), &f.store).unwrap(),
            Reach::Projects(HashSet::from([
                f.project_a.clone(),
                created.project.clone()
            ]))
        );
    }

    #[test]
    fn a_read_only_grant_reads_what_its_actor_created_and_writes_nothing() {
        let f = fixture();
        let created = create_project_by(&f, "gamma", CALLER);
        let g = grant(Scope::Project(f.project_a.clone()), Access::ReadOnly);
        for (cmd, args) in calls_addressing(&created, &f.project_a) {
            let writes = CONTROL_SCOPE
                .iter()
                .any(|(name, _, access)| *name == cmd && *access == Access::ReadWrite);
            let result = check_with_store(cmd, &args, &g, &f.store);
            if writes {
                assert_denied(result, "read-only");
            } else {
                result.unwrap_or_else(|e| panic!("{cmd} {args} is a read: {e}"));
            }
        }
    }

    #[test]
    fn create_tag_may_assign_to_a_created_project() {
        let f = fixture();
        let created = create_project_by(&f, "gamma", CALLER);
        let g = scoped(&f.project_a);
        check_with_store(
            "create_tag",
            &json!({ "name": "q4", "assignProjectId": created.project }),
            &g,
            &f.store,
        )
        .unwrap();
        // The other rules for a confined grant are unchanged: no grouped
        // tag, no unassigned tag, no other project.
        assert_denied(
            check_with_store(
                "create_tag",
                &json!({ "name": "q4", "groupId": "g", "assignProjectId": created.project }),
                &g,
                &f.store,
            ),
            "groupId",
        );
        assert_denied(
            check_with_store("create_tag", &json!({ "name": "q4" }), &g, &f.store),
            "assignProjectId",
        );
        assert_denied(
            check_with_store(
                "create_tag",
                &json!({ "name": "q4", "assignProjectId": f.project_b }),
                &g,
                &f.store,
            ),
            "another project",
        );
        // Creating a project does not make a credential unconfined.
        for (cmd, args) in catalog_writes() {
            assert_denied(check_with_store(cmd, &args, &g, &f.store), "covers project");
        }
    }

    /// What the three result-filtered arms return for `grant`, as the
    /// project ids of the rows each keeps: `list_projects`,
    /// `get_active_sessions` and the assignments of `get_tag_state`.
    fn filtered_results(f: &Fixture, grant: &ControlGrant) -> [HashSet<String>; 3] {
        let reach = reach(&DispatchCaller::Control(grant.clone()), &f.store).unwrap();
        let projects = super::super::confine_projects(f.store.list_projects().unwrap(), &reach);
        let sessions = super::super::confine_active_sessions(
            crate::session_commands::get_active_sessions_impl(
                &f.store,
                &crate::session_runner::SessionRegistry::new(),
            )
            .unwrap(),
            &reach,
        );
        let tag_state = super::super::confine_tag_state(f.store.get_tag_state().unwrap(), &reach);
        [
            projects.into_iter().map(|project| project.id).collect(),
            sessions
                .into_iter()
                .map(|session| session.project_id.unwrap_or_else(|| "none".to_string()))
                .collect(),
            tag_state
                .assignments
                .into_iter()
                .map(|assignment| assignment.project_id)
                .collect(),
        ]
    }

    #[test]
    fn result_filters_include_the_created_project_and_nothing_else_new() {
        let f = fixture();
        let created = create_project_by(&f, "gamma", CALLER);
        // Projects other actors created, which must stay hidden.
        let by_other = create_project_by(&f, "delta", "session:other");
        let by_app = create_project_by(&f, "epsilon", "app:g1");
        let every_project = [
            &f.project_a,
            &f.project_b,
            &created.project,
            &by_other.project,
            &by_app.project,
        ];
        // One tag assignment per project, so each has a row in all three
        // results. Each already has a running branch session.
        for (index, project_id) in every_project.iter().enumerate() {
            f.store
                .create_tag(
                    &crate::store::NewTag {
                        name: format!("tag-{index}"),
                        color: None,
                        icon: None,
                    },
                    None,
                    Some(project_id.as_str()),
                )
                .unwrap();
        }
        let ids = |projects: &[&String]| -> HashSet<String> {
            projects.iter().map(|id| id.to_string()).collect()
        };

        // The creating session: its own project and the one it created.
        let own_and_created = ids(&[&f.project_a, &created.project]);
        for results in filtered_results(&f, &scoped(&f.project_a)) {
            assert_eq!(results, own_and_created);
        }
        // A later run of that session sees the same.
        for results in filtered_results(&f, &session_grant("caller", 2, &f.project_a)) {
            assert_eq!(results, own_and_created);
        }
        // A session that created nothing sees its own project only, as
        // before the creator rule.
        for results in filtered_results(&f, &session_grant("idle", 1, &f.project_a)) {
            assert_eq!(results, ids(&[&f.project_a]));
        }
        // An unconfined grant sees every project. Its session list also
        // carries the sessions that belong to no project.
        let [projects, sessions, assignments] =
            filtered_results(&f, &ControlGrant::test_all_projects());
        let everything = ids(&every_project);
        assert_eq!(projects, everything);
        assert_eq!(assignments, everything);
        let mut with_unowned = everything.clone();
        with_unowned.insert("none".to_string());
        assert_eq!(sessions, with_unowned);
    }

    // -------------------------------------------------------------------
    // Tags
    // -------------------------------------------------------------------

    #[test]
    fn catalog_reads_pass_for_any_grant() {
        let f = fixture();
        for g in [
            scoped(&f.project_a),
            grant(Scope::Project(f.project_a.clone()), Access::ReadOnly),
            ControlGrant::test_all_projects(),
            grant(Scope::AllProjects, Access::ReadOnly),
        ] {
            check_with_store("get_tag_state", &json!({}), &g, &f.store).unwrap();
        }
    }

    #[test]
    fn tag_assignments_follow_the_project_argument() {
        let f = fixture();
        let g = scoped(&f.project_a);
        check_with_store(
            "set_project_group_tag",
            &json!({ "projectId": f.project_a, "groupId": "g", "tagId": null }),
            &g,
            &f.store,
        )
        .unwrap();
        check_with_store(
            "set_project_tag_assigned",
            &json!({ "projectId": f.project_a, "tagId": "t", "assigned": true }),
            &g,
            &f.store,
        )
        .unwrap();
        for cmd in ["set_project_group_tag", "set_project_tag_assigned"] {
            assert_denied(
                check_with_store(
                    cmd,
                    &json!({ "projectId": f.project_b, "groupId": "g", "tagId": "t", "assigned": true }),
                    &g,
                    &f.store,
                ),
                "another project",
            );
        }
    }

    #[test]
    fn create_tag_by_a_scoped_grant_must_assign_its_own_project() {
        let f = fixture();
        let g = scoped(&f.project_a);
        // The picker's create: a tag made and assigned to the own project.
        check_with_store(
            "create_tag",
            &json!({ "name": "q4", "assignProjectId": f.project_a }),
            &g,
            &f.store,
        )
        .unwrap();
        // An unassigned tag is a catalog edit, which a scoped grant can't make.
        assert_denied(
            check_with_store("create_tag", &json!({ "name": "q4" }), &g, &f.store),
            "assignProjectId",
        );
        assert_denied(
            check_with_store(
                "create_tag",
                &json!({ "name": "q4", "assignProjectId": null }),
                &g,
                &f.store,
            ),
            "assignProjectId",
        );
        // Nor assign one to another project.
        assert_denied(
            check_with_store(
                "create_tag",
                &json!({ "name": "q4", "assignProjectId": f.project_b }),
                &g,
                &f.store,
            ),
            "another project",
        );
        // Nor put the tag in a group, even assigned to its own project: that
        // adds an option every project's pill for the group then offers.
        assert_denied(
            check_with_store(
                "create_tag",
                &json!({ "name": "q4", "groupId": "g", "assignProjectId": f.project_a }),
                &g,
                &f.store,
            ),
            "groupId",
        );
        // An explicit null groupId is the ungrouped case.
        check_with_store(
            "create_tag",
            &json!({ "name": "q4", "groupId": null, "assignProjectId": f.project_a }),
            &g,
            &f.store,
        )
        .unwrap();
        // An unconfined grant may create a tag anywhere, assigned or not.
        let all = ControlGrant::test_all_projects();
        check_with_store("create_tag", &json!({ "name": "q4" }), &all, &f.store).unwrap();
        check_with_store(
            "create_tag",
            &json!({ "name": "q4", "groupId": "g", "assignProjectId": f.project_b }),
            &all,
            &f.store,
        )
        .unwrap();
    }

    /// Every catalog edit except `create_tag`, with arguments that would
    /// pass the arm.
    fn catalog_writes() -> Vec<(&'static str, serde_json::Value)> {
        vec![
            (
                "update_tag",
                json!({ "tagId": "t", "name": "Done", "color": "green", "icon": null }),
            ),
            (
                "move_tag",
                json!({ "tagId": "t", "groupId": "g", "index": 0 }),
            ),
            ("create_tag_group", json!({ "name": "Status", "tags": [] })),
            (
                "rename_tag_group",
                json!({ "groupId": "g", "name": "State" }),
            ),
            ("set_icon_tag_group", json!({ "groupId": "g" })),
            (
                "set_tag_group_sidebar",
                json!({ "groupId": "g", "shown": true }),
            ),
            ("reorder_tag_groups", json!({ "groupIds": ["g"] })),
        ]
    }

    #[test]
    fn catalog_writes_need_an_unconfined_grant() {
        let f = fixture();
        let session = scoped(&f.project_a);
        let all = ControlGrant::test_all_projects();
        for (cmd, args) in catalog_writes() {
            assert_denied(
                check_with_store(cmd, &args, &session, &f.store),
                "covers project",
            );
            check_with_store(cmd, &args, &all, &f.store).unwrap();
        }
        let unconfined: Vec<&str> = CONTROL_SCOPE
            .iter()
            .filter(|(_, key, _)| *key == ScopeKey::Unconfined)
            .map(|(name, ..)| *name)
            .collect();
        let expected: Vec<&str> = catalog_writes().iter().map(|(cmd, _)| *cmd).collect();
        assert_eq!(
            unconfined, expected,
            "the unconfined set is exactly the catalog edits"
        );
    }

    // -------------------------------------------------------------------
    // open_project
    // -------------------------------------------------------------------

    /// A connected app's grant, as `stagedctl connect` leaves the user with.
    fn connected_app_grant(scope: Scope, access: Access) -> ControlGrant {
        ControlGrant {
            actor: Actor::App {
                grant_id: "g1".to_string(),
                client: "terminal".to_string(),
            },
            scope,
            branch_id: None,
            access,
        }
    }

    #[test]
    fn open_project_follows_a_connected_apps_project_scope() {
        let f = fixture();
        let confined = connected_app_grant(Scope::Project(f.project_a.clone()), Access::ReadWrite);
        check_with_store(
            "open_project",
            &json!({ "projectId": f.project_a }),
            &confined,
            &f.store,
        )
        .unwrap();
        assert_denied(
            check_with_store(
                "open_project",
                &json!({ "projectId": f.project_b }),
                &confined,
                &f.store,
            ),
            "another project",
        );
        let all = connected_app_grant(Scope::AllProjects, Access::ReadWrite);
        for project_id in [&f.project_a, &f.project_b] {
            check_with_store(
                "open_project",
                &json!({ "projectId": project_id }),
                &all,
                &f.store,
            )
            .unwrap();
        }
        let err = check_with_store("open_project", &json!({}), &all, &f.store).unwrap_err();
        assert!(
            err.contains("missing argument: projectId") && !err.starts_with(SCOPE_DENIED_PREFIX),
            "a missing argument is a plain command error: {err}"
        );
    }

    /// Navigation marks the project read and persists it as the last shown,
    /// so a read-only grant cannot open even a project it may read.
    #[test]
    fn open_project_is_refused_to_a_read_only_grant() {
        let f = fixture();
        for scope in [Scope::Project(f.project_a.clone()), Scope::AllProjects] {
            assert_denied(
                check_with_store(
                    "open_project",
                    &json!({ "projectId": f.project_a }),
                    &connected_app_grant(scope, Access::ReadOnly),
                    &f.store,
                ),
                "read-only",
            );
        }
    }

    /// An agent must not pull the user's window to its project, so a
    /// session's credential is refused for its own project too, and before
    /// the argument is looked at.
    #[test]
    fn open_project_is_refused_to_a_session_credential() {
        let f = fixture();
        let session = scoped(&f.project_a);
        for args in [
            json!({ "projectId": f.project_a }),
            json!({ "projectId": f.project_b }),
            json!({}),
        ] {
            assert_denied(
                check_with_store("open_project", &args, &session, &f.store),
                "spawned session may not",
            );
        }
        // Whatever its scope: no session grant is unconfined today, and one
        // that was would still be an agent's.
        assert_denied(
            check_with_store(
                "open_project",
                &json!({ "projectId": f.project_a }),
                &grant(Scope::AllProjects, Access::ReadWrite),
                &f.store,
            ),
            "spawned session may not",
        );
        // No other command is decided by the actor.
        let actor_keyed: Vec<&str> = CONTROL_SCOPE
            .iter()
            .filter(|(_, key, _)| *key == ScopeKey::OpenProject)
            .map(|(name, ..)| *name)
            .collect();
        assert_eq!(actor_keyed, ["open_project"]);
    }

    // -------------------------------------------------------------------
    // Creating a project and adding a repo
    // -------------------------------------------------------------------

    #[test]
    fn a_session_and_an_all_projects_grant_may_create_a_project() {
        let f = fixture();
        let args = json!({ "name": "gamma" });
        // A session is confined to one project and may still create one:
        // that is how its reach grows.
        check_with_store("create_project", &args, &scoped(&f.project_a), &f.store).unwrap();
        check_with_store(
            "create_project",
            &args,
            &session_grant("other", 3, &f.project_b),
            &f.store,
        )
        .unwrap();
        check_with_store(
            "create_project",
            &args,
            &ControlGrant::test_all_projects(),
            &f.store,
        )
        .unwrap();
    }

    #[test]
    fn create_project_is_refused_to_read_only_and_to_single_project_app_grants() {
        let f = fixture();
        let args = json!({ "name": "gamma" });
        for read_only in [
            grant(Scope::Project(f.project_a.clone()), Access::ReadOnly),
            grant(Scope::AllProjects, Access::ReadOnly),
        ] {
            assert_denied(
                check_with_store("create_project", &args, &read_only, &f.store),
                "read-only",
            );
        }
        // The user approved this app for one project. The refusal says how
        // to get a credential that may create.
        let err = check_with_store(
            "create_project",
            &args,
            &app_grant("g1", &f.project_a),
            &f.store,
        );
        assert_denied(err.clone(), "approved for every project");
        assert_denied(err.clone(), &format!("project {} only", f.project_a));
        assert_denied(err, "Reconnect without --project");
        // Having created a project by some other route changes nothing.
        create_project_by(&f, "delta", "app:g1");
        assert_denied(
            check_with_store(
                "create_project",
                &args,
                &app_grant("g1", &f.project_a),
                &f.store,
            ),
            "approved for every project",
        );
    }

    #[test]
    fn add_project_repo_reaches_the_own_project_and_a_created_one_only() {
        let f = fixture();
        let created = create_project_by(&f, "gamma", CALLER);
        let g = scoped(&f.project_a);
        let add_to = |project_id: &str| json!({ "projectId": project_id, "githubRepo": "a/b" });
        check_with_store("add_project_repo", &add_to(&f.project_a), &g, &f.store).unwrap();
        check_with_store("add_project_repo", &add_to(&created.project), &g, &f.store).unwrap();
        assert_denied(
            check_with_store("add_project_repo", &add_to(&f.project_b), &g, &f.store),
            "another project",
        );
        assert_denied(
            check_with_store("add_project_repo", &add_to("nope"), &g, &f.store),
            "another project",
        );
        // Another session reaches neither the creator's project nor the
        // one it created.
        let other = session_grant("other", 1, &f.project_b);
        for project_id in [&f.project_a, &created.project] {
            assert_denied(
                check_with_store("add_project_repo", &add_to(project_id), &other, &f.store),
                "another project",
            );
        }
        // A read-only grant adds nothing, even where it may read.
        for project_id in [&f.project_a, &created.project] {
            assert_denied(
                check_with_store(
                    "add_project_repo",
                    &add_to(project_id),
                    &grant(Scope::Project(f.project_a.clone()), Access::ReadOnly),
                    &f.store,
                ),
                "read-only",
            );
        }
        check_with_store(
            "add_project_repo",
            &add_to(&f.project_b),
            &ControlGrant::test_all_projects(),
            &f.store,
        )
        .unwrap();
        // The project is the one thing the check needs of the arguments.
        let err = check_with_store(
            "add_project_repo",
            &json!({ "githubRepo": "a/b" }),
            &g,
            &f.store,
        )
        .unwrap_err();
        assert!(err.contains("missing argument: projectId"), "{err}");
    }

    /// A confined credential is no longer confined to one project, so no
    /// refusal may say it is. Each names the own project and says that
    /// created projects are covered too.
    #[test]
    fn refusals_describe_reach_as_the_own_project_and_the_created_ones() {
        let f = fixture();
        let g = scoped(&f.project_a);
        let covers = format!(
            "this credential covers project {} and the projects it created",
            f.project_a
        );
        let mut refusals = vec![
            ("list_project_repos", json!({ "projectId": f.project_b })),
            ("get_branch_timeline", json!({ "branchId": f.branch_b })),
            ("get_session", json!({ "sessionId": f.session_b })),
            (
                "start_project_session",
                json!({ "projectId": f.project_a, "imageIds": [f.image_b] }),
            ),
            (
                "create_image",
                json!({ "branchId": f.branch_b, "projectId": f.project_b, "filePath": "/tmp/x" }),
            ),
            ("create_tag", json!({ "name": "q4" })),
            ("create_tag", json!({ "name": "q4", "groupId": "g" })),
            (
                "create_tag",
                json!({ "name": "q4", "assignProjectId": f.project_b }),
            ),
        ];
        refusals.extend(catalog_writes());
        for (cmd, args) in refusals {
            let err = check_with_store(cmd, &args, &g, &f.store)
                .expect_err(&format!("{cmd} {args} should be refused"));
            assert!(err.contains(&covers), "{cmd} {args}: {err}");
            assert!(
                !err.contains("confined to"),
                "{cmd} {args} still says the credential is confined to one project: {err}"
            );
        }
    }

    // -------------------------------------------------------------------
    // Access and scope breadth
    // -------------------------------------------------------------------

    #[test]
    fn read_only_grants_are_refused_the_writing_commands_only() {
        let f = fixture();
        let g = grant(Scope::Project(f.project_a.clone()), Access::ReadOnly);
        let mut writes = vec![
            ("create_project", json!({ "name": "gamma" })),
            (
                "add_project_repo",
                json!({ "projectId": f.project_a, "githubRepo": "acme/widgets" }),
            ),
            ("open_project", json!({ "projectId": f.project_a })),
            ("start_branch_session", json!({ "branchId": f.branch_a })),
            ("start_project_session", json!({ "projectId": f.project_a })),
            ("cancel_session", json!({ "sessionId": f.session_a })),
            (
                "create_image",
                json!({ "branchId": f.branch_a, "projectId": f.project_a, "filePath": "/tmp/x" }),
            ),
            (
                "set_project_group_tag",
                json!({ "projectId": f.project_a, "groupId": "g", "tagId": "t" }),
            ),
            (
                "set_project_tag_assigned",
                json!({ "projectId": f.project_a, "tagId": "t", "assigned": true }),
            ),
            (
                "create_tag",
                json!({ "name": "q4", "assignProjectId": f.project_a }),
            ),
        ];
        writes.extend(catalog_writes());
        for (cmd, args) in &writes {
            assert_denied(check_with_store(cmd, args, &g, &f.store), "read-only");
        }
        // The same holds for an unconfined read-only grant: the catalog
        // edits it could otherwise make are writes.
        let all_read = grant(Scope::AllProjects, Access::ReadOnly);
        for (cmd, args) in &writes {
            assert_denied(
                check_with_store(cmd, args, &all_read, &f.store),
                "read-only",
            );
        }
        let writing: Vec<&str> = CONTROL_SCOPE
            .iter()
            .filter(|(_, _, access)| *access == Access::ReadWrite)
            .map(|(name, ..)| *name)
            .collect();
        let expected: Vec<&str> = writes.iter().map(|(cmd, _)| *cmd).collect();
        assert_eq!(writing, expected, "the write set is exactly these commands");
        // Reads on the own project still pass.
        check_with_store(
            "get_session",
            &json!({ "sessionId": f.session_a }),
            &g,
            &f.store,
        )
        .unwrap();
        check_with_store("list_projects", &json!({}), &g, &f.store).unwrap();
        check_with_store("get_tag_state", &json!({}), &g, &f.store).unwrap();
        check_with_store("get_tag_state", &json!({}), &all_read, &f.store).unwrap();
    }

    #[test]
    fn an_all_projects_grant_skips_the_identifier_checks() {
        let f = fixture();
        let g = ControlGrant::test_all_projects();
        check_with_store(
            "list_project_repos",
            &json!({ "projectId": f.project_b }),
            &g,
            &f.store,
        )
        .unwrap();
        check_with_store(
            "get_branch_timeline",
            &json!({ "branchId": f.branch_b }),
            &g,
            &f.store,
        )
        .unwrap();
        check_with_store(
            "cancel_session",
            &json!({ "sessionId": f.session_b }),
            &g,
            &f.store,
        )
        .unwrap();
        // Unknown ids are the arm's problem to report, not a scope matter.
        check_with_store(
            "get_branch_timeline",
            &json!({ "branchId": "nope" }),
            &g,
            &f.store,
        )
        .unwrap();
        check_with_store(
            "start_branch_session",
            &json!({ "branchId": f.branch_b, "imageIds": [f.image_a, f.image_b] }),
            &g,
            &f.store,
        )
        .unwrap();
    }

    #[test]
    fn a_command_without_a_scope_row_is_refused() {
        let f = fixture();
        assert_denied(
            check_with_store(
                "delete_project",
                &json!({ "projectId": f.project_a }),
                &ControlGrant::test_all_projects(),
                &f.store,
            ),
            "no control scope rule",
        );
    }
}
