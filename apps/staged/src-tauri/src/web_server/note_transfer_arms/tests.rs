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

fn widgets_repo() -> RepoKey {
    RepoKey {
        github_repo: "acme/widgets".into(),
        subpath: Some("apps/web".into()),
    }
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

/// Only a repo the transfer added is named as added. A destination that
/// tracks the note's repo already gains nothing, and the note lands on the
/// branch it has for that repo.
#[tokio::test]
async fn a_branch_note_sent_to_a_project_that_tracks_its_repo_adds_none() {
    let f = fixture();
    let (note, _) = f.session_note(&f.widgets, "Findings");
    f.attach(&f.created.id, widgets_repo()).unwrap();

    for command in ["copy_note_to_project", "move_note_to_project"] {
        let answer = f
            .control(
                command,
                json!({ "noteId": note.id, "targetProjectId": f.created.id }),
            )
            .await
            .unwrap();
        assert_eq!(answer["addedRepos"], json!([]), "{command}");
        assert_eq!(answer["branch"]["projectId"], f.created.id, "{command}");
        assert_eq!(
            answer["branch"]["branchName"], "landing-acme-widgets",
            "{command}"
        );
        assert_eq!(answer["repo"]["githubRepo"], "acme/widgets", "{command}");
    }
    assert_eq!(
        f.store.list_project_repos(&f.created.id).unwrap().len(),
        1,
        "the destination tracks the one repo it had"
    );
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
    f.attach(&f.created.id, widgets_repo()).unwrap();

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
    // Beside a reference with no row, which is no reason to refuse. Its id
    // sorts ahead of any the store makes, so the check passes over it first
    // and the refusal is for the media that is someone else's.
    const NO_ROW: &str = "00000000-0000-4000-8000-000000000000";
    assert!(NO_ROW < foreign.id.as_str());
    assert!(f.store.get_image(NO_ROW).unwrap().is_none());
    let body = format!(
        "see ![gone](staged-media://{NO_ROW}.png) and ![theirs](staged-media://{}.png)",
        foreign.id
    );
    assert_eq!(crate::note_media::refs::media_ids(&body).len(), 2);
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

/// A reference to media that has no row carries nothing, so it is nobody
/// else's to protect. The scope check lets it through and the transfer
/// leaves it as written, for a confined caller as for the app's windows.
///
/// The id is a well-formed one. Anything else is not a reference to stored
/// media at all, and would reach neither the check's row lookup nor the
/// transfer's.
#[tokio::test]
async fn a_reference_to_media_with_no_row_is_let_through_and_left_as_written() {
    let f = fixture();
    let dangling = uuid::Uuid::new_v4().to_string();
    assert!(f.store.get_image(&dangling).unwrap().is_none());
    let body = format!("see ![gone](staged-media://{dangling}.png)");
    assert!(
        crate::note_media::refs::media_ids(&body).contains(&dangling),
        "the reference must be one the check and the transfer look up"
    );
    let (note, _) = f.session_note(&f.widgets, "Findings");
    f.store.update_note_content(&note.id, &body).unwrap();
    let (parent, children) = f.family();
    f.store
        .update_project_note_title_and_content(&parent.id, "Plan", &body, None, None)
        .unwrap();
    f.store.update_note_content(&children[1].id, &body).unwrap();

    let copy = f
        .control(
            "copy_note_to_project",
            json!({ "noteId": note.id, "targetProjectId": f.created.id }),
        )
        .await
        .unwrap();
    assert_eq!(copy["note"]["content"], body.as_str());
    let moved = f
        .control(
            "move_note_to_project",
            json!({ "noteId": note.id, "targetProjectId": f.created.id }),
        )
        .await
        .unwrap();
    assert_eq!(moved["note"]["content"], body.as_str());
    assert_ne!(f.branch_of(&note.id), f.widgets.id, "the note moved");

    // In a project note and in a child that travels with it.
    let moved = f
        .control(
            "move_project_note_to_project",
            json!({ "projectNoteId": parent.id, "targetProjectId": f.created.id }),
        )
        .await
        .unwrap();
    assert_eq!(moved["note"]["projectId"], f.created.id);
    assert_eq!(moved["note"]["content"], body.as_str());
    let child = f.store.get_note(&children[1].id).unwrap().unwrap();
    assert_eq!(child.content, body);
    assert_ne!(child.branch_id, children[1].branch_id, "the child moved");

    assert!(
        f.store.get_image(&dangling).unwrap().is_none(),
        "no row is made for it"
    );
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

// ---------------------------------------------------------------------------
// The answers stagedctl reads
// ---------------------------------------------------------------------------

/// One answer of each kind, as a control caller receives it, cut down to
/// the fields `stagedctl` reads. `stagedctl` parses this same file in its
/// own tests (`notes/tests.rs`), so the names the app writes and the names
/// the CLI reads are held to one document. The app's answer has to contain
/// it. A field it pins that is renamed, removed or answered differently
/// fails a test until both sides agree. A field it does not pin, such as
/// one a model gains, passes.
const ANSWERS_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/note_transfer/control/fixtures/answers.json"
);

/// Set to `1` to rewrite the fixture's values from what the app answers
/// now. The fields it pins stay the ones it has, so add or drop one by
/// hand. A field the answer no longer has stops the rewrite.
const UPDATE_ANSWERS_FIXTURE: &str = "STAGED_UPDATE_NOTE_TRANSFER_ANSWERS";

/// The part of `answer` that `pinned` names: of an object, the fields
/// `pinned` has, and of a list, each item against the item `pinned` has in
/// its place. Anything else is `answer` as it stands. `answer` contains
/// `pinned` when this is equal to `pinned`, whatever else `answer` carries.
///
/// A field `pinned` has and `answer` lacks is an error that names it, and
/// so is a list of another length. `null` is a value like any other: a
/// field pinned as `null` has to be there.
fn pinned_part(answer: &Value, pinned: &Value, path: &str) -> Result<Value, String> {
    match (answer, pinned) {
        (Value::Object(answer), Value::Object(pinned)) => pinned
            .iter()
            .map(|(key, pinned)| {
                let path = match path {
                    "" => key.clone(),
                    path => format!("{path}.{key}"),
                };
                let value = answer
                    .get(key)
                    .ok_or_else(|| format!("the answer has no `{path}`"))?;
                Ok((key.clone(), pinned_part(value, pinned, &path)?))
            })
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(Value::Object),
        (Value::Array(answer), Value::Array(pinned)) => {
            if answer.len() != pinned.len() {
                return Err(format!(
                    "`{path}` has {} items in the answer and {} in the fixture",
                    answer.len(),
                    pinned.len()
                ));
            }
            answer
                .iter()
                .zip(pinned)
                .enumerate()
                .map(|(at, (answer, pinned))| pinned_part(answer, pinned, &format!("{path}[{at}]")))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        _ => Ok(answer.clone()),
    }
}

/// `answer` with what differs from run to run made the same: each id is
/// replaced by the name `names` gives it, and each timestamp by `0`. An id
/// left without a name is a mistake in the test, since it would never
/// compare equal twice.
fn settled(answer: &Value, names: &[(String, &str)]) -> Value {
    match answer {
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| {
                let value = if key.ends_with("At") && value.is_number() {
                    json!(0)
                } else {
                    settled(value, names)
                };
                (key.clone(), value)
            })
            .collect::<serde_json::Map<_, _>>()
            .into(),
        Value::Array(items) => items.iter().map(|item| settled(item, names)).collect(),
        Value::String(text) => {
            let named = names
                .iter()
                .fold(text.clone(), |text, (id, name)| text.replace(id, name));
            let id = named.rsplit(':').next().unwrap_or(&named);
            assert!(
                uuid::Uuid::parse_str(id).is_err(),
                "{text:?} is an id the fixture has no name for"
            );
            json!(named)
        }
        other => other.clone(),
    }
}

/// A move of a branch note and a move of a project note, each into a
/// project that has to gain a repo for it, answered to the session. These
/// are the two shapes `stagedctl` reads: the note item, the landing branch
/// and repo, the children and theirs, and the repos added. Each has to
/// contain what the fixture pins of it.
#[tokio::test]
async fn the_answers_contain_the_ones_stagedctl_is_tested_against() {
    // A branch note, whose repo the destination lacks.
    let f = fixture();
    let (note, _) = f.session_note(&f.widgets, "Findings");
    let branch_note = f
        .control(
            "move_note_to_project",
            json!({ "noteId": note.id, "targetProjectId": f.created.id }),
        )
        .await
        .unwrap();
    let mut names = vec![
        (f.branch_of(&note.id), "branch-widgets"),
        (note.id, "note-findings"),
    ];

    // A project note with two children. The destination tracks the repo of
    // one and gains the other's.
    let f = fixture();
    let (parent, children) = f.family();
    f.attach(&f.created.id, widgets_repo()).unwrap();
    let mut project_note = f
        .control(
            "move_project_note_to_project",
            json!({ "projectNoteId": parent.id, "targetProjectId": f.created.id }),
        )
        .await
        .unwrap();
    // The app lists children newest first, and these two were written in
    // the same instant. By title, so the fixture has one order.
    project_note["children"]
        .as_array_mut()
        .expect("the children were read back")
        .sort_by_key(|child| child["title"].as_str().map(str::to_owned));
    names.extend([
        (parent.id, "project-note-plan"),
        (f.branch_of(&children[0].id), "branch-widgets"),
        (children[0].id.clone(), "note-web"),
        (f.branch_of(&children[1].id), "branch-gadgets"),
        (children[1].id.clone(), "note-gadgets"),
    ]);

    let pinned: Value = serde_json::from_str(
        &std::fs::read_to_string(ANSWERS_FIXTURE).expect("the answers fixture"),
    )
    .expect("the answers fixture is JSON");
    let remedy = format!(
        "If that is meant, change what stagedctl reads (`Landed` in its `notes.rs`) and the \
         fixture to match. {UPDATE_ANSWERS_FIXTURE}=1 rewrites the fixture's values. \
         stagedctl's tests read the same file"
    );
    let answers = json!({ "branchNote": branch_note, "projectNote": project_note });
    let answers = match pinned_part(&answers, &pinned, "") {
        Ok(part) => settled(&part, &names),
        Err(gone) => panic!("{gone}, which stagedctl reads. {remedy}"),
    };
    if std::env::var(UPDATE_ANSWERS_FIXTURE).as_deref() == Ok("1") {
        let mut text = serde_json::to_string_pretty(&answers).unwrap();
        text.push('\n');
        std::fs::write(ANSWERS_FIXTURE, text).expect("write the answers fixture");
        return;
    }
    assert_eq!(
        answers, pinned,
        "a control caller's answer no longer contains the fixture. {remedy}"
    );
}

/// The check the fixture is held to, on answers made up for it: a field
/// the answer gains passes, and one that is renamed, removed or answered
/// differently does not.
#[test]
fn an_answer_contains_the_fixture_whatever_else_it_carries() {
    let pinned = json!({
        "note": { "id": "n1", "title": "Plan" },
        "repo": { "githubRepo": "acme/widgets", "subpath": null },
        "addedRepos": [{ "githubRepo": "acme/widgets" }],
    });
    let answer = json!({
        "note": { "id": "n1", "title": "Plan", "content": "body", "readAt": null },
        "repo": { "id": "r1", "githubRepo": "acme/widgets", "subpath": null },
        "addedRepos": [{ "githubRepo": "acme/widgets", "subpath": null }],
        "branch": null,
    });
    assert_eq!(pinned_part(&answer, &pinned, ""), Ok(pinned.clone()));

    // Renamed or removed: a pinned field is not there, `null` or not.
    let mut renamed = answer.clone();
    let repo = renamed["repo"].as_object_mut().unwrap();
    let name = repo.remove("githubRepo").unwrap();
    repo.insert("github_repo".into(), name);
    assert_eq!(
        pinned_part(&renamed, &pinned, ""),
        Err("the answer has no `repo.githubRepo`".to_string())
    );
    let mut removed = answer.clone();
    removed["repo"].as_object_mut().unwrap().remove("subpath");
    assert_eq!(
        pinned_part(&removed, &pinned, ""),
        Err("the answer has no `repo.subpath`".to_string())
    );

    // Answered differently: there, with another value or another shape.
    for (field, value) in [
        ("/note/title", json!("Plans")),
        ("/repo/subpath", json!("apps/web")),
        ("/repo", Value::Null),
        ("/addedRepos/0/githubRepo", json!("acme/gadgets")),
    ] {
        let mut changed = answer.clone();
        *changed.pointer_mut(field).unwrap() = value;
        assert_ne!(
            pinned_part(&changed, &pinned, ""),
            Ok(pinned.clone()),
            "{field}"
        );
    }

    // A list is held item for item, so one that grew is not contained.
    let mut grown = answer.clone();
    let added = grown["addedRepos"].as_array_mut().unwrap();
    added.push(json!({ "githubRepo": "acme/gadgets" }));
    assert_eq!(
        pinned_part(&grown, &pinned, ""),
        Err("`addedRepos` has 2 items in the answer and 1 in the fixture".to_string())
    );
}
