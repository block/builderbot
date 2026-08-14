//! Re-parenting a branch into a different project.
//!
//! Everything else a branch owns — commits, notes, reviews, sessions and their
//! messages, comments, reviewed files — hangs off `branch_id` and follows the
//! branch for free. Only four things carry a `project_id` of their own and have
//! to be rewritten in step with it: the branch row, its `project_repos` row,
//! its `workdirs` row, and its images. That rewrite is what lives here, in one
//! transaction, as `UPDATE`s and one `INSERT` only — the `AFTER DELETE`
//! triggers on the artifact tables garbage-collect sessions, so a
//! delete-and-reinsert would take transcripts with it.

use rusqlite::{params, OptionalExtension};

use super::models::ProjectRepo;
use super::{now_timestamp, Store, StoreError};

/// Which `project_repos` row the moved branch points at once it lands.
///
/// The relationship is N:1 — sibling branches can share one row — so the row
/// only travels when the moved branch is the last one on it.
#[derive(Debug, Clone)]
pub enum RepoPlacement {
    /// Carry the branch's own row into the destination project.
    Reparent { repo_id: String },
    /// Insert this clone in the destination and leave the source row for the
    /// siblings still pointing at it.
    Clone(ProjectRepo),
}

impl RepoPlacement {
    fn repo_id(&self) -> &str {
        match self {
            Self::Reparent { repo_id } => repo_id,
            Self::Clone(repo) => &repo.id,
        }
    }
}

/// The worktree relocation half of a move: which `workdirs` row to rewrite and
/// where to. Absent for a branch whose worktree was never set up.
#[derive(Debug, Clone)]
pub struct WorkdirMove {
    pub workdir_id: String,
    pub old_path: String,
    pub new_path: String,
}

/// Everything [`Store::move_branch_to_project`] rewrites, resolved by the
/// caller before any of it is applied.
#[derive(Debug, Clone)]
pub struct BranchMove {
    pub branch_id: String,
    pub source_project_id: String,
    pub target_project_id: String,
    pub repo: RepoPlacement,
    pub workdir: Option<WorkdirMove>,
}

impl Store {
    /// Move a branch into another project, rewriting every `project_id` that
    /// points at the old one.
    ///
    /// The `branches(project_id, project_repo_id, branch_name)` and
    /// `workdirs(project_id, path)` unique indexes are the backstop against a
    /// destination that already holds this branch, so violations surface as
    /// user-readable errors rather than raw SQLite text.
    pub fn move_branch_to_project(&self, mv: &BranchMove) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = now_timestamp();

        match &mv.repo {
            RepoPlacement::Reparent { repo_id } => {
                // Primary is re-elected per project below, so hand the row over
                // unelected and let the destination decide.
                tx.execute(
                    "UPDATE project_repos SET project_id = ?1, is_primary = 0, updated_at = ?2
                     WHERE id = ?3",
                    params![mv.target_project_id, now, repo_id],
                )
                .map_err(|e| duplicate_repo_error(&e))?;
            }
            RepoPlacement::Clone(repo) => {
                tx.execute(
                    "INSERT INTO project_repos (id, project_id, github_repo, branch_name, subpath,
                        is_primary, reason, head_repo, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9)",
                    params![
                        repo.id,
                        mv.target_project_id,
                        repo.github_repo,
                        repo.branch_name,
                        repo.subpath,
                        repo.reason,
                        repo.head_repo,
                        repo.created_at,
                        now,
                    ],
                )
                .map_err(|e| duplicate_repo_error(&e))?;
            }
        }

        tx.execute(
            "UPDATE branches SET project_id = ?1, project_repo_id = ?2, updated_at = ?3
             WHERE id = ?4",
            params![mv.target_project_id, mv.repo.repo_id(), now, mv.branch_id],
        )
        .map_err(|e| duplicate_branch_error(&e))?;

        if let Some(wd) = &mv.workdir {
            tx.execute(
                "UPDATE workdirs SET project_id = ?1, path = ?2, updated_at = ?3 WHERE id = ?4",
                params![mv.target_project_id, wd.new_path, now, wd.workdir_id],
            )
            .map_err(|e| duplicate_workdir_error(&e))?;

            // `sessions.working_dir` is an absolute-path snapshot taken when the
            // session started, so it has to be rewritten by hand. Matching on
            // the old worktree path rather than on the branch's artifacts is
            // both narrower and wider in the right ways: a path under this
            // worktree belongs to this branch and nothing else, and it catches
            // sessions whose link to the branch runs through something this
            // query would otherwise have to enumerate. The suffix is preserved
            // so a session rooted at a repo subpath keeps it, separator and
            // all — `substr` starts at the separator character itself.
            let (old_slash, old_backslash) = session_dir_prefixes(&wd.old_path, cfg!(windows));
            tx.execute(
                "UPDATE sessions
                    SET working_dir = ?1 || substr(working_dir, ?2), updated_at = ?3
                  WHERE working_dir = ?4
                     OR instr(working_dir, ?5) = 1
                     OR instr(working_dir, ?6) = 1",
                params![
                    wd.new_path,
                    wd.old_path.chars().count() as i64 + 1,
                    now,
                    wd.old_path,
                    old_slash,
                    old_backslash,
                ],
            )?;
        }

        // Branch-scoped images only: an image with no `branch_id` belongs to a
        // project note and stays where it is.
        tx.execute(
            "UPDATE images SET project_id = ?1 WHERE branch_id = ?2",
            params![mv.target_project_id, mv.branch_id],
        )?;

        elect_primary_repo(&tx, &mv.source_project_id, now)?;
        elect_primary_repo(&tx, &mv.target_project_id, now)?;

        tx.commit()?;
        Ok(())
    }
}

/// The prefixes a `sessions.working_dir` can start with when it lives under
/// `old_path`. Working dirs are stored via `Path::to_string_lossy`, so on
/// Windows the separator `PathBuf::join` inserts after the worktree root is
/// `\`, while a path that arrived as a string can still use `/`; both have to
/// match. On Unix the pair collapses to `/` alone: a backslash is an ordinary
/// filename character there, and matching it would move a sibling directory
/// that merely has one in its name.
fn session_dir_prefixes(old_path: &str, windows: bool) -> (String, String) {
    let slash = format!("{old_path}/");
    let backslash = if windows {
        format!("{old_path}\\")
    } else {
        slash.clone()
    };
    (slash, backslash)
}

/// Make sure a project has exactly one primary repo and that
/// `projects.github_repo`/`subpath` still denormalize it.
///
/// Same shape as `remove_project_repo`'s re-election: the sitting primary keeps
/// the job, a project that just lost its primary promotes its oldest remaining
/// repo, and a project with no repos left denormalizes to NULL.
fn elect_primary_repo(
    tx: &rusqlite::Transaction<'_>,
    project_id: &str,
    now: i64,
) -> Result<(), StoreError> {
    let primary: Option<(String, String, Option<String>)> = tx
        .query_row(
            "SELECT id, github_repo, subpath FROM project_repos
              WHERE project_id = ?1 AND is_primary = 1
              ORDER BY created_at ASC LIMIT 1",
            params![project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    let elected = match primary {
        Some(existing) => Some(existing),
        None => {
            let next: Option<(String, String, Option<String>)> = tx
                .query_row(
                    "SELECT id, github_repo, subpath FROM project_repos
                      WHERE project_id = ?1 ORDER BY created_at ASC LIMIT 1",
                    params![project_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            if let Some((id, _, _)) = &next {
                tx.execute(
                    "UPDATE project_repos SET is_primary = 1, updated_at = ?1 WHERE id = ?2",
                    params![now, id],
                )?;
            }
            next
        }
    };

    match elected {
        Some((_, github_repo, subpath)) => tx.execute(
            "UPDATE projects SET github_repo = ?1, subpath = ?2, updated_at = ?3 WHERE id = ?4",
            params![github_repo, subpath, now, project_id],
        )?,
        None => tx.execute(
            "UPDATE projects SET github_repo = NULL, subpath = NULL, updated_at = ?1 WHERE id = ?2",
            params![now, project_id],
        )?,
    };
    Ok(())
}

/// Whether `err` is a constraint violation naming one of `needles`.
///
/// The index or column name only comes back as message text, so the code has to
/// be checked too: without it any future error that happens to mention
/// `project_repos` would be rewritten into a confidently wrong explanation.
fn is_unique_violation(err: &rusqlite::Error, needles: &[&str]) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(inner, Some(msg))
            if inner.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            needles.iter().any(|needle| msg.contains(needle))
        }
        _ => false,
    }
}

fn duplicate_repo_error(err: &rusqlite::Error) -> StoreError {
    if is_unique_violation(
        err,
        &["idx_project_repos_unique", "project_repos.github_repo"],
    ) {
        return StoreError(
            "The destination project already has this repository attached.".to_string(),
        );
    }
    StoreError(err.to_string())
}

fn duplicate_branch_error(err: &rusqlite::Error) -> StoreError {
    if is_unique_violation(err, &["branches.branch_name"]) {
        return StoreError(
            "The destination project already tracks a branch with this name for this repository."
                .to_string(),
        );
    }
    StoreError(err.to_string())
}

fn duplicate_workdir_error(err: &rusqlite::Error) -> StoreError {
    if is_unique_violation(err, &["workdirs.path"]) {
        return StoreError(
            "The destination project already has a worktree at that path.".to_string(),
        );
    }
    StoreError(err.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::store::models::{Branch, Image, Note, Project, Session, Workdir};

    struct Fixture {
        store: Store,
        source: Project,
        target: Project,
        repo: ProjectRepo,
        branch: Branch,
    }

    /// A branch on its own `project_repos` row in `source`, plus an empty
    /// `target` project to move it into.
    fn fixture() -> Fixture {
        let store = Store::in_memory().unwrap();
        let source = Project::named("source").with_primary_repo("acme/widgets");
        let target = Project::named("target");
        store.create_project(&source).unwrap();
        store.create_project(&target).unwrap();

        let repo = ProjectRepo::new(&source.id, "acme/widgets", "feature", None).primary();
        store.create_project_repo(&repo).unwrap();

        let branch = Branch::new(&source.id, "feature", "origin/main").with_project_repo(&repo.id);
        store.create_branch(&branch).unwrap();

        Fixture {
            store,
            source,
            target,
            repo,
            branch,
        }
    }

    fn reparent(f: &Fixture, workdir: Option<WorkdirMove>) -> BranchMove {
        BranchMove {
            branch_id: f.branch.id.clone(),
            source_project_id: f.source.id.clone(),
            target_project_id: f.target.id.clone(),
            repo: RepoPlacement::Reparent {
                repo_id: f.repo.id.clone(),
            },
            workdir,
        }
    }

    #[test]
    fn carries_the_branch_its_repo_row_and_its_images() {
        let f = fixture();
        let image = Image::new(
            Some(&f.branch.id),
            &f.source.id,
            "shot.png",
            "image/png",
            12,
            false,
        );
        f.store.create_image(&image).unwrap();
        // A project-note image has no branch and must not follow.
        let project_image = Image::new(None, &f.source.id, "note.png", "image/png", 12, false);
        f.store.create_image(&project_image).unwrap();

        f.store.move_branch_to_project(&reparent(&f, None)).unwrap();

        let moved = f.store.get_branch(&f.branch.id).unwrap().unwrap();
        assert_eq!(moved.project_id, f.target.id);
        assert_eq!(moved.project_repo_id.as_deref(), Some(f.repo.id.as_str()));
        assert_eq!(
            f.store
                .get_project_repo(&f.repo.id)
                .unwrap()
                .unwrap()
                .project_id,
            f.target.id
        );
        assert_eq!(
            f.store.get_image(&image.id).unwrap().unwrap().project_id,
            f.target.id
        );
        assert_eq!(
            f.store
                .get_image(&project_image.id)
                .unwrap()
                .unwrap()
                .project_id,
            f.source.id
        );
    }

    /// The point of moving via `UPDATE` rather than delete-and-reinsert: the
    /// `AFTER DELETE` triggers would have taken the session transcript with it.
    #[test]
    fn keeps_the_branch_artifacts_and_their_sessions() {
        let f = fixture();
        let session = Session::new_running("do the thing", Path::new("/tmp/wt"));
        f.store.create_session(&session).unwrap();
        let note = Note::new(&f.branch.id, "Findings", "body").with_session(&session.id);
        f.store.create_note(&note).unwrap();

        f.store.move_branch_to_project(&reparent(&f, None)).unwrap();

        assert_eq!(
            f.store.list_notes_for_branch(&f.branch.id).unwrap().len(),
            1
        );
        assert!(f.store.get_session(&session.id).unwrap().is_some());
        // The session resolves to the destination through the branch join.
        assert_eq!(
            f.store
                .get_project_id_for_session(&session.id)
                .unwrap()
                .as_deref(),
            Some(f.target.id.as_str())
        );
    }

    #[test]
    fn rewrites_the_workdir_and_the_session_working_dirs_under_it() {
        let f = fixture();
        let workdir = Workdir::new(&f.source.id, "/wt/source/acme-widgets--feature")
            .with_branch(&f.branch.id);
        f.store.create_workdir(&workdir).unwrap();

        let at_root = Session::new_running("root", Path::new("/wt/source/acme-widgets--feature"));
        let at_subpath = Session::new_running(
            "sub",
            Path::new("/wt/source/acme-widgets--feature/apps/web"),
        );
        // A sibling path that merely shares the prefix's characters must not move.
        let elsewhere =
            Session::new_running("other", Path::new("/wt/source/acme-widgets--feature-two"));
        for session in [&at_root, &at_subpath, &elsewhere] {
            f.store.create_session(session).unwrap();
        }

        f.store
            .move_branch_to_project(&reparent(
                &f,
                Some(WorkdirMove {
                    workdir_id: workdir.id.clone(),
                    old_path: workdir.path.clone(),
                    new_path: "/wt/target/acme-widgets--feature".to_string(),
                }),
            ))
            .unwrap();

        let moved = f.store.get_workdir(&workdir.id).unwrap().unwrap();
        assert_eq!(moved.project_id, f.target.id);
        assert_eq!(moved.path, "/wt/target/acme-widgets--feature");

        let working_dir = |id: &str| f.store.get_session(id).unwrap().unwrap().working_dir;
        assert_eq!(working_dir(&at_root.id), "/wt/target/acme-widgets--feature");
        assert_eq!(
            working_dir(&at_subpath.id),
            "/wt/target/acme-widgets--feature/apps/web"
        );
        assert_eq!(
            working_dir(&elsewhere.id),
            "/wt/source/acme-widgets--feature-two"
        );
    }

    /// Windows records a `\` after the worktree root but can still hold `/`
    /// in paths that arrived as strings; Unix must treat `\` as a filename
    /// character, not a separator.
    #[test]
    fn session_prefixes_match_the_platforms_separators() {
        assert_eq!(
            session_dir_prefixes(r"C:\wt\x", true),
            (r"C:\wt\x/".to_string(), r"C:\wt\x\".to_string())
        );
        assert_eq!(
            session_dir_prefixes("/wt/x", false),
            ("/wt/x/".to_string(), "/wt/x/".to_string())
        );
    }

    /// A sibling directory whose name happens to continue the worktree's with
    /// a backslash is not under the worktree on Unix and must stay put.
    #[cfg(not(windows))]
    #[test]
    fn leaves_a_backslash_named_sibling_alone_on_unix() {
        let f = fixture();
        let workdir = Workdir::new(&f.source.id, "/wt/source/acme-widgets--feature")
            .with_branch(&f.branch.id);
        f.store.create_workdir(&workdir).unwrap();
        let sibling =
            Session::new_running("other", Path::new(r"/wt/source/acme-widgets--feature\evil"));
        f.store.create_session(&sibling).unwrap();

        f.store
            .move_branch_to_project(&reparent(
                &f,
                Some(WorkdirMove {
                    workdir_id: workdir.id.clone(),
                    old_path: workdir.path.clone(),
                    new_path: "/wt/target/acme-widgets--feature".to_string(),
                }),
            ))
            .unwrap();

        assert_eq!(
            f.store
                .get_session(&sibling.id)
                .unwrap()
                .unwrap()
                .working_dir,
            r"/wt/source/acme-widgets--feature\evil"
        );
    }

    /// On Windows the session paths under the worktree continue with `\`, and
    /// have to follow the move just as `/`-separated ones do.
    #[cfg(windows)]
    #[test]
    fn rewrites_backslash_separated_session_paths_on_windows() {
        let f = fixture();
        let workdir = Workdir::new(&f.source.id, r"C:\wt\source\acme-widgets--feature")
            .with_branch(&f.branch.id);
        f.store.create_workdir(&workdir).unwrap();
        let at_subpath = Session::new_running(
            "sub",
            Path::new(r"C:\wt\source\acme-widgets--feature\apps\web"),
        );
        f.store.create_session(&at_subpath).unwrap();

        f.store
            .move_branch_to_project(&reparent(
                &f,
                Some(WorkdirMove {
                    workdir_id: workdir.id.clone(),
                    old_path: workdir.path.clone(),
                    new_path: r"C:\wt\target\acme-widgets--feature".to_string(),
                }),
            ))
            .unwrap();

        assert_eq!(
            f.store
                .get_session(&at_subpath.id)
                .unwrap()
                .unwrap()
                .working_dir,
            r"C:\wt\target\acme-widgets--feature\apps\web"
        );
    }

    #[test]
    fn clones_a_repo_row_that_sibling_branches_still_need() {
        let f = fixture();
        let sibling =
            Branch::new(&f.source.id, "other", "origin/main").with_project_repo(&f.repo.id);
        f.store.create_branch(&sibling).unwrap();

        let clone = ProjectRepo::new(&f.target.id, "acme/widgets", "feature", None);
        f.store
            .move_branch_to_project(&BranchMove {
                repo: RepoPlacement::Clone(clone.clone()),
                ..reparent(&f, None)
            })
            .unwrap();

        // The source row stays behind for the sibling.
        let source_repo = f.store.get_project_repo(&f.repo.id).unwrap().unwrap();
        assert_eq!(source_repo.project_id, f.source.id);
        assert_eq!(
            f.store
                .get_branch(&sibling.id)
                .unwrap()
                .unwrap()
                .project_repo_id,
            Some(f.repo.id.clone())
        );
        // …and the moved branch points at the clone in the destination.
        let moved = f.store.get_branch(&f.branch.id).unwrap().unwrap();
        assert_eq!(moved.project_repo_id.as_deref(), Some(clone.id.as_str()));
        assert_eq!(
            f.store
                .get_project_repo(&clone.id)
                .unwrap()
                .unwrap()
                .project_id,
            f.target.id
        );
    }

    #[test]
    fn promotes_the_arriving_repo_when_the_destination_had_none() {
        let f = fixture();

        f.store.move_branch_to_project(&reparent(&f, None)).unwrap();

        let target_primary = f
            .store
            .get_primary_project_repo(&f.target.id)
            .unwrap()
            .unwrap();
        assert_eq!(target_primary.id, f.repo.id);
        assert_eq!(
            f.store
                .get_project(&f.target.id)
                .unwrap()
                .unwrap()
                .github_repo
                .as_deref(),
            Some("acme/widgets")
        );
        // The source lost its only repo, so it denormalizes to NULL.
        let source = f.store.get_project(&f.source.id).unwrap().unwrap();
        assert!(source.github_repo.is_none());
        assert!(f
            .store
            .get_primary_project_repo(&f.source.id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn leaves_the_destinations_own_primary_in_place() {
        let f = fixture();
        let existing = ProjectRepo::new(&f.target.id, "acme/other", "main", None).primary();
        f.store.create_project_repo(&existing).unwrap();

        f.store.move_branch_to_project(&reparent(&f, None)).unwrap();

        assert_eq!(
            f.store
                .get_primary_project_repo(&f.target.id)
                .unwrap()
                .unwrap()
                .id,
            existing.id
        );
        assert!(
            !f.store
                .get_project_repo(&f.repo.id)
                .unwrap()
                .unwrap()
                .is_primary
        );
    }

    #[test]
    fn re_elects_the_sources_next_repo_when_the_primary_leaves() {
        let f = fixture();
        let remaining = ProjectRepo::new(&f.source.id, "acme/other", "main", None);
        f.store.create_project_repo(&remaining).unwrap();

        f.store.move_branch_to_project(&reparent(&f, None)).unwrap();

        assert_eq!(
            f.store
                .get_primary_project_repo(&f.source.id)
                .unwrap()
                .unwrap()
                .id,
            remaining.id
        );
        assert_eq!(
            f.store
                .get_project(&f.source.id)
                .unwrap()
                .unwrap()
                .github_repo
                .as_deref(),
            Some("acme/other")
        );
    }

    /// The unique indexes are the backstop inside the transaction, and a
    /// violation has to leave the branch where it was.
    #[test]
    fn rolls_back_when_the_destination_already_has_the_repo() {
        let f = fixture();
        let conflict = ProjectRepo::new(&f.target.id, "acme/widgets", "main", None);
        f.store.create_project_repo(&conflict).unwrap();

        let err = f
            .store
            .move_branch_to_project(&reparent(&f, None))
            .unwrap_err();

        assert!(
            err.to_string().contains("already has this repository"),
            "unexpected error: {err}"
        );
        assert_eq!(
            f.store
                .get_branch(&f.branch.id)
                .unwrap()
                .unwrap()
                .project_id,
            f.source.id
        );
        assert_eq!(
            f.store
                .get_project_repo(&f.repo.id)
                .unwrap()
                .unwrap()
                .project_id,
            f.source.id
        );
    }

    /// Only a constraint violation may be rewritten into the friendly message:
    /// the index name is matched in the error's text, so an unrelated failure
    /// that happens to mention it has to keep its own words.
    #[test]
    fn explains_only_constraint_violations() {
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("database is locked writing idx_project_repos_unique".to_string()),
        );
        let violation = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE),
            Some("UNIQUE constraint failed: index 'idx_project_repos_unique'".to_string()),
        );

        assert!(duplicate_repo_error(&busy).to_string().contains("locked"));
        assert!(duplicate_repo_error(&violation)
            .to_string()
            .contains("already has this repository"));
    }
}
