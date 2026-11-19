//! The note-transfer arms, run the way `dispatch` runs them for a control
//! caller: the scope check, then the arm's body, with a stand-in for the
//! app's repo add. The transfers are real and so are the answers, which are
//! asserted as the JSON a caller receives.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Value};

use super::*;
use crate::control_server::{Access, Actor, ControlGrant, Scope};
use crate::store::models::{
    Branch, Image, Note, Project, ProjectNote, ProjectRepo, Session, SessionStatus,
};
use crate::web_server::control_scope::{self, SCOPE_DENIED_PREFIX};

/// What the stand-in for the app's repo add does.
#[derive(Clone, Copy)]
enum Add {
    /// What the app's add leaves, minus git: the repo row and a branch that
    /// tracks it.
    Attach,
    /// The add fails.
    Fail,
    /// The add succeeds and the session `Fixture::resumed` names is running
    /// again when it returns, so the transfer's next read refuses the note.
    AttachAndResume,
}

/// The caller is a session in `own`. It created `created`, which tracks no
/// repo, and the user made `third`, which it does not reach. `own` tracks
/// `acme/widgets` at `apps/web` and `acme/gadgets`, with a branch on each.
struct Fixture {
    store: Store,
    own: Project,
    created: Project,
    third: Project,
    widgets: Branch,
    gadgets: Branch,
    third_branch: Branch,
    resumed: std::cell::RefCell<Option<String>>,
}

const CALLER: &str = "session:caller";

fn fixture() -> Fixture {
    let store = Store::in_memory().unwrap();
    let own = Project::named("own");
    let created = Project::named("created").with_created_by(CALLER);
    let third = Project::named("third");
    for project in [&own, &created, &third] {
        store.create_project(project).unwrap();
    }
    let branch_on = |project: &Project, github_repo: &str, subpath: Option<&str>, name: &str| {
        let repo = ProjectRepo::new(&project.id, github_repo, name, subpath.map(str::to_owned));
        store.create_project_repo(&repo).unwrap();
        let branch = Branch::new(&project.id, name, "origin/main").with_project_repo(&repo.id);
        store.create_branch(&branch).unwrap();
        branch
    };
    let widgets = branch_on(&own, "acme/widgets", Some("apps/web"), "feature");
    let gadgets = branch_on(&own, "acme/gadgets", None, "feature-2");
    let third_branch = branch_on(&third, "acme/widgets", Some("apps/web"), "theirs");
    Fixture {
        store,
        own,
        created,
        third,
        widgets,
        gadgets,
        third_branch,
        resumed: Default::default(),
    }
}

/// The caller's credential: a session's, confined to `own`.
fn session_grant(f: &Fixture) -> ControlGrant {
    ControlGrant {
        actor: Actor::Session {
            session_id: "caller".to_string(),
            epoch: 1,
        },
        scope: Scope::Project(f.own.id.clone()),
        branch_id: None,
        access: Access::ReadWrite,
    }
}

impl Fixture {
    /// A session that ran on `branch_id` and has completed.
    fn finished_session(&self, branch_id: Option<&str>) -> Session {
        let mut session = Session::new_running("write it up", Path::new("/tmp/wt"));
        if let Some(branch_id) = branch_id {
            session = session.with_branch(branch_id);
        }
        self.store.create_session(&session).unwrap();
        self.store
            .update_session_status(&session.id, SessionStatus::Completed, None, None)
            .unwrap();
        session
    }

    /// A branch note a finished session wrote.
    fn session_note(&self, branch: &Branch, title: &str) -> (Note, Session) {
        let session = self.finished_session(Some(&branch.id));
        let note = Note::new_standalone(&branch.id, title, "body").with_session(&session.id);
        self.store.create_note(&note).unwrap();
        (note, session)
    }

    /// A project note in `own` a finished session wrote, with a child note
    /// on each of `own`'s branches, each with its own finished session.
    fn family(&self) -> (ProjectNote, Vec<Note>) {
        let session = self.finished_session(None);
        let parent = ProjectNote::new(&self.own.id, "Plan", "body").with_session(&session.id);
        self.store.create_project_note(&parent).unwrap();
        let children = [(&self.widgets, "Web"), (&self.gadgets, "Gadgets")]
            .into_iter()
            .map(|(branch, title)| {
                let session = self.finished_session(Some(&branch.id));
                let child = Note::new_standalone(&branch.id, title, "child body")
                    .with_session(&session.id)
                    .with_parent_project_note(&parent.id);
                self.store.create_note(&child).unwrap();
                child
            })
            .collect();
        (parent, children)
    }

    /// An image row in `project_id`. No file is written: these tests stop at
    /// the scope check or carry no media.
    fn image_in(&self, project_id: &str, branch_id: Option<&str>) -> Image {
        let image = Image::new(branch_id, project_id, "shot.png", "image/png", 1, false);
        self.store.create_image(&image).unwrap();
        image
    }

    fn attach(&self, project_id: &str, key: RepoKey) -> Result<(), String> {
        let name = format!("landing-{}", key.github_repo.replace('/', "-"));
        let repo = ProjectRepo::new(project_id, &key.github_repo, &name, key.subpath);
        self.store
            .create_project_repo(&repo)
            .map_err(|e| e.to_string())?;
        let branch = Branch::new(project_id, &name, "origin/main").with_project_repo(&repo.id);
        self.store.create_branch(&branch).map_err(|e| e.to_string())
    }

    fn add(&self, how: Add, project_id: &str, key: RepoKey) -> Result<(), String> {
        match how {
            Add::Attach => self.attach(project_id, key),
            Add::Fail => Err("clone failed".to_string()),
            Add::AttachAndResume => {
                let session_id = self.resumed.borrow().clone().expect("a session to resume");
                self.store
                    .update_session_status(&session_id, SessionStatus::Running, None, None)
                    .unwrap();
                self.attach(project_id, key)
            }
        }
    }

    /// `command` as `dispatch` runs it: for a control caller the scope check
    /// ahead of every arm, then the arm's body.
    async fn dispatch(
        &self,
        caller: DispatchCaller,
        command: &str,
        args: Value,
        how: Add,
    ) -> Result<Value, String> {
        if let DispatchCaller::Control(grant) = &caller {
            control_scope::check_with_store(command, &args, grant, &self.store)?;
        }
        let add_repo =
            |project_id: String, key: RepoKey| std::future::ready(self.add(how, &project_id, key));
        match command {
            "move_note_to_project" | "copy_note_to_project" => {
                branch_note(&self.store, command, &args, &caller, add_repo).await
            }
            "move_project_note_to_project" | "copy_project_note_to_project" => {
                project_note(&self.store, command, &args, &caller, add_repo).await
            }
            other => panic!("{other} is not a note transfer"),
        }
    }

    /// [`Self::dispatch`] as the session, with a repo add that works.
    async fn control(&self, command: &str, args: Value) -> Result<Value, String> {
        self.dispatch(
            DispatchCaller::Control(session_grant(self)),
            command,
            args,
            Add::Attach,
        )
        .await
    }

    fn branch_of(&self, note_id: &str) -> String {
        self.store.get_note(note_id).unwrap().unwrap().branch_id
    }
}

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("an object: {value}"))
        .keys()
        .map(String::as_str)
        .collect()
}

/// The fields of a branch note as every caller is answered one: the
/// timeline's note item.
const NOTE_ITEM_KEYS: [&str; 19] = [
    "chatReadAt",
    "completedAt",
    "completionReason",
    "content",
    "createdAt",
    "id",
    "isPollingForNote",
    "lastChatMessageAt",
    "rateLimitKind",
    "rateLimitResetsAt",
    "readAt",
    "sessionErrorMessage",
    "sessionId",
    "sessionStatus",
    "subtype",
    "suggestedNextCommitStep",
    "suggestedNextNoteStep",
    "title",
    "updatedAt",
];

fn widgets_key() -> Value {
    json!({ "githubRepo": "acme/widgets", "subpath": "apps/web" })
}

fn gadgets_key() -> Value {
    json!({ "githubRepo": "acme/gadgets", "subpath": null })
}

// ---------------------------------------------------------------------------
// Branch notes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_moved_branch_note_is_answered_with_its_note_item_and_where_it_landed() {
    let f = fixture();
    let (note, session) = f.session_note(&f.widgets, "Findings");

    let answer = f
        .control(
            "move_note_to_project",
            json!({ "noteId": note.id, "targetProjectId": f.created.id }),
        )
        .await
        .unwrap();

    assert_eq!(
        keys(&answer),
        BTreeSet::from(["addedRepos", "branch", "note", "repo"])
    );
    // The note is the item a window gets, with its session resolved.
    assert_eq!(keys(&answer["note"]), BTreeSet::from(NOTE_ITEM_KEYS));
    assert_eq!(answer["note"]["id"], note.id, "a move keeps the note's id");
    assert_eq!(answer["note"]["title"], "Findings");
    assert_eq!(answer["note"]["content"], "body");
    assert_eq!(answer["note"]["sessionId"], session.id);
    assert_eq!(answer["note"]["sessionStatus"], "completed");
    // Where it landed, as the branch list and the repo list give a row.
    let landed = f.branch_of(&note.id);
    assert_eq!(answer["branch"]["id"], landed);
    assert_eq!(answer["branch"]["projectId"], f.created.id);
    assert_eq!(answer["branch"]["branchName"], "landing-acme-widgets");
    assert_eq!(answer["branch"].get("worktreePath"), Some(&Value::Null));
    assert_eq!(answer["repo"]["id"], answer["branch"]["projectRepoId"]);
    assert_eq!(answer["repo"]["projectId"], f.created.id);
    assert_eq!(answer["repo"]["githubRepo"], "acme/widgets");
    assert_eq!(answer["repo"]["subpath"], "apps/web");
    assert_eq!(answer["addedRepos"], json!([widgets_key()]));
}

#[tokio::test]
async fn a_copied_branch_note_is_answered_with_the_new_standalone_note() {
    let f = fixture();
    let (note, _) = f.session_note(&f.widgets, "Findings");

    let answer = f
        .control(
            "copy_note_to_project",
            json!({ "noteId": note.id, "targetProjectId": f.created.id }),
        )
        .await
        .unwrap();

    assert_eq!(
        keys(&answer),
        BTreeSet::from(["addedRepos", "branch", "note", "repo"])
    );
    assert_eq!(keys(&answer["note"]), BTreeSet::from(NOTE_ITEM_KEYS));
    let copy_id = answer["note"]["id"].as_str().unwrap();
    assert_ne!(copy_id, note.id, "a copy is a new note");
    assert_eq!(answer["note"]["title"], "Findings");
    assert_eq!(answer["note"]["sessionId"], Value::Null);
    assert_eq!(answer["note"]["sessionStatus"], Value::Null);
    assert_eq!(answer["note"]["subtype"], "written");
    assert_eq!(answer["branch"]["id"], f.branch_of(copy_id));
    assert_eq!(answer["branch"]["projectId"], f.created.id);
    assert_eq!(answer["repo"]["githubRepo"], "acme/widgets");
    assert_eq!(answer["addedRepos"], json!([widgets_key()]));
    assert_eq!(f.branch_of(&note.id), f.widgets.id, "the original stays");
}

/// The transfer is the one a window runs, and a window is answered with the
/// note alone, as it was before a control caller could ask.
#[tokio::test]
async fn a_window_is_answered_with_the_note_alone() {
    let f = fixture();
    let (note, _) = f.session_note(&f.widgets, "Findings");
    let answer = f
        .dispatch(
            DispatchCaller::Web,
            "move_note_to_project",
            json!({ "noteId": note.id, "targetProjectId": f.created.id }),
            Add::Attach,
        )
        .await
        .unwrap();
    assert_eq!(keys(&answer), BTreeSet::from(NOTE_ITEM_KEYS));
    assert_eq!(answer["id"], note.id);

    let (parent, _) = f.family();
    let before = f.store.get_project_note(&parent.id).unwrap().unwrap();
    let answer = f
        .dispatch(
            DispatchCaller::Web,
            "move_project_note_to_project",
            json!({ "projectNoteId": parent.id, "targetProjectId": f.created.id }),
            Add::Attach,
        )
        .await
        .unwrap();
    assert_eq!(keys(&answer), keys(&serde_json::to_value(&before).unwrap()));
    assert_eq!(answer["id"], parent.id);
    assert_eq!(answer["projectId"], f.created.id);
}

// ---------------------------------------------------------------------------
// Project notes
// ---------------------------------------------------------------------------

/// A moved project note keeps its session, and the answer says how that
/// session stands. The row the move returns does not, so the answer reads
/// the note again.
#[tokio::test]
async fn a_moved_project_note_is_answered_with_its_session_status_and_its_children() {
    let f = fixture();
    let (parent, children) = f.family();
    // The destination already tracks one of the two repos.
    f.attach(
        &f.created.id,
        RepoKey {
            github_repo: "acme/widgets".into(),
            subpath: Some("apps/web".into()),
        },
    )
    .unwrap();

    let answer = f
        .control(
            "move_project_note_to_project",
            json!({ "projectNoteId": parent.id, "targetProjectId": f.created.id }),
        )
        .await
        .unwrap();

    assert_eq!(
        keys(&answer),
        BTreeSet::from(["addedRepos", "children", "note"])
    );
    assert_eq!(answer["note"]["id"], parent.id);
    assert_eq!(answer["note"]["projectId"], f.created.id);
    assert_eq!(answer["note"]["title"], "Plan");
    assert_eq!(
        answer["note"]["sessionId"],
        parent.session_id.clone().unwrap()
    );
    assert_eq!(answer["note"]["sessionStatus"], "completed");
    // The note reads as the project's note list gives it.
    let listed = f
        .store
        .list_project_notes_with_status(&f.created.id)
        .unwrap();
    assert_eq!(answer["note"], serde_json::to_value(&listed[0]).unwrap());

    // Each child, by its reference, on the destination's branch for its
    // repo.
    let listed: Vec<&Value> = answer["children"].as_array().unwrap().iter().collect();
    assert_eq!(listed.len(), 2);
    for (child, title, github_repo, subpath, branch_name) in [
        (
            &children[0],
            "Web",
            "acme/widgets",
            json!("apps/web"),
            "landing-acme-widgets",
        ),
        (
            &children[1],
            "Gadgets",
            "acme/gadgets",
            Value::Null,
            "landing-acme-gadgets",
        ),
    ] {
        let reference = format!("#note:{}", child.id);
        let row = listed
            .iter()
            .find(|row| row["reference"] == reference.as_str())
            .unwrap_or_else(|| panic!("{reference} is not among {listed:?}"));
        assert_eq!(
            keys(row),
            BTreeSet::from(["branch", "reference", "repo", "title"])
        );
        assert_eq!(row["title"], title);
        assert_eq!(row["branch"]["id"], f.branch_of(&child.id));
        assert_eq!(row["branch"]["projectId"], f.created.id);
        assert_eq!(row["branch"]["branchName"], branch_name);
        assert_eq!(row["repo"]["id"], row["branch"]["projectRepoId"]);
        assert_eq!(row["repo"]["githubRepo"], github_repo);
        assert_eq!(row["repo"]["subpath"], subpath);
    }
    // Only the repo the destination lacked was added.
    assert_eq!(answer["addedRepos"], json!([gadgets_key()]));
}

#[tokio::test]
async fn a_copied_project_note_is_answered_without_children_or_repos() {
    let f = fixture();
    let (parent, children) = f.family();

    let answer = f
        .control(
            "copy_project_note_to_project",
            json!({ "projectNoteId": parent.id, "targetProjectId": f.created.id }),
        )
        .await
        .unwrap();

    assert_eq!(
        keys(&answer),
        BTreeSet::from(["addedRepos", "children", "note"])
    );
    assert_ne!(answer["note"]["id"], parent.id);
    assert_eq!(answer["note"]["projectId"], f.created.id);
    assert_eq!(answer["note"]["sessionId"], Value::Null);
    assert_eq!(answer["note"]["sessionStatus"], Value::Null);
    assert_eq!(answer["children"], json!([]));
    assert_eq!(answer["addedRepos"], json!([]));
    for child in &children {
        assert_eq!(f.branch_of(&child.id), child.branch_id, "children stay");
    }
}

// ---------------------------------------------------------------------------
// A failure after a repo was added
// ---------------------------------------------------------------------------

/// The repo is in the destination by the time the transfer refuses the
/// note. A control caller cannot see that, so its error says so. A window's
/// is the transfer's, as it was.
#[tokio::test]
async fn a_branch_note_failure_after_the_repo_was_added_names_it_to_a_control_caller() {
    const REFUSAL: &str =
        "\"Findings\"'s session is running again; wait for it to finish before moving the note.";
    for (caller_is_control, expected) in [
        (
            true,
            format!(
                "{REFUSAL} acme/widgets at apps/web was added to created along the way and \
                 remains tracked there."
            ),
        ),
        (false, REFUSAL.to_string()),
    ] {
        let f = fixture();
        let (note, session) = f.session_note(&f.widgets, "Findings");
        *f.resumed.borrow_mut() = Some(session.id.clone());
        let caller = if caller_is_control {
            DispatchCaller::Control(session_grant(&f))
        } else {
            DispatchCaller::Web
        };

        let err = f
            .dispatch(
                caller,
                "move_note_to_project",
                json!({ "noteId": note.id, "targetProjectId": f.created.id }),
                Add::AttachAndResume,
            )
            .await
            .unwrap_err();

        assert_eq!(err, expected);
        assert_eq!(f.branch_of(&note.id), f.widgets.id, "the note stays put");
        assert!(
            f.store
                .find_project_repo(&f.created.id, "acme/widgets", Some("apps/web"))
                .unwrap()
                .is_some(),
            "the repo stays added"
        );
    }
}

/// A project note's failure names the added repo for every caller, in the
/// transfer's own words. The control answer must not say it a second time.
#[tokio::test]
async fn a_project_note_failure_after_a_repo_was_added_names_it_once() {
    let f = fixture();
    let (parent, children) = f.family();
    *f.resumed.borrow_mut() = parent.session_id.clone();

    let err = f
        .dispatch(
            DispatchCaller::Control(session_grant(&f)),
            "move_project_note_to_project",
            json!({ "projectNoteId": parent.id, "targetProjectId": f.created.id }),
            Add::AttachAndResume,
        )
        .await
        .unwrap_err();

    // The children's repos are added in the order the children are listed.
    let said = |first: &str, second: &str| {
        format!(
            "\"Plan\"'s session is running again; wait for it to finish before moving the \
             note. {first} and {second} were added to created along the way and remain tracked \
             there."
        )
    };
    assert!(
        err == said("acme/widgets at apps/web", "acme/gadgets")
            || err == said("acme/gadgets", "acme/widgets at apps/web"),
        "{err}"
    );
    assert_eq!(
        f.store
            .get_project_note(&parent.id)
            .unwrap()
            .unwrap()
            .project_id,
        f.own.id
    );
    for child in &children {
        assert_eq!(f.branch_of(&child.id), child.branch_id);
    }
}

/// A repo whose add failed was not added, and the error does not say it
/// was.
#[tokio::test]
async fn a_failed_repo_add_is_reported_as_the_transfer_words_it() {
    let f = fixture();
    let (note, _) = f.session_note(&f.widgets, "Findings");

    let err = f
        .dispatch(
            DispatchCaller::Control(session_grant(&f)),
            "copy_note_to_project",
            json!({ "noteId": note.id, "targetProjectId": f.created.id }),
            Add::Fail,
        )
        .await
        .unwrap_err();

    assert_eq!(
        err,
        "Could not add acme/widgets at apps/web to created: clone failed"
    );
}

// ---------------------------------------------------------------------------
// Scope, end to end
// ---------------------------------------------------------------------------

/// A note in the caller's own project whose body embeds media from a
/// project out of its reach is refused before the transfer runs: nothing
/// moves, and nothing of that media lands in the destination.
#[tokio::test]
async fn media_from_a_project_out_of_reach_stops_the_transfer_before_it_runs() {
    let f = fixture();
    let foreign = f.image_in(&f.third.id, Some(&f.third_branch.id));
    let body = format!("see ![theirs](staged-media://{}.png)", foreign.id);
    let (note, _) = f.session_note(&f.widgets, "Findings");
    f.store.update_note_content(&note.id, &body).unwrap();
    let (parent, children) = f.family();
    f.store.update_note_content(&children[1].id, &body).unwrap();

    for (command, args) in [
        (
            "move_note_to_project",
            json!({ "noteId": note.id, "targetProjectId": f.created.id }),
        ),
        (
            "copy_note_to_project",
            json!({ "noteId": note.id, "targetProjectId": f.created.id }),
        ),
        // Into the note's own project too: a copy there duplicates the
        // media just the same.
        (
            "copy_note_to_project",
            json!({ "noteId": note.id, "targetProjectId": f.own.id }),
        ),
        // Through a child note, which a move takes along.
        (
            "move_project_note_to_project",
            json!({ "projectNoteId": parent.id, "targetProjectId": f.created.id }),
        ),
    ] {
        let err = f.control(command, args).await.unwrap_err();
        assert!(
            err.starts_with(SCOPE_DENIED_PREFIX),
            "{command} must be refused by scope: {err}"
        );
        assert!(
            err.contains(&format!("embeds media {}", foreign.id))
                && err.contains("belongs to another project"),
            "{command}: {err}"
        );
    }

    assert_eq!(f.branch_of(&note.id), f.widgets.id);
    assert_eq!(
        f.store
            .get_project_note(&parent.id)
            .unwrap()
            .unwrap()
            .project_id,
        f.own.id
    );
    assert!(
        f.store
            .list_project_repos(&f.created.id)
            .unwrap()
            .is_empty(),
        "a refused transfer adds no repo"
    );
    let still = f.store.get_image(&foreign.id).unwrap().unwrap();
    assert_eq!(still.project_id, f.third.id);

    // A copy of the project note leaves its children where they are, so
    // what a child embeds is not the copy's to answer for.
    let answer = f
        .control(
            "copy_project_note_to_project",
            json!({ "projectNoteId": parent.id, "targetProjectId": f.created.id }),
        )
        .await
        .unwrap();
    assert_eq!(answer["children"], json!([]));
}

/// A note the confined caller cannot be shown is a scope refusal, whether
/// its id is unknown or names the other kind of note. An unconfined caller
/// is told by the transfer.
#[tokio::test]
async fn a_note_id_that_names_no_note_of_that_kind_is_refused_by_scope_when_confined() {
    let f = fixture();
    let (parent, _) = f.family();
    for (command, args, wording) in [
        (
            "move_note_to_project",
            json!({ "noteId": "nope", "targetProjectId": f.created.id }),
            "note nope not found".to_string(),
        ),
        (
            "copy_note_to_project",
            json!({ "noteId": parent.id, "targetProjectId": f.created.id }),
            format!("note {} not found", parent.id),
        ),
    ] {
        let err = f.control(command, args.clone()).await.unwrap_err();
        assert_eq!(err, format!("{SCOPE_DENIED_PREFIX}{wording}"));

        let err = f
            .dispatch(
                DispatchCaller::Control(ControlGrant::test_all_projects()),
                command,
                args,
                Add::Attach,
            )
            .await
            .unwrap_err();
        assert!(err.starts_with("Note not found: "), "{err}");
    }
}
