//! Per-project last activity: when work last started, changed or finished in
//! each project.
//!
//! Derived on every read from the timestamps the notes, reviews and commits
//! already carry, so nothing is stored and nothing can drift. The frontend
//! sorts projects by it ("Last updated") and floors it at the project's own
//! creation time, which it already has.
//!
//! The read scans the four tables once, a cost of the same order as the
//! search index read, and the frontend asks for it only while a sort uses it.

use serde::Serialize;

use super::{Store, StoreError};

/// When a project last saw activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectActivity {
    pub project_id: String,
    /// The latest timestamp across the project's notes, project notes,
    /// reviews and commits, in milliseconds since the epoch.
    pub last_activity_at: i64,
}

/// One row per project that has at least one item, with the latest timestamp
/// across them.
///
/// Each item counts from the moment its row is written, which for a session
/// is when it starts: Staged creates the note, review or pending commit then.
/// A note or review counts again when it completes. `completed_at` is read
/// beside `updated_at` because completing a note stamps only `completed_at`.
/// A commit has no `completed_at`; its `updated_at` moves when its SHA lands.
///
/// Branch items reach a project through `branches.project_id`, the branch's
/// current project, so they follow a branch that moves.
///
/// Deliberately not read: `projects.updated_at` and tag assignments, which
/// are housekeeping, and `read_at`, so opening a note reorders nothing.
const ACTIVITY_QUERY: &str = "
    SELECT project_id, MAX(at) FROM (
        SELECT project_id, MAX(updated_at, IFNULL(completed_at, 0)) AS at
          FROM project_notes
        UNION ALL
        SELECT b.project_id, MAX(n.updated_at, IFNULL(n.completed_at, 0))
          FROM notes n JOIN branches b ON b.id = n.branch_id
        UNION ALL
        SELECT b.project_id, MAX(r.updated_at, IFNULL(r.completed_at, 0))
          FROM reviews r JOIN branches b ON b.id = r.branch_id
        UNION ALL
        SELECT b.project_id, c.updated_at
          FROM commits c JOIN branches b ON b.id = c.branch_id
    ) GROUP BY project_id";

impl Store {
    /// When each project last saw activity, in no particular order. A project
    /// with no note, project note, review or commit has no row. Read in one
    /// statement, so the four sources agree.
    pub fn list_project_activity(&self) -> Result<Vec<ProjectActivity>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(ACTIVITY_QUERY)?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectActivity {
                project_id: row.get(0)?,
                last_activity_at: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::store::models::{Branch, Commit, Note, Project, ProjectNote, ProjectRepo, Review};
    use crate::store::{BranchMove, RepoPlacement, ReviewScope};

    struct Fixture {
        store: Store,
        project: Project,
        other: Project,
        repo: ProjectRepo,
        branch: Branch,
    }

    /// A project with one branch on its own repo row, plus an empty second
    /// project to move things into. In memory, like every store test.
    fn fixture() -> Fixture {
        let store = Store::in_memory().unwrap();
        let project = Project::named("checkout").with_primary_repo("acme/widgets");
        let other = Project::named("billing");
        store.create_project(&project).unwrap();
        store.create_project(&other).unwrap();
        let repo = ProjectRepo::new(&project.id, "acme/widgets", "feature", None).primary();
        store.create_project_repo(&repo).unwrap();
        let branch = Branch::new(&project.id, "feature", "origin/main").with_project_repo(&repo.id);
        store.create_branch(&branch).unwrap();
        Fixture {
            store,
            project,
            other,
            repo,
            branch,
        }
    }

    // The rows below carry small fixed timestamps, far below the wall clock.
    // A write that stamps "now" on a column the query reads therefore shows
    // up as a changed value.

    fn project_note(f: &Fixture, updated_at: i64, completed_at: Option<i64>) -> ProjectNote {
        let mut note = ProjectNote::new(&f.project.id, "Plan", "body");
        note.updated_at = updated_at;
        note.completed_at = completed_at;
        f.store.create_project_note(&note).unwrap();
        note
    }

    fn note(f: &Fixture, updated_at: i64, completed_at: Option<i64>) -> Note {
        let mut note = Note::new(&f.branch.id, "Research", "body");
        note.updated_at = updated_at;
        note.completed_at = completed_at;
        f.store.create_note(&note).unwrap();
        note
    }

    fn review(f: &Fixture, updated_at: i64, completed_at: Option<i64>) -> Review {
        let mut review = Review::new(&f.branch.id, "abc123", ReviewScope::Commit);
        review.updated_at = updated_at;
        review.completed_at = completed_at;
        f.store.create_review(&review).unwrap();
        review
    }

    fn commit(f: &Fixture, updated_at: i64) -> Commit {
        let mut commit = Commit::new_pending(&f.branch.id);
        commit.updated_at = updated_at;
        f.store.create_commit(&commit).unwrap();
        commit
    }

    fn activity(store: &Store) -> BTreeMap<String, i64> {
        let rows = store.list_project_activity().unwrap();
        let count = rows.len();
        let map: BTreeMap<String, i64> = rows
            .into_iter()
            .map(|row| (row.project_id, row.last_activity_at))
            .collect();
        assert_eq!(map.len(), count, "one row per project");
        map
    }

    /// The whole result, when only the fixture's first project has activity.
    fn only(f: &Fixture, at: i64) -> BTreeMap<String, i64> {
        BTreeMap::from([(f.project.id.clone(), at)])
    }

    #[test]
    fn takes_the_latest_timestamp_across_the_four_sources() {
        // Each source in turn holds the latest timestamp.
        for latest in 0..4 {
            let f = fixture();
            let mut at = [100, 200, 300, 400];
            at[latest] = 900;
            project_note(&f, at[0], None);
            note(&f, at[1], None);
            review(&f, at[2], None);
            commit(&f, at[3]);

            assert_eq!(activity(&f.store), only(&f, 900), "source {latest}");
        }
    }

    #[test]
    fn counts_a_completion_later_than_the_last_update() {
        // Completing a note or review stamps `completed_at` and leaves
        // `updated_at` alone.
        let f = fixture();
        project_note(&f, 100, Some(500));
        assert_eq!(activity(&f.store), only(&f, 500));

        let f = fixture();
        note(&f, 100, Some(600));
        assert_eq!(activity(&f.store), only(&f, 600));

        let f = fixture();
        review(&f, 100, Some(700));
        assert_eq!(activity(&f.store), only(&f, 700));
    }

    #[test]
    fn counts_an_update_later_than_the_completion() {
        // A comment on a completed review, or an edit to a finished note.
        let f = fixture();
        project_note(&f, 500, Some(100));
        note(&f, 400, Some(100));
        review(&f, 300, Some(100));
        assert_eq!(activity(&f.store), only(&f, 500));
    }

    #[test]
    fn counts_a_session_that_has_started_and_not_finished() {
        // The stub row is written when the session starts, uncompleted.
        let f = fixture();
        let mut stub = Note::new(&f.branch.id, "Prompt", "");
        assert_eq!(stub.completed_at, None);
        stub.updated_at = 250;
        f.store.create_note(&stub).unwrap();
        assert_eq!(activity(&f.store), only(&f, 250));

        // Finishing it counts again, at the wall clock.
        f.store.mark_note_completed(&stub.id).unwrap();
        assert!(activity(&f.store)[&f.project.id] > 250);
    }

    #[test]
    fn a_commit_counts_again_when_its_sha_lands() {
        let f = fixture();
        let pending = commit(&f, 100);
        assert_eq!(activity(&f.store), only(&f, 100));

        f.store.update_commit_sha(&pending.id, "abc123").unwrap();
        assert!(activity(&f.store)[&f.project.id] > 100);
    }

    #[test]
    fn a_project_with_no_activity_has_no_row() {
        // `other` has nothing at all; `project` has a branch but no items.
        let f = fixture();
        assert_eq!(activity(&f.store), BTreeMap::new());

        let mut elsewhere = ProjectNote::new(&f.other.id, "Invoices", "body");
        elsewhere.updated_at = 100;
        elsewhere.completed_at = Some(100);
        f.store.create_project_note(&elsewhere).unwrap();
        assert_eq!(
            activity(&f.store),
            BTreeMap::from([(f.other.id.clone(), 100)])
        );
    }

    #[test]
    fn a_read_mark_changes_nothing() {
        let f = fixture();
        let n = note(&f, 100, Some(200));
        let pn = project_note(&f, 100, Some(300));
        let before = activity(&f.store);
        assert_eq!(before, only(&f, 300));

        // Both writes land: the notes are completed and unread.
        let read = f.store.set_note_read(&n.id, true).unwrap();
        assert!(read.read_at.is_some());
        let read = f.store.set_project_note_read(&pn.id, true).unwrap();
        assert!(read.read_at.is_some());
        assert_eq!(activity(&f.store), before);

        f.store.set_note_read(&n.id, false).unwrap();
        f.store.set_project_note_read(&pn.id, false).unwrap();
        assert_eq!(activity(&f.store), before);
    }

    #[test]
    fn a_rename_changes_nothing() {
        // `projects.updated_at` moves on a rename and is not read.
        let f = fixture();
        project_note(&f, 100, Some(100));
        f.store
            .rename_project(&f.project.id, "checkout v2")
            .unwrap();
        assert_eq!(activity(&f.store), only(&f, 100));
    }

    #[test]
    fn branch_items_follow_a_moved_branch() {
        let f = fixture();
        project_note(&f, 100, Some(100));
        note(&f, 200, Some(200));
        review(&f, 300, Some(300));
        commit(&f, 400);
        assert_eq!(activity(&f.store), only(&f, 400));

        f.store
            .move_branch_to_project(&BranchMove {
                branch_id: f.branch.id.clone(),
                source_project_id: f.project.id.clone(),
                target_project_id: f.other.id.clone(),
                repo: RepoPlacement::Reparent {
                    repo_id: f.repo.id.clone(),
                },
                workdir: None,
            })
            .unwrap();

        // The note, review and commit went with the branch. The project note
        // belongs to the project, not the branch: it stays.
        assert_eq!(
            activity(&f.store),
            BTreeMap::from([(f.project.id.clone(), 100), (f.other.id.clone(), 400)])
        );
    }

    #[test]
    fn deleting_the_latest_item_lowers_the_value() {
        let f = fixture();
        let pn = project_note(&f, 100, Some(100));
        let older = note(&f, 200, Some(200));
        let newest = note(&f, 300, Some(300));
        assert_eq!(activity(&f.store), only(&f, 300));

        f.store.delete_note(&newest.id).unwrap();
        assert_eq!(activity(&f.store), only(&f, 200));

        f.store.delete_note(&older.id).unwrap();
        assert_eq!(activity(&f.store), only(&f, 100));

        // With its last item gone the project drops out.
        f.store.delete_project_note(&pn.id).unwrap();
        assert_eq!(activity(&f.store), BTreeMap::new());
    }

    #[test]
    fn serializes_camel_case_for_the_frontend() {
        let value = serde_json::to_value(ProjectActivity {
            project_id: "p1".into(),
            last_activity_at: 7,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "projectId": "p1", "lastActivityAt": 7 })
        );
    }
}
