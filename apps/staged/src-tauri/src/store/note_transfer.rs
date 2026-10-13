//! Moving and copying notes between projects — the transactional half.
//!
//! The command layer (`crate::note_transfer`) resolves where a note lands and
//! copies its media files into place first; what remains is rewriting or
//! inserting the note row together with the media rows that describe those
//! files, in one transaction, so a failure can never leave a note half-moved
//! or a copy without its attachments.
//!
//! A move is an `UPDATE`, never a delete-and-reinsert: the note keeps its id
//! (so `#note:<id>` references elsewhere stay valid) and its `session_id`, and
//! the `AFTER DELETE` triggers that garbage-collect sessions are never fired.

use rusqlite::{params, OptionalExtension};

use super::models::{Branch, Image, Note, ProjectNote, ProjectRepo};
use super::{now_timestamp, Store, StoreChange, StoreError};

impl Store {
    /// The `project_repos` row in `project_id` for `github_repo` at `subpath`,
    /// matching the `idx_project_repos_unique` key (`NULL` and `''` subpaths
    /// are the same repo).
    pub fn find_project_repo(
        &self,
        project_id: &str,
        github_repo: &str,
        subpath: Option<&str>,
    ) -> Result<Option<ProjectRepo>, StoreError> {
        let id: Option<String> = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT id FROM project_repos
                 WHERE project_id = ?1 AND github_repo = ?2
                   AND COALESCE(subpath, '') = COALESCE(?3, '')
                 ORDER BY created_at ASC LIMIT 1",
                params![project_id, github_repo, subpath],
                |row| row.get(0),
            )
            .optional()?
        };
        match id {
            Some(id) => self.get_project_repo(&id),
            None => Ok(None),
        }
    }

    /// The branch in `project_id` that tracks `github_repo` at `subpath`.
    ///
    /// Several branches can share one repo row; the oldest is the one the
    /// project was set up with, so it is the one a note lands on.
    pub fn find_branch_for_repo(
        &self,
        project_id: &str,
        github_repo: &str,
        subpath: Option<&str>,
    ) -> Result<Option<Branch>, StoreError> {
        let id: Option<String> = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT b.id FROM branches b
                 INNER JOIN project_repos pr ON pr.id = b.project_repo_id
                 WHERE pr.project_id = ?1 AND pr.github_repo = ?2
                   AND COALESCE(pr.subpath, '') = COALESCE(?3, '')
                 ORDER BY b.created_at ASC LIMIT 1",
                params![project_id, github_repo, subpath],
                |row| row.get(0),
            )
            .optional()?
        };
        match id {
            Some(id) => self.get_branch(&id),
            None => Ok(None),
        }
    }

    /// Re-home a branch note onto `target_branch_id`, swapping in `content`
    /// (the body with its media references rewritten to `media`, the copied
    /// rows). Returns the note as it now reads.
    ///
    /// The title is made unique among the destination branch's notes, the
    /// same way a fresh save would be. `parent_project_note_id` is cleared:
    /// a child note's parent project note stays in the source project, and a
    /// cross-project parent link would let deleting that parent cascade into
    /// another project.
    pub fn move_note_to_branch(
        &self,
        note_id: &str,
        target_branch_id: &str,
        content: &str,
        media: &[Image],
    ) -> Result<Note, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let existing = tx
            .query_row(
                "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, parent_project_note_id, subtype
                 FROM notes WHERE id = ?1",
                params![note_id],
                Self::row_to_note,
            )
            .optional()?
            .ok_or_else(|| StoreError(format!("Note not found: {note_id}")))?;
        let title = if existing.title.is_empty() {
            String::new()
        } else {
            Self::resolve_unique_note_title(&tx, target_branch_id, &existing.title, None)?
        };
        for image in media {
            Self::insert_image(&tx, image)?;
        }
        let now = now_timestamp();
        tx.execute(
            "UPDATE notes
             SET branch_id = ?1, title = ?2, content = ?3, parent_project_note_id = NULL, updated_at = ?4
             WHERE id = ?5",
            params![target_branch_id, title, content, now, note_id],
        )?;
        tx.commit()?;

        // Both timelines changed: the note left one and arrived on the other.
        self.publish(StoreChange::Notes {
            branch_id: Some(existing.branch_id.clone()),
            project_id: None,
        });
        self.publish(StoreChange::Notes {
            branch_id: Some(target_branch_id.to_string()),
            project_id: None,
        });
        Ok(Note {
            branch_id: target_branch_id.to_string(),
            title,
            content: content.to_string(),
            updated_at: now,
            parent_project_note_id: None,
            ..existing
        })
    }

    /// Insert `note` (already pointed at its destination branch, with its
    /// content rewritten to `media`) and the media rows together. The title is
    /// made unique on the destination branch and written back into `note`.
    pub fn copy_note_to_branch(&self, note: &mut Note, media: &[Image]) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        if !note.title.is_empty() {
            note.title = Self::resolve_unique_note_title(&tx, &note.branch_id, &note.title, None)?;
        }
        for image in media {
            Self::insert_image(&tx, image)?;
        }
        Self::insert_note(&tx, note)?;
        tx.commit()?;
        self.publish(StoreChange::Notes {
            branch_id: Some(note.branch_id.clone()),
            project_id: None,
        });
        Ok(())
    }

    /// Re-home a project note into `target_project_id`; see
    /// [`Store::move_note_to_branch`] for the content/media contract.
    ///
    /// Child notes aggregated under it are detached rather than moved: they are
    /// branch notes bound to the source project's repos, which the destination
    /// need not have. Detached, they surface as ordinary notes on their own
    /// branch timelines, and the `#note:<id>` references in the parent's body
    /// keep resolving (that lookup has no project scope).
    pub fn move_project_note_to_project(
        &self,
        note_id: &str,
        target_project_id: &str,
        content: &str,
        media: &[Image],
    ) -> Result<ProjectNote, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let existing = tx
            .query_row(
                "SELECT id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step
                 FROM project_notes WHERE id = ?1",
                params![note_id],
                Self::row_to_project_note,
            )
            .optional()?
            .ok_or_else(|| StoreError(format!("Project note not found: {note_id}")))?;
        let child_branch_ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT branch_id FROM notes WHERE parent_project_note_id = ?1",
            )?;
            let rows = stmt.query_map(params![note_id], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for image in media {
            Self::insert_image(&tx, image)?;
        }
        tx.execute(
            "UPDATE notes SET parent_project_note_id = NULL WHERE parent_project_note_id = ?1",
            params![note_id],
        )?;
        let now = now_timestamp();
        tx.execute(
            "UPDATE project_notes SET project_id = ?1, content = ?2, updated_at = ?3 WHERE id = ?4",
            params![target_project_id, content, now, note_id],
        )?;
        tx.commit()?;

        self.publish(StoreChange::Notes {
            branch_id: None,
            project_id: Some(existing.project_id.clone()),
        });
        self.publish(StoreChange::Notes {
            branch_id: None,
            project_id: Some(target_project_id.to_string()),
        });
        // Detached children just appeared on their branch timelines.
        for branch_id in child_branch_ids {
            self.publish(StoreChange::Notes {
                branch_id: Some(branch_id),
                project_id: None,
            });
        }
        Ok(ProjectNote {
            project_id: target_project_id.to_string(),
            content: content.to_string(),
            updated_at: now,
            ..existing
        })
    }

    /// Insert `note` (already pointed at its destination project) and the
    /// media rows together.
    pub fn copy_project_note_to_project(
        &self,
        note: &ProjectNote,
        media: &[Image],
    ) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for image in media {
            Self::insert_image(&tx, image)?;
        }
        Self::insert_project_note(&tx, note)?;
        tx.commit()?;
        self.publish(StoreChange::Notes {
            branch_id: None,
            project_id: Some(note.project_id.clone()),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::models::Project;

    struct Fixture {
        store: Store,
        source: Project,
        target: Project,
        source_branch: Branch,
        target_branch: Branch,
    }

    /// Two projects that both track `acme/widgets` at the repo root, each
    /// with the branch that repo row was set up with.
    fn fixture() -> Fixture {
        fixture_in(Store::in_memory().unwrap())
    }

    /// [`fixture`] over a caller-supplied store, so the change-feed test can
    /// build the same shape on a store with a sender attached.
    fn fixture_in(store: Store) -> Fixture {
        let source = Project::named("source");
        let target = Project::named("target");
        store.create_project(&source).unwrap();
        store.create_project(&target).unwrap();

        let source_repo = ProjectRepo::new(&source.id, "acme/widgets", "feature", None).primary();
        let target_repo = ProjectRepo::new(&target.id, "acme/widgets", "other", None).primary();
        store.create_project_repo(&source_repo).unwrap();
        store.create_project_repo(&target_repo).unwrap();

        let source_branch =
            Branch::new(&source.id, "feature", "origin/main").with_project_repo(&source_repo.id);
        let target_branch =
            Branch::new(&target.id, "other", "origin/main").with_project_repo(&target_repo.id);
        store.create_branch(&source_branch).unwrap();
        store.create_branch(&target_branch).unwrap();

        Fixture {
            store,
            source,
            target,
            source_branch,
            target_branch,
        }
    }

    #[test]
    fn finds_the_repo_and_branch_by_repo_and_subpath() {
        let f = fixture();
        let sub = ProjectRepo::new(
            &f.target.id,
            "acme/widgets",
            "other-apps-web",
            Some("apps/web".into()),
        );
        f.store.create_project_repo(&sub).unwrap();
        let sub_branch =
            Branch::new(&f.target.id, "other-apps-web", "origin/main").with_project_repo(&sub.id);
        f.store.create_branch(&sub_branch).unwrap();

        let root = f
            .store
            .find_branch_for_repo(&f.target.id, "acme/widgets", None)
            .unwrap()
            .unwrap();
        assert_eq!(root.id, f.target_branch.id);
        // An empty subpath is the repo root, like the unique index says.
        assert_eq!(
            f.store
                .find_branch_for_repo(&f.target.id, "acme/widgets", Some(""))
                .unwrap()
                .unwrap()
                .id,
            f.target_branch.id
        );
        assert_eq!(
            f.store
                .find_branch_for_repo(&f.target.id, "acme/widgets", Some("apps/web"))
                .unwrap()
                .unwrap()
                .id,
            sub_branch.id
        );
        assert!(f
            .store
            .find_branch_for_repo(&f.target.id, "acme/gadgets", None)
            .unwrap()
            .is_none());
        assert_eq!(
            f.store
                .find_project_repo(&f.target.id, "acme/widgets", Some("apps/web"))
                .unwrap()
                .unwrap()
                .id,
            sub.id
        );
        assert!(f
            .store
            .find_project_repo(&f.source.id, "acme/widgets", Some("apps/web"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn move_rewrites_the_branch_and_detaches_the_parent_link() {
        let f = fixture();
        let parent = ProjectNote::new(&f.source.id, "Parent", "body");
        f.store.create_project_note(&parent).unwrap();
        let note = Note::new_standalone(&f.source_branch.id, "Findings", "body")
            .with_parent_project_note(&parent.id);
        f.store.create_note(&note).unwrap();
        let image = Image::new(
            Some(&f.target_branch.id),
            &f.target.id,
            "shot.png",
            "image/png",
            3,
            false,
        );

        let moved = f
            .store
            .move_note_to_branch(
                &note.id,
                &f.target_branch.id,
                "new body",
                std::slice::from_ref(&image),
            )
            .unwrap();

        assert_eq!(moved.id, note.id);
        assert_eq!(moved.branch_id, f.target_branch.id);
        assert_eq!(moved.content, "new body");
        assert!(moved.parent_project_note_id.is_none());
        let stored = f.store.get_note(&note.id).unwrap().unwrap();
        assert_eq!(stored.branch_id, f.target_branch.id);
        assert!(stored.parent_project_note_id.is_none());
        assert!(f.store.get_image(&image.id).unwrap().is_some());
        assert!(f
            .store
            .list_notes_for_branch(&f.source_branch.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn move_makes_the_title_unique_on_the_destination() {
        let f = fixture();
        let taken = Note::new_standalone(&f.target_branch.id, "Findings", "theirs");
        f.store.create_note(&taken).unwrap();
        let note = Note::new_standalone(&f.source_branch.id, "Findings", "mine");
        f.store.create_note(&note).unwrap();

        let moved = f
            .store
            .move_note_to_branch(&note.id, &f.target_branch.id, "mine", &[])
            .unwrap();

        assert_eq!(moved.title, "Findings (2)");
    }

    /// Foreign keys are on, so a destination branch that does not exist fails
    /// the `UPDATE` — and the media rows inserted before it must go with it.
    #[test]
    fn move_rolls_back_the_media_rows_when_the_note_update_fails() {
        let f = fixture();
        let note = Note::new_standalone(&f.source_branch.id, "Findings", "body");
        f.store.create_note(&note).unwrap();
        let image = Image::new(None, &f.target.id, "shot.png", "image/png", 3, false);

        let err = f
            .store
            .move_note_to_branch(
                &note.id,
                "no-such-branch",
                "body",
                std::slice::from_ref(&image),
            )
            .unwrap_err();

        assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
        assert!(f.store.get_image(&image.id).unwrap().is_none());
        assert_eq!(
            f.store.get_note(&note.id).unwrap().unwrap().branch_id,
            f.source_branch.id
        );
    }

    #[test]
    fn copy_inserts_the_note_and_media_with_a_unique_title() {
        let f = fixture();
        let taken = Note::new_standalone(&f.target_branch.id, "Findings", "theirs");
        f.store.create_note(&taken).unwrap();
        let image = Image::new(
            Some(&f.target_branch.id),
            &f.target.id,
            "shot.png",
            "image/png",
            3,
            false,
        );
        let mut copy = Note::new_standalone(&f.target_branch.id, "Findings", "copied");

        f.store
            .copy_note_to_branch(&mut copy, std::slice::from_ref(&image))
            .unwrap();

        assert_eq!(copy.title, "Findings (2)");
        let stored = f.store.get_note(&copy.id).unwrap().unwrap();
        assert_eq!(stored.title, "Findings (2)");
        assert_eq!(stored.content, "copied");
        assert!(f.store.get_image(&image.id).unwrap().is_some());
    }

    #[test]
    fn project_note_move_detaches_its_children_and_publishes_their_branches() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(64);
        let f = fixture_in(Store::in_memory().unwrap().with_change_sender(tx));
        let branch = &f.source_branch;
        let parent = ProjectNote::new(&f.source.id, "Parent", "body");
        f.store.create_project_note(&parent).unwrap();
        let child =
            Note::new_standalone(&branch.id, "Child", "body").with_parent_project_note(&parent.id);
        f.store.create_note(&child).unwrap();
        while rx.try_recv().is_ok() {}

        let moved = f
            .store
            .move_project_note_to_project(&parent.id, &f.target.id, "body", &[])
            .unwrap();

        assert_eq!(moved.project_id, f.target.id);
        assert_eq!(
            f.store
                .get_project_note(&parent.id)
                .unwrap()
                .unwrap()
                .project_id,
            f.target.id
        );
        let child = f.store.get_note(&child.id).unwrap().unwrap();
        assert!(child.parent_project_note_id.is_none());
        assert_eq!(child.branch_id, branch.id);
        assert!(f.store.list_project_notes(&f.source.id).unwrap().is_empty());

        let project_change = |project_id: &str| StoreChange::Notes {
            branch_id: None,
            project_id: Some(project_id.to_string()),
        };
        assert_eq!(rx.try_recv().unwrap(), project_change(&f.source.id));
        assert_eq!(rx.try_recv().unwrap(), project_change(&f.target.id));
        assert_eq!(
            rx.try_recv().unwrap(),
            StoreChange::Notes {
                branch_id: Some(branch.id.clone()),
                project_id: None,
            }
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn project_note_copy_inserts_the_note_and_media() {
        let f = fixture();
        let image = Image::new(None, &f.target.id, "shot.png", "image/png", 3, false);
        let copy = ProjectNote::new(&f.target.id, "Parent", "copied");

        f.store
            .copy_project_note_to_project(&copy, std::slice::from_ref(&image))
            .unwrap();

        let stored = f.store.get_project_note(&copy.id).unwrap().unwrap();
        assert_eq!(stored.project_id, f.target.id);
        assert_eq!(stored.content, "copied");
        assert!(f.store.get_image(&image.id).unwrap().is_some());
    }
}
