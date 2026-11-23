//! Tests for note versions: which writes keep one, what it holds, how many
//! are kept, and when they go.

use rusqlite::params;

use super::models::*;
use super::note_versions::MAX_NOTE_VERSIONS;
use super::Store;

struct Fixture {
    store: Store,
    project: Project,
    branch: Branch,
}

fn fixture() -> Fixture {
    let store = Store::in_memory().unwrap();
    let project = Project::named("source");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    Fixture {
        store,
        project,
        branch,
    }
}

/// A note as the session runner first sees it: an empty stub tied to the
/// session that will fill it.
fn stub_note(f: &Fixture) -> Note {
    let note = Note::new(&f.branch.id, "", "").with_session("session-1");
    f.store.create_note(&note).unwrap();
    note
}

fn stub_project_note(f: &Fixture) -> ProjectNote {
    let note = ProjectNote::new(&f.project.id, "", "").with_session("session-1");
    f.store.create_project_note(&note).unwrap();
    note
}

/// The session runner's write, with no suggested steps.
fn revise(store: &Store, note_id: &str, title: &str, content: &str) {
    store
        .update_note_title_and_content(note_id, title, content, None, None)
        .unwrap();
}

fn revise_project_note(store: &Store, note_id: &str, title: &str, content: &str) {
    store
        .update_project_note_title_and_content(note_id, title, content, None, None)
        .unwrap();
}

/// Every stored version of `note_id`, oldest first.
fn versions(store: &Store, note_id: &str) -> Vec<NoteVersion> {
    let conn = store.conn.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT id, note_id, title, content, saved_at
             FROM note_versions WHERE note_id = ?1 ORDER BY saved_at ASC",
        )
        .unwrap();
    let rows = stmt
        .query_map(params![note_id], |row| {
            Ok(NoteVersion {
                id: row.get(0)?,
                note_id: row.get(1)?,
                title: row.get(2)?,
                content: row.get(3)?,
                saved_at: row.get(4)?,
            })
        })
        .unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

fn version_count(store: &Store) -> i64 {
    let conn = store.conn.lock().unwrap();
    conn.query_row("SELECT COUNT(*) FROM note_versions", [], |row| row.get(0))
        .unwrap()
}

/// Pin a note's `updated_at`, so a test can tell it from the time of the
/// write that follows.
fn set_updated_at(store: &Store, table: &str, note_id: &str, updated_at: i64) {
    let conn = store.conn.lock().unwrap();
    let changed = conn
        .execute(
            &format!("UPDATE {table} SET updated_at = ?1 WHERE id = ?2"),
            params![updated_at, note_id],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

#[test]
fn first_fill_of_a_stub_keeps_no_version() {
    let f = fixture();
    let note = stub_note(&f);

    revise(&f.store, &note.id, "Plan", "body");

    assert_eq!(version_count(&f.store), 0);
    assert_eq!(f.store.get_previous_note_version(&note.id).unwrap(), None);
}

#[test]
fn a_stub_given_a_title_before_its_body_keeps_no_version() {
    let f = fixture();
    let note = stub_note(&f);

    // The body is what makes a version: a title over an empty body is still
    // a note nobody has read.
    revise(&f.store, &note.id, "Plan", "");
    revise(&f.store, &note.id, "Plan", "body");

    assert_eq!(version_count(&f.store), 0);
}

#[test]
fn a_changed_body_keeps_the_text_it_replaced() {
    let f = fixture();
    let note = stub_note(&f);
    revise(&f.store, &note.id, "Plan", "first body");
    set_updated_at(&f.store, "notes", &note.id, 1_000);

    revise(&f.store, &note.id, "Plan, revised", "second body");

    let previous = f
        .store
        .get_previous_note_version(&note.id)
        .unwrap()
        .expect("the revision keeps a version");
    assert_eq!(previous.note_id, note.id);
    assert_eq!(previous.title, "Plan");
    assert_eq!(previous.content, "first body");
    // When the replaced text was written, not when it was replaced.
    assert_eq!(previous.saved_at, 1_000);
    assert_eq!(versions(&f.store, &note.id), vec![previous]);

    // The note itself holds the new text only.
    let stored = f.store.get_note(&note.id).unwrap().unwrap();
    assert_eq!(stored.title, "Plan, revised");
    assert_eq!(stored.content, "second body");
    assert!(stored.updated_at > 1_000);
}

#[test]
fn a_changed_title_alone_keeps_a_version() {
    let f = fixture();
    let note = stub_note(&f);
    revise(&f.store, &note.id, "Plan", "body");

    revise(&f.store, &note.id, "The plan", "body");

    let previous = f
        .store
        .get_previous_note_version(&note.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (previous.title.as_str(), previous.content.as_str()),
        ("Plan", "body")
    );
}

#[test]
fn an_identical_republish_and_a_steps_only_change_keep_no_version() {
    let f = fixture();
    let note = stub_note(&f);
    revise(&f.store, &note.id, "Plan", "body");

    revise(&f.store, &note.id, "Plan", "body");
    f.store
        .update_note_title_and_content(&note.id, "Plan", "body", Some("Commit it"), None)
        .unwrap();
    f.store
        .update_note_title_and_content(&note.id, "Plan", "body", Some("Commit it"), Some("Next"))
        .unwrap();

    assert_eq!(version_count(&f.store), 0);
    // The steps-only writes did land.
    let stored = f.store.get_note(&note.id).unwrap().unwrap();
    assert_eq!(
        stored.suggested_next_commit_step.as_deref(),
        Some("Commit it")
    );
    assert_eq!(stored.suggested_next_note_step.as_deref(), Some("Next"));
}

#[test]
fn a_write_to_a_missing_note_keeps_no_version() {
    let f = fixture();

    revise(&f.store, "missing", "Plan", "body");
    revise_project_note(&f.store, "missing", "Plan", "body");

    assert_eq!(version_count(&f.store), 0);
    assert_eq!(f.store.get_previous_note_version("missing").unwrap(), None);
}

#[test]
fn update_written_note_keeps_the_text_it_replaced() {
    let f = fixture();
    let mut note =
        Note::new(&f.branch.id, "Draft", "first body").with_subtype(Note::SUBTYPE_WRITTEN);
    f.store.create_note_with_unique_title(&mut note).unwrap();
    set_updated_at(&f.store, "notes", &note.id, 2_000);

    f.store
        .update_written_note(&note.id, "Final", "second body")
        .unwrap();

    let all = versions(&f.store, &note.id);
    assert_eq!(all.len(), 1);
    assert_eq!(
        (
            all[0].title.as_str(),
            all[0].content.as_str(),
            all[0].saved_at
        ),
        ("Draft", "first body", 2_000)
    );

    // A save that changes nothing advances `updated_at` but is no revision.
    f.store
        .update_written_note(&note.id, "Final", "second body")
        .unwrap();
    assert_eq!(versions(&f.store, &note.id), all);
}

#[test]
fn update_written_note_judges_the_title_it_stores() {
    let f = fixture();
    let mut other =
        Note::new(&f.branch.id, "Notes", "other body").with_subtype(Note::SUBTYPE_WRITTEN);
    f.store.create_note_with_unique_title(&mut other).unwrap();
    let mut note = Note::new(&f.branch.id, "Notes", "body").with_subtype(Note::SUBTYPE_WRITTEN);
    f.store.create_note_with_unique_title(&mut note).unwrap();
    assert_eq!(note.title, "Notes (2)");

    // Saved under a title another note holds, it is stored suffixed, and
    // the suffixed title is the one it already had: nothing changed.
    let saved = f
        .store
        .update_written_note(&note.id, "Notes", "body")
        .unwrap();
    assert_eq!(saved.title, "Notes (2)");
    assert_eq!(version_count(&f.store), 0);

    // A title that is stored as given is a change, and keeps the old one.
    let saved = f
        .store
        .update_written_note(&note.id, "Field notes", "body")
        .unwrap();
    assert_eq!(saved.title, "Field notes");
    let all = versions(&f.store, &note.id);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].title, "Notes (2)");
}

#[test]
fn a_refused_written_note_save_keeps_no_version() {
    let f = fixture();
    let agent = Note::new(&f.branch.id, "Agent note", "body").with_session("session-1");
    f.store.create_note(&agent).unwrap();

    assert!(f
        .store
        .update_written_note(&agent.id, "Hijacked", "mine now")
        .is_err());

    assert_eq!(version_count(&f.store), 0);
    assert_eq!(
        f.store.get_note(&agent.id).unwrap().unwrap().content,
        "body"
    );
}

#[test]
fn project_notes_keep_versions_the_same_way() {
    let f = fixture();
    let note = stub_project_note(&f);

    // First fill, identical republish, steps only: nothing kept.
    revise_project_note(&f.store, &note.id, "Research", "first body");
    revise_project_note(&f.store, &note.id, "Research", "first body");
    f.store
        .update_project_note_title_and_content(
            &note.id,
            "Research",
            "first body",
            Some("Commit it"),
            None,
        )
        .unwrap();
    assert_eq!(version_count(&f.store), 0);
    set_updated_at(&f.store, "project_notes", &note.id, 3_000);

    revise_project_note(&f.store, &note.id, "Research", "second body");

    let previous = f
        .store
        .get_previous_note_version(&note.id)
        .unwrap()
        .expect("the revision keeps a version");
    assert_eq!(previous.note_id, note.id);
    assert_eq!(previous.title, "Research");
    assert_eq!(previous.content, "first body");
    assert_eq!(previous.saved_at, 3_000);
    assert_eq!(versions(&f.store, &note.id).len(), 1);
}

#[test]
fn the_previous_version_is_the_latest_one_kept() {
    let f = fixture();
    let note = stub_note(&f);
    for body in ["one", "two", "three", "four"] {
        revise(&f.store, &note.id, "Plan", body);
    }

    let bodies: Vec<String> = versions(&f.store, &note.id)
        .into_iter()
        .map(|v| v.content)
        .collect();
    assert_eq!(bodies, ["one", "two", "three"]);
    assert_eq!(
        f.store
            .get_previous_note_version(&note.id)
            .unwrap()
            .unwrap()
            .content,
        "three"
    );
}

#[test]
fn versions_stay_in_order_when_the_clock_does_not_advance() {
    let f = fixture();
    let note = stub_note(&f);
    revise(&f.store, &note.id, "Plan", "one");

    // Each outgoing row claims the same `updated_at`, as revisions inside
    // one millisecond do, and then an earlier one, as after a clock step.
    for (body, updated_at) in [("two", 5_000), ("three", 5_000), ("four", 4_000)] {
        set_updated_at(&f.store, "notes", &note.id, updated_at);
        revise(&f.store, &note.id, "Plan", body);
    }

    let all = versions(&f.store, &note.id);
    let bodies: Vec<&str> = all.iter().map(|v| v.content.as_str()).collect();
    assert_eq!(bodies, ["one", "two", "three"]);
    let saved: Vec<i64> = all.iter().map(|v| v.saved_at).collect();
    assert_eq!(saved, [5_000, 5_001, 5_002]);
    assert_eq!(
        f.store
            .get_previous_note_version(&note.id)
            .unwrap()
            .unwrap()
            .content,
        "three"
    );
}

#[test]
fn the_eleventh_revision_prunes_the_oldest_version() {
    let f = fixture();
    let note = stub_note(&f);
    let bystander = stub_note(&f);
    revise(&f.store, &bystander.id, "Other", "bystander one");
    revise(&f.store, &bystander.id, "Other", "bystander two");

    revise(&f.store, &note.id, "Plan", "body 0");
    for n in 1..=MAX_NOTE_VERSIONS {
        revise(&f.store, &note.id, "Plan", &format!("body {n}"));
    }
    // Ten revisions keep all ten replaced bodies.
    let bodies: Vec<String> = versions(&f.store, &note.id)
        .into_iter()
        .map(|v| v.content)
        .collect();
    let expected: Vec<String> = (0..MAX_NOTE_VERSIONS)
        .map(|n| format!("body {n}"))
        .collect();
    assert_eq!(bodies, expected);

    // The eleventh drops the oldest and keeps the one it replaced.
    revise(&f.store, &note.id, "Plan", "body 11");
    let bodies: Vec<String> = versions(&f.store, &note.id)
        .into_iter()
        .map(|v| v.content)
        .collect();
    let expected: Vec<String> = (1..=MAX_NOTE_VERSIONS)
        .map(|n| format!("body {n}"))
        .collect();
    assert_eq!(bodies, expected);
    assert_eq!(
        f.store
            .get_previous_note_version(&note.id)
            .unwrap()
            .unwrap()
            .content,
        format!("body {MAX_NOTE_VERSIONS}")
    );

    // Pruning is per note.
    let bystander_bodies: Vec<String> = versions(&f.store, &bystander.id)
        .into_iter()
        .map(|v| v.content)
        .collect();
    assert_eq!(bystander_bodies, ["bystander one"]);
}

#[test]
fn deleting_a_note_removes_its_versions_only() {
    let f = fixture();
    let note = stub_note(&f);
    let kept = stub_note(&f);
    for id in [&note.id, &kept.id] {
        revise(&f.store, id, "Plan", "first body");
        revise(&f.store, id, "Plan", "second body");
    }
    assert_eq!(version_count(&f.store), 2);

    f.store.delete_note(&note.id).unwrap();

    assert!(versions(&f.store, &note.id).is_empty());
    assert_eq!(f.store.get_previous_note_version(&note.id).unwrap(), None);
    assert_eq!(versions(&f.store, &kept.id).len(), 1);
}

#[test]
fn deleting_a_project_note_removes_its_versions_and_its_childrens() {
    let f = fixture();
    let parent = stub_project_note(&f);
    revise_project_note(&f.store, &parent.id, "Research", "first body");
    revise_project_note(&f.store, &parent.id, "Research", "second body");
    let child = Note::new(&f.branch.id, "Child", "child first")
        .with_session("session-child")
        .with_parent_project_note(&parent.id);
    f.store.create_note(&child).unwrap();
    revise(&f.store, &child.id, "Child", "child second");
    let outsider = stub_note(&f);
    revise(&f.store, &outsider.id, "Plan", "first body");
    revise(&f.store, &outsider.id, "Plan", "second body");
    assert_eq!(version_count(&f.store), 3);

    f.store.delete_project_note(&parent.id).unwrap();

    assert!(versions(&f.store, &parent.id).is_empty());
    assert!(versions(&f.store, &child.id).is_empty());
    assert_eq!(versions(&f.store, &outsider.id).len(), 1);
}

#[test]
fn deleting_a_branch_removes_its_notes_versions() {
    let f = fixture();
    let note = stub_note(&f);
    revise(&f.store, &note.id, "Plan", "first body");
    revise(&f.store, &note.id, "Plan", "second body");
    let project_note = stub_project_note(&f);
    revise_project_note(&f.store, &project_note.id, "Research", "first body");
    revise_project_note(&f.store, &project_note.id, "Research", "second body");

    // The note row goes in the branch's cascade, which fires the trigger.
    f.store.delete_branch(&f.branch.id).unwrap();

    assert!(f.store.get_note(&note.id).unwrap().is_none());
    assert!(versions(&f.store, &note.id).is_empty());
    // The project note is not on the branch.
    assert_eq!(versions(&f.store, &project_note.id).len(), 1);
}

#[test]
fn deleting_a_project_removes_every_version_under_it() {
    let f = fixture();
    let note = stub_note(&f);
    revise(&f.store, &note.id, "Plan", "first body");
    revise(&f.store, &note.id, "Plan", "second body");
    let project_note = stub_project_note(&f);
    revise_project_note(&f.store, &project_note.id, "Research", "first body");
    revise_project_note(&f.store, &project_note.id, "Research", "second body");

    f.store.delete_project(&f.project.id).unwrap();

    assert_eq!(version_count(&f.store), 0);
}

#[test]
fn a_moved_note_keeps_its_versions() {
    let f = fixture();
    let target = Project::named("target");
    f.store.create_project(&target).unwrap();
    let target_branch = Branch::new(&target.id, "other", "main");
    f.store.create_branch(&target_branch).unwrap();
    let note = stub_note(&f);
    revise(&f.store, &note.id, "Plan", "first body");
    revise(&f.store, &note.id, "Plan", "second body");
    let before = versions(&f.store, &note.id);
    assert_eq!(before.len(), 1);

    let moved = f
        .store
        .move_note_to_branch(
            &note.id,
            &f.branch.id,
            &target_branch.id,
            "second body",
            &[],
            &[],
        )
        .unwrap();

    // Same id, new branch, and the move itself is not a revision.
    assert_eq!(moved.id, note.id);
    assert_eq!(moved.branch_id, target_branch.id);
    assert_eq!(versions(&f.store, &note.id), before);
    assert_eq!(
        f.store
            .get_previous_note_version(&note.id)
            .unwrap()
            .unwrap()
            .content,
        "first body"
    );
}

#[test]
fn a_moved_project_note_keeps_its_versions() {
    let f = fixture();
    let target = Project::named("target");
    f.store.create_project(&target).unwrap();
    let note = stub_project_note(&f);
    revise_project_note(&f.store, &note.id, "Research", "first body");
    revise_project_note(&f.store, &note.id, "Research", "second body");
    let before = versions(&f.store, &note.id);
    assert_eq!(before.len(), 1);

    let moved = f
        .store
        .move_project_note_to_project(
            &note.id,
            &f.project.id,
            "second body",
            &target.id,
            "second body",
            &[],
            &[],
            &[],
        )
        .unwrap();

    assert_eq!(moved.id, note.id);
    assert_eq!(moved.project_id, target.id);
    assert_eq!(versions(&f.store, &note.id), before);
}
