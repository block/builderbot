use serde_json::json;

use super::*;
use crate::note_transfer::{transfer_branch_note, transfer_project_note, TransferMode};
use crate::store::models::{Branch, Note, Project, ProjectNote, ProjectRepo};

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
/// transfer added them, and not a repo the destination already had.
#[tokio::test]
async fn a_project_note_move_lists_the_repos_its_children_brought() {
    let f = fixture();
    let parent = ProjectNote::new(&f.source.id, "Plan", "body");
    f.store.create_project_note(&parent).unwrap();
    for (branch, title) in [(&f.widgets, "Web"), (&f.gadgets, "Gadgets")] {
        let child =
            Note::new_standalone(&branch.id, title, "body").with_parent_project_note(&parent.id);
        f.store.create_note(&child).unwrap();
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
    let answer = project_note_answer(serde_json::to_value(&moved).unwrap(), added);

    assert_eq!(answer["note"]["id"], parent.id);
    assert_eq!(answer["note"]["projectId"], f.target.id);
    assert_eq!(
        answer["addedRepos"],
        json!([{ "githubRepo": "acme/gadgets", "subpath": null }])
    );
}

#[tokio::test]
async fn a_project_note_copy_adds_no_repo() {
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
    let answer = project_note_answer(serde_json::to_value(&copy).unwrap(), added);

    assert_ne!(answer["note"]["id"], parent.id);
    assert_eq!(answer["note"]["projectId"], f.target.id);
    assert_eq!(answer["addedRepos"], json!([]));
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
