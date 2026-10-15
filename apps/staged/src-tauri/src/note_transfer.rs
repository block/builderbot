//! Moving and copying notes between projects.
//!
//! The steps run in an order chosen so that no failure can lose a note:
//!
//! 1. Resolve the destination. A branch note lands on the branch in the target
//!    project that tracks the same repo + subpath; when the target lacks that
//!    repo it is added first, through the regular add-repo path. Nothing about
//!    the note has been touched yet, so an add-repo failure is simply reported
//!    and the note stays where it was. Adding takes long enough for a save to
//!    land, so the note is read again afterwards and that row is what moves.
//! 2. Copy the note's media files into the target project's media directory.
//!    Files are cheap to remove and rows are not, so the filesystem goes first.
//! 3. One transaction rewrites (move) or inserts (copy) the note row together
//!    with the rows for the copied media. On failure the copied files are
//!    removed again.
//! 4. A move then garbage-collects whatever source media the note no longer
//!    references, under the same rules a note delete uses.
//!
//! Media is always copied rather than re-homed, even on a move: two notes in
//! one scope can share an attachment row, and relocating the file out from
//! under the other note would break it. The copied rows belong to the note
//! they land with: they carry its `session_id`, or `NOTE_MEDIA_SESSION_ID`
//! when the note has none (every copy, since a copy is standalone). Either
//! marks them as note media rather than branch-timeline attachments
//! (`list_images_for_branch` filters on a null `session_id`), and neither
//! references the source note's session — the session-GC triggers only
//! collect a session once no `images` row names it, so rows that kept the
//! source's id would keep its session and transcript alive after the source
//! note was deleted.
//!
//! What a move carries and what it leaves (see the store half in
//! `store::note_transfer` for the row-level reasoning): the note keeps its id
//! and its session (the transcript stays reachable; a resumed session keeps
//! its original working directory), a child note is detached from its parent
//! project note, and a project note's children are detached rather than moved.
//! A copy is a standalone note: no session, no parent, no suggested next steps,
//! and — having no session to overwrite it — editable like a note the user
//! wrote.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::note_media::{cleanup_unreferenced, files, refs};
use crate::store::images::image_file_path;
use crate::store::models::{Image, Note, ProjectNote, NOTE_MEDIA_SESSION_ID};
use crate::store::{Store, StoreError};
use crate::NoteTimelineItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferMode {
    Move,
    Copy,
}

impl TransferMode {
    fn gerund(self) -> &'static str {
        match self {
            Self::Move => "moving",
            Self::Copy => "copying",
        }
    }
}

/// The repository a branch note is bound to — what the target project has to
/// track before the note can land there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoKey {
    pub github_repo: String,
    pub subpath: Option<String>,
}

impl RepoKey {
    fn describe(&self) -> String {
        match self
            .subpath
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(sub) => format!("{} at {sub}", self.github_repo),
            None => self.github_repo.clone(),
        }
    }
}

/// Move or copy a branch note into `target_project_id`.
///
/// `ensure_repo` runs when the target project does not yet track the note's
/// repo + subpath; in production that is [`add_repo_to_project`]. It is a
/// parameter so the failure path can be exercised without git or GitHub.
pub(crate) async fn transfer_branch_note<F, Fut>(
    store: &Store,
    note_id: &str,
    target_project_id: &str,
    mode: TransferMode,
    ensure_repo: F,
) -> Result<Note, String>
where
    F: FnOnce(RepoKey) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let note = store
        .get_note(note_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Note not found: {note_id}"))?;
    ensure_settled(
        &note.title,
        note.session_id.as_deref(),
        note.completed_at,
        mode,
    )?;
    let source_branch = store
        .get_branch(&note.branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {}", note.branch_id))?;
    let target = store
        .get_project(target_project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {target_project_id}"))?;
    if mode == TransferMode::Move && source_branch.project_id == target.id {
        return Err(format!(
            "{} is already in {}.",
            quoted(&note.title),
            target.name
        ));
    }
    let repo_id = source_branch.project_repo_id.as_deref().ok_or_else(|| {
        format!(
            "Branch '{}' is not bound to a repository, so its notes cannot be {} to another project.",
            source_branch.branch_name,
            match mode {
                TransferMode::Move => "moved",
                TransferMode::Copy => "copied",
            }
        )
    })?;
    let repo = store
        .get_project_repo(repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Repository not found: {repo_id}"))?;
    let key = RepoKey {
        github_repo: repo.github_repo,
        subpath: repo.subpath,
    };

    // Step 1: the repo has to be in the target before the note can be.
    let already_tracked = store
        .find_project_repo(&target.id, &key.github_repo, key.subpath.as_deref())
        .map_err(|e| e.to_string())?
        .is_some();
    if !already_tracked {
        ensure_repo(key.clone())
            .await
            .map_err(|e| format!("Could not add {} to {}: {e}", key.describe(), target.name))?;
    }
    // Adding a repo can take seconds, and a save that landed meanwhile must
    // not be overwritten by the body read before it: from here on the note is
    // the row as it reads now.
    let note = refetch_branch_note(store, &note, mode)?;
    // A copy into the note's own project lands beside the original. The
    // lookup would send it to the project's oldest branch on the repo, which
    // is only the same branch when there is one.
    let target_branch = if source_branch.project_id == target.id {
        source_branch
    } else {
        store
            .find_branch_for_repo(&target.id, &key.github_repo, key.subpath.as_deref())
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                format!(
                    "{} tracks {} but has no branch for it.",
                    target.name,
                    key.describe()
                )
            })?
    };

    // Step 2: media files, then step 3: the transaction.
    let owner_session = match mode {
        TransferMode::Move => note.session_id.as_deref(),
        TransferMode::Copy => None,
    };
    let media = stage_media_copies(
        store,
        &note.content,
        Some(&target_branch.id),
        &target.id,
        owner_session,
    )?;
    let result = match mode {
        TransferMode::Move => store.move_note_to_branch(
            &note.id,
            &note.branch_id,
            &target_branch.id,
            &media.content,
            &media.images,
        ),
        TransferMode::Copy => {
            let mut copy = Note::new_standalone(&target_branch.id, &note.title, &media.content)
                .with_subtype(Note::SUBTYPE_WRITTEN);
            store
                .copy_note_to_branch(&mut copy, &media.images)
                .map(|()| copy)
        }
    };
    finish(store, mode, &note.content, media, result)
}

/// Move or copy a project note into `target_project_id`.
pub(crate) fn transfer_project_note(
    store: &Store,
    note_id: &str,
    target_project_id: &str,
    mode: TransferMode,
) -> Result<ProjectNote, String> {
    let note = store
        .get_project_note(note_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project note not found: {note_id}"))?;
    ensure_settled(
        &note.title,
        note.session_id.as_deref(),
        note.completed_at,
        mode,
    )?;
    let target = store
        .get_project(target_project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {target_project_id}"))?;
    if mode == TransferMode::Move && note.project_id == target.id {
        return Err(format!(
            "{} is already in {}.",
            quoted(&note.title),
            target.name
        ));
    }

    let owner_session = match mode {
        TransferMode::Move => note.session_id.as_deref(),
        TransferMode::Copy => None,
    };
    let media = stage_media_copies(store, &note.content, None, &target.id, owner_session)?;
    let result = match mode {
        TransferMode::Move => store.move_project_note_to_project(
            &note.id,
            &note.project_id,
            &target.id,
            &media.content,
            &media.images,
        ),
        TransferMode::Copy => {
            let copy = ProjectNote::new(&target.id, &note.title, &media.content);
            store
                .copy_project_note_to_project(&copy, &media.images)
                .map(|()| copy)
        }
    };
    finish(store, mode, &note.content, media, result)
}

/// The production repo-adding step: the same path the "Add repository"
/// button takes, background worktree setup included.
pub(crate) async fn add_repo_to_project(
    app_handle: tauri::AppHandle,
    store: Arc<Store>,
    project_id: String,
    key: RepoKey,
) -> Result<(), String> {
    let repo = crate::project_commands::add_project_repo_impl(
        Arc::clone(&store),
        project_id.clone(),
        key.github_repo,
        None,
        key.subpath,
        None,
        None,
        None,
        None,
        None,
    )
    .await?;
    crate::project_commands::spawn_project_repo_setup(app_handle, store, project_id, repo.id);
    Ok(())
}

/// A note whose session is still producing it has no settled content to carry,
/// and the runner would keep writing to it wherever it went.
fn ensure_settled(
    title: &str,
    session_id: Option<&str>,
    completed_at: Option<i64>,
    mode: TransferMode,
) -> Result<(), String> {
    if session_id.is_some() && completed_at.is_none() {
        return Err(format!(
            "{} is still being generated; wait for its session to finish before {} it.",
            quoted(title),
            mode.gerund()
        ));
    }
    Ok(())
}

/// The note as it reads after an await: a save that landed in the meantime
/// is carried rather than overwritten, a session that has since started
/// writing to it stops the transfer, and a note that was deleted or moved
/// meanwhile is left alone — the branch and repo resolved for it no longer
/// describe where it is.
fn refetch_branch_note(store: &Store, before: &Note, mode: TransferMode) -> Result<Note, String> {
    let fresh = store
        .get_note(&before.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            format!(
                "{} was deleted while {} it.",
                quoted(&before.title),
                mode.gerund()
            )
        })?;
    ensure_settled(
        &fresh.title,
        fresh.session_id.as_deref(),
        fresh.completed_at,
        mode,
    )?;
    if fresh.branch_id != before.branch_id {
        return Err(format!(
            "{} was moved while {} it.",
            quoted(&fresh.title),
            mode.gerund()
        ));
    }
    Ok(fresh)
}

fn quoted(title: &str) -> String {
    if title.trim().is_empty() {
        "This note".to_string()
    } else {
        format!("\"{title}\"")
    }
}

fn finish<T>(
    store: &Store,
    mode: TransferMode,
    previous_content: &str,
    media: StagedMedia,
    result: Result<T, StoreError>,
) -> Result<T, String> {
    match result {
        Ok(value) => {
            if mode == TransferMode::Move {
                cleanup_unreferenced(store, refs::media_ids(previous_content));
            }
            Ok(value)
        }
        Err(e) => {
            media.discard();
            Err(e.to_string())
        }
    }
}

/// Media copies written to disk but not yet recorded: the rewritten note body,
/// the rows to insert alongside it, and the files to remove if that never
/// happens.
struct StagedMedia {
    content: String,
    images: Vec<Image>,
    files: Vec<PathBuf>,
}

impl StagedMedia {
    fn discard(self) {
        for path in self.files {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Copy every stored attachment `content` references into the destination
/// scope and rewrite the references to the copies.
///
/// A reference whose row or file is already gone is left as it is: the viewer
/// renders it unavailable today and will keep doing so after the transfer,
/// which is better than failing the transfer over media that was lost before.
/// Copy every stored attachment `content` references into `project_id`
/// (and `branch_id`), returning the rewritten body and the rows to insert.
/// The rows are owned by the note that will carry them: `owner_session` is
/// that note's session, or `None` for a note without one.
fn stage_media_copies(
    store: &Store,
    content: &str,
    branch_id: Option<&str>,
    project_id: &str,
    owner_session: Option<&str>,
) -> Result<StagedMedia, String> {
    let mut staged = StagedMedia {
        content: content.to_owned(),
        images: Vec::new(),
        files: Vec::new(),
    };
    let mut copies = HashMap::<String, Image>::new();
    // Names handed out in this batch: the rows are not inserted until the
    // transaction, so the store alone cannot keep two copies apart.
    let mut reserved = HashSet::<String>::new();
    let mut edits = Vec::new();
    let result = (|| -> Result<(), String> {
        for reference in refs::extract_media_refs(content) {
            let Some((id, _)) = refs::stored_ref(&reference.target) else {
                continue;
            };
            let copy = match copies.get(id) {
                Some(copy) => copy.clone(),
                None => {
                    let Some(source) = store.get_image(id).map_err(|e| e.to_string())? else {
                        continue;
                    };
                    let source_path =
                        image_file_path(&source.project_id, &source.id, &source.filename)?;
                    let bytes = match files::read_local(&source_path, files::MAX_VIDEO_SIZE) {
                        Ok(bytes) => bytes,
                        Err(files::ReadError::NotFound(_)) => continue,
                        Err(e) => return Err(e.into()),
                    };
                    let filename = store
                        .unique_image_filename_excluding(
                            branch_id,
                            project_id,
                            &source.filename,
                            &reserved,
                        )
                        .map_err(|e| e.to_string())?;
                    reserved.insert(filename.clone());
                    let mut copy = Image::new(
                        branch_id,
                        project_id,
                        &filename,
                        &source.mime_type,
                        source.size_bytes,
                        false,
                    );
                    copy.session_id =
                        Some(owner_session.unwrap_or(NOTE_MEDIA_SESSION_ID).to_owned());
                    let path = image_file_path(project_id, &copy.id, &filename)?;
                    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
                    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
                    staged.files.push(path);
                    staged.images.push(copy.clone());
                    copies.insert(id.to_owned(), copy.clone());
                    copy
                }
            };
            edits.push((
                reference.destination,
                format!(
                    "staged-media://{}.{}",
                    copy.id,
                    files::extension(&copy.filename)
                ),
            ));
        }
        Ok(())
    })();
    if let Err(e) = result {
        staged.discard();
        return Err(e);
    }
    // Apply back to front so earlier offsets stay valid.
    for (span, replacement) in edits.into_iter().rev() {
        staged.content.replace_range(span, &replacement);
    }
    Ok(staged)
}

// =============================================================================
// Tauri commands
// =============================================================================

/// Move a branch note into another project, adding the note's repo + subpath
/// to that project first if it is not tracked there yet.
#[tauri::command(rename_all = "camelCase")]
pub async fn move_note_to_project(
    app_handle: tauri::AppHandle,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
    target_project_id: String,
) -> Result<NoteTimelineItem, String> {
    let store = crate::get_store(&store)?;
    let note = transfer_branch_note(
        &store,
        &note_id,
        &target_project_id,
        TransferMode::Move,
        |key| {
            add_repo_to_project(
                app_handle,
                Arc::clone(&store),
                target_project_id.clone(),
                key,
            )
        },
    )
    .await?;
    Ok(crate::note_commands::note_to_timeline_item(&store, note))
}

/// Copy a branch note into another project (or the same one); see
/// [`move_note_to_project`] for the repo handling.
#[tauri::command(rename_all = "camelCase")]
pub async fn copy_note_to_project(
    app_handle: tauri::AppHandle,
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
    target_project_id: String,
) -> Result<NoteTimelineItem, String> {
    let store = crate::get_store(&store)?;
    let note = transfer_branch_note(
        &store,
        &note_id,
        &target_project_id,
        TransferMode::Copy,
        |key| {
            add_repo_to_project(
                app_handle,
                Arc::clone(&store),
                target_project_id.clone(),
                key,
            )
        },
    )
    .await?;
    Ok(crate::note_commands::standalone_note_to_timeline_item(note))
}

#[tauri::command(rename_all = "camelCase")]
pub fn move_project_note_to_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_note_id: String,
    target_project_id: String,
) -> Result<ProjectNote, String> {
    let store = crate::get_store(&store)?;
    transfer_project_note(
        &store,
        &project_note_id,
        &target_project_id,
        TransferMode::Move,
    )
}

#[tauri::command(rename_all = "camelCase")]
pub fn copy_project_note_to_project(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    project_note_id: String,
    target_project_id: String,
) -> Result<ProjectNote, String> {
    let store = crate::get_store(&store)?;
    transfer_project_note(
        &store,
        &project_note_id,
        &target_project_id,
        TransferMode::Copy,
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::note_media::tests::PNG;
    use crate::store::models::{Branch, Project, ProjectRepo, Session, SessionStatus};

    struct Fixture {
        store: Store,
        source: Project,
        target: Project,
        source_branch: Branch,
    }

    /// `source` tracks `acme/widgets` at `apps/web` with one branch; `target`
    /// starts empty, so a repo note can only land there once the repo is added.
    fn fixture() -> Fixture {
        let store = Store::in_memory().unwrap();
        let source = Project::named("source");
        let target = Project::named("target");
        store.create_project(&source).unwrap();
        store.create_project(&target).unwrap();
        let repo = ProjectRepo::new(
            &source.id,
            "acme/widgets",
            "feature",
            Some("apps/web".into()),
        )
        .primary();
        store.create_project_repo(&repo).unwrap();
        let source_branch =
            Branch::new(&source.id, "feature", "origin/main").with_project_repo(&repo.id);
        store.create_branch(&source_branch).unwrap();
        Fixture {
            store,
            source,
            target,
            source_branch,
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            for project in [&self.source, &self.target] {
                if let Ok(path) = crate::git::project_worktree_root_for(&project.id) {
                    let _ = std::fs::remove_dir_all(path);
                }
            }
        }
    }

    impl Fixture {
        fn note(&self, title: &str, content: &str) -> Note {
            self.note_on(&self.source_branch.id, title, content)
        }

        fn note_on(&self, branch_id: &str, title: &str, content: &str) -> Note {
            let note =
                Note::new_standalone(branch_id, title, content).with_subtype(Note::SUBTYPE_WRITTEN);
            self.store.create_note(&note).unwrap();
            note
        }

        /// A second branch in `source` on the same repo, created after
        /// `source_branch` so the oldest-branch lookup never picks it.
        fn later_source_branch(&self) -> Branch {
            let mut branch = Branch::new(&self.source.id, "feature-2", "origin/main")
                .with_project_repo(self.source_branch.project_repo_id.as_deref().unwrap());
            branch.created_at = self.source_branch.created_at + 1_000;
            self.store.create_branch(&branch).unwrap();
            branch
        }

        /// What `add_project_repo_impl` leaves behind, minus git: the repo row
        /// and the branch that tracks it.
        fn attach_repo(&self, key: RepoKey) -> Result<(), String> {
            let repo = ProjectRepo::new(
                &self.target.id,
                &key.github_repo,
                "target-feature",
                key.subpath,
            );
            self.store
                .create_project_repo(&repo)
                .map_err(|e| e.to_string())?;
            let branch = Branch::new(&self.target.id, "target-feature", "origin/main")
                .with_project_repo(&repo.id);
            self.store.create_branch(&branch).map_err(|e| e.to_string())
        }

        fn target_branch(&self) -> Option<Branch> {
            self.store
                .find_branch_for_repo(&self.target.id, "acme/widgets", Some("apps/web"))
                .unwrap()
        }

        /// A stored attachment on the source branch, written to disk.
        fn attachment(&self, filename: &str) -> Image {
            let session = Session::new_running("note", Path::new("/tmp/wt"));
            self.store.create_session(&session).unwrap();
            let mut image = Image::new(
                Some(&self.source_branch.id),
                &self.source.id,
                filename,
                "image/png",
                PNG.len() as i64,
                false,
            );
            image.session_id = Some(session.id.clone());
            let path = image_file_path(&self.source.id, &image.id, &image.filename).unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, PNG).unwrap();
            self.store.create_image(&image).unwrap();
            image
        }

        /// An attachment and the note whose markdown references it.
        fn media_note(&self) -> (Note, Image) {
            let image = self.attachment("shot.png");
            let note = self.note(
                "Shots",
                &format!("before ![a](staged-media://{}.png) after", image.id),
            );
            (note, image)
        }
    }

    fn never_called(_: RepoKey) -> std::future::Ready<Result<(), String>> {
        panic!("the target already tracks the repo; nothing should be added")
    }

    #[tokio::test]
    async fn move_adds_the_repo_the_target_lacks_and_lands_on_its_branch() {
        let f = fixture();
        let note = f.note("Findings", "body");
        let added = Mutex::new(None);

        let moved = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Move,
            |key| {
                *added.lock().unwrap() = Some(key.clone());
                std::future::ready(f.attach_repo(key))
            },
        )
        .await
        .unwrap();

        assert_eq!(
            added.lock().unwrap().take(),
            Some(RepoKey {
                github_repo: "acme/widgets".into(),
                subpath: Some("apps/web".into()),
            })
        );
        let target_branch = f.target_branch().expect("repo was added with a branch");
        assert_eq!(moved.id, note.id);
        assert_eq!(moved.branch_id, target_branch.id);
        assert_eq!(
            f.store.get_note(&note.id).unwrap().unwrap().branch_id,
            target_branch.id
        );
        assert!(f
            .store
            .list_notes_for_branch(&f.source_branch.id)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn move_reuses_the_branch_when_the_target_already_tracks_the_repo() {
        let f = fixture();
        f.attach_repo(RepoKey {
            github_repo: "acme/widgets".into(),
            subpath: Some("apps/web".into()),
        })
        .unwrap();
        let note = f.note("Findings", "body");

        let moved = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Move,
            never_called,
        )
        .await
        .unwrap();

        assert_eq!(moved.branch_id, f.target_branch().unwrap().id);
    }

    /// The add-repo step runs before anything about the note changes, so when
    /// it fails the note is exactly where and what it was.
    #[tokio::test]
    async fn failed_repo_add_leaves_the_note_in_the_source_project() {
        let f = fixture();
        let note = f.note("Findings", "body");

        let err =
            transfer_branch_note(&f.store, &note.id, &f.target.id, TransferMode::Move, |_| {
                std::future::ready(Err::<(), _>("Workspace is not running".to_string()))
            })
            .await
            .unwrap_err();

        assert!(
            err.contains("Could not add acme/widgets at apps/web to target")
                && err.contains("Workspace is not running"),
            "{err}"
        );
        let unchanged = f.store.get_note(&note.id).unwrap().unwrap();
        assert_eq!(unchanged.branch_id, f.source_branch.id);
        assert_eq!(unchanged.content, "body");
        assert!(f.target_branch().is_none());
        assert!(f.store.list_project_repos(&f.target.id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn copy_leaves_the_original_and_creates_an_editable_standalone_copy() {
        let f = fixture();
        let note = f.note("Findings", "body");

        let copy = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            |key| std::future::ready(f.attach_repo(key)),
        )
        .await
        .unwrap();

        assert_ne!(copy.id, note.id);
        assert_eq!(copy.branch_id, f.target_branch().unwrap().id);
        assert_eq!(copy.title, "Findings");
        assert_eq!(copy.content, "body");
        assert!(copy.is_written());
        assert!(copy.session_id.is_none());
        assert!(copy.completed_at.is_some());
        let original = f.store.get_note(&note.id).unwrap().unwrap();
        assert_eq!(original.branch_id, f.source_branch.id);
        assert_eq!(original.content, "body");
        assert_eq!(
            f.store.get_note(&copy.id).unwrap().unwrap().title,
            "Findings"
        );
    }

    #[tokio::test]
    async fn move_to_the_same_project_is_refused_but_copy_duplicates() {
        let f = fixture();
        let note = f.note("Findings", "body");

        let err = transfer_branch_note(
            &f.store,
            &note.id,
            &f.source.id,
            TransferMode::Move,
            never_called,
        )
        .await
        .unwrap_err();
        assert_eq!(err, "\"Findings\" is already in source.");

        let copy = transfer_branch_note(
            &f.store,
            &note.id,
            &f.source.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();
        assert_eq!(copy.branch_id, f.source_branch.id);
        assert_eq!(copy.title, "Findings (2)");
        assert_eq!(
            f.store
                .list_notes_for_branch(&f.source_branch.id)
                .unwrap()
                .len(),
            2
        );
    }

    /// With two branches on the repo in one project, the oldest-branch lookup
    /// would send a same-project copy to the other branch.
    #[tokio::test]
    async fn copy_into_the_same_project_lands_beside_the_original() {
        let f = fixture();
        let later = f.later_source_branch();
        let note = f.note_on(&later.id, "Findings", "body");

        let copy = transfer_branch_note(
            &f.store,
            &note.id,
            &f.source.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();

        assert_eq!(copy.branch_id, later.id);
        assert!(f
            .store
            .list_notes_for_branch(&f.source_branch.id)
            .unwrap()
            .is_empty());
        assert_eq!(f.store.list_notes_for_branch(&later.id).unwrap().len(), 2);
    }

    /// The add-repo step is the one await in a transfer; a save that lands
    /// during it is what the moved note carries, not the body read before.
    #[tokio::test]
    async fn a_save_landing_while_the_repo_is_added_is_carried_by_the_move() {
        let f = fixture();
        let note = f.note("Findings", "first draft");

        let moved = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Move,
            |key| {
                f.store
                    .update_note_content(&note.id, "revised while adding the repo")
                    .unwrap();
                std::future::ready(f.attach_repo(key))
            },
        )
        .await
        .unwrap();

        assert_eq!(moved.content, "revised while adding the repo");
        assert_eq!(
            f.store.get_note(&note.id).unwrap().unwrap().content,
            "revised while adding the repo"
        );
    }

    /// Likewise a note deleted during the add-repo step is not brought back
    /// by the UPDATE or INSERT that follows; the repo stays added.
    #[tokio::test]
    async fn a_note_deleted_while_the_repo_is_added_is_not_recreated() {
        let f = fixture();
        let note = f.note("Findings", "body");

        let err = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            |key| {
                f.store.delete_note(&note.id).unwrap();
                std::future::ready(f.attach_repo(key))
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err, "\"Findings\" was deleted while copying it.");
        let target_branch = f.target_branch().expect("the repo was still added");
        assert!(f
            .store
            .list_notes_for_branch(&target_branch.id)
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn a_note_still_being_generated_stays_put() {
        let f = fixture();
        let session = Session::new_running("note", Path::new("/tmp/wt"));
        f.store.create_session(&session).unwrap();
        let note = Note::new(&f.source_branch.id, "Pending", "").with_session(&session.id);
        f.store.create_note(&note).unwrap();

        let err = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Move,
            never_called,
        )
        .await
        .unwrap_err();

        assert!(err.contains("still being generated"), "{err}");
        assert_eq!(
            f.store.get_note(&note.id).unwrap().unwrap().branch_id,
            f.source_branch.id
        );
    }

    #[tokio::test]
    async fn copy_duplicates_the_media_into_the_target_project() {
        let f = fixture();
        let (note, image) = f.media_note();

        let copy = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            |key| std::future::ready(f.attach_repo(key)),
        )
        .await
        .unwrap();

        let copied_ids = refs::media_ids(&copy.content);
        assert_eq!(copied_ids.len(), 1);
        let copied_id = copied_ids.into_iter().next().unwrap();
        assert_ne!(copied_id, image.id);
        assert_eq!(
            copy.content,
            format!("before ![a](staged-media://{copied_id}.png) after")
        );
        let copied = f.store.get_image(&copied_id).unwrap().unwrap();
        assert_eq!(copied.project_id, f.target.id);
        assert_eq!(copied.branch_id, Some(f.target_branch().unwrap().id));
        // The copy is a session-less note, so its media is marked as such
        // rather than borrowing the source's session.
        assert_eq!(copied.session_id.as_deref(), Some(NOTE_MEDIA_SESSION_ID));
        assert!(f
            .store
            .list_images_for_branch(&f.target_branch().unwrap().id)
            .unwrap()
            .is_empty());
        assert_eq!(copied.filename, "shot.png");
        let copied_path = image_file_path(&f.target.id, &copied.id, &copied.filename).unwrap();
        assert_eq!(std::fs::read(copied_path).unwrap(), PNG);
        // The original row and file are untouched.
        assert!(f.store.get_image(&image.id).unwrap().is_some());
        let original_path = image_file_path(&f.source.id, &image.id, &image.filename).unwrap();
        assert!(original_path.exists());
    }

    /// Deleting a note GCs its session unless some `images` row still names
    /// it. A copy's media must not be such a row: the copy lives on in the
    /// other project, but the source's session and transcript must not.
    #[tokio::test]
    async fn deleting_the_source_after_a_copy_still_collects_its_session() {
        let f = fixture();
        let image = f.attachment("shot.png");
        let session_id = image.session_id.clone().unwrap();
        f.store
            .update_session_status(&session_id, SessionStatus::Completed, None, None)
            .unwrap();
        let note = Note::new_standalone(
            &f.source_branch.id,
            "Shots",
            &format!("![a](staged-media://{}.png)", image.id),
        )
        .with_session(&session_id);
        f.store.create_note(&note).unwrap();

        let copy = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            |key| std::future::ready(f.attach_repo(key)),
        )
        .await
        .unwrap();
        let copied_id = refs::media_ids(&copy.content).into_iter().next().unwrap();

        // What `note_commands::delete_note` does: drop the row, then the media
        // only it referenced. The session goes with the last row naming it.
        f.store.delete_note(&note.id).unwrap();
        cleanup_unreferenced(&f.store, refs::media_ids(&note.content));

        assert!(f.store.get_image(&image.id).unwrap().is_none());
        assert!(f.store.get_session(&session_id).unwrap().is_none());
        let copied = f.store.get_image(&copied_id).unwrap().unwrap();
        assert_eq!(copied.project_id, f.target.id);
        assert!(image_file_path(&f.target.id, &copied.id, &copied.filename)
            .unwrap()
            .exists());
        assert_eq!(
            f.store.get_note(&copy.id).unwrap().unwrap().content,
            format!("![a](staged-media://{copied_id}.png)")
        );
    }

    /// A moved note keeps its session, and so does the media that moves with
    /// it — the rows must stay tied to the note that owns them, not to
    /// whichever row they were copied from.
    #[tokio::test]
    async fn moved_media_belongs_to_the_moved_note() {
        let f = fixture();
        let image = f.attachment("shot.png");
        let session = Session::new_running("note", Path::new("/tmp/wt"));
        f.store.create_session(&session).unwrap();
        f.store
            .update_session_status(&session.id, SessionStatus::Completed, None, None)
            .unwrap();
        let note = Note::new_standalone(
            &f.source_branch.id,
            "Shots",
            &format!("![a](staged-media://{}.png)", image.id),
        )
        .with_session(&session.id);
        f.store.create_note(&note).unwrap();

        let moved = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Move,
            |key| std::future::ready(f.attach_repo(key)),
        )
        .await
        .unwrap();

        let copied_id = refs::media_ids(&moved.content).into_iter().next().unwrap();
        let copied = f.store.get_image(&copied_id).unwrap().unwrap();
        assert_eq!(copied.session_id.as_deref(), Some(session.id.as_str()));
    }

    #[tokio::test]
    async fn move_duplicates_the_media_and_garbage_collects_the_source_copy() {
        let f = fixture();
        let (note, image) = f.media_note();

        let moved = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Move,
            |key| std::future::ready(f.attach_repo(key)),
        )
        .await
        .unwrap();

        let copied_id = refs::media_ids(&moved.content).into_iter().next().unwrap();
        assert_ne!(copied_id, image.id);
        assert!(f.store.get_image(&copied_id).unwrap().is_some());
        // Nothing references the source attachment any more, so it is gone.
        assert!(f.store.get_image(&image.id).unwrap().is_none());
        let original_path = image_file_path(&f.source.id, &image.id, &image.filename).unwrap();
        assert!(!original_path.exists());
    }

    /// Two attachments can share a filename in one scope. Their copies are
    /// staged before any row is inserted, so the names have to be kept apart
    /// within the batch and not only against the destination's rows.
    #[tokio::test]
    async fn identically_named_attachments_get_distinct_copies() {
        let f = fixture();
        let first = f.attachment("image.png");
        let second = f.attachment("image.png");
        let note = f.note(
            "Shots",
            &format!(
                "![a](staged-media://{}.png)\n![b](staged-media://{}.png)",
                first.id, second.id
            ),
        );

        let copy = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            |key| std::future::ready(f.attach_repo(key)),
        )
        .await
        .unwrap();

        let mut filenames: Vec<String> = refs::media_ids(&copy.content)
            .into_iter()
            .map(|id| f.store.get_image(&id).unwrap().unwrap().filename)
            .collect();
        filenames.sort();
        assert_eq!(filenames, ["image 2.png", "image.png"]);
    }

    /// A reference whose file was lost before the transfer is carried over
    /// verbatim instead of failing the whole operation.
    #[tokio::test]
    async fn media_already_missing_on_disk_is_left_referenced_as_is() {
        let f = fixture();
        let (note, image) = f.media_note();
        std::fs::remove_file(image_file_path(&f.source.id, &image.id, &image.filename).unwrap())
            .unwrap();

        let copy = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            |key| std::future::ready(f.attach_repo(key)),
        )
        .await
        .unwrap();

        assert_eq!(copy.content, note.content);
    }

    #[test]
    fn project_note_moves_directly_and_copies_duplicate() {
        let f = fixture();
        let note = ProjectNote::new(&f.source.id, "Plan", "body");
        f.store.create_project_note(&note).unwrap();

        let err = transfer_project_note(&f.store, &note.id, &f.source.id, TransferMode::Move)
            .unwrap_err();
        assert_eq!(err, "\"Plan\" is already in source.");

        let copy =
            transfer_project_note(&f.store, &note.id, &f.target.id, TransferMode::Copy).unwrap();
        assert_ne!(copy.id, note.id);
        assert_eq!(copy.project_id, f.target.id);
        assert_eq!(copy.content, "body");
        assert!(copy.session_id.is_none());

        let moved =
            transfer_project_note(&f.store, &note.id, &f.target.id, TransferMode::Move).unwrap();
        assert_eq!(moved.id, note.id);
        assert_eq!(moved.project_id, f.target.id);
        assert!(f.store.list_project_notes(&f.source.id).unwrap().is_empty());
        assert_eq!(f.store.list_project_notes(&f.target.id).unwrap().len(), 2);
    }

    #[test]
    fn project_note_media_follows_the_copy() {
        let f = fixture();
        let session = Session::new_running("note", Path::new("/tmp/wt"));
        f.store.create_session(&session).unwrap();
        let mut image = Image::new(
            None,
            &f.source.id,
            "shot.png",
            "image/png",
            PNG.len() as i64,
            false,
        );
        image.session_id = Some(session.id.clone());
        let path = image_file_path(&f.source.id, &image.id, &image.filename).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, PNG).unwrap();
        f.store.create_image(&image).unwrap();
        let note = ProjectNote::new(
            &f.source.id,
            "Plan",
            &format!("![a](staged-media://{}.png)", image.id),
        );
        f.store.create_project_note(&note).unwrap();

        let copy =
            transfer_project_note(&f.store, &note.id, &f.target.id, TransferMode::Copy).unwrap();

        let copied_id = refs::media_ids(&copy.content).into_iter().next().unwrap();
        let copied = f.store.get_image(&copied_id).unwrap().unwrap();
        assert_eq!(copied.project_id, f.target.id);
        assert!(copied.branch_id.is_none());
        assert!(image_file_path(&f.target.id, &copied.id, &copied.filename)
            .unwrap()
            .exists());
    }
}
