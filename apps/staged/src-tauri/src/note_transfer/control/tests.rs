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

    /// A branch-note transfer as the dispatch arm runs it: the repo add
    /// watched, then the control answer.
    async fn transfer(&self, note: &Note, mode: TransferMode) -> Result<Value, String> {
        let added = AddedRepos::default();
        let landed = transfer_branch_note(&self.store, &note.id, &self.target.id, mode, |key| {
            added.watch(key.clone(), std::future::ready(self.attach(key)))
        })
        .await?;
        let item = json!({ "id": landed.id, "title": landed.title });
        Ok(branch_note_answer(
            &self.store,
            item,
            &landed.branch_id,
            added,
        ))
    }
}

fn widgets_key() -> Value {
    json!({ "githubRepo": "acme/widgets", "subpath": "apps/web" })
}

#[tokio::test]
async fn a_move_that_adds_the_repo_names_it_and_where_the_note_landed() {
    let f = fixture();
    let note = f.note_on(&f.widgets, "Findings");

    let answer = f.transfer(&note, TransferMode::Move).await.unwrap();

    assert_eq!(answer["addedRepos"], json!([widgets_key()]));
    assert_eq!(answer["note"]["id"], note.id, "a move keeps the note's id");
    assert_eq!(answer["branch"]["projectId"], f.target.id);
    assert_eq!(answer["branch"]["branchName"], "target-acme-widgets");
    assert_eq!(answer["repo"]["githubRepo"], "acme/widgets");
    assert_eq!(answer["repo"]["subpath"], "apps/web");
    assert_eq!(
        answer["branch"]["projectRepoId"], answer["repo"]["id"],
        "the repo is the one the landing branch tracks"
    );
    assert_eq!(
        answer["branch"].get("worktreePath"),
        Some(&Value::Null),
        "the branch reads as the branch list gives one, worktree included"
    );
}

#[tokio::test]
async fn a_transfer_into_a_project_that_tracks_the_repo_adds_nothing() {
    let f = fixture();
    let note = f.note_on(&f.widgets, "Findings");
    f.attach(RepoKey {
        github_repo: "acme/widgets".into(),
        subpath: Some("apps/web".into()),
    })
    .unwrap();

    for mode in [TransferMode::Copy, TransferMode::Move] {
        let answer = f.transfer(&note, mode).await.unwrap();
        assert_eq!(answer["addedRepos"], json!([]), "{mode:?}");
        assert_eq!(answer["branch"]["projectId"], f.target.id, "{mode:?}");
        assert_eq!(answer["repo"]["githubRepo"], "acme/widgets", "{mode:?}");
    }
}

#[tokio::test]
async fn a_copy_answers_with_the_new_note_and_leaves_the_original() {
    let f = fixture();
    let note = f.note_on(&f.widgets, "Findings");

    let answer = f.transfer(&note, TransferMode::Copy).await.unwrap();

    let copy_id = answer["note"]["id"].as_str().unwrap();
    assert_ne!(copy_id, note.id, "a copy is a new note");
    assert_eq!(answer["addedRepos"], json!([widgets_key()]));
    assert_eq!(
        f.store.get_note(&note.id).unwrap().unwrap().branch_id,
        f.widgets.id
    );
    assert_eq!(
        f.store.get_note(copy_id).unwrap().unwrap().branch_id,
        answer["branch"]["id"].as_str().unwrap()
    );
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
