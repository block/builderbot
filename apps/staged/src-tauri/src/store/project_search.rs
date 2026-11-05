//! The project search index: every titled item project search matches on,
//! across all projects.
//!
//! Titles only, never bodies, each cut to [`MAX_TITLE_CHARS`], so the whole
//! index is a few hundred short rows read in about a millisecond. The
//! frontend loads it once per search and does the matching itself, which
//! keeps each keystroke synchronous.

use rusqlite::Connection;
use serde::Serialize;

use super::{Store, StoreError};

/// What a [`ProjectSearchItem`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectSearchItemKind {
    /// A branch note, including a child note filed under a project note.
    Note,
    ProjectNote,
    Review,
}

/// One titled item of a project in the search index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSearchItem {
    /// The project the item belongs to; for branch items, the branch's
    /// current project, so an item follows its branch when the branch moves.
    pub project_id: String,
    /// The branch a note or review is on; `None` for project notes.
    pub branch_id: Option<String>,
    pub kind: ProjectSearchItemKind,
    pub id: String,
    pub title: String,
    pub updated_at: i64,
}

/// The longest title the index carries, in characters. A branch note's
/// session stub is titled with the prompt that started it, which can run to
/// kilobytes and stays that way if the session errors or is cancelled. The
/// frontend shows one line of a title, so the rest only costs transfer and
/// matching.
const MAX_TITLE_CHARS: i64 = 300;

/// `(sql, kind)` for each item kind. Each statement selects
/// `id, project_id, branch_id, title, updated_at`, the title cut to `?1`
/// characters, and skips untitled rows: a project note's session stub before
/// its session titles it, and untitled reviews. A branch note's stub is
/// titled with its prompt from the start, so it is listed, as the timeline
/// shows it.
/// A review's stored title is used as is: the timeline's rules for showing a
/// review (its commit still on the branch, a running review's live ACP title)
/// need git or the session, which a title read doesn't touch.
const ITEM_QUERIES: [(&str, ProjectSearchItemKind); 3] = [
    (
        "SELECT n.id, b.project_id, n.branch_id, substr(n.title, 1, ?1), n.updated_at
         FROM notes n JOIN branches b ON b.id = n.branch_id
         WHERE n.title <> ''",
        ProjectSearchItemKind::Note,
    ),
    (
        "SELECT id, project_id, NULL, substr(title, 1, ?1), updated_at
         FROM project_notes
         WHERE title <> ''",
        ProjectSearchItemKind::ProjectNote,
    ),
    (
        "SELECT r.id, b.project_id, r.branch_id, substr(r.title, 1, ?1), r.updated_at
         FROM reviews r JOIN branches b ON b.id = r.branch_id
         WHERE r.title IS NOT NULL AND r.title <> ''",
        ProjectSearchItemKind::Review,
    ),
];

impl Store {
    /// Every note, project note and review title across all projects, in no
    /// particular order. Read under one lock, so the three kinds agree.
    pub fn list_project_search_items(&self) -> Result<Vec<ProjectSearchItem>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut items = Vec::new();
        for (sql, kind) in ITEM_QUERIES {
            collect_items(&conn, sql, kind, &mut items)?;
        }
        Ok(items)
    }
}

fn collect_items(
    conn: &Connection,
    sql: &str,
    kind: ProjectSearchItemKind,
    items: &mut Vec<ProjectSearchItem>,
) -> Result<(), StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([MAX_TITLE_CHARS], |row| {
        Ok(ProjectSearchItem {
            id: row.get(0)?,
            project_id: row.get(1)?,
            branch_id: row.get(2)?,
            title: row.get(3)?,
            updated_at: row.get(4)?,
            kind,
        })
    })?;
    for row in rows {
        items.push(row?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::models::{Branch, Note, Project, ProjectNote, ProjectRepo, Review};
    use crate::store::{BranchMove, RepoPlacement, ReviewScope};

    struct Fixture {
        store: Store,
        project: Project,
        other: Project,
        repo: ProjectRepo,
        branch: Branch,
    }

    /// A project with one branch on its own repo row, plus an empty second
    /// project to move things into.
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

    fn items_by_id(store: &Store) -> Vec<ProjectSearchItem> {
        let mut items = store.list_project_search_items().unwrap();
        items.sort_by(|a, b| a.id.cmp(&b.id));
        items
    }

    fn item(items: &[ProjectSearchItem], id: &str) -> ProjectSearchItem {
        items
            .iter()
            .find(|i| i.id == id)
            .unwrap_or_else(|| panic!("no search item {id} in {items:#?}"))
            .clone()
    }

    #[test]
    fn lists_every_titled_kind_with_its_project() {
        let f = fixture();
        let note = Note::new(&f.branch.id, "Login research", "body");
        f.store.create_note(&note).unwrap();
        let project_note = ProjectNote::new(&f.project.id, "Login plan", "body");
        f.store.create_project_note(&project_note).unwrap();
        let review = Review::new(&f.branch.id, "abc123", ReviewScope::Commit);
        f.store.create_review(&review).unwrap();
        f.store
            .update_review_title(&review.id, "Review login flow")
            .unwrap();
        let other_note = ProjectNote::new(&f.other.id, "Invoices", "body");
        f.store.create_project_note(&other_note).unwrap();

        let items = items_by_id(&f.store);
        assert_eq!(items.len(), 4, "{items:#?}");

        let n = item(&items, &note.id);
        assert_eq!(n.kind, ProjectSearchItemKind::Note);
        assert_eq!(n.project_id, f.project.id);
        assert_eq!(n.branch_id.as_deref(), Some(f.branch.id.as_str()));
        assert_eq!(n.title, "Login research");
        assert_eq!(n.updated_at, note.updated_at);

        let pn = item(&items, &project_note.id);
        assert_eq!(pn.kind, ProjectSearchItemKind::ProjectNote);
        assert_eq!(pn.project_id, f.project.id);
        assert_eq!(pn.branch_id, None);

        let r = item(&items, &review.id);
        assert_eq!(r.kind, ProjectSearchItemKind::Review);
        assert_eq!(r.project_id, f.project.id);
        assert_eq!(r.branch_id.as_deref(), Some(f.branch.id.as_str()));
        assert_eq!(r.title, "Review login flow");

        assert_eq!(item(&items, &other_note.id).project_id, f.other.id);
    }

    #[test]
    fn skips_untitled_project_notes_and_reviews() {
        let f = fixture();
        // A project-note session stub before its session titles it.
        let stub = ProjectNote::new(&f.project.id, "", "");
        f.store.create_project_note(&stub).unwrap();
        let untitled = Review::new(&f.branch.id, "abc123", ReviewScope::Commit);
        f.store.create_review(&untitled).unwrap();

        assert!(f.store.list_project_search_items().unwrap().is_empty());
    }

    #[test]
    fn lists_a_prompt_titled_branch_note_stub_with_its_title_cut() {
        let f = fixture();
        // A note session's stub carries its prompt as the title; one whose
        // session failed keeps it. Multi-byte characters count as one.
        let prompt = format!("Écris {}", "a long prompt ".repeat(100));
        let stub = Note::new(&f.branch.id, &prompt, "");
        f.store.create_note(&stub).unwrap();
        let short = ProjectNote::new(&f.project.id, "Login plan", "body");
        f.store.create_project_note(&short).unwrap();

        let items = items_by_id(&f.store);
        let title = item(&items, &stub.id).title;
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS as usize);
        assert!(prompt.starts_with(&title));
        assert_eq!(item(&items, &short.id).title, "Login plan");
    }

    #[test]
    fn child_notes_are_listed_under_their_branch_project() {
        let f = fixture();
        let parent = ProjectNote::new(&f.project.id, "Plan", "body");
        f.store.create_project_note(&parent).unwrap();
        let child = Note::new(&f.branch.id, "Step 1", "body").with_parent_project_note(&parent.id);
        f.store.create_note(&child).unwrap();

        let c = item(&items_by_id(&f.store), &child.id);
        assert_eq!(c.kind, ProjectSearchItemKind::Note);
        assert_eq!(c.project_id, f.project.id);
    }

    #[test]
    fn branch_items_follow_a_moved_branch() {
        let f = fixture();
        let note = Note::new(&f.branch.id, "Login research", "body");
        f.store.create_note(&note).unwrap();
        let review = Review::new(&f.branch.id, "abc123", ReviewScope::Commit);
        f.store.create_review(&review).unwrap();
        f.store.update_review_title(&review.id, "Review").unwrap();
        let project_note = ProjectNote::new(&f.project.id, "Login plan", "body");
        f.store.create_project_note(&project_note).unwrap();

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

        let items = items_by_id(&f.store);
        assert_eq!(item(&items, &note.id).project_id, f.other.id);
        assert_eq!(item(&items, &review.id).project_id, f.other.id);
        // Project notes belong to the project, not the branch: they stay.
        assert_eq!(item(&items, &project_note.id).project_id, f.project.id);
    }

    #[test]
    fn serializes_camel_case_for_the_frontend() {
        let value = serde_json::to_value(ProjectSearchItem {
            project_id: "p1".into(),
            branch_id: None,
            kind: ProjectSearchItemKind::ProjectNote,
            id: "pn1".into(),
            title: "Plan".into(),
            updated_at: 7,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "projectId": "p1",
                "branchId": null,
                "kind": "projectNote",
                "id": "pn1",
                "title": "Plan",
                "updatedAt": 7,
            })
        );
    }
}
