//! Moving and copying notes between projects — the transactional half.
//!
//! The command layer (`crate::note_transfer`) resolves where a note lands and
//! puts its media files into place first; what remains is rewriting or
//! inserting the note row together with the media rows that describe those
//! files, in one transaction, so a failure can never leave a note half-moved
//! or a copy without its attachments.
//!
//! A move hands over two kinds of media row: copies to insert, for media the
//! note shares with something else, and rows to re-home, for media only the
//! note uses. A re-homed row is updated in place, so its id — and every
//! reference to it, including ones outside the note's current body — keeps
//! resolving. Whether a row is the note's alone is checked again here, under
//! the transaction: a note or chat message that picked it up since the caller
//! looked would lose it to the other project, so the move is refused instead.
//!
//! A move is an `UPDATE`, never a delete-and-reinsert: the note keeps its id
//! (so `#note:<id>` references elsewhere stay valid) and its `session_id`, and
//! the `AFTER DELETE` triggers that garbage-collect sessions are never fired.
//!
//! The `UPDATE` is keyed on where the caller last saw the note as well as its
//! id. The command layer resolves the destination from the source location,
//! and a note relocated (or deleted) between that read and this transaction
//! would otherwise be silently overwritten; matching zero rows fails the
//! transaction instead, so the media rows roll back with it.
//!
//! A project-note move keys the parent's and every child's `UPDATE` on the
//! body the caller read, too. The body it hands over, and the media rows
//! that come with it, were rewritten from that read, and staging the media
//! can take long enough for a resumed session to finish again (or an edit to
//! land). That note is settled again but newer than the body that would
//! replace it, so the move is refused and a retry reads it afresh.

use rusqlite::{params, OptionalExtension, Transaction};

use super::models::{Branch, Image, Note, ProjectNote, ProjectRepo};
use super::{now_timestamp, Store, StoreChange, StoreError};

/// One child note's share of a project-note move: where the command layer
/// last saw it and what its body read then, the destination branch it
/// resolved, that body with its media references rewritten to `media` (the
/// copied rows), and the rows only the child uses, re-homed in place
/// (`rehomed`; see `rehome_note_media`).
#[derive(Debug, Clone)]
pub struct ChildNoteMove {
    pub note_id: String,
    pub expected_branch_id: String,
    pub expected_content: String,
    pub target_branch_id: String,
    pub content: String,
    pub media: Vec<Image>,
    pub rehomed: Vec<Image>,
}

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
    /// rows) and re-pointing `rehomed` (each row as it should read in the
    /// destination; see `rehome_note_media`). Returns the note as it
    /// now reads.
    ///
    /// The note must still be on `expected_branch_id`, the branch the caller
    /// resolved the destination from; otherwise nothing is written and the
    /// move is refused.
    ///
    /// The title is made unique among the destination branch's notes, the
    /// same way a fresh save would be. `parent_project_note_id` is cleared:
    /// moving one child on its own leaves its parent project note in the
    /// source project, and a cross-project parent link would let deleting
    /// that parent cascade into another project. (A project note moved
    /// through [`Store::move_project_note_to_project`] takes its children
    /// with it instead.)
    pub fn move_note_to_branch(
        &self,
        note_id: &str,
        expected_branch_id: &str,
        target_branch_id: &str,
        content: &str,
        media: &[Image],
        rehomed: &[Image],
    ) -> Result<Note, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let existing = tx
            .query_row(
                "SELECT id, branch_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, parent_project_note_id, subtype, read_at
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
        Self::rehome_note_media(&tx, note_id, &existing.title, rehomed)?;
        let now = now_timestamp();
        let changed = tx.execute(
            "UPDATE notes
             SET branch_id = ?1, title = ?2, content = ?3, parent_project_note_id = NULL, updated_at = ?4
             WHERE id = ?5 AND branch_id = ?6",
            params![
                target_branch_id,
                title,
                content,
                now,
                note_id,
                expected_branch_id
            ],
        )?;
        if changed != 1 {
            return Err(relocated_during_transfer(&existing.title));
        }
        // The note's session is scheduled by `sessions.branch_id`, so it moves
        // with the note; otherwise the destination branch would never see the
        // session as queued or running work, and its completion would drain
        // the branch it left.
        if let Some(session_id) = existing.session_id.as_deref() {
            move_settled_session(&tx, session_id, target_branch_id, &existing.title)?;
        }
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

    /// Re-home a project note into `target_project_id`, taking its child notes
    /// along; see [`Store::move_note_to_branch`] for the content/media/rehomed
    /// contract and the role of `expected_project_id`.
    ///
    /// `expected_content` is the body the caller read and rewrote `content`
    /// from (each child carries its own); a note whose body has changed since
    /// fails the whole transaction rather than losing the newer body.
    ///
    /// `children` must cover every note whose `parent_project_note_id` is this
    /// note. Each is rewritten onto its `target_branch_id` — a branch in the
    /// destination tracking the same repo, resolved by the command layer —
    /// together with its session and its own media, and keeps the parent link
    /// (and its read state), so the family stays one unit: the parent view's
    /// child list, the `#note:<id>` references in the parent's body and the
    /// delete cascade all keep working and none of them crosses a project. A
    /// child the plan did not cover (spawned, moved or re-parented since it was
    /// made) fails the whole transaction rather than leaving a cross-project
    /// link behind.
    #[allow(clippy::too_many_arguments)]
    pub fn move_project_note_to_project(
        &self,
        note_id: &str,
        expected_project_id: &str,
        expected_content: &str,
        target_project_id: &str,
        content: &str,
        media: &[Image],
        rehomed: &[Image],
        children: &[ChildNoteMove],
    ) -> Result<ProjectNote, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let existing = tx
            .query_row(
                "SELECT id, project_id, session_id, title, content, created_at, updated_at, completed_at, suggested_next_commit_step, suggested_next_note_step, read_at
                 FROM project_notes WHERE id = ?1",
                params![note_id],
                Self::row_to_project_note,
            )
            .optional()?
            .ok_or_else(|| StoreError(format!("Project note not found: {note_id}")))?;
        for image in media {
            Self::insert_image(&tx, image)?;
        }
        Self::rehome_note_media(&tx, note_id, &existing.title, rehomed)?;
        let now = now_timestamp();
        let mut touched_branches: Vec<String> = Vec::new();
        for child in children {
            for image in &child.media {
                Self::insert_image(&tx, image)?;
            }
            let (child_title, session_id): (String, Option<String>) = tx
                .query_row(
                    "SELECT title, session_id FROM notes WHERE id = ?1",
                    params![child.note_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .ok_or_else(|| StoreError(format!("Note not found: {}", child.note_id)))?;
            Self::rehome_note_media(&tx, &child.note_id, &child_title, &child.rehomed)?;
            let title = if child_title.is_empty() {
                String::new()
            } else {
                Self::resolve_unique_note_title(
                    &tx,
                    &child.target_branch_id,
                    &child_title,
                    Some(&child.note_id),
                )?
            };
            // Keyed on the parent too: a child re-parented since the plan was
            // made is not ours to move. And on the body the plan read: a
            // child whose session was resumed and finished again since then
            // passes the session check below, but `child.content` (and the
            // media staged from it) would overwrite the body that run wrote.
            // Zero rows means the child left, or its body changed; a second
            // read tells them apart.
            let changed = tx.execute(
                "UPDATE notes
                 SET branch_id = ?1, title = ?2, content = ?3, updated_at = ?4
                 WHERE id = ?5 AND branch_id = ?6 AND parent_project_note_id = ?7
                   AND content = ?8",
                params![
                    child.target_branch_id,
                    title,
                    child.content,
                    now,
                    child.note_id,
                    child.expected_branch_id,
                    note_id,
                    child.expected_content
                ],
            )?;
            if changed != 1 {
                let still_here: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM notes
                      WHERE id = ?1 AND branch_id = ?2 AND parent_project_note_id = ?3)",
                    params![child.note_id, child.expected_branch_id, note_id],
                    |row| row.get(0),
                )?;
                return Err(if still_here {
                    updated_during_transfer(&child_title)
                } else {
                    relocated_during_transfer(&child_title)
                });
            }
            // Same as `move_note_to_branch`: the session is scheduled by
            // `sessions.branch_id` and has to arrive with its note.
            if let Some(session_id) = session_id.as_deref() {
                move_settled_session(&tx, session_id, &child.target_branch_id, &child_title)?;
            }
            for branch_id in [&child.expected_branch_id, &child.target_branch_id] {
                if !touched_branches.contains(branch_id) {
                    touched_branches.push(branch_id.clone());
                }
            }
        }
        let child_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM notes WHERE parent_project_note_id = ?1",
            params![note_id],
            |row| row.get(0),
        )?;
        if child_count != children.len() as i64 {
            return Err(StoreError(format!(
                "The child notes of '{}' changed during the transfer.",
                existing.title
            )));
        }
        // The parent's own session is keyed on too: the caller checked it was
        // settled, but a resume from another window can land before this
        // write, and a running project session keeps creating child notes in
        // the project it started in — which would be the wrong one after the
        // move. So is its body, as for a child: a session that resumed and
        // finished again (or an edit) since the caller read it is settled
        // again, and `content` would overwrite what it wrote. Zero rows means
        // a parent that left the expected project, a live session or a newer
        // body; the second read tells them apart, a live session first, since
        // a retry would only stop on it again.
        let changed = tx.execute(
            "UPDATE project_notes SET project_id = ?1, content = ?2, updated_at = ?3
             WHERE id = ?4 AND project_id = ?5 AND content = ?6
               AND NOT EXISTS (
                   SELECT 1 FROM sessions s
                    WHERE s.id = project_notes.session_id
                      AND s.status IN ('queued', 'running'))",
            params![
                target_project_id,
                content,
                now,
                note_id,
                expected_project_id,
                expected_content
            ],
        )?;
        if changed != 1 {
            let session_live: Option<bool> = tx
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM sessions s
                          WHERE s.id = project_notes.session_id
                            AND s.status IN ('queued', 'running'))
                     FROM project_notes WHERE id = ?1 AND project_id = ?2",
                    params![note_id, expected_project_id],
                    |row| row.get(0),
                )
                .optional()?;
            return Err(match session_live {
                None => relocated_during_transfer(&existing.title),
                Some(true) => session_resumed_during_transfer(&existing.title),
                Some(false) => updated_during_transfer(&existing.title),
            });
        }
        tx.commit()?;

        self.publish(StoreChange::Notes {
            branch_id: None,
            project_id: Some(existing.project_id.clone()),
        });
        self.publish(StoreChange::Notes {
            branch_id: None,
            project_id: Some(target_project_id.to_string()),
        });
        // The children's sessions left one branch's queue and joined another's.
        for branch_id in touched_branches {
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

    /// Re-point media rows that move with note `note_id` at its destination,
    /// keeping their ids: each of `rows` is written over the row with its id
    /// (scope, filename and owning session; the caller has already linked
    /// the file into the destination's media directory).
    ///
    /// A row some other note or a chat message has started using since the
    /// caller checked, or one deleted meanwhile, refuses the move rather than
    /// being taken from under that user. What uses the rows is read once, for
    /// all of them, before the first is written: the transaction is the
    /// snapshot, and nothing below adds a reference. Collecting a displaced
    /// session can only remove one, with that session's messages; a row those
    /// messages attached still refuses the move, as it did whenever the row
    /// came before the session's in `rows`.
    ///
    /// The row takes the moved note's session, as a copy would. Ingest already
    /// stores a note's media under its session, so that rarely changes
    /// anything; when it does, the session the row named loses a reference
    /// the way it would have had the row been deleted, and is collected by the
    /// rule `trg_cleanup_session_after_image_delete` applies to a delete.
    fn rehome_note_media(
        tx: &rusqlite::Transaction<'_>,
        note_id: &str,
        title: &str,
        rows: &[Image],
    ) -> Result<(), StoreError> {
        if rows.is_empty() {
            return Ok(());
        }
        let references =
            Self::media_references_on(tx, rows.iter().map(|row| row.id.as_str()), Some(note_id))?;
        for row in rows {
            let displaced: Option<Option<String>> = tx
                .query_row(
                    "SELECT session_id FROM images WHERE id = ?1",
                    params![row.id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(displaced) = displaced else {
                return Err(media_changed_during_transfer(title));
            };
            if !references.unused(&row.id) {
                return Err(media_changed_during_transfer(title));
            }
            tx.execute(
                "UPDATE images SET project_id = ?1, branch_id = ?2, filename = ?3, session_id = ?4
                 WHERE id = ?5",
                params![
                    row.project_id,
                    row.branch_id,
                    row.filename,
                    row.session_id,
                    row.id
                ],
            )?;
            if let Some(displaced) = displaced.filter(|s| Some(s) != row.session_id.as_ref()) {
                tx.execute(
                    "DELETE FROM sessions
                     WHERE id = ?1
                       AND status != 'running'
                       AND NOT EXISTS (SELECT 1 FROM commits WHERE session_id = ?1)
                       AND NOT EXISTS (SELECT 1 FROM notes WHERE session_id = ?1)
                       AND NOT EXISTS (SELECT 1 FROM reviews WHERE session_id = ?1)
                       AND NOT EXISTS (SELECT 1 FROM project_notes WHERE session_id = ?1)
                       AND NOT EXISTS (SELECT 1 FROM images WHERE session_id = ?1)",
                    params![displaced],
                )?;
            }
        }
        Ok(())
    }
}

/// Media the note was taking along as-is is now used elsewhere (or gone), so
/// moving it would take it from under that user; a retry copies it instead.
fn media_changed_during_transfer(title: &str) -> StoreError {
    StoreError(format!(
        "Media in note '{title}' changed during the transfer; try again."
    ))
}

/// Re-home a note's session with the note. The caller checked the session
/// was settled before planning, but a resume can land between that read and
/// this write, and a live drain must not have its session handed to a branch
/// that is not running it — so the status is part of the key, and a live
/// session fails the whole move. A session row that no longer exists is not
/// a live one: the planner reads a missing session as settled, and so does
/// this, or a retry could never clear the error.
fn move_settled_session(
    tx: &Transaction<'_>,
    session_id: &str,
    target_branch_id: &str,
    note_title: &str,
) -> Result<(), StoreError> {
    let changed = tx.execute(
        "UPDATE sessions SET branch_id = ?1
         WHERE id = ?2 AND status NOT IN ('queued', 'running')",
        params![target_branch_id, session_id],
    )?;
    if changed == 1 {
        return Ok(());
    }
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
        params![session_id],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    Err(session_resumed_during_transfer(note_title))
}

/// The note's session went live again between the caller's check and the
/// transaction; the move has to wait for it.
fn session_resumed_during_transfer(title: &str) -> StoreError {
    StoreError(format!(
        "The session of '{title}' started again during the transfer; wait for it to finish before moving the note."
    ))
}

/// The note's body is no longer the one the caller read — a resumed session
/// finished again, or an edit landed — so the body rewritten from that read,
/// and the media staged from it, would overwrite the newer one. Unlike a live
/// session, nothing has to settle first: a retry reads the body afresh.
fn updated_during_transfer(title: &str) -> StoreError {
    StoreError(format!(
        "Note '{title}' was updated during the transfer; try again."
    ))
}

/// The note was not where the caller last saw it, so the destination it
/// resolved no longer applies.
fn relocated_during_transfer(title: &str) -> StoreError {
    StoreError(format!(
        "Note '{title}' was moved or deleted during the transfer."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::models::{Project, SessionStatus};

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
                &f.source_branch.id,
                &f.target_branch.id,
                "new body",
                std::slice::from_ref(&image),
                &[],
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

    /// The note's session is queued and drained by `sessions.branch_id`, so
    /// it has to arrive on the destination with the note.
    #[test]
    fn move_takes_the_notes_session_to_the_destination_branch() {
        let f = fixture();
        let session = crate::store::Session::new_running("write", std::path::Path::new("/tmp"))
            .with_branch(&f.source_branch.id);
        f.store.create_session(&session).unwrap();
        f.store
            .update_session_status(&session.id, SessionStatus::Completed, None, None)
            .unwrap();
        let note = Note::new(&f.source_branch.id, "Findings", "body").with_session(&session.id);
        f.store.create_note(&note).unwrap();

        f.store
            .move_note_to_branch(
                &note.id,
                &f.source_branch.id,
                &f.target_branch.id,
                "body",
                &[],
                &[],
            )
            .unwrap();

        assert_eq!(
            f.store.get_branch_id_for_session(&session.id).unwrap(),
            Some(f.target_branch.id.clone())
        );
    }

    /// The planner's status check and this write are separate reads: a session
    /// that went live again in between fails the move inside the transaction.
    #[test]
    fn move_is_refused_when_the_notes_session_resumed_after_planning() {
        let f = fixture();
        let session = crate::store::Session::new_running("write", std::path::Path::new("/tmp"))
            .with_branch(&f.source_branch.id);
        f.store.create_session(&session).unwrap();
        let note = Note::new(&f.source_branch.id, "Findings", "body").with_session(&session.id);
        f.store.create_note(&note).unwrap();
        let image = Image::new(
            Some(&f.target_branch.id),
            &f.target.id,
            "shot.png",
            "image/png",
            3,
            false,
        );

        for status in [SessionStatus::Queued, SessionStatus::Running] {
            f.store
                .update_session_status(&session.id, status, None, None)
                .unwrap();

            let err = f
                .store
                .move_note_to_branch(
                    &note.id,
                    &f.source_branch.id,
                    &f.target_branch.id,
                    "new body",
                    std::slice::from_ref(&image),
                    &[],
                )
                .unwrap_err();

            assert_eq!(
                err.to_string(),
                "The session of 'Findings' started again during the transfer; wait for it to finish before moving the note."
            );
            let stored = f.store.get_note(&note.id).unwrap().unwrap();
            assert_eq!(stored.branch_id, f.source_branch.id);
            assert_eq!(stored.content, "body");
            assert_eq!(
                f.store.get_branch_id_for_session(&session.id).unwrap(),
                Some(f.source_branch.id.clone())
            );
            assert!(f.store.get_image(&image.id).unwrap().is_none());
        }
    }

    /// A note whose session row is gone has nothing live to protect; the
    /// move goes ahead rather than failing on a session no retry can settle.
    #[test]
    fn move_proceeds_when_the_notes_session_row_is_gone() {
        let f = fixture();
        let note = Note::new(&f.source_branch.id, "Findings", "body").with_session("ghost");
        f.store.create_note(&note).unwrap();

        let moved = f
            .store
            .move_note_to_branch(
                &note.id,
                &f.source_branch.id,
                &f.target_branch.id,
                "body",
                &[],
                &[],
            )
            .unwrap();

        assert_eq!(moved.branch_id, f.target_branch.id);
        assert_eq!(moved.session_id.as_deref(), Some("ghost"));
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
            .move_note_to_branch(
                &note.id,
                &f.source_branch.id,
                &f.target_branch.id,
                "mine",
                &[],
                &[],
            )
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
                &f.source_branch.id,
                "no-such-branch",
                "body",
                std::slice::from_ref(&image),
                &[],
            )
            .unwrap_err();

        assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
        assert!(f.store.get_image(&image.id).unwrap().is_none());
        assert_eq!(
            f.store.get_note(&note.id).unwrap().unwrap().branch_id,
            f.source_branch.id
        );
    }

    /// The destination was resolved from where the note was; if it has since
    /// been relocated, the `UPDATE` matches nothing and the transaction — media
    /// rows included — is rolled back rather than overwriting the newer home.
    #[test]
    fn move_is_refused_when_the_note_is_no_longer_on_the_expected_branch() {
        let f = fixture();
        let note = Note::new_standalone(&f.source_branch.id, "Findings", "body");
        f.store.create_note(&note).unwrap();
        let image = Image::new(
            Some(&f.target_branch.id),
            &f.target.id,
            "shot.png",
            "image/png",
            3,
            false,
        );

        let err = f
            .store
            .move_note_to_branch(
                &note.id,
                "stale-branch",
                &f.target_branch.id,
                "new body",
                std::slice::from_ref(&image),
                &[],
            )
            .unwrap_err();

        assert!(err.to_string().contains("moved or deleted"), "{err}");
        let stored = f.store.get_note(&note.id).unwrap().unwrap();
        assert_eq!(stored.branch_id, f.source_branch.id);
        assert_eq!(stored.content, "body");
        assert!(f.store.get_image(&image.id).unwrap().is_none());
    }

    /// A child's plan for the move, with nothing to rewrite in its body.
    fn child_move(child: &Note, target_branch_id: &str) -> ChildNoteMove {
        ChildNoteMove {
            note_id: child.id.clone(),
            expected_branch_id: child.branch_id.clone(),
            expected_content: child.content.clone(),
            target_branch_id: target_branch_id.to_string(),
            content: child.content.clone(),
            media: Vec::new(),
            rehomed: Vec::new(),
        }
    }

    #[test]
    fn project_note_move_is_refused_when_the_note_left_the_expected_project() {
        let f = fixture();
        let note = ProjectNote::new(&f.source.id, "Plan", "body");
        f.store.create_project_note(&note).unwrap();
        let session = crate::store::Session::new_running("child", std::path::Path::new("/tmp"))
            .with_branch(&f.source_branch.id);
        f.store.create_session(&session).unwrap();
        f.store
            .update_session_status(&session.id, SessionStatus::Completed, None, None)
            .unwrap();
        let child = Note::new_standalone(&f.source_branch.id, "Child", "body")
            .with_parent_project_note(&note.id)
            .with_session(&session.id);
        f.store.create_note(&child).unwrap();
        let image = Image::new(None, &f.target.id, "shot.png", "image/png", 3, false);

        let err = f
            .store
            .move_project_note_to_project(
                &note.id,
                "stale-project",
                &note.content,
                &f.target.id,
                "new body",
                std::slice::from_ref(&image),
                &[],
                &[child_move(&child, &f.target_branch.id)],
            )
            .unwrap_err();

        assert!(err.to_string().contains("moved or deleted"), "{err}");
        let stored = f.store.get_project_note(&note.id).unwrap().unwrap();
        assert_eq!(stored.project_id, f.source.id);
        assert_eq!(stored.content, "body");
        // The child's move ran inside the same transaction and was undone:
        // it is still on its branch, still linked, and so is its session.
        let child = f.store.get_note(&child.id).unwrap().unwrap();
        assert_eq!(child.branch_id, f.source_branch.id);
        assert_eq!(
            child.parent_project_note_id.as_deref(),
            Some(note.id.as_str())
        );
        assert_eq!(
            f.store.get_branch_id_for_session(&session.id).unwrap(),
            Some(f.source_branch.id.clone())
        );
        assert!(f.store.get_image(&image.id).unwrap().is_none());
    }

    /// A child whose session went live again after the plan was made — the
    /// planner's status check and this write are separate reads — fails the
    /// move inside the transaction, and nothing of the family changes.
    #[test]
    fn project_note_move_is_refused_when_a_child_session_resumed_after_planning() {
        let f = fixture();
        let note = ProjectNote::new(&f.source.id, "Plan", "body");
        f.store.create_project_note(&note).unwrap();
        let session = crate::store::Session::new_running("child", std::path::Path::new("/tmp"))
            .with_branch(&f.source_branch.id);
        f.store.create_session(&session).unwrap();
        f.store
            .update_session_status(&session.id, SessionStatus::Completed, None, None)
            .unwrap();
        let child = Note::new_standalone(&f.source_branch.id, "Child", "body")
            .with_parent_project_note(&note.id)
            .with_session(&session.id);
        f.store.create_note(&child).unwrap();
        let plan = child_move(&child, &f.target_branch.id);
        let image = Image::new(None, &f.target.id, "shot.png", "image/png", 3, false);

        for status in [SessionStatus::Queued, SessionStatus::Running] {
            f.store
                .update_session_status(&session.id, status, None, None)
                .unwrap();

            let err = f
                .store
                .move_project_note_to_project(
                    &note.id,
                    &f.source.id,
                    &note.content,
                    &f.target.id,
                    "new body",
                    std::slice::from_ref(&image),
                    &[],
                    std::slice::from_ref(&plan),
                )
                .unwrap_err();

            assert_eq!(
                err.to_string(),
                "The session of 'Child' started again during the transfer; wait for it to finish before moving the note."
            );
            let stored = f.store.get_project_note(&note.id).unwrap().unwrap();
            assert_eq!(stored.project_id, f.source.id);
            assert_eq!(stored.content, "body");
            let child = f.store.get_note(&child.id).unwrap().unwrap();
            assert_eq!(child.branch_id, f.source_branch.id);
            assert_eq!(child.content, "body");
            assert_eq!(
                child.parent_project_note_id.as_deref(),
                Some(note.id.as_str())
            );
            assert_eq!(
                f.store.get_branch_id_for_session(&session.id).unwrap(),
                Some(f.source_branch.id.clone())
            );
            assert!(f.store.get_image(&image.id).unwrap().is_none());
        }
    }

    /// The parent's own session, resumed after the plan was made, fails the
    /// move inside the transaction like a child's would.
    #[test]
    fn project_note_move_is_refused_when_the_parent_session_resumed_after_planning() {
        let f = fixture();
        let parent_session =
            crate::store::Session::new_running("plan", std::path::Path::new("/tmp"));
        f.store.create_session(&parent_session).unwrap();
        f.store
            .update_session_status(&parent_session.id, SessionStatus::Completed, None, None)
            .unwrap();
        let note = ProjectNote::new(&f.source.id, "Plan", "body").with_session(&parent_session.id);
        f.store.create_project_note(&note).unwrap();
        let session = crate::store::Session::new_running("child", std::path::Path::new("/tmp"))
            .with_branch(&f.source_branch.id);
        f.store.create_session(&session).unwrap();
        f.store
            .update_session_status(&session.id, SessionStatus::Completed, None, None)
            .unwrap();
        let child = Note::new_standalone(&f.source_branch.id, "Child", "body")
            .with_parent_project_note(&note.id)
            .with_session(&session.id);
        f.store.create_note(&child).unwrap();
        let plan = child_move(&child, &f.target_branch.id);
        let image = Image::new(None, &f.target.id, "shot.png", "image/png", 3, false);

        for status in [SessionStatus::Queued, SessionStatus::Running] {
            f.store
                .update_session_status(&parent_session.id, status, None, None)
                .unwrap();

            let err = f
                .store
                .move_project_note_to_project(
                    &note.id,
                    &f.source.id,
                    &note.content,
                    &f.target.id,
                    "new body",
                    std::slice::from_ref(&image),
                    &[],
                    std::slice::from_ref(&plan),
                )
                .unwrap_err();

            assert_eq!(
                err.to_string(),
                "The session of 'Plan' started again during the transfer; wait for it to finish before moving the note."
            );
            let stored = f.store.get_project_note(&note.id).unwrap().unwrap();
            assert_eq!(stored.project_id, f.source.id);
            assert_eq!(stored.content, "body");
            // The child's move ran first in the same transaction and rolled
            // back with it.
            let child = f.store.get_note(&child.id).unwrap().unwrap();
            assert_eq!(child.branch_id, f.source_branch.id);
            assert_eq!(
                f.store.get_branch_id_for_session(&session.id).unwrap(),
                Some(f.source_branch.id.clone())
            );
            assert!(f.store.get_image(&image.id).unwrap().is_none());
        }
    }

    /// A child whose session row is gone moves like one whose session is
    /// settled.
    #[test]
    fn project_note_move_proceeds_when_a_child_session_row_is_gone() {
        let f = fixture();
        let note = ProjectNote::new(&f.source.id, "Plan", "body");
        f.store.create_project_note(&note).unwrap();
        let child = Note::new_standalone(&f.source_branch.id, "Child", "body")
            .with_parent_project_note(&note.id)
            .with_session("ghost");
        f.store.create_note(&child).unwrap();

        let moved = f
            .store
            .move_project_note_to_project(
                &note.id,
                &f.source.id,
                &note.content,
                &f.target.id,
                "body",
                &[],
                &[],
                &[child_move(&child, &f.target_branch.id)],
            )
            .unwrap();

        assert_eq!(moved.project_id, f.target.id);
        let child = f.store.get_note(&child.id).unwrap().unwrap();
        assert_eq!(child.branch_id, f.target_branch.id);
        assert_eq!(child.session_id.as_deref(), Some("ghost"));
    }

    /// The mirror image: a child that is no longer where the plan saw it
    /// fails the move, and the parent stays put with it.
    #[test]
    fn project_note_move_is_refused_when_a_child_left_its_expected_branch() {
        let f = fixture();
        let note = ProjectNote::new(&f.source.id, "Plan", "body");
        f.store.create_project_note(&note).unwrap();
        let child = Note::new_standalone(&f.source_branch.id, "Child", "body")
            .with_parent_project_note(&note.id);
        f.store.create_note(&child).unwrap();
        let image = Image::new(
            Some(&f.target_branch.id),
            &f.target.id,
            "shot.png",
            "image/png",
            3,
            false,
        );

        let err = f
            .store
            .move_project_note_to_project(
                &note.id,
                &f.source.id,
                &note.content,
                &f.target.id,
                "body",
                &[],
                &[],
                &[ChildNoteMove {
                    expected_branch_id: "stale-branch".to_string(),
                    media: vec![image.clone()],
                    ..child_move(&child, &f.target_branch.id)
                }],
            )
            .unwrap_err();

        assert!(err.to_string().contains("moved or deleted"), "{err}");
        assert_eq!(
            f.store
                .get_project_note(&note.id)
                .unwrap()
                .unwrap()
                .project_id,
            f.source.id
        );
        assert_eq!(
            f.store.get_note(&child.id).unwrap().unwrap().branch_id,
            f.source_branch.id
        );
        assert!(f.store.get_image(&image.id).unwrap().is_none());
    }

    /// A child the plan does not cover would be left linked to a parent in
    /// another project — the cross-project cascade the move exists to avoid.
    #[test]
    fn project_note_move_is_refused_when_a_child_was_not_planned_for() {
        let f = fixture();
        let note = ProjectNote::new(&f.source.id, "Plan", "body");
        f.store.create_project_note(&note).unwrap();
        let planned = Note::new_standalone(&f.source_branch.id, "Planned", "body")
            .with_parent_project_note(&note.id);
        let unplanned = Note::new_standalone(&f.source_branch.id, "Unplanned", "body")
            .with_parent_project_note(&note.id);
        f.store.create_note(&planned).unwrap();
        f.store.create_note(&unplanned).unwrap();

        let err = f
            .store
            .move_project_note_to_project(
                &note.id,
                &f.source.id,
                &note.content,
                &f.target.id,
                "body",
                &[],
                &[],
                &[child_move(&planned, &f.target_branch.id)],
            )
            .unwrap_err();

        assert!(err.to_string().contains("changed during"), "{err}");
        assert_eq!(
            f.store
                .get_project_note(&note.id)
                .unwrap()
                .unwrap()
                .project_id,
            f.source.id
        );
        assert_eq!(
            f.store.get_note(&planned.id).unwrap().unwrap().branch_id,
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
    fn project_note_move_takes_its_children_along_and_publishes_both_branches() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(64);
        let f = fixture_in(Store::in_memory().unwrap().with_change_sender(tx));
        let parent = ProjectNote::new(&f.source.id, "Parent", "body");
        f.store.create_project_note(&parent).unwrap();
        let session = crate::store::Session::new_running("child", std::path::Path::new("/tmp"))
            .with_branch(&f.source_branch.id);
        f.store.create_session(&session).unwrap();
        f.store
            .update_session_status(&session.id, SessionStatus::Completed, None, None)
            .unwrap();
        let child = Note::new_standalone(&f.source_branch.id, "Child", "body")
            .with_parent_project_note(&parent.id)
            .with_session(&session.id);
        f.store.create_note(&child).unwrap();
        // A note by the same title already on the destination branch.
        let taken = Note::new_standalone(&f.target_branch.id, "Child", "theirs");
        f.store.create_note(&taken).unwrap();
        let image = Image::new(
            Some(&f.target_branch.id),
            &f.target.id,
            "shot.png",
            "image/png",
            3,
            false,
        );
        while rx.try_recv().is_ok() {}

        let moved = f
            .store
            .move_project_note_to_project(
                &parent.id,
                &f.source.id,
                &parent.content,
                &f.target.id,
                "body",
                &[],
                &[],
                &[ChildNoteMove {
                    content: "child body, rewritten".to_string(),
                    media: vec![image.clone()],
                    ..child_move(&child, &f.target_branch.id)
                }],
            )
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
        assert_eq!(child.branch_id, f.target_branch.id);
        assert_eq!(
            child.parent_project_note_id.as_deref(),
            Some(parent.id.as_str())
        );
        assert_eq!(child.title, "Child (2)");
        assert_eq!(child.content, "child body, rewritten");
        assert_eq!(
            f.store.get_branch_id_for_session(&session.id).unwrap(),
            Some(f.target_branch.id.clone())
        );
        assert!(f.store.get_image(&image.id).unwrap().is_some());
        assert!(f.store.list_project_notes(&f.source.id).unwrap().is_empty());
        // Still a child: listed under the parent, hidden from the timeline.
        assert_eq!(
            f.store
                .list_child_notes(&parent.id)
                .unwrap()
                .iter()
                .map(|n| n.id.as_str())
                .collect::<Vec<_>>(),
            [child.id.as_str()]
        );
        assert_eq!(
            f.store
                .list_notes_for_branch(&f.target_branch.id)
                .unwrap()
                .len(),
            1
        );

        let project_change = |project_id: &str| StoreChange::Notes {
            branch_id: None,
            project_id: Some(project_id.to_string()),
        };
        let branch_change = |branch_id: &str| StoreChange::Notes {
            branch_id: Some(branch_id.to_string()),
            project_id: None,
        };
        assert_eq!(rx.try_recv().unwrap(), project_change(&f.source.id));
        assert_eq!(rx.try_recv().unwrap(), project_change(&f.target.id));
        assert_eq!(rx.try_recv().unwrap(), branch_change(&f.source_branch.id));
        assert_eq!(rx.try_recv().unwrap(), branch_change(&f.target_branch.id));
        assert!(rx.try_recv().is_err());
    }

    /// A childless move publishes only the two project surfaces.
    #[test]
    fn project_note_move_without_children_touches_no_branch() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(64);
        let f = fixture_in(Store::in_memory().unwrap().with_change_sender(tx));
        let parent = ProjectNote::new(&f.source.id, "Parent", "body");
        f.store.create_project_note(&parent).unwrap();
        while rx.try_recv().is_ok() {}

        f.store
            .move_project_note_to_project(
                &parent.id,
                &f.source.id,
                &parent.content,
                &f.target.id,
                "body",
                &[],
                &[],
                &[],
            )
            .unwrap();

        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_ok());
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

    /// Media attached on the source branch under `session_id`.
    fn source_media(f: &Fixture, session_id: &str) -> Image {
        let mut image = Image::new(
            Some(&f.source_branch.id),
            &f.source.id,
            "shot.png",
            "image/png",
            3,
            false,
        );
        image.session_id = Some(session_id.to_string());
        f.store.create_image(&image).unwrap();
        image
    }

    /// The row as a move re-homes it onto the target branch, owned by
    /// `session_id`.
    fn rehomed_onto(f: &Fixture, image: &Image, filename: &str, session_id: &str) -> Image {
        Image {
            project_id: f.target.id.clone(),
            branch_id: Some(f.target_branch.id.clone()),
            filename: filename.to_string(),
            session_id: Some(session_id.to_string()),
            ..image.clone()
        }
    }

    fn completed_session(f: &Fixture) -> crate::store::Session {
        let session = crate::store::Session::new_running("note", std::path::Path::new("/tmp"));
        f.store.create_session(&session).unwrap();
        f.store
            .update_session_status(
                &session.id,
                crate::store::SessionStatus::Completed,
                None,
                None,
            )
            .unwrap();
        session
    }

    /// A re-homed row is updated in place: same id, the destination's scope
    /// and filename, and the moved note's session. The session it named
    /// before, now named by nothing, is collected as a delete of the row
    /// would have collected it.
    #[test]
    fn move_rehomes_media_in_place_and_collects_the_session_it_leaves() {
        let f = fixture();
        let owner = completed_session(&f);
        let previous = completed_session(&f);
        let image = source_media(&f, &previous.id);
        let content = format!("![a](staged-media://{}.png)", image.id);
        let note =
            Note::new_standalone(&f.source_branch.id, "Shots", &content).with_session(&owner.id);
        f.store.create_note(&note).unwrap();

        f.store
            .move_note_to_branch(
                &note.id,
                &f.source_branch.id,
                &f.target_branch.id,
                &content,
                &[],
                &[rehomed_onto(&f, &image, "shot 2.png", &owner.id)],
            )
            .unwrap();

        let row = f.store.get_image(&image.id).unwrap().unwrap();
        assert_eq!(row.project_id, f.target.id);
        assert_eq!(row.branch_id.as_ref(), Some(&f.target_branch.id));
        assert_eq!(row.filename, "shot 2.png");
        assert_eq!(row.session_id.as_ref(), Some(&owner.id));
        assert!(f.store.get_session(&previous.id).unwrap().is_none());
        assert!(f.store.get_session(&owner.id).unwrap().is_some());
    }

    /// Whether a row is the note's alone is settled under the transaction:
    /// one another note or a chat message picked up after the caller looked
    /// is not taken from under it, and nothing about the move is written.
    #[test]
    fn move_is_refused_when_media_it_would_rehome_gained_another_user() {
        for other_user in ["note", "chat"] {
            let f = fixture();
            let session = completed_session(&f);
            let image = source_media(&f, &session.id);
            let content = format!("![a](staged-media://{}.png)", image.id);
            let note = Note::new_standalone(&f.source_branch.id, "Shots", &content)
                .with_session(&session.id);
            f.store.create_note(&note).unwrap();
            match other_user {
                "note" => f
                    .store
                    .create_note(&Note::new_standalone(
                        &f.source_branch.id,
                        "Other",
                        &content,
                    ))
                    .unwrap(),
                _ => {
                    f.store
                        .add_session_message_with_images(
                            &session.id,
                            crate::store::models::MessageRole::User,
                            "look",
                            std::slice::from_ref(&image.id),
                        )
                        .unwrap();
                }
            }

            let err = f
                .store
                .move_note_to_branch(
                    &note.id,
                    &f.source_branch.id,
                    &f.target_branch.id,
                    &content,
                    &[],
                    &[rehomed_onto(&f, &image, "shot.png", &session.id)],
                )
                .unwrap_err();

            assert!(
                err.to_string().contains("changed during the transfer"),
                "{other_user}: {err}"
            );
            let row = f.store.get_image(&image.id).unwrap().unwrap();
            assert_eq!(row.project_id, f.source.id, "{other_user}");
            assert_eq!(row.branch_id.as_ref(), Some(&f.source_branch.id));
            assert_eq!(
                f.store.get_note(&note.id).unwrap().unwrap().branch_id,
                f.source_branch.id
            );
        }
    }

    /// The check under the transaction reads what uses the rows once, for
    /// all of them, and a move with nothing to re-home reads nothing.
    #[test]
    fn move_confirms_every_rehomed_row_in_one_pass() {
        use crate::store::images::{take_reference_passes, ReferencePasses};

        let f = fixture();
        let session = completed_session(&f);
        let images: Vec<Image> = (0..3).map(|_| source_media(&f, &session.id)).collect();
        let content = images
            .iter()
            .map(|image| format!("![a](staged-media://{}.png)", image.id))
            .collect::<Vec<_>>()
            .join("\n");
        let note =
            Note::new_standalone(&f.source_branch.id, "Shots", &content).with_session(&session.id);
        f.store.create_note(&note).unwrap();
        let rehomed: Vec<Image> = images
            .iter()
            .enumerate()
            .map(|(n, image)| rehomed_onto(&f, image, &format!("shot {n}.png"), &session.id))
            .collect();

        take_reference_passes();
        f.store
            .move_note_to_branch(
                &note.id,
                &f.source_branch.id,
                &f.target_branch.id,
                &content,
                &[],
                &rehomed,
            )
            .unwrap();
        assert_eq!(
            take_reference_passes(),
            ReferencePasses { notes: 1, chat: 1 }
        );
        for image in &images {
            let row = f.store.get_image(&image.id).unwrap().unwrap();
            assert_eq!(row.project_id, f.target.id);
        }

        f.store
            .move_note_to_branch(
                &note.id,
                &f.target_branch.id,
                &f.source_branch.id,
                &content,
                &[],
                &[],
            )
            .unwrap();
        assert_eq!(take_reference_passes(), ReferencePasses::default());
    }

    /// The one read covers every row: whichever of several gained another
    /// user, the move is refused and none of them is written.
    #[test]
    fn move_is_refused_when_any_of_several_rehomed_rows_gained_another_user() {
        for (taken, other_user) in [(0, "note"), (2, "note"), (0, "chat"), (2, "chat")] {
            let f = fixture();
            let session = completed_session(&f);
            let images: Vec<Image> = (0..3).map(|_| source_media(&f, &session.id)).collect();
            let embed = |image: &Image| format!("![a](staged-media://{}.png)", image.id);
            let content = images.iter().map(embed).collect::<Vec<_>>().join("\n");
            let note = Note::new_standalone(&f.source_branch.id, "Shots", &content)
                .with_session(&session.id);
            f.store.create_note(&note).unwrap();
            match other_user {
                "note" => f
                    .store
                    .create_note(&Note::new_standalone(
                        &f.source_branch.id,
                        "Other",
                        &embed(&images[taken]),
                    ))
                    .unwrap(),
                _ => {
                    f.store
                        .add_session_message_with_images(
                            &session.id,
                            crate::store::models::MessageRole::User,
                            "look",
                            std::slice::from_ref(&images[taken].id),
                        )
                        .unwrap();
                }
            }
            // What the per-image check says about each row decides the move.
            for (n, image) in images.iter().enumerate() {
                assert_eq!(
                    f.store.media_is_exclusive_to(&image.id, &note.id).unwrap(),
                    n != taken,
                    "{other_user} {taken}: row {n}"
                );
            }
            let rehomed: Vec<Image> = images
                .iter()
                .enumerate()
                .map(|(n, image)| rehomed_onto(&f, image, &format!("shot {n}.png"), &session.id))
                .collect();

            let err = f
                .store
                .move_note_to_branch(
                    &note.id,
                    &f.source_branch.id,
                    &f.target_branch.id,
                    &content,
                    &[],
                    &rehomed,
                )
                .unwrap_err();

            assert!(
                err.to_string().contains("changed during the transfer"),
                "{other_user} {taken}: {err}"
            );
            for image in &images {
                let row = f.store.get_image(&image.id).unwrap().unwrap();
                assert_eq!(row.project_id, f.source.id, "{other_user} {taken}");
            }
            assert_eq!(
                f.store.get_note(&note.id).unwrap().unwrap().branch_id,
                f.source_branch.id
            );
        }
    }

    #[test]
    fn project_note_move_rehomes_its_media() {
        let f = fixture();
        let session = completed_session(&f);
        let mut image = Image::new(None, &f.source.id, "shot.png", "image/png", 3, false);
        image.session_id = Some(session.id.clone());
        f.store.create_image(&image).unwrap();
        let content = format!("![a](staged-media://{}.png)", image.id);
        let note = ProjectNote::new(&f.source.id, "Plan", &content);
        f.store.create_project_note(&note).unwrap();

        f.store
            .move_project_note_to_project(
                &note.id,
                &f.source.id,
                &note.content,
                &f.target.id,
                &content,
                &[],
                &[Image {
                    project_id: f.target.id.clone(),
                    ..image.clone()
                }],
                &[],
            )
            .unwrap();

        let row = f.store.get_image(&image.id).unwrap().unwrap();
        assert_eq!(row.project_id, f.target.id);
        assert!(row.branch_id.is_none());
    }

    /// A parent and one child, each with a settled session and a media row
    /// only it uses, and the plan a move would make for them: both bodies
    /// rewritten, each note's own row re-homed and one more row copied in.
    struct PlannedFamily {
        note: ProjectNote,
        parent_session: crate::store::Session,
        parent_image: Image,
        child: Note,
        child_session: crate::store::Session,
        child_image: Image,
        content: String,
        media: Vec<Image>,
        rehomed: Vec<Image>,
        plan: ChildNoteMove,
    }

    fn planned_family(f: &Fixture) -> PlannedFamily {
        let parent_session = completed_session(f);
        let mut parent_image = Image::new(None, &f.source.id, "plan.png", "image/png", 3, false);
        parent_image.session_id = Some(parent_session.id.clone());
        f.store.create_image(&parent_image).unwrap();
        let note = ProjectNote::new(
            &f.source.id,
            "Plan",
            &format!("![a](staged-media://{}.png)", parent_image.id),
        )
        .with_session(&parent_session.id);
        f.store.create_project_note(&note).unwrap();

        let child_session =
            crate::store::Session::new_running("child", std::path::Path::new("/tmp"))
                .with_branch(&f.source_branch.id);
        f.store.create_session(&child_session).unwrap();
        f.store
            .update_session_status(&child_session.id, SessionStatus::Completed, None, None)
            .unwrap();
        let child_image = source_media(f, &child_session.id);
        let child = Note::new_standalone(
            &f.source_branch.id,
            "Child",
            &format!("![a](staged-media://{}.png)", child_image.id),
        )
        .with_parent_project_note(&note.id)
        .with_session(&child_session.id);
        f.store.create_note(&child).unwrap();

        let plan = ChildNoteMove {
            content: format!("{}, rewritten", child.content),
            media: vec![Image::new(
                Some(&f.target_branch.id),
                &f.target.id,
                "child copy.png",
                "image/png",
                3,
                false,
            )],
            rehomed: vec![rehomed_onto(f, &child_image, "shot.png", &child_session.id)],
            ..child_move(&child, &f.target_branch.id)
        };
        PlannedFamily {
            content: format!("{}, rewritten", note.content),
            media: vec![Image::new(
                None,
                &f.target.id,
                "plan copy.png",
                "image/png",
                3,
                false,
            )],
            rehomed: vec![Image {
                project_id: f.target.id.clone(),
                ..parent_image.clone()
            }],
            note,
            parent_session,
            parent_image,
            child,
            child_session,
            child_image,
            plan,
        }
    }

    impl PlannedFamily {
        fn move_as_planned(&self, f: &Fixture) -> Result<ProjectNote, StoreError> {
            f.store.move_project_note_to_project(
                &self.note.id,
                &f.source.id,
                &self.note.content,
                &f.target.id,
                &self.content,
                &self.media,
                &self.rehomed,
                std::slice::from_ref(&self.plan),
            )
        }

        /// Nothing the move wrote survived: both notes are where they were,
        /// still linked, the child's session is still scheduled on its
        /// branch, the re-homed rows are back in the source and the copied
        /// rows are gone.
        fn assert_rolled_back(&self, f: &Fixture) {
            let parent = f.store.get_project_note(&self.note.id).unwrap().unwrap();
            assert_eq!(parent.project_id, f.source.id);
            let child = f.store.get_note(&self.child.id).unwrap().unwrap();
            assert_eq!(child.branch_id, f.source_branch.id);
            assert_eq!(
                child.parent_project_note_id.as_deref(),
                Some(self.note.id.as_str())
            );
            assert_eq!(
                f.store
                    .get_branch_id_for_session(&self.child_session.id)
                    .unwrap(),
                Some(f.source_branch.id.clone())
            );
            let parent_row = f.store.get_image(&self.parent_image.id).unwrap().unwrap();
            assert_eq!(parent_row.project_id, f.source.id);
            assert!(parent_row.branch_id.is_none());
            let child_row = f.store.get_image(&self.child_image.id).unwrap().unwrap();
            assert_eq!(child_row.project_id, f.source.id);
            assert_eq!(child_row.branch_id.as_ref(), Some(&f.source_branch.id));
            assert_eq!(child_row.filename, self.child_image.filename);
            for copied in self.media.iter().chain(&self.plan.media) {
                assert!(f.store.get_image(&copied.id).unwrap().is_none());
            }
        }
    }

    /// A child whose session was resumed and finished again after the plan
    /// was made reads as settled — only its body shows the run — and the
    /// planned body was rewritten from the one before it. The move is
    /// refused rather than overwriting that run, and nothing of the family
    /// or its media changes; a plan made from the new body goes through.
    #[test]
    fn project_note_move_is_refused_when_a_child_body_changed_after_planning() {
        let f = fixture();
        let family = planned_family(&f);
        let newer = format!("{}\n\nMore findings.", family.child.content);
        f.store
            .update_session_status(&family.child_session.id, SessionStatus::Running, None, None)
            .unwrap();
        f.store
            .update_note_content(&family.child.id, &newer)
            .unwrap();
        f.store
            .update_session_status(
                &family.child_session.id,
                SessionStatus::Completed,
                None,
                None,
            )
            .unwrap();

        let err = family.move_as_planned(&f).unwrap_err();

        assert_eq!(
            err.to_string(),
            "Note 'Child' was updated during the transfer; try again."
        );
        family.assert_rolled_back(&f);
        assert_eq!(
            f.store.get_note(&family.child.id).unwrap().unwrap().content,
            newer
        );
        assert_eq!(
            f.store
                .get_project_note(&family.note.id)
                .unwrap()
                .unwrap()
                .content,
            family.note.content
        );

        // The retry reads the child again and plans from what it reads now.
        let replanned = PlannedFamily {
            plan: ChildNoteMove {
                expected_content: newer.clone(),
                content: format!("{newer}, rewritten"),
                ..family.plan.clone()
            },
            ..family
        };
        replanned.move_as_planned(&f).unwrap();
        let child = f.store.get_note(&replanned.child.id).unwrap().unwrap();
        assert_eq!(child.branch_id, f.target_branch.id);
        assert_eq!(child.content, format!("{newer}, rewritten"));
    }

    /// The parent's body is keyed on the same way. A parent whose own session
    /// was resumed and finished again since the plan was made is refused with
    /// the family and its media untouched; one whose session is live again
    /// too is told to wait for it, since a retry would only stop there.
    #[test]
    fn project_note_move_is_refused_when_the_parent_body_changed_after_planning() {
        let f = fixture();
        let family = planned_family(&f);
        let newer = format!("{}\n\nNext steps.", family.note.content);
        f.store
            .update_session_status(
                &family.parent_session.id,
                SessionStatus::Running,
                None,
                None,
            )
            .unwrap();
        f.store
            .update_project_note_title_and_content(&family.note.id, "Plan", &newer, None, None)
            .unwrap();
        f.store
            .update_session_status(
                &family.parent_session.id,
                SessionStatus::Completed,
                None,
                None,
            )
            .unwrap();

        let err = family.move_as_planned(&f).unwrap_err();

        assert_eq!(
            err.to_string(),
            "Note 'Plan' was updated during the transfer; try again."
        );
        family.assert_rolled_back(&f);
        assert_eq!(
            f.store
                .get_project_note(&family.note.id)
                .unwrap()
                .unwrap()
                .content,
            newer
        );
        assert_eq!(
            f.store.get_note(&family.child.id).unwrap().unwrap().content,
            family.child.content
        );

        f.store
            .update_session_status(
                &family.parent_session.id,
                SessionStatus::Running,
                None,
                None,
            )
            .unwrap();
        let err = family.move_as_planned(&f).unwrap_err();
        assert_eq!(
            err.to_string(),
            "The session of 'Plan' started again during the transfer; wait for it to finish before moving the note."
        );
        family.assert_rolled_back(&f);
    }
}
