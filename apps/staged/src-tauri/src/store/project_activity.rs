//! Per-project last activity: when work last landed in each project.
//!
//! Derived on every read from the timestamps the notes, reviews, comments
//! and commits already carry, so nothing is stored and nothing can drift.
//! The frontend sorts projects by it ("Last updated") and floors it at the
//! project's own creation time, which it already has.
//!
//! A session starting is not activity. Staged writes the session's note,
//! review or commit row when the session is queued or started, and that row
//! is left out until the session has put something in it. See
//! [`ACTIVITY_QUERY`] for how such a row is told apart.
//!
//! The read scans the four tables once, and the comments of the reviews
//! still in flight. That is a cost of the same order as the search index
//! read, and the frontend asks for it only while a sort uses it.

use serde::Serialize;

use super::{Store, StoreError};

/// When a project last saw activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectActivity {
    pub project_id: String,
    /// The latest timestamp across the project's notes, project notes,
    /// reviews and commits that count (see [`ACTIVITY_QUERY`]), in
    /// milliseconds since the epoch.
    pub last_activity_at: i64,
}

/// One row per project that has at least one item that counts, with the
/// latest timestamp across them.
///
/// A row that a session only started does not count. Every such row is
/// written, and may be stamped again, before the session has produced
/// anything: when it is queued, when it is claimed
/// (`mark_session_artifact_started`), and for a review when the branch tip is
/// filled in (`update_review_commit_sha`). These columns tell it apart:
///
/// - **Notes and project notes:** `session_id` set and `completed_at` unset.
///   Every write that puts a title or body in a note, and the session
///   finishing without one, stamps `completed_at`.
/// - **Reviews:** the same test. The session finishing stamps `completed_at`.
/// - **Commits:** `sha` unset. A pending commit has no SHA until the git
///   commit lands.
///
/// Everything else counts as it did:
///
/// - A note or review a session has finished counts from `completed_at` and
///   from `updated_at`, whichever is later. `completed_at` is read because
///   completing a note stamps only that. `updated_at` moves on a later
///   revision, edit, comment or file marked reviewed.
/// - A note or review with no session counts from `updated_at`. No session
///   start wrote it: it is a note the user saved, or the review the diff
///   viewer creates on the first comment or file marked reviewed.
/// - A commit with a SHA counts from `updated_at`, which moves when the SHA
///   lands and when a rebase remaps it.
///
/// A review whose session is still running is left out as a row, because its
/// `updated_at` is stamped by the start and cannot say what else moved it.
/// Its comments carry their own timestamps, so a comment added to or deleted
/// from it still counts, from the comment's `created_at` and `deleted_at`.
/// The writes that stamp only the review's `updated_at` (a file marked or
/// unmarked reviewed, a reference file, a comment edited or restored) wait
/// there for the session to finish.
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
         WHERE session_id IS NULL OR completed_at IS NOT NULL
        UNION ALL
        SELECT b.project_id, MAX(n.updated_at, IFNULL(n.completed_at, 0))
          FROM notes n JOIN branches b ON b.id = n.branch_id
         WHERE n.session_id IS NULL OR n.completed_at IS NOT NULL
        UNION ALL
        SELECT b.project_id, MAX(r.updated_at, IFNULL(r.completed_at, 0))
          FROM reviews r JOIN branches b ON b.id = r.branch_id
         WHERE r.session_id IS NULL OR r.completed_at IS NOT NULL
        UNION ALL
        SELECT b.project_id, MAX(c.created_at, IFNULL(c.deleted_at, 0))
          FROM comments c
          JOIN reviews r ON r.id = c.review_id
          JOIN branches b ON b.id = r.branch_id
         WHERE r.session_id IS NOT NULL AND r.completed_at IS NULL
        UNION ALL
        SELECT b.project_id, c.updated_at
          FROM commits c JOIN branches b ON b.id = c.branch_id
         WHERE c.sha IS NOT NULL
    ) GROUP BY project_id";

impl Store {
    /// When each project last saw activity, in no particular order. A project
    /// with no note, project note, review or commit that counts has no row,
    /// which includes one whose only items are sessions still running. Read
    /// in one statement, so the sources agree.
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
    use crate::git::Span;
    use crate::store::models::{
        Branch, Comment, Commit, Note, Project, ProjectNote, ProjectRepo, Review,
    };
    use crate::store::{now_timestamp, BranchMove, RepoPlacement, ReviewScope};

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

    /// A commit whose SHA has landed.
    fn commit(f: &Fixture, updated_at: i64) -> Commit {
        let mut commit = Commit::new_with_sha(&f.branch.id, "abc123");
        commit.updated_at = updated_at;
        f.store.create_commit(&commit).unwrap();
        commit
    }

    // The rows a session start writes, built as `session_commands` builds
    // them: linked to the session, with nothing in them yet.

    const SESSION: &str = "session-1";

    fn note_stub(f: &Fixture, updated_at: i64) -> Note {
        let mut stub = Note::new(&f.branch.id, "Prompt", "").with_session(SESSION);
        assert_eq!(stub.completed_at, None);
        stub.updated_at = updated_at;
        f.store.create_note(&stub).unwrap();
        stub
    }

    fn project_note_stub(f: &Fixture, updated_at: i64) -> ProjectNote {
        let mut stub = ProjectNote::new(&f.project.id, "", "").with_session(SESSION);
        assert_eq!(stub.completed_at, None);
        stub.updated_at = updated_at;
        f.store.create_project_note(&stub).unwrap();
        stub
    }

    fn started_review(f: &Fixture, updated_at: i64) -> Review {
        let mut review = Review::new(&f.branch.id, "", ReviewScope::Branch).with_session(SESSION);
        assert_eq!(review.completed_at, None);
        review.updated_at = updated_at;
        f.store.create_review(&review).unwrap();
        review
    }

    fn pending_commit(f: &Fixture, updated_at: i64) -> Commit {
        let mut commit = Commit::new_pending(&f.branch.id).with_session(SESSION);
        assert_eq!(commit.sha, None);
        commit.updated_at = updated_at;
        f.store.create_commit(&commit).unwrap();
        commit
    }

    fn comment(created_at: i64) -> Comment {
        let mut comment = Comment::new("src/lib.rs", Span::new(1, 2), "Why?");
        comment.created_at = created_at;
        comment
    }

    /// Runs `write` and checks the project's activity moved to the wall clock
    /// time of the write: later than every fixed timestamp in these tests.
    fn assert_counts_now(f: &Fixture, what: &str, write: impl FnOnce()) {
        let before = now_timestamp();
        write();
        let at = activity(&f.store).get(&f.project.id).copied();
        assert!(
            at.is_some_and(|at| at >= before),
            "{what}: expected at least {before}, got {at:?}"
        );
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
        // A comment on a completed review, or a revision of a finished note.
        let f = fixture();
        project_note(&f, 500, Some(100));
        note(&f, 400, Some(100));
        review(&f, 300, Some(100));
        assert_eq!(activity(&f.store), only(&f, 500));
    }

    #[test]
    fn a_started_session_changes_nothing() {
        // Alone in a project, each row a session start writes leaves the
        // project with no activity at all.
        let f = fixture();
        note_stub(&f, 250);
        assert_eq!(activity(&f.store), BTreeMap::new(), "note stub");

        let f = fixture();
        project_note_stub(&f, 250);
        assert_eq!(activity(&f.store), BTreeMap::new(), "project note stub");

        let f = fixture();
        started_review(&f, 250);
        assert_eq!(activity(&f.store), BTreeMap::new(), "started review");

        let f = fixture();
        pending_commit(&f, 250);
        assert_eq!(activity(&f.store), BTreeMap::new(), "pending commit");

        // Beside older work, all four together leave its time in place.
        let f = fixture();
        project_note(&f, 100, Some(100));
        note_stub(&f, 250);
        project_note_stub(&f, 250);
        started_review(&f, 250);
        pending_commit(&f, 250);
        assert_eq!(activity(&f.store), only(&f, 100));
    }

    #[test]
    fn the_writes_a_session_start_makes_later_change_nothing() {
        // A queued session's rows are stamped again when it is claimed, and a
        // review's again when the branch tip is filled in. Both stamp the
        // wall clock, far above the 100 here.
        let f = fixture();
        project_note(&f, 100, Some(100));
        note_stub(&f, 250);
        let review = started_review(&f, 250);
        pending_commit(&f, 250);

        f.store.mark_session_artifact_started(SESSION).unwrap();
        assert_eq!(activity(&f.store), only(&f, 100), "claimed");

        f.store
            .update_review_commit_sha(&review.id, "abc123")
            .unwrap();
        let stored = f.store.get_review(&review.id).unwrap().unwrap();
        assert!(stored.updated_at > 250, "the write landed");
        assert_eq!(activity(&f.store), only(&f, 100), "review tip filled in");
    }

    #[test]
    fn a_note_counts_once_its_session_writes_or_finishes() {
        // Finished with no note written.
        let f = fixture();
        let stub = note_stub(&f, 250);
        assert_counts_now(&f, "note completed", || {
            f.store.mark_note_completed(&stub.id).unwrap();
        });

        // The runner storing the turn's note.
        let f = fixture();
        let stub = note_stub(&f, 250);
        assert_counts_now(&f, "note title and content", || {
            f.store
                .update_note_title_and_content(&stub.id, "Findings", "body", None, None)
                .unwrap();
        });

        let f = fixture();
        let stub = note_stub(&f, 250);
        assert_counts_now(&f, "note content", || {
            f.store.update_note_content(&stub.id, "body").unwrap();
        });

        let f = fixture();
        let stub = project_note_stub(&f, 250);
        assert_counts_now(&f, "project note completed", || {
            f.store.mark_project_note_completed(&stub.id).unwrap();
        });

        let f = fixture();
        let stub = project_note_stub(&f, 250);
        assert_counts_now(&f, "project note title and content", || {
            f.store
                .update_project_note_title_and_content(&stub.id, "Plan", "body", None, None)
                .unwrap();
        });
    }

    #[test]
    fn a_finished_session_note_counts_again_when_it_is_revised() {
        // A later turn rewriting a note that completed long ago.
        let f = fixture();
        let mut done = Note::new(&f.branch.id, "Findings", "body").with_session(SESSION);
        done.updated_at = 100;
        done.completed_at = Some(200);
        f.store.create_note(&done).unwrap();
        assert_eq!(activity(&f.store), only(&f, 200));

        assert_counts_now(&f, "note revised", || {
            f.store
                .update_note_title_and_content(&done.id, "Findings", "new body", None, None)
                .unwrap();
        });
    }

    #[test]
    fn a_note_the_user_writes_counts_when_saved() {
        // A written note has no session and is complete when saved.
        let f = fixture();
        let mut written = Note::new_standalone(&f.branch.id, "Bump the lockfile", "")
            .with_subtype(Note::SUBTYPE_WRITTEN);
        written.updated_at = 300;
        written.completed_at = Some(300);
        f.store.create_note_with_unique_title(&mut written).unwrap();
        assert_eq!(activity(&f.store), only(&f, 300));

        assert_counts_now(&f, "written note edited", || {
            f.store
                .update_written_note(&written.id, "Bump the lockfile", "before release")
                .unwrap();
        });

        // A title-only project note has no `completed_at`. With no session it
        // is not a stub, so it counts.
        let f = fixture();
        let mut title_only = ProjectNote::new(&f.project.id, "Ship on Friday", "");
        assert_eq!(title_only.completed_at, None);
        title_only.updated_at = 300;
        f.store.create_project_note(&title_only).unwrap();
        assert_eq!(activity(&f.store), only(&f, 300));
    }

    #[test]
    fn a_review_counts_once_its_session_finishes() {
        let f = fixture();
        let review = started_review(&f, 250);
        assert_counts_now(&f, "review titled", || {
            f.store
                .update_review_title(&review.id, "Ready to ship")
                .unwrap();
        });

        // Finished with no title found.
        let f = fixture();
        let review = started_review(&f, 250);
        assert_counts_now(&f, "review completed", || {
            f.store.mark_review_completed(&review.id).unwrap();
        });
    }

    #[test]
    fn writes_to_a_finished_review_count() {
        let finished = |f: &Fixture| {
            let mut review =
                Review::new(&f.branch.id, "abc123", ReviewScope::Branch).with_session(SESSION);
            review.updated_at = 100;
            review.completed_at = Some(200);
            f.store.create_review(&review).unwrap();
            assert_eq!(activity(&f.store), only(f, 200));
            review
        };

        let f = fixture();
        let review = finished(&f);
        assert_counts_now(&f, "comment added", || {
            f.store.add_comment(&review.id, &comment(150)).unwrap();
        });

        let f = fixture();
        let review = finished(&f);
        assert_counts_now(&f, "file marked reviewed", || {
            f.store.mark_reviewed(&review.id, "src/lib.rs").unwrap();
        });
    }

    #[test]
    fn writes_to_a_review_with_no_session_count() {
        // The diff viewer creates the review on the first comment or file
        // marked reviewed. No session start wrote it, so it counts from the
        // first write on.
        let f = fixture();
        assert_counts_now(&f, "review created by the first action", || {
            f.store
                .ensure_review(&f.branch.id, "abc123", ReviewScope::Commit)
                .unwrap();
        });

        let f = fixture();
        let manual = review(&f, 100, None);
        assert_eq!(activity(&f.store), only(&f, 100));
        assert_counts_now(&f, "comment added", || {
            f.store.add_comment(&manual.id, &comment(150)).unwrap();
        });

        let f = fixture();
        let manual = review(&f, 100, None);
        assert_counts_now(&f, "file marked reviewed", || {
            f.store.mark_reviewed(&manual.id, "src/lib.rs").unwrap();
        });
    }

    #[test]
    fn a_comment_on_a_review_still_in_flight_counts_from_its_own_time() {
        // The review row is ignored until its session finishes, so the
        // comment's own timestamp is what counts, not the `updated_at` the
        // write stamped on the review.
        let f = fixture();
        let review = started_review(&f, 250);
        let added = comment(700);
        f.store.add_comment(&review.id, &added).unwrap();
        assert_eq!(activity(&f.store), only(&f, 700));

        // A file marked reviewed stamps only the review row. It cannot be
        // told apart from the start, so it waits for the session to finish.
        f.store.mark_reviewed(&review.id, "src/lib.rs").unwrap();
        assert_eq!(activity(&f.store), only(&f, 700));

        // Deleting the comment stamps the comment.
        assert_counts_now(&f, "comment deleted", || {
            f.store.delete_comment(&added.id).unwrap();
        });
    }

    #[test]
    fn a_file_marked_reviewed_in_flight_counts_when_the_session_finishes() {
        let f = fixture();
        project_note(&f, 100, Some(100));
        let review = started_review(&f, 250);
        f.store.mark_reviewed(&review.id, "src/lib.rs").unwrap();
        assert_eq!(activity(&f.store), only(&f, 100));

        assert_counts_now(&f, "review completed", || {
            f.store.mark_review_completed(&review.id).unwrap();
        });
    }

    #[test]
    fn a_commit_counts_once_its_sha_lands() {
        let f = fixture();
        let pending = pending_commit(&f, 250);
        assert_eq!(activity(&f.store), BTreeMap::new());
        assert_counts_now(&f, "sha set", || {
            f.store.update_commit_sha(&pending.id, "abc123").unwrap();
        });

        let f = fixture();
        let pending = pending_commit(&f, 250);
        assert_counts_now(&f, "pending commit completed", || {
            let owns_sha = f
                .store
                .complete_pending_commit_sha(&pending.id, &f.branch.id, "abc123")
                .unwrap();
            assert!(owns_sha);
        });
    }

    #[test]
    fn a_landed_commit_counts_again_when_a_rebase_remaps_it() {
        let f = fixture();
        let landed = commit(&f, 100);
        assert_eq!(activity(&f.store), only(&f, 100));
        assert_counts_now(&f, "sha remapped", || {
            let remapped = f
                .store
                .remap_commit_shas(&f.branch.id, &[(landed.id.as_str(), "abc123", "def456")])
                .unwrap();
            assert_eq!(remapped, 1);
        });
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
