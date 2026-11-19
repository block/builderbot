use serde_json::json;

use super::*;
use crate::note_transfer::{transfer_branch_note, transfer_project_note, TransferMode};
use crate::store::models::{
    Branch, Note, Project, ProjectNote, ProjectRepo, Session, SessionStatus,
};

/// `source` tracks `acme/widgets` at `apps/web` and `acme/gadgets`, with a
/// branch on each. `target` tracks nothing.
struct Fixture {
    store: Store,
    source: Project,
    target: Project,
    widgets: Branch,
    gadgets: Branch,
}

fn fixture() -> Fixture {
    let store = Store::in_memory().unwrap();
    let source = Project::named("source");
    let target = Project::named("target");
    store.create_project(&source).unwrap();
    store.create_project(&target).unwrap();
    let branch_on = |github_repo: &str, subpath: Option<&str>, name: &str| {
        let repo = ProjectRepo::new(&source.id, github_repo, name, subpath.map(str::to_owned));
        store.create_project_repo(&repo).unwrap();
        let branch = Branch::new(&source.id, name, "origin/main").with_project_repo(&repo.id);
        store.create_branch(&branch).unwrap();
        branch
    };
    let widgets = branch_on("acme/widgets", Some("apps/web"), "feature");
    let gadgets = branch_on("acme/gadgets", None, "feature-2");
    Fixture {
        store,
        source,
        target,
        widgets,
        gadgets,
    }
}

impl Fixture {
    fn note_on(&self, branch: &Branch, title: &str) -> Note {
        let note = Note::new_standalone(&branch.id, title, "body");
        self.store.create_note(&note).unwrap();
        note
    }

    /// What the app's repo add leaves in `target`, minus git: the repo row
    /// and a branch that tracks it.
    fn attach(&self, key: RepoKey) -> Result<(), String> {
        let name = format!("target-{}", key.github_repo.replace('/', "-"));
        let repo = ProjectRepo::new(&self.target.id, &key.github_repo, &name, key.subpath);
        self.store
            .create_project_repo(&repo)
            .map_err(|e| e.to_string())?;
        let branch = Branch::new(&self.target.id, &name, "origin/main").with_project_repo(&repo.id);
        self.store.create_branch(&branch).map_err(|e| e.to_string())
    }
}

#[tokio::test]
async fn a_repo_whose_add_failed_is_not_reported_as_added() {
    let f = fixture();
    let note = f.note_on(&f.widgets, "Findings");
    let added = AddedRepos::default();

    let err = transfer_branch_note(
        &f.store,
        &note.id,
        &f.target.id,
        TransferMode::Move,
        |key| added.watch(key, std::future::ready(Err("clone failed".to_string()))),
    )
    .await
    .unwrap_err();

    assert_eq!(
        err,
        "Could not add acme/widgets at apps/web to target: clone failed"
    );
    assert_eq!(added.into_json(), json!([]));
}

/// A moved project note brings its children, and each child's repo is added
/// where the destination lacks it: the answer lists those, in the order the
/// transfer added them, and not a repo the destination already had. It
/// names each child and the branch it landed on, and it says how the note's
/// session stands, which the row the move returns does not.
#[tokio::test]
async fn a_project_note_move_lists_its_children_and_the_repos_they_brought() {
    let f = fixture();
    let session = Session::new_running("plan", std::path::Path::new("/tmp/wt"));
    f.store.create_session(&session).unwrap();
    f.store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    let parent = ProjectNote::new(&f.source.id, "Plan", "body").with_session(&session.id);
    f.store.create_project_note(&parent).unwrap();
    let mut children = Vec::new();
    for (branch, title) in [(&f.widgets, "Web"), (&f.gadgets, "Gadgets")] {
        let child =
            Note::new_standalone(&branch.id, title, "body").with_parent_project_note(&parent.id);
        f.store.create_note(&child).unwrap();
        children.push(child);
    }
    f.attach(RepoKey {
        github_repo: "acme/widgets".into(),
        subpath: Some("apps/web".into()),
    })
    .unwrap();

    let added = AddedRepos::default();
    let moved = transfer_project_note(
        &f.store,
        &parent.id,
        &f.target.id,
        TransferMode::Move,
        |key| added.watch(key.clone(), std::future::ready(f.attach(key))),
    )
    .await
    .unwrap();
    assert_eq!(
        moved.session_status, None,
        "the move's own row resolves no session, which is why the answer reads it again"
    );
    let answer = project_note_answer(&f.store, moved, added);

    assert_eq!(answer["note"]["id"], parent.id);
    assert_eq!(answer["note"]["projectId"], f.target.id);
    assert_eq!(answer["note"]["sessionId"], session.id);
    assert_eq!(answer["note"]["sessionStatus"], "completed");
    assert_eq!(
        answer["addedRepos"],
        json!([{ "githubRepo": "acme/gadgets", "subpath": null }])
    );
    let listed = answer["children"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    for (child, github_repo, branch_name) in [
        (&children[0], "acme/widgets", "target-acme-widgets"),
        (&children[1], "acme/gadgets", "target-acme-gadgets"),
    ] {
        let reference = format!("#note:{}", child.id);
        let row = listed
            .iter()
            .find(|row| row["reference"] == reference.as_str())
            .unwrap_or_else(|| panic!("{reference} is not among {listed:?}"));
        assert_eq!(row["title"], child.title);
        assert_eq!(row["branch"]["projectId"], f.target.id);
        assert_eq!(row["branch"]["branchName"], branch_name);
        assert_eq!(row["repo"]["githubRepo"], github_repo);
        assert_eq!(row["repo"]["id"], row["branch"]["projectRepoId"]);
    }
}

#[tokio::test]
async fn a_project_note_copy_adds_no_repo_and_has_no_children() {
    let f = fixture();
    let parent = ProjectNote::new(&f.source.id, "Plan", "body");
    f.store.create_project_note(&parent).unwrap();
    let child =
        Note::new_standalone(&f.widgets.id, "Web", "body").with_parent_project_note(&parent.id);
    f.store.create_note(&child).unwrap();

    let added = AddedRepos::default();
    let copy = transfer_project_note(
        &f.store,
        &parent.id,
        &f.target.id,
        TransferMode::Copy,
        |key| added.watch(key.clone(), std::future::ready(f.attach(key))),
    )
    .await
    .unwrap();
    let answer = project_note_answer(&f.store, copy, added);

    assert_ne!(answer["note"]["id"], parent.id);
    assert_eq!(answer["note"]["projectId"], f.target.id);
    assert_eq!(answer["note"]["sessionStatus"], Value::Null);
    assert_eq!(answer["children"], json!([]));
    assert_eq!(answer["addedRepos"], json!([]));
}

/// As for a branch note's rows: a project note that cannot be read again is
/// answered as the transfer returned it, and it is not an error.
#[test]
fn a_project_note_that_cannot_be_read_back_is_answered_as_the_transfer_gave_it() {
    let f = fixture();
    let note = ProjectNote::new(&f.target.id, "Plan", "body");
    let answer = project_note_answer(&f.store, note.clone(), AddedRepos::default());
    assert_eq!(
        answer,
        json!({ "note": note, "children": [], "addedRepos": [] })
    );
}

/// Child notes that cannot be read back are answered as `null`, which is
/// not the empty list of a note without any: the move took them along, and
/// the answer must not read as though the note has none. The rest of the
/// answer stands.
#[tokio::test]
async fn children_that_cannot_be_read_back_are_answered_as_null() {
    let f = fixture();
    let parent = ProjectNote::new(&f.source.id, "Plan", "body");
    f.store.create_project_note(&parent).unwrap();
    let child =
        Note::new_standalone(&f.widgets.id, "Web", "body").with_parent_project_note(&parent.id);
    f.store.create_note(&child).unwrap();

    let added = AddedRepos::default();
    let moved = transfer_project_note(
        &f.store,
        &parent.id,
        &f.target.id,
        TransferMode::Move,
        |key| added.watch(key.clone(), std::future::ready(f.attach(key))),
    )
    .await
    .unwrap();
    assert_eq!(
        f.store.list_child_notes(&parent.id).unwrap().len(),
        1,
        "the child came along"
    );

    // The move has committed. From here the notes cannot be listed.
    f.store
        .hold_connection()
        .execute_batch("ALTER TABLE notes RENAME COLUMN parent_project_note_id TO unreadable")
        .unwrap();
    assert!(f.store.list_child_notes(&parent.id).is_err());

    let answer = project_note_answer(&f.store, moved, added);

    assert_eq!(answer["children"], Value::Null);
    assert_eq!(answer["note"]["id"], parent.id);
    assert_eq!(answer["note"]["projectId"], f.target.id);
    assert_eq!(
        answer["addedRepos"],
        json!([{ "githubRepo": "acme/widgets", "subpath": "apps/web" }])
    );
}

/// A failure is given the repos the transfer had added by then, in the
/// sentence a project-note failure ends with. With none added, or for a
/// destination that is not there, the error is left as it came.
#[tokio::test]
async fn a_failure_names_the_repos_added_before_it() {
    let f = fixture();
    let added = AddedRepos::default();
    assert_eq!(
        added.explain(&f.store, &f.target.id, "It failed".to_string()),
        "It failed"
    );

    let key = RepoKey {
        github_repo: "acme/widgets".into(),
        subpath: Some("apps/web".into()),
    };
    added
        .watch(key.clone(), std::future::ready(Ok(())))
        .await
        .unwrap();
    assert_eq!(
        added.explain(&f.store, &f.target.id, "It failed".to_string()),
        "It failed. acme/widgets at apps/web was added to target along the way and remains \
         tracked there."
    );
    assert_eq!(
        added.explain(&f.store, "no-such-project", "It failed".to_string()),
        "It failed"
    );
    // Explaining a failure does not use the list up.
    assert_eq!(
        added.into_json(),
        json!([{ "githubRepo": "acme/widgets", "subpath": "apps/web" }])
    );
}

/// The transfer has committed before the answer is worded, so a landing row
/// that cannot be read back is left out of the answer. It is not an error.
#[test]
fn a_landing_branch_that_cannot_be_read_back_is_answered_as_null() {
    let f = fixture();
    let answer = branch_note_answer(
        &f.store,
        json!({ "id": "n1" }),
        "no-such-branch",
        AddedRepos::default(),
    );
    assert_eq!(
        answer,
        json!({ "note": { "id": "n1" }, "branch": null, "repo": null, "addedRepos": [] })
    );
}
