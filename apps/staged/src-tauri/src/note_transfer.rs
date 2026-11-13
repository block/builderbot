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
//! 2. Put the note's media files in place in the target project's media
//!    directory: copied, or on a move hard-linked (see below). Files are
//!    cheap to put back and rows are not, so the filesystem goes first.
//! 3. One transaction rewrites (move) or inserts (copy) the note row together
//!    with the rows for its media. Once it commits, the paths re-homed rows
//!    no longer name are unlinked; if it fails, the copies and links are
//!    removed. A source file is never touched before the commit, so a row
//!    finds its file at every point, and a crash leaves at worst a spare link.
//!    Steps 2 and 3 run under one process-wide lock (`TRANSFER_MEDIA`), since
//!    two moves of the same note link the same destination path.
//! 4. A move then garbage-collects whatever source media the note no longer
//!    references, under the same rules a note delete uses.
//!
//! A move takes media only the note uses along as it is: the row is re-homed
//! in place and its file linked into the destination, so the id survives.
//! Anything that still holds it — the note's session repeating a reference
//! on its next turn, a view opened before the move — keeps resolving. That
//! is a row in the scope the note is leaving, with a session (a session-less
//! row is a branch-timeline attachment, which stays on its timeline), that no
//! other note embeds and no chat message attaches: exactly what the move
//! would otherwise copy and then garbage-collect.
//!
//! Everything else is copied, as is all media on a copy: two notes in one
//! scope can share an attachment row, and relocating the file out from under
//! the other note would break it. Rows that land with the note, copied or re-homed,
//! belong to it: they carry its `session_id`, or `NOTE_MEDIA_SESSION_ID`
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
//! its original working directory). A branch note moved on its own is
//! detached from its parent project note. A project note takes its child
//! notes along: each lands on the branch in the target that tracks the same
//! repo and subpath (added first when missing, as in step 1), with its session
//! and its parent link, in the parent's transaction. The move waits for the
//! parent's session and any child session that is still queued or running
//! (`sessions.branch_id` is the branch queue's scheduling key, and a session
//! that was resumed is live again however settled its note reads), and is
//! refused when a child sits on a remote branch, on a branch outside the
//! parent's project, or either project is remote, the way a branch move is.
//! A repo added for a child in step 1 is not removed when a later step fails:
//! the error names it, and it stays tracked the way a manual add would.
//! The commits and code reviews a project session produced carry no link back
//! to it and stay on their branch. A copy is a standalone note: no session,
//! no parent, no children, no suggested next steps, and — having no session
//! to overwrite it — editable like a note the user wrote. It only needs the
//! note to have finished once; a session that was resumed does not hold a
//! copy back, since the copy leaves the session where it is.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::note_media::{cleanup_unreferenced, files, refs};
use crate::store::images::image_file_path;
use crate::store::models::{
    Branch, BranchType, Image, Note, Project, ProjectLocation, ProjectNote, ProjectRepo,
    SessionStatus, NOTE_MEDIA_SESSION_ID,
};
use crate::store::{ChildNoteMove, Store, StoreError};
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
        store,
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
    let repo = bound_repo(store, &source_branch)?.ok_or_else(|| {
        format!(
            "Branch '{}' is not bound to a repository, so its notes cannot be {} to another project.",
            source_branch.branch_name,
            match mode {
                TransferMode::Move => "moved",
                TransferMode::Copy => "copied",
            }
        )
    })?;
    let key = RepoKey {
        github_repo: repo.github_repo,
        subpath: repo.subpath,
    };
    let source_project_id = source_branch.project_id.clone();

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

    // Step 2: media files, then step 3: the transaction. There is no await
    // past this point, so the lock is never held across one.
    let _media = lock_transfer_media();
    let (owner_session, rehome) = match mode {
        TransferMode::Move => (
            note.session_id.as_deref(),
            Some(MoveSource {
                note_id: &note.id,
                project_id: &source_project_id,
                branch_id: Some(&note.branch_id),
            }),
        ),
        TransferMode::Copy => (None, None),
    };
    let media = stage_media(
        store,
        &note.content,
        Some(&target_branch.id),
        &target.id,
        owner_session,
        rehome,
        &mut HashSet::new(),
    )?;
    let result = match mode {
        TransferMode::Move => store.move_note_to_branch(
            &note.id,
            &note.branch_id,
            &target_branch.id,
            &media.content,
            &media.images,
            &media.rehomed,
        ),
        TransferMode::Copy => {
            let mut copy = Note::new_standalone(&target_branch.id, &note.title, &media.content)
                .with_subtype(Note::SUBTYPE_WRITTEN);
            // The user made the copy, so it is no newer to them than the source.
            copy.set_initial_read(note.is_read());
            store
                .copy_note_to_branch(&mut copy, &media.images)
                .map(|()| copy)
        }
    };
    finish(store, mode, &note.content, media, result)
}

/// The repo a branch note's branch is bound to — the one
/// [`transfer_branch_note`] needs its destination to track — or `None` when
/// the branch is bound to none (such a note can't be transferred).
pub(crate) fn branch_note_repo(
    store: &Store,
    note_id: &str,
) -> Result<Option<ProjectRepo>, String> {
    let note = store
        .get_note(note_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Note not found: {note_id}"))?;
    let branch = store
        .get_branch(&note.branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {}", note.branch_id))?;
    bound_repo(store, &branch)
}

fn bound_repo(store: &Store, branch: &Branch) -> Result<Option<ProjectRepo>, String> {
    let Some(repo_id) = branch.project_repo_id.as_deref() else {
        return Ok(None);
    };
    store
        .get_project_repo(repo_id)
        .map_err(|e| e.to_string())?
        .map(Some)
        .ok_or_else(|| format!("Repository not found: {repo_id}"))
}

/// Move or copy a project note into `target_project_id`.
///
/// A move takes the note's child notes and their sessions along, each onto
/// the branch in the target that tracks its repo + subpath; `ensure_repo`
/// runs for every such repo the target does not track yet, as it does in
/// [`transfer_branch_note`]. A copy is standalone and has no children.
pub(crate) async fn transfer_project_note<F, Fut>(
    store: &Store,
    note_id: &str,
    target_project_id: &str,
    mode: TransferMode,
    mut ensure_repo: F,
) -> Result<ProjectNote, String>
where
    F: FnMut(RepoKey) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let note = store
        .get_project_note(note_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project note not found: {note_id}"))?;
    ensure_settled(
        store,
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

    if mode == TransferMode::Copy {
        let _media = lock_transfer_media();
        let media = stage_media(
            store,
            &note.content,
            None,
            &target.id,
            None,
            None,
            &mut HashSet::new(),
        )?;
        let mut copy = ProjectNote::new(&target.id, &note.title, &media.content);
        copy.set_initial_read(note.is_read());
        let result = store
            .copy_project_note_to_project(&copy, &media.images)
            .map(|()| copy);
        return finish(store, mode, &note.content, media, result);
    }

    // Step 1: every repo the children are bound to has to be in the target.
    let children = store
        .list_child_notes(&note.id)
        .map_err(|e| e.to_string())?;
    if !children.is_empty() {
        ensure_local_projects(store, &note, &target)?;
    }
    let mut keys: Vec<RepoKey> = Vec::new();
    for child in &children {
        let (_, key) = child_repo(store, &note, child)?;
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    // A repo that was added stays added when a later step fails — the next
    // repo's add included: removing it would tear down a worktree the user
    // may already be looking at, and the add is what a manual "Add
    // repository" would have done anyway. Every error from here on says so,
    // so the user is not left wondering where the repo came from.
    let mut added: Vec<RepoKey> = Vec::new();
    for key in keys {
        let already_tracked = store
            .find_project_repo(&target.id, &key.github_repo, key.subpath.as_deref())
            .map_err(|e| with_added_repos(e.to_string(), &added, &target))?
            .is_some();
        if !already_tracked {
            ensure_repo(key.clone()).await.map_err(|e| {
                with_added_repos(
                    format!("Could not add {} to {}: {e}", key.describe(), target.name),
                    &added,
                    &target,
                )
            })?;
            added.push(key);
        }
    }
    move_project_note_now(store, &note, &target).map_err(|e| with_added_repos(e, &added, &target))
}

/// The rest of a project-note move, once every repo it needs is in `target`:
/// read the family again, resolve each child's destination branch, stage the
/// media and run the transaction. Synchronous, so nothing can change between
/// the second read and the transaction except through the store itself —
/// which the transaction's own guards cover, keyed on each note's location,
/// session and the body read here (staging large media can take long enough
/// for a resumed session to finish again).
fn move_project_note_now(
    store: &Store,
    before: &ProjectNote,
    target: &Project,
) -> Result<ProjectNote, String> {
    let mode = TransferMode::Move;
    // Adding repos can take seconds; what moves is the family as it reads
    // now — including a child that finished, or a save that landed, meanwhile.
    let note = refetch_project_note(store, before)?;
    let mut children_with_destination: Vec<(Note, Branch)> = Vec::new();
    for child in store
        .list_child_notes(&note.id)
        .map_err(|e| e.to_string())?
    {
        let (_, key) = child_repo(store, &note, &child)?;
        let target_branch = store
            .find_branch_for_repo(&target.id, &key.github_repo, key.subpath.as_deref())
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                format!(
                    "{} tracks {} but has no branch for it.",
                    target.name,
                    key.describe()
                )
            })?;
        children_with_destination.push((child, target_branch));
    }

    // Step 2: media files for the parent and every child, staged together so
    // a failure part-way discards the lot, then step 3: the transaction. There
    // is no await past this point, so the lock is never held across one.
    let _media = lock_transfer_media();
    let mut staged: Vec<StagedMedia> = Vec::new();
    let mut reserved = HashSet::new();
    let mut plan: Vec<ChildNoteMove> = Vec::with_capacity(children_with_destination.len());
    let outcome = (|| -> Result<(), String> {
        staged.push(stage_media(
            store,
            &note.content,
            None,
            &target.id,
            note.session_id.as_deref(),
            Some(MoveSource {
                note_id: &note.id,
                project_id: &note.project_id,
                branch_id: None,
            }),
            &mut reserved,
        )?);
        for (child, target_branch) in &children_with_destination {
            let media = stage_media(
                store,
                &child.content,
                Some(&target_branch.id),
                &target.id,
                child.session_id.as_deref(),
                Some(MoveSource {
                    note_id: &child.id,
                    project_id: &note.project_id,
                    branch_id: Some(&child.branch_id),
                }),
                &mut reserved,
            )?;
            plan.push(ChildNoteMove {
                note_id: child.id.clone(),
                expected_branch_id: child.branch_id.clone(),
                expected_content: child.content.clone(),
                target_branch_id: target_branch.id.clone(),
                content: media.content.clone(),
                media: media.images.clone(),
                rehomed: media.rehomed.clone(),
            });
            staged.push(media);
        }
        Ok(())
    })();
    if let Err(e) = outcome {
        discard_all(store, staged);
        return Err(e);
    }

    // Step 3: the transaction, parent and children together.
    let parent_media = &staged[0];
    let result = store.move_project_note_to_project(
        &note.id,
        &note.project_id,
        &note.content,
        &target.id,
        &parent_media.content,
        &parent_media.images,
        &parent_media.rehomed,
        &plan,
    );
    let previous_contents: Vec<&str> = std::iter::once(note.content.as_str())
        .chain(
            children_with_destination
                .iter()
                .map(|(child, _)| child.content.as_str()),
        )
        .collect();
    finish_all(store, mode, &previous_contents, staged, result)
}

/// A failure after step 1 left `added` tracked by `target`; say so. Errors
/// from the repo add (and from git underneath it) rarely end in a period, so
/// one is supplied where needed to keep the two sentences apart.
fn with_added_repos(err: String, added: &[RepoKey], target: &Project) -> String {
    let mut names: Vec<String> = added.iter().map(RepoKey::describe).collect();
    let (list, verb, remain) = match names.len() {
        0 => return err,
        1 => (names.pop().unwrap(), "was", "remains"),
        _ => {
            let last = names.pop().unwrap();
            (format!("{} and {last}", names.join(", ")), "were", "remain")
        }
    };
    let err = err.trim_end();
    let stop = if err.ends_with(['.', '!', '?']) {
        ""
    } else {
        "."
    };
    format!(
        "{err}{stop} {list} {verb} added to {} along the way and {remain} tracked there.",
        target.name
    )
}

/// Remote branches of a project share one Blox workspace, so a child note's
/// branch cannot change projects — the same rule the branch move applies. A
/// project note without children has nothing on a workspace and still moves.
fn ensure_local_projects(
    store: &Store,
    note: &ProjectNote,
    target: &Project,
) -> Result<(), String> {
    let source = store
        .get_project(&note.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", note.project_id))?;
    for project in [&source, target] {
        if project.location == ProjectLocation::Remote {
            return Err(format!(
                "Project '{}' runs on a remote workspace; {} has child notes, which can only move between local projects.",
                project.name,
                quoted(&note.title)
            ));
        }
    }
    Ok(())
}

/// Where a child note is bound: its branch, and the repo + subpath that
/// branch tracks (what the target has to track before the child can land).
///
/// Refused while the child's session is queued or running: `sessions.branch_id`
/// is the branch queue's scheduling key, and rewriting it under a live drain
/// would hand the session to a branch that is not running it. Also refused
/// for a child on a remote branch, which the branch move excludes too, and
/// for a child whose branch is not in the parent's project: a branch move
/// detaches such children (and migration 0039 cleared the links older moves
/// left), so that link is stale, and following it would pull a note out of a
/// project this move was never asked to touch.
fn child_repo(
    store: &Store,
    parent: &ProjectNote,
    child: &Note,
) -> Result<(Branch, RepoKey), String> {
    if let Some(session_id) = child.session_id.as_deref() {
        let status = store
            .get_session(session_id)
            .map_err(|e| e.to_string())?
            .map(|session| session.status);
        if let Some(status @ (SessionStatus::Queued | SessionStatus::Running)) = status {
            return Err(format!(
                "{} has {} whose session is still {}; wait for it to finish before moving the note.",
                quoted(&parent.title),
                child_label(child),
                status.as_str()
            ));
        }
    }
    let branch = store
        .get_branch(&child.branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {}", child.branch_id))?;
    if branch.branch_type == BranchType::Remote {
        return Err(format!(
            "{} has {} on remote branch '{}'; notes on remote branches can't move between projects.",
            quoted(&parent.title),
            child_label(child),
            branch.branch_name
        ));
    }
    if branch.project_id != parent.project_id {
        return Err(format!(
            "{} is linked to {} on branch '{}', which is in a different project; the note can't move while that link stands.",
            quoted(&parent.title),
            child_label(child),
            branch.branch_name
        ));
    }
    let repo_id = branch.project_repo_id.as_deref().ok_or_else(|| {
        format!(
            "Branch '{}' is not bound to a repository, so {} of {} cannot move with it.",
            branch.branch_name,
            child_label(child),
            quoted(&parent.title)
        )
    })?;
    let repo = store
        .get_project_repo(repo_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Repository not found: {repo_id}"))?;
    Ok((
        branch,
        RepoKey {
            github_repo: repo.github_repo,
            subpath: repo.subpath,
        },
    ))
}

fn child_label(child: &Note) -> String {
    if child.title.trim().is_empty() {
        "a child note".to_string()
    } else {
        format!("child note \"{}\"", child.title)
    }
}

/// [`refetch_branch_note`] for a project note being moved.
fn refetch_project_note(store: &Store, before: &ProjectNote) -> Result<ProjectNote, String> {
    let mode = TransferMode::Move;
    let fresh = store
        .get_project_note(&before.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            format!(
                "{} was deleted while {} it.",
                quoted(&before.title),
                mode.gerund()
            )
        })?;
    ensure_settled(
        store,
        &fresh.title,
        fresh.session_id.as_deref(),
        fresh.completed_at,
        mode,
    )?;
    if fresh.project_id != before.project_id {
        return Err(format!(
            "{} was moved while {} it.",
            quoted(&fresh.title),
            mode.gerund()
        ));
    }
    Ok(fresh)
}

/// The production repo-adding step: the same path the "Add repository"
/// button takes, background worktree setup included.
pub(crate) async fn add_repo_to_project(
    app_handle: tauri::AppHandle,
    store: Arc<Store>,
    project_id: String,
    key: RepoKey,
) -> Result<(), String> {
    let added = crate::project_commands::add_project_repo_impl(
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
    crate::project_commands::spawn_project_repo_setup(
        app_handle,
        store,
        project_id,
        added.repo.id,
        added.provisions_workspace,
    );
    Ok(())
}

/// A note whose session is still producing it has no settled content to carry,
/// and the runner would keep writing to it wherever it went.
///
/// `completed_at` alone does not say so: it is set once and never cleared, so
/// a note whose session was resumed still reads as finished. For a move the
/// session's own status is checked too: a live session is scheduled by
/// `sessions.branch_id`, which the move rewrites. A copy leaves the session
/// where it is and takes a snapshot of the note as it reads now, so a
/// resumed session is no reason to refuse it — only one that never finished.
fn ensure_settled(
    store: &Store,
    title: &str,
    session_id: Option<&str>,
    completed_at: Option<i64>,
    mode: TransferMode,
) -> Result<(), String> {
    let Some(session_id) = session_id else {
        return Ok(());
    };
    if completed_at.is_none() {
        return Err(format!(
            "{} is still being generated; wait for its session to finish before {} it.",
            quoted(title),
            mode.gerund()
        ));
    }
    if mode == TransferMode::Copy {
        return Ok(());
    }
    let status = store
        .get_session(session_id)
        .map_err(|e| e.to_string())?
        .map(|session| session.status);
    if let Some(status @ (SessionStatus::Queued | SessionStatus::Running)) = status {
        return Err(format!(
            "{}'s session is {} again; wait for it to finish before {} the note.",
            quoted(title),
            status.as_str(),
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
        store,
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

/// Held from staging a transfer's media to settling it ([`finish`]). Two
/// moves of one note into one project link the same id-keyed path, so if they
/// overlapped, one could remove that path while the other's row was about to
/// name it, or already did: the second replaces the first's link, and the
/// move that loses the race discards it (see [`StagedMedia::discard`] for the
/// part a re-read can catch and the part it can't). The desktop app and the
/// web client share this process, so this covers two windows as well as two
/// clicks. Everything before staging — the add-repo await above all — stays
/// outside it; the transaction re-checks what was resolved there.
static TRANSFER_MEDIA: Mutex<()> = Mutex::new(());

fn lock_transfer_media() -> MutexGuard<'static, ()> {
    // The lock guards no data, so a transfer that panicked under it left
    // nothing half-written behind.
    TRANSFER_MEDIA
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn finish<T>(
    store: &Store,
    mode: TransferMode,
    previous_content: &str,
    media: StagedMedia,
    result: Result<T, StoreError>,
) -> Result<T, String> {
    finish_all(store, mode, &[previous_content], vec![media], result)
}

/// Settle the transaction's outcome for every note it carried: on success the
/// paths re-homed rows no longer name are unlinked, and a move
/// garbage-collects the source media the notes' old bodies referenced; on
/// failure the staged copies and links are removed again.
fn finish_all<T>(
    store: &Store,
    mode: TransferMode,
    previous_contents: &[&str],
    staged: Vec<StagedMedia>,
    result: Result<T, StoreError>,
) -> Result<T, String> {
    match result {
        Ok(value) => {
            for media in staged {
                media.commit();
            }
            if mode == TransferMode::Move {
                // Every note's media at once: one pass for the whole move.
                cleanup_unreferenced(
                    store,
                    previous_contents
                        .iter()
                        .flat_map(|content| refs::media_ids(content))
                        .collect(),
                );
            }
            Ok(value)
        }
        Err(e) => {
            discard_all(store, staged);
            Err(e.to_string())
        }
    }
}

fn discard_all(store: &Store, staged: Vec<StagedMedia>) {
    for media in staged {
        media.discard(store);
    }
}

/// Media put in place on disk but not yet recorded: the rewritten note body,
/// the copied rows to insert alongside it and the re-homed rows to re-point,
/// and the files to settle once the transaction commits or fails.
struct StagedMedia {
    content: String,
    images: Vec<Image>,
    rehomed: Vec<Image>,
    /// The file written for each of `images`, under the copy's fresh id.
    copies: Vec<PathBuf>,
    /// A hard link for each of `rehomed` (by id), at the path its row names
    /// once re-homed.
    links: Vec<(String, PathBuf)>,
    /// The paths `rehomed` rows named before the move. Each shares its file
    /// with one of `links`, so it goes once the rows name the links.
    superseded: Vec<PathBuf>,
}

impl StagedMedia {
    /// The transaction recorded the rows: drop the paths re-homed rows no
    /// longer name. One left behind is only a spare link to the same file.
    fn commit(self) {
        for path in self.superseded {
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != ErrorKind::NotFound {
                    log::warn!("Failed to unlink moved media {}: {e}", path.display());
                }
            }
        }
    }

    /// The transaction never happened: remove what was written for it. The
    /// sources were left alone, so every row still finds its file.
    ///
    /// Except where another move of the same note committed in the meantime:
    /// it linked the same path (the name is the row's id), its row names that
    /// path now, and its commit unlinked the source, so the link is the only
    /// file left. A link is kept when its row reads as naming it. That covers
    /// the other move committing first; one that commits between this read and
    /// the removal is what [`TRANSFER_MEDIA`] keeps from happening.
    fn discard(self, store: &Store) {
        for path in self.copies {
            let _ = std::fs::remove_file(path);
        }
        for (id, path) in self.links {
            if !row_names(store, &id, &path) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Whether media row `id` names `path` as its file. A row that can't be read
/// counts as naming it: a spare link costs nothing, a missing file costs the
/// media.
fn row_names(store: &Store, id: &str, path: &Path) -> bool {
    match store.get_image(id) {
        Ok(Some(row)) => image_file_path(&row.project_id, &row.id, &row.filename)
            .map_or(true, |named| named == path),
        Ok(None) => false,
        Err(e) => {
            log::warn!("Keeping media link {}: {e}", path.display());
            true
        }
    }
}

/// The scope a moving note is leaving, whose media rows that only the note
/// uses go with it instead of being copied.
struct MoveSource<'a> {
    note_id: &'a str,
    project_id: &'a str,
    branch_id: Option<&'a str>,
}

impl MoveSource<'_> {
    /// Whether `image` is a row the note could take along as it is: one in
    /// the scope the note is leaving, with a session. It goes when the note
    /// is also the only thing using it; see the module docs.
    fn holds(&self, image: &Image) -> bool {
        image.project_id == self.project_id
            && image.branch_id.as_deref() == self.branch_id
            && image.session_id.is_some()
    }
}

/// Put every stored attachment `content` references in place in
/// `project_id` (and `branch_id`), returning the rewritten body and the rows
/// to record. The rows are owned by the note that will carry them:
/// `owner_session` is that note's session, or `None` for a note without one.
///
/// On a move (`rehome` set), media only the note uses is linked into place
/// and keeps its id and references; everything else is copied under a fresh
/// id and its references are rewritten to the copy.
///
/// A reference whose row or file is already gone is left as it is: the viewer
/// renders it unavailable today and will keep doing so after the transfer,
/// which is better than failing the transfer over media that was lost before.
///
/// `reserved` holds the filenames handed out so far in this transfer: the
/// rows are not recorded until the transaction, so the store alone cannot
/// keep two of them apart, within one note or across the notes of one move.
fn stage_media(
    store: &Store,
    content: &str,
    branch_id: Option<&str>,
    project_id: &str,
    owner_session: Option<&str>,
    rehome: Option<MoveSource<'_>>,
    reserved: &mut HashSet<String>,
) -> Result<StagedMedia, String> {
    let mut staged = StagedMedia {
        content: content.to_owned(),
        images: Vec::new(),
        rehomed: Vec::new(),
        copies: Vec::new(),
        links: Vec::new(),
        superseded: Vec::new(),
    };
    let owner_session = owner_session.unwrap_or(NOTE_MEDIA_SESSION_ID);
    // How each source id is referenced from now on: `None` when it is
    // re-homed and its references stay as written. The same id is only put
    // in place once however often the note embeds it.
    let mut landed = HashMap::<String, Option<String>>::new();
    let mut edits = Vec::new();
    // What else uses the note's media, read for all of it the first time a
    // row could move as it is: one pass per note, not one per attachment.
    let mut references = None;
    let result = (|| -> Result<(), String> {
        for reference in refs::extract_media_refs(content) {
            let Some((id, _)) = refs::stored_ref(&reference.target) else {
                continue;
            };
            let replacement = match landed.get(id) {
                Some(replacement) => replacement.clone(),
                None => {
                    let Some(source) = store.get_image(id).map_err(|e| e.to_string())? else {
                        continue;
                    };
                    let source_path =
                        image_file_path(&source.project_id, &source.id, &source.filename)?;
                    let filename = store
                        .unique_image_filename_excluding(
                            branch_id,
                            project_id,
                            &source.filename,
                            reserved,
                        )
                        .map_err(|e| e.to_string())?;
                    let rehomes = match &rehome {
                        Some(from) if from.holds(&source) => {
                            if references.is_none() {
                                references = Some(
                                    store
                                        .media_references(
                                            refs::media_ids(content).iter().map(String::as_str),
                                            Some(from.note_id),
                                        )
                                        .map_err(|e| e.to_string())?,
                                );
                            }
                            references
                                .as_ref()
                                .is_some_and(|references| references.unused(&source.id))
                        }
                        _ => false,
                    };
                    let replacement = if rehomes {
                        if !rehome_file(&source_path, project_id, &source, &mut staged)? {
                            continue;
                        }
                        staged.rehomed.push(Image {
                            project_id: project_id.to_owned(),
                            branch_id: branch_id.map(str::to_owned),
                            filename: filename.clone(),
                            session_id: Some(owner_session.to_owned()),
                            ..source
                        });
                        None
                    } else {
                        let Some(copy) = copy_file(
                            &source_path,
                            &source,
                            branch_id,
                            project_id,
                            &filename,
                            owner_session,
                            &mut staged,
                        )?
                        else {
                            continue;
                        };
                        Some(format!(
                            "staged-media://{}.{}",
                            copy.id,
                            files::extension(&copy.filename)
                        ))
                    };
                    reserved.insert(filename);
                    landed.insert(id.to_owned(), replacement.clone());
                    replacement
                }
            };
            if let Some(replacement) = replacement {
                edits.push((reference.destination, replacement));
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        staged.discard(store);
        return Err(e);
    }
    // Apply back to front so earlier offsets stay valid.
    for (span, replacement) in edits.into_iter().rev() {
        staged.content.replace_range(span, &replacement);
    }
    Ok(staged)
}

/// Hard-link `source`'s file into `project_id`'s media directory, where its
/// row will say it is once re-homed; the name is the id and extension, neither
/// of which changes. The source path stays until the transaction commits
/// ([`StagedMedia::commit`]), so whatever resolves the row in the meantime
/// still finds the file. Notes cannot embed PDFs, so there is no cached
/// thumbnail beside it to carry. False when the file is already gone.
fn rehome_file(
    source_path: &Path,
    project_id: &str,
    source: &Image,
    staged: &mut StagedMedia,
) -> Result<bool, String> {
    if !regular_file(source_path)? {
        return Ok(false);
    }
    let path = image_file_path(project_id, &source.id, &source.filename)?;
    if path == source_path {
        return Err("media is already in the destination project".into());
    }
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::hard_link(source_path, &path) {
        if e.kind() != ErrorKind::AlreadyExists {
            return Err(e.to_string());
        }
        // A link an earlier attempt left when it never committed. The path is
        // keyed by this row's id and the row names the source, and no other
        // transfer is staging (`TRANSFER_MEDIA`), so nothing else is using it.
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        std::fs::hard_link(source_path, &path).map_err(|e| e.to_string())?;
    }
    staged.links.push((source.id.clone(), path));
    staged.superseded.push(source_path.to_owned());
    Ok(true)
}

/// Whether the media file at `path` is there to transfer. Only a file that
/// does not exist counts as already gone; any other failure to inspect it
/// fails the transfer rather than leaving the note pointing at the source,
/// and so does anything but a regular file.
fn regular_file(path: &Path) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err("media must be a regular file".into()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("cannot inspect media file: {e}")),
    }
}

/// Copy `source`'s file to a fresh id in the destination and stage the row
/// for it. The bytes are copied by the filesystem (a clone on APFS) rather
/// than read into memory, since a note video can run to 100 MB; only an
/// image whose row predates the dimension columns is read, to probe it.
/// `None` when the file is already gone.
fn copy_file(
    source_path: &Path,
    source: &Image,
    branch_id: Option<&str>,
    project_id: &str,
    filename: &str,
    owner_session: &str,
    staged: &mut StagedMedia,
) -> Result<Option<Image>, String> {
    if !regular_file(source_path)? {
        return Ok(None);
    }
    let mut copy = Image::new(
        branch_id,
        project_id,
        filename,
        &source.mime_type,
        source.size_bytes,
        false,
    );
    copy.session_id = Some(owner_session.to_owned());
    copy.dimensions = source.dimensions.or_else(|| {
        source
            .mime_type
            .starts_with("image/")
            .then(|| files::read_local(source_path, files::MAX_IMAGE_SIZE).ok())
            .flatten()
            .and_then(|bytes| crate::note_media::dimensions::probe(&bytes))
    });
    let path = image_file_path(project_id, &copy.id, filename)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::copy(source_path, &path) {
        let _ = std::fs::remove_file(&path);
        return Err(e.to_string());
    }
    staged.copies.push(path);
    staged.images.push(copy.clone());
    Ok(Some(copy))
}

// =============================================================================
// Tauri commands
// =============================================================================

/// The repo a branch note is bound to, read from the stored note's branch, so
/// the picker can create a remote destination from the repo the transfer will
/// look for. `None` when the branch is bound to none.
#[tauri::command(rename_all = "camelCase")]
pub async fn get_note_repo(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    note_id: String,
) -> Result<Option<ProjectRepo>, String> {
    let store = crate::get_store(&store)?;
    tauri::async_runtime::spawn_blocking(move || branch_note_repo(&store, &note_id))
        .await
        .map_err(|e| format!("get_note_repo task failed: {e}"))?
}

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

/// Move a project note and its child notes into another project, adding any
/// repo a child is bound to that the project does not track yet.
#[tauri::command(rename_all = "camelCase")]
pub async fn move_project_note_to_project(
    app_handle: tauri::AppHandle,
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
        |key| {
            add_repo_to_project(
                app_handle.clone(),
                Arc::clone(&store),
                target_project_id.clone(),
                key,
            )
        },
    )
    .await
}

/// Copy a project note into another project (or the same one). A copy is
/// standalone, so no child moves along and no repo is ever added.
#[tauri::command(rename_all = "camelCase")]
pub async fn copy_project_note_to_project(
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
        |key| {
            std::future::ready(Err(format!(
                "A copy never adds a repository ({}).",
                key.describe()
            )))
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::note_media::tests::PNG;
    use crate::store::images::{take_reference_passes, ReferencePasses};
    use crate::store::models::{Project, ProjectRepo, Session};

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
            self.attachment_with_session(filename, Some(session.id))
        }

        /// [`Self::attachment`] under `session_id`; `None` makes it a
        /// branch-timeline attachment.
        fn attachment_with_session(&self, filename: &str, session_id: Option<String>) -> Image {
            let mut image = Image::new(
                Some(&self.source_branch.id),
                &self.source.id,
                filename,
                "image/png",
                PNG.len() as i64,
                false,
            );
            image.session_id = session_id;
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

        /// A stored project-note attachment in `source`, written to disk.
        fn project_attachment(&self) -> Image {
            let session = Session::new_running("note", Path::new("/tmp/wt"));
            self.store.create_session(&session).unwrap();
            let mut image = Image::new(
                None,
                &self.source.id,
                "shot.png",
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

    /// The user made the copy, so it reads as its source did rather than
    /// lighting up as new.
    #[tokio::test]
    async fn a_copy_takes_its_sources_read_state() {
        let f = fixture();
        let unread = f.note("Unread", "body");
        let read = f.note("Read", "body");
        f.store.set_note_read(&read.id, true).unwrap();

        let unread_copy = transfer_branch_note(
            &f.store,
            &unread.id,
            &f.source.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();
        let read_copy = transfer_branch_note(
            &f.store,
            &read.id,
            &f.source.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();

        assert!(!unread_copy.is_read());
        assert!(read_copy.is_read());
        let stored_read_at = |id: &str| f.store.get_note(id).unwrap().unwrap().read_at;
        assert_eq!(stored_read_at(&unread_copy.id), None);
        assert_eq!(stored_read_at(&read_copy.id), read_copy.read_at);
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
    /// whichever session they were attached under before.
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

        let moved_id = refs::media_ids(&moved.content).into_iter().next().unwrap();
        let moved_media = f.store.get_image(&moved_id).unwrap().unwrap();
        assert_eq!(moved_media.session_id.as_deref(), Some(session.id.as_str()));
    }

    /// Media only the moving note uses goes with it as it is: the row is
    /// re-homed and the file linked into the destination (and unlinked from
    /// the source once the move commits), so the id — and any reference to it
    /// outside the note's body — survives the move.
    #[tokio::test]
    async fn move_takes_media_only_the_note_uses_along_with_its_id() {
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

        assert_eq!(moved.content, note.content);
        let rehomed = f.store.get_image(&image.id).unwrap().unwrap();
        assert_eq!(rehomed.project_id, f.target.id);
        assert_eq!(rehomed.branch_id, Some(f.target_branch().unwrap().id));
        // A written note has no session, so its media is marked as such.
        assert_eq!(rehomed.session_id.as_deref(), Some(NOTE_MEDIA_SESSION_ID));
        assert_eq!(rehomed.filename, "shot.png");
        let moved_path = image_file_path(&f.target.id, &image.id, &image.filename).unwrap();
        assert_eq!(std::fs::read(moved_path).unwrap(), PNG);
        let original_path = image_file_path(&f.source.id, &image.id, &image.filename).unwrap();
        assert!(!original_path.exists());
    }

    /// The bug a re-home fixes: the note's session takes another turn after
    /// the move and repeats a reference it saw before it, either stored or as
    /// the `/tmp` path an earlier context build materialized it to. A copy
    /// had a fresh id and the original was collected, so both resolved to
    /// nothing.
    #[tokio::test]
    async fn a_moved_notes_next_turn_keeps_media_it_referenced_before_the_move() {
        let f = fixture();
        let image = f.attachment("shot.png");
        let session_id = image.session_id.clone().unwrap();
        f.store
            .update_session_status(&session_id, SessionStatus::Completed, None, None)
            .unwrap();
        let before = format!("![a](staged-media://{}.png)", image.id);
        let note =
            Note::new_standalone(&f.source_branch.id, "Shots", &before).with_session(&session_id);
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

        // What session_runner's note completion does with the next turn's body,
        // scoped to where the note now is.
        let target_branch = f.target_branch().unwrap();
        let rewrite = format!(
            "{}/staged-image-{}.png",
            std::env::temp_dir()
                .display()
                .to_string()
                .trim_end_matches('/'),
            image.id
        );
        for body in [before.clone(), format!("![a]({rewrite})")] {
            let outcome = crate::note_media::ingest_note_media(
                &f.store,
                crate::note_media::NoteScope {
                    branch_id: Some(&target_branch.id),
                    project_id: &f.target.id,
                    previous_content: &moved.content,
                },
                &session_id,
                None,
                Path::new("/tmp/wt"),
                &body,
            );
            assert!(
                !outcome.markdown.contains("unavailable"),
                "rewrite {body:?} became {:?}",
                outcome.markdown
            );
        }
    }

    /// A row the note shares — with another note, a chat message, or the
    /// branch timeline — is copied on a move, and the original stays where
    /// it is for whatever else uses it.
    #[tokio::test]
    async fn move_copies_media_something_else_still_uses() {
        let f = fixture();
        let embedded = f.attachment("embedded.png");
        let other = f.note(
            "Other",
            &format!("![o](staged-media://{}.png)", embedded.id),
        );
        let chatted = f.attachment("chatted.png");
        f.store
            .add_session_message_with_images(
                chatted.session_id.as_deref().unwrap(),
                crate::store::models::MessageRole::User,
                "look",
                std::slice::from_ref(&chatted.id),
            )
            .unwrap();
        let timeline = f.attachment_with_session("timeline.png", None);
        let note = f.note(
            "Shots",
            &format!(
                "![a](staged-media://{}.png) ![b](staged-media://{}.png) ![c](staged-media://{}.png)",
                embedded.id, chatted.id, timeline.id
            ),
        );

        let moved = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Move,
            |key| std::future::ready(f.attach_repo(key)),
        )
        .await
        .unwrap();

        let moved_ids = refs::media_ids(&moved.content);
        assert_eq!(moved_ids.len(), 3);
        for original in [&embedded, &chatted, &timeline] {
            assert!(
                !moved_ids.contains(&original.id),
                "{} was taken along",
                original.filename
            );
            let kept = f.store.get_image(&original.id).unwrap().unwrap();
            assert_eq!(kept.project_id, f.source.id);
            assert_eq!(kept.branch_id.as_ref(), Some(&f.source_branch.id));
            assert_eq!(kept.session_id, original.session_id);
            assert!(
                image_file_path(&f.source.id, &original.id, &original.filename)
                    .unwrap()
                    .exists(),
                "{} lost its file",
                original.filename
            );
        }
        for id in &moved_ids {
            let copy = f.store.get_image(id).unwrap().unwrap();
            assert_eq!(copy.project_id, f.target.id);
            assert!(image_file_path(&f.target.id, &copy.id, &copy.filename)
                .unwrap()
                .exists());
        }
        assert_eq!(
            f.store.get_note(&other.id).unwrap().unwrap().content,
            other.content
        );
        assert_eq!(
            f.store
                .list_images_for_branch(&f.source_branch.id)
                .unwrap()
                .len(),
            1
        );
    }

    /// Staging a move reads what else uses the note's media once, and takes
    /// along exactly the rows the per-image check calls the note's alone.
    #[test]
    fn staging_a_move_checks_its_media_in_one_pass_and_rehomes_what_the_per_image_check_would() {
        let f = fixture();
        let alone = [f.attachment("one.png"), f.attachment("two.png")];
        let embedded = f.attachment("embedded.png");
        f.note(
            "Other",
            &format!("![o](staged-media://{}.png)", embedded.id),
        );
        let chatted = f.attachment("chatted.png");
        f.store
            .add_session_message_with_images(
                chatted.session_id.as_deref().unwrap(),
                crate::store::models::MessageRole::User,
                "look",
                std::slice::from_ref(&chatted.id),
            )
            .unwrap();
        let timeline = f.attachment_with_session("timeline.png", None);
        let all = [&alone[0], &alone[1], &embedded, &chatted, &timeline];
        let note = f.note(
            "Shots",
            &all.map(|image| format!("![a](staged-media://{}.png)", image.id))
                .join(" "),
        );
        f.attach_repo(RepoKey {
            github_repo: "acme/widgets".into(),
            subpath: Some("apps/web".into()),
        })
        .unwrap();
        let target_branch = f.target_branch().unwrap();
        let source = || MoveSource {
            note_id: &note.id,
            project_id: &f.source.id,
            branch_id: Some(&f.source_branch.id),
        };
        // The rule, one image at a time, as staging applied it before.
        let expected: HashSet<String> = all
            .iter()
            .filter(|image| {
                source().holds(image) && f.store.media_is_exclusive_to(&image.id, &note.id).unwrap()
            })
            .map(|image| image.id.clone())
            .collect();
        assert_eq!(
            expected,
            HashSet::from([alone[0].id.clone(), alone[1].id.clone()])
        );

        take_reference_passes();
        let media = stage_media(
            &f.store,
            &note.content,
            Some(&target_branch.id),
            &f.target.id,
            None,
            Some(source()),
            &mut HashSet::new(),
        )
        .unwrap();
        assert_eq!(
            take_reference_passes(),
            ReferencePasses { notes: 1, chat: 1 }
        );

        let rehomed: HashSet<String> = media.rehomed.iter().map(|row| row.id.clone()).collect();
        assert_eq!(rehomed, expected);
        assert_eq!(media.images.len(), 3);
        media.discard(&f.store);

        // A copy takes nothing along, so it has nothing to ask.
        let copy = stage_media(
            &f.store,
            &note.content,
            Some(&target_branch.id),
            &f.target.id,
            None,
            None,
            &mut HashSet::new(),
        )
        .unwrap();
        assert_eq!(take_reference_passes(), ReferencePasses::default());
        assert!(copy.rehomed.is_empty());
        assert_eq!(copy.images.len(), 5);
        copy.discard(&f.store);
    }

    /// A move passes over the note bodies and the chat attachments three
    /// times (staging, the check under the transaction, the collection after
    /// the commit), whether the note embeds one image or eight.
    #[tokio::test]
    async fn a_move_makes_three_reference_passes_however_many_images_the_note_embeds() {
        for count in [1, 8] {
            let f = fixture();
            let images: Vec<Image> = (0..count)
                .map(|n| f.attachment(&format!("shot-{n}.png")))
                .collect();
            let note = f.note(
                "Shots",
                &images
                    .iter()
                    .map(|image| format!("![a](staged-media://{}.png)", image.id))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            );
            take_reference_passes();

            transfer_branch_note(
                &f.store,
                &note.id,
                &f.target.id,
                TransferMode::Move,
                |key| std::future::ready(f.attach_repo(key)),
            )
            .await
            .unwrap();

            assert_eq!(
                take_reference_passes(),
                ReferencePasses { notes: 3, chat: 3 },
                "{count} images"
            );
            // Every image went along as it is and survived the collection.
            for image in &images {
                let row = f.store.get_image(&image.id).unwrap().unwrap();
                assert_eq!(row.project_id, f.target.id);
                assert!(image_file_path(&f.target.id, &row.id, &row.filename)
                    .unwrap()
                    .exists());
            }
        }
    }

    /// A copy takes no media from the source, so it never asks what uses it.
    #[tokio::test]
    async fn a_copy_makes_no_reference_pass() {
        let f = fixture();
        let (note, _) = f.media_note();
        take_reference_passes();

        transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            |key| std::future::ready(f.attach_repo(key)),
        )
        .await
        .unwrap();

        assert_eq!(take_reference_passes(), ReferencePasses::default());
    }

    /// A project-note move stages and re-checks each note against its own
    /// body, so those two passes are per note that has media to take along;
    /// the collection after the commit is one pass for the whole family.
    #[tokio::test]
    async fn a_project_note_move_passes_once_per_note_to_stage_and_recheck_and_once_to_collect() {
        let f = fixture();
        let parent_image = f.project_attachment();
        let parent = ProjectNote::new(
            &f.source.id,
            "Plan",
            &format!("![a](staged-media://{}.png)", parent_image.id),
        );
        f.store.create_project_note(&parent).unwrap();
        let child_images = [f.attachment("one.png"), f.attachment("two.png")];
        let (child, _) = f.child_of(&parent, &f.source_branch.id, "Findings");
        f.store
            .update_note_content(
                &child.id,
                &child_images
                    .each_ref()
                    .map(|image| format!("![a](staged-media://{}.png)", image.id))
                    .join(" "),
            )
            .unwrap();
        take_reference_passes();

        transfer_project_note(
            &f.store,
            &parent.id,
            &f.target.id,
            TransferMode::Move,
            |key| std::future::ready(f.attach_repo(key)),
        )
        .await
        .unwrap();

        // Two notes staged, two re-checked, one collection.
        assert_eq!(
            take_reference_passes(),
            ReferencePasses { notes: 5, chat: 5 }
        );
        for image in [&parent_image, &child_images[0], &child_images[1]] {
            let row = f.store.get_image(&image.id).unwrap().unwrap();
            assert_eq!(row.project_id, f.target.id);
        }
    }

    /// The files are linked into the destination before the transaction that
    /// re-homes their rows, and the source path stays until it commits: the
    /// file is at whichever path the row names throughout. A transaction that
    /// fails removes the link, and the row that stayed in the source still
    /// finds its file.
    #[test]
    fn a_failed_move_leaves_the_media_where_its_row_says() {
        let f = fixture();
        let (note, image) = f.media_note();
        f.attach_repo(RepoKey {
            github_repo: "acme/widgets".into(),
            subpath: Some("apps/web".into()),
        })
        .unwrap();
        let target_branch = f.target_branch().unwrap();
        let original_path = image_file_path(&f.source.id, &image.id, &image.filename).unwrap();
        let moved_path = image_file_path(&f.target.id, &image.id, &image.filename).unwrap();

        let media = stage_media(
            &f.store,
            &note.content,
            Some(&target_branch.id),
            &f.target.id,
            None,
            Some(MoveSource {
                note_id: &note.id,
                project_id: &f.source.id,
                branch_id: Some(&f.source_branch.id),
            }),
            &mut HashSet::new(),
        )
        .unwrap();
        // Staged but not yet recorded: the row still names the source.
        assert_eq!(std::fs::read(&original_path).unwrap(), PNG);
        assert_eq!(std::fs::read(&moved_path).unwrap(), PNG);
        // The note left the branch the move was resolved from in the
        // meantime, so the transaction is refused.
        let result = f.store.move_note_to_branch(
            &note.id,
            "stale-branch",
            &target_branch.id,
            &media.content,
            &media.images,
            &media.rehomed,
        );
        let err = finish(&f.store, TransferMode::Move, &note.content, media, result).unwrap_err();

        assert!(err.contains("moved or deleted"), "{err}");
        assert_eq!(std::fs::read(&original_path).unwrap(), PNG);
        assert!(!moved_path.exists());
        let row = f.store.get_image(&image.id).unwrap().unwrap();
        assert_eq!(row.project_id, f.source.id);
        assert_eq!(row.branch_id.as_ref(), Some(&f.source_branch.id));
        assert_eq!(row.session_id, image.session_id);
        assert_eq!(
            f.store.get_note(&note.id).unwrap().unwrap().branch_id,
            f.source_branch.id
        );
    }

    /// Two moves of one note into one project link the same id-keyed path,
    /// the second replacing the first's link. Whichever commits unlinks the
    /// source, so the link is then the only path to the file, and the move
    /// that is refused (its note has left the branch) must keep it when it
    /// discards. The transfer lock stops two moves in one process from
    /// interleaving at all; this stages them by hand, in both commit orders.
    #[test]
    fn a_refused_concurrent_move_keeps_the_link_the_other_committed() {
        for first_commits in [true, false] {
            let f = fixture();
            let (note, image) = f.media_note();
            f.attach_repo(RepoKey {
                github_repo: "acme/widgets".into(),
                subpath: Some("apps/web".into()),
            })
            .unwrap();
            let target_branch = f.target_branch().unwrap();
            let stage = || {
                stage_media(
                    &f.store,
                    &note.content,
                    Some(&target_branch.id),
                    &f.target.id,
                    None,
                    Some(MoveSource {
                        note_id: &note.id,
                        project_id: &f.source.id,
                        branch_id: Some(&f.source_branch.id),
                    }),
                    &mut HashSet::new(),
                )
                .unwrap()
            };
            let record = |media: &StagedMedia| {
                f.store.move_note_to_branch(
                    &note.id,
                    &f.source_branch.id,
                    &target_branch.id,
                    &media.content,
                    &media.images,
                    &media.rehomed,
                )
            };

            let first = stage();
            // Finds the first's link in place and replaces it.
            let second = stage();
            let (winner, loser) = if first_commits {
                (first, second)
            } else {
                (second, first)
            };
            let committed = record(&winner);
            finish(
                &f.store,
                TransferMode::Move,
                &note.content,
                winner,
                committed,
            )
            .unwrap();
            let refused = record(&loser);
            let err =
                finish(&f.store, TransferMode::Move, &note.content, loser, refused).unwrap_err();

            assert!(err.contains("moved or deleted"), "{err}");
            let row = f.store.get_image(&image.id).unwrap().unwrap();
            assert_eq!(row.project_id, f.target.id);
            let path = image_file_path(&row.project_id, &row.id, &row.filename).unwrap();
            assert_eq!(
                std::fs::read(&path).ok().as_deref(),
                Some(PNG),
                "first_commits = {first_commits}: the committed row lost its file"
            );
            assert!(!image_file_path(&f.source.id, &image.id, &image.filename)
                .unwrap()
                .exists());
        }
    }

    /// A process that died between linking and committing leaves a link at
    /// the destination that no row names. Moving the note again replaces it
    /// rather than failing on the existing path.
    #[tokio::test]
    async fn a_link_left_by_an_interrupted_move_does_not_block_the_next() {
        let f = fixture();
        let (note, image) = f.media_note();
        let moved_path = image_file_path(&f.target.id, &image.id, &image.filename).unwrap();
        std::fs::create_dir_all(moved_path.parent().unwrap()).unwrap();
        std::fs::write(&moved_path, b"stale").unwrap();

        let moved = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Move,
            |key| std::future::ready(f.attach_repo(key)),
        )
        .await
        .unwrap();

        assert_eq!(moved.content, note.content);
        assert_eq!(std::fs::read(&moved_path).unwrap(), PNG);
        let original_path = image_file_path(&f.source.id, &image.id, &image.filename).unwrap();
        assert!(!original_path.exists());
    }

    /// Only a file that does not exist is skipped as already gone. One that
    /// is not a regular file, or that can't be inspected at all, fails the
    /// transfer: carrying the reference over as it is would leave a copy
    /// relying on the source project's media, or a moved note on a row that
    /// stayed behind.
    #[tokio::test]
    async fn media_that_cannot_be_inspected_fails_the_transfer() {
        let f = fixture();
        let (note, image) = f.media_note();
        let original_path = image_file_path(&f.source.id, &image.id, &image.filename).unwrap();
        let images_dir = original_path.parent().unwrap().to_owned();
        let not_a_file = || {
            std::fs::remove_file(&original_path).unwrap();
            std::fs::create_dir(&original_path).unwrap();
        };
        // Inspecting a path under a regular file fails with ENOTDIR, an error
        // that is not "not found".
        let unreadable = || {
            std::fs::remove_dir_all(&images_dir).unwrap();
            std::fs::write(&images_dir, b"").unwrap();
        };
        let cases: [(&dyn Fn(), &str); 2] = [
            (&not_a_file, "media must be a regular file"),
            (&unreadable, "cannot inspect media file"),
        ];
        for (break_source, expected) in cases {
            break_source();
            for mode in [TransferMode::Copy, TransferMode::Move] {
                let err = transfer_branch_note(&f.store, &note.id, &f.target.id, mode, |key| {
                    std::future::ready(f.attach_repo(key))
                })
                .await
                .unwrap_err();
                assert!(err.contains(expected), "{mode:?}: {err}");
                assert_eq!(
                    f.store.get_note(&note.id).unwrap().unwrap().branch_id,
                    f.source_branch.id
                );
            }
            // Put the source back as a regular file for the next case.
            let _ = std::fs::remove_dir_all(&images_dir);
            let _ = std::fs::remove_file(&images_dir);
            std::fs::create_dir_all(&images_dir).unwrap();
            std::fs::write(&original_path, PNG).unwrap();
        }
        assert!(f
            .store
            .list_notes_for_branch(&f.target_branch().unwrap().id)
            .unwrap()
            .is_empty());
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
    fn note_repo_is_the_one_its_branch_is_bound_to() {
        let f = fixture();
        let note = f.note("Findings", "body");

        let repo = branch_note_repo(&f.store, &note.id).unwrap().unwrap();
        assert_eq!(repo.id, f.source_branch.project_repo_id.clone().unwrap());
        assert_eq!(repo.github_repo, "acme/widgets");
        assert_eq!(repo.subpath.as_deref(), Some("apps/web"));
    }

    #[test]
    fn note_repo_is_none_for_a_branch_bound_to_no_repo() {
        let f = fixture();
        let unbound = Branch::new(&f.source.id, "loose", "origin/main");
        f.store.create_branch(&unbound).unwrap();
        let note = f.note_on(&unbound.id, "Findings", "body");

        assert!(branch_note_repo(&f.store, &note.id).unwrap().is_none());
    }

    #[test]
    fn note_repo_of_a_missing_note_is_an_error() {
        let f = fixture();

        let err = branch_note_repo(&f.store, "gone").unwrap_err();
        assert_eq!(err, "Note not found: gone");
    }

    impl Fixture {
        /// A project note in `source` with one finished child note on
        /// `source_branch`, the shape `start_repo_session` leaves behind.
        fn family(&self) -> (ProjectNote, Note, Session) {
            let parent = ProjectNote::new(&self.source.id, "Plan", "body");
            self.store.create_project_note(&parent).unwrap();
            let (child, session) = self.child_of(&parent, &self.source_branch.id, "Findings");
            (parent, child, session)
        }

        /// A child note of `parent` on `branch_id` whose session has completed.
        fn child_of(&self, parent: &ProjectNote, branch_id: &str, title: &str) -> (Note, Session) {
            let session =
                Session::new_running("child", Path::new("/tmp/wt")).with_branch(branch_id);
            self.store.create_session(&session).unwrap();
            self.store
                .update_session_status(&session.id, SessionStatus::Completed, None, None)
                .unwrap();
            let child = Note::new_standalone(branch_id, title, "child body")
                .with_session(&session.id)
                .with_parent_project_note(&parent.id);
            self.store.create_note(&child).unwrap();
            (child, session)
        }

        fn assert_family_unmoved(&self, parent: &ProjectNote, child: &Note, session: &Session) {
            assert_eq!(
                self.store
                    .get_project_note(&parent.id)
                    .unwrap()
                    .unwrap()
                    .project_id,
                self.source.id
            );
            let stored = self.store.get_note(&child.id).unwrap().unwrap();
            assert_eq!(stored.branch_id, child.branch_id);
            assert_eq!(
                stored.parent_project_note_id.as_deref(),
                Some(parent.id.as_str())
            );
            assert_eq!(
                self.store.get_branch_id_for_session(&session.id).unwrap(),
                Some(child.branch_id.clone())
            );
        }
    }

    #[tokio::test]
    async fn project_note_move_takes_its_children_adding_the_repo_the_target_lacks() {
        let f = fixture();
        let parent = ProjectNote::new(&f.source.id, "Plan", "body");
        f.store.create_project_note(&parent).unwrap();
        let image = f.attachment("shot.png");
        let (child, session) = f.child_of(&parent, &f.source_branch.id, "Findings");
        f.store
            .update_note_content(
                &child.id,
                &format!("see ![a](staged-media://{}.png)", image.id),
            )
            .unwrap();
        let added = Mutex::new(Vec::new());

        let moved = transfer_project_note(
            &f.store,
            &parent.id,
            &f.target.id,
            TransferMode::Move,
            |key| {
                added.lock().unwrap().push(key.clone());
                std::future::ready(f.attach_repo(key))
            },
        )
        .await
        .unwrap();

        assert_eq!(
            added.lock().unwrap().as_slice(),
            [RepoKey {
                github_repo: "acme/widgets".into(),
                subpath: Some("apps/web".into()),
            }]
        );
        assert_eq!(moved.project_id, f.target.id);
        let target_branch = f.target_branch().expect("repo was added with a branch");
        let child = f.store.get_note(&child.id).unwrap().unwrap();
        assert_eq!(child.branch_id, target_branch.id);
        assert_eq!(
            child.parent_project_note_id.as_deref(),
            Some(parent.id.as_str())
        );
        assert_eq!(
            f.store.get_branch_id_for_session(&session.id).unwrap(),
            Some(target_branch.id.clone())
        );
        assert_eq!(f.store.list_child_notes(&parent.id).unwrap().len(), 1);
        assert!(f
            .store
            .list_notes_for_branch(&f.source_branch.id)
            .unwrap()
            .is_empty());
        // Media only the child uses moved into the target with it, keeping
        // its id (so the body is unchanged), owned by its session, and the
        // source path was unlinked.
        assert_eq!(
            refs::media_ids(&child.content)
                .into_iter()
                .collect::<Vec<_>>(),
            [image.id.clone()]
        );
        let rehomed = f.store.get_image(&image.id).unwrap().unwrap();
        assert_eq!(rehomed.project_id, f.target.id);
        assert_eq!(
            rehomed.branch_id.as_deref(),
            Some(target_branch.id.as_str())
        );
        assert_eq!(rehomed.session_id.as_deref(), Some(session.id.as_str()));
        assert!(
            image_file_path(&f.target.id, &rehomed.id, &rehomed.filename)
                .unwrap()
                .exists()
        );
        assert!(!image_file_path(&f.source.id, &image.id, &image.filename)
            .unwrap()
            .exists());
    }

    #[tokio::test]
    async fn project_note_move_reuses_the_branch_the_target_already_has() {
        let f = fixture();
        f.attach_repo(RepoKey {
            github_repo: "acme/widgets".into(),
            subpath: Some("apps/web".into()),
        })
        .unwrap();
        let (parent, child, _) = f.family();

        transfer_project_note(
            &f.store,
            &parent.id,
            &f.target.id,
            TransferMode::Move,
            never_called,
        )
        .await
        .unwrap();

        assert_eq!(
            f.store.get_note(&child.id).unwrap().unwrap().branch_id,
            f.target_branch().unwrap().id
        );
    }

    /// `sessions.branch_id` is the branch queue's scheduling key; a child
    /// mid-flight cannot change branches under its drain.
    #[tokio::test]
    async fn project_note_move_waits_for_a_queued_or_running_child_session() {
        let f = fixture();
        let (parent, child, session) = f.family();

        for status in [SessionStatus::Running, SessionStatus::Queued] {
            f.store
                .update_session_status(&session.id, status.clone(), None, None)
                .unwrap();

            let err = transfer_project_note(
                &f.store,
                &parent.id,
                &f.target.id,
                TransferMode::Move,
                never_called,
            )
            .await
            .unwrap_err();

            assert_eq!(
                err,
                format!(
                    "\"Plan\" has child note \"Findings\" whose session is still {}; wait for it to finish before moving the note.",
                    status.as_str()
                )
            );
            f.assert_family_unmoved(&parent, &child, &session);
            assert!(f.store.list_project_repos(&f.target.id).unwrap().is_empty());
        }
    }

    /// `completed_at` is never cleared, so a parent whose session was resumed
    /// still reads as finished; its live status is what refuses the move.
    #[tokio::test]
    async fn project_note_move_waits_for_a_resumed_parent_session() {
        let f = fixture();
        let parent_session = Session::new_running("plan", Path::new("/tmp/wt"));
        f.store.create_session(&parent_session).unwrap();
        f.store
            .update_session_status(&parent_session.id, SessionStatus::Completed, None, None)
            .unwrap();
        let parent =
            ProjectNote::new(&f.source.id, "Plan", "body").with_session(&parent_session.id);
        f.store.create_project_note(&parent).unwrap();
        f.store.mark_project_note_completed(&parent.id).unwrap();
        let (child, session) = f.child_of(&parent, &f.source_branch.id, "Findings");

        for status in [SessionStatus::Running, SessionStatus::Queued] {
            f.store
                .update_session_status(&parent_session.id, status.clone(), None, None)
                .unwrap();

            let err = transfer_project_note(
                &f.store,
                &parent.id,
                &f.target.id,
                TransferMode::Move,
                never_called,
            )
            .await
            .unwrap_err();

            assert_eq!(
                err,
                format!(
                    "\"Plan\"'s session is {} again; wait for it to finish before moving the note.",
                    status.as_str()
                )
            );
            f.assert_family_unmoved(&parent, &child, &session);
        }

        // Resumed while the repo was being added: the second read refuses too,
        // and the repo that was added stays.
        f.store
            .update_session_status(&parent_session.id, SessionStatus::Completed, None, None)
            .unwrap();
        let err = transfer_project_note(
            &f.store,
            &parent.id,
            &f.target.id,
            TransferMode::Move,
            |key| {
                f.store
                    .update_session_status(&parent_session.id, SessionStatus::Running, None, None)
                    .unwrap();
                std::future::ready(f.attach_repo(key))
            },
        )
        .await
        .unwrap_err();
        assert_eq!(
            err,
            "\"Plan\"'s session is running again; wait for it to finish before moving the note. \
             acme/widgets at apps/web was added to target along the way and remains tracked there."
        );
        f.assert_family_unmoved(&parent, &child, &session);
        assert!(f.target_branch().is_some());
    }

    /// A child that is resumed while the repo is being added fails the second
    /// read; the repo stays added, and the error says so.
    #[tokio::test]
    async fn child_resumed_during_the_repo_add_stops_the_move_and_names_the_added_repo() {
        let f = fixture();
        let (parent, child, session) = f.family();

        let err = transfer_project_note(
            &f.store,
            &parent.id,
            &f.target.id,
            TransferMode::Move,
            |key| {
                f.store
                    .update_session_status(&session.id, SessionStatus::Running, None, None)
                    .unwrap();
                std::future::ready(f.attach_repo(key))
            },
        )
        .await
        .unwrap_err();

        assert_eq!(
            err,
            "\"Plan\" has child note \"Findings\" whose session is still running; wait for it to finish before moving the note. \
             acme/widgets at apps/web was added to target along the way and remains tracked there."
        );
        f.assert_family_unmoved(&parent, &child, &session);
        assert!(f.target_branch().is_some());
    }

    #[test]
    fn with_added_repos_lists_every_repo_that_was_added() {
        let target = Project::named("target");
        let widgets = RepoKey {
            github_repo: "acme/widgets".into(),
            subpath: Some("apps/web".into()),
        };
        let gadgets = RepoKey {
            github_repo: "acme/gadgets".into(),
            subpath: None,
        };
        let tools = RepoKey {
            github_repo: "acme/tools".into(),
            subpath: None,
        };
        assert_eq!(with_added_repos("Boom.".into(), &[], &target), "Boom.");
        assert_eq!(
            with_added_repos("Boom.".into(), std::slice::from_ref(&widgets), &target),
            "Boom. acme/widgets at apps/web was added to target along the way and remains tracked there."
        );
        assert_eq!(
            with_added_repos("Boom.".into(), std::slice::from_ref(&widgets), &target),
            "Boom. acme/widgets at apps/web was added to target along the way and remains tracked there."
        );
        // An error that ends mid-sentence gets its full stop.
        assert_eq!(
            with_added_repos("Workspace is not running".into(), std::slice::from_ref(&widgets), &target),
            "Workspace is not running. acme/widgets at apps/web was added to target along the way and remains tracked there."
        );
        assert_eq!(
            with_added_repos("Boom.".into(), &[widgets, gadgets, tools], &target),
            "Boom. acme/widgets at apps/web, acme/gadgets and acme/tools were added to target along the way and remain tracked there."
        );
    }

    /// A child whose branch is in another project is a stale link (branch
    /// moves detach as they go; migration 0039 cleared the old ones), not a
    /// note this move may pull along.
    #[tokio::test]
    async fn project_note_move_is_refused_for_a_child_on_a_branch_in_another_project() {
        let f = fixture();
        let (parent, _, _) = f.family();
        f.attach_repo(RepoKey {
            github_repo: "acme/widgets".into(),
            subpath: Some("apps/web".into()),
        })
        .unwrap();
        let elsewhere = f.target_branch().unwrap();
        let (stray, _) = f.child_of(&parent, &elsewhere.id, "Stray");

        let err = transfer_project_note(
            &f.store,
            &parent.id,
            &f.target.id,
            TransferMode::Move,
            never_called,
        )
        .await
        .unwrap_err();

        assert_eq!(
            err,
            "\"Plan\" is linked to child note \"Stray\" on branch 'target-feature', which is in a different project; the note can't move while that link stands."
        );
        assert_eq!(
            f.store
                .get_project_note(&parent.id)
                .unwrap()
                .unwrap()
                .project_id,
            f.source.id
        );
        assert_eq!(
            f.store.get_note(&stray.id).unwrap().unwrap().branch_id,
            elsewhere.id
        );
    }

    #[tokio::test]
    async fn failed_repo_add_leaves_the_family_in_the_source_project() {
        let f = fixture();
        let (parent, child, session) = f.family();

        let err = transfer_project_note(
            &f.store,
            &parent.id,
            &f.target.id,
            TransferMode::Move,
            |_| std::future::ready(Err::<(), _>("Workspace is not running".to_string())),
        )
        .await
        .unwrap_err();

        assert!(
            err.contains("Could not add acme/widgets at apps/web to target")
                && err.contains("Workspace is not running"),
            "{err}"
        );
        f.assert_family_unmoved(&parent, &child, &session);
        assert!(f.target_branch().is_none());
    }

    /// Children on two repos the target lacks: when the second add fails, the
    /// error still names the first, which stays added.
    #[tokio::test]
    async fn failed_second_repo_add_names_the_repo_added_before_it() {
        let f = fixture();
        let (parent, child, session) = f.family();
        let gadgets = ProjectRepo::new(&f.source.id, "acme/gadgets", "gadgets", None);
        f.store.create_project_repo(&gadgets).unwrap();
        let gadgets_branch =
            Branch::new(&f.source.id, "gadgets", "origin/main").with_project_repo(&gadgets.id);
        f.store.create_branch(&gadgets_branch).unwrap();
        let (gadget_child, gadget_session) =
            f.child_of(&parent, &gadgets_branch.id, "Gadget findings");
        let calls = Mutex::new(Vec::new());

        let err = transfer_project_note(
            &f.store,
            &parent.id,
            &f.target.id,
            TransferMode::Move,
            |key| {
                let mut calls = calls.lock().unwrap();
                calls.push(key.clone());
                std::future::ready(if calls.len() == 1 {
                    f.attach_repo(key)
                } else {
                    Err("Workspace is not running".to_string())
                })
            },
        )
        .await
        .unwrap_err();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert_eq!(
            err,
            format!(
                "Could not add {} to target: Workspace is not running. {} was added to target along the way and remains tracked there.",
                calls[1].describe(),
                calls[0].describe()
            )
        );
        f.assert_family_unmoved(&parent, &child, &session);
        f.assert_family_unmoved(&parent, &gadget_child, &gadget_session);
        // The first repo is tracked by the target; the second is not.
        let tracked: Vec<String> = f
            .store
            .list_project_repos(&f.target.id)
            .unwrap()
            .into_iter()
            .map(|r| r.github_repo)
            .collect();
        assert_eq!(tracked, [calls[0].github_repo.clone()]);
    }

    /// A copy is a snapshot that leaves the session where it is, so a session
    /// that is running again does not stop it — only a move.
    #[tokio::test]
    async fn copies_proceed_while_the_notes_session_is_running_again() {
        let f = fixture();
        let session =
            Session::new_running("note", Path::new("/tmp/wt")).with_branch(&f.source_branch.id);
        f.store.create_session(&session).unwrap();
        let note = Note::new(&f.source_branch.id, "Findings", "body").with_session(&session.id);
        f.store.create_note(&note).unwrap();
        let project_session = Session::new_running("plan", Path::new("/tmp/wt"));
        f.store.create_session(&project_session).unwrap();
        let project_note =
            ProjectNote::new(&f.source.id, "Plan", "body").with_session(&project_session.id);
        f.store.create_project_note(&project_note).unwrap();
        f.store
            .mark_project_note_completed(&project_note.id)
            .unwrap();
        f.attach_repo(RepoKey {
            github_repo: "acme/widgets".into(),
            subpath: Some("apps/web".into()),
        })
        .unwrap();

        let copy = transfer_branch_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();
        assert_eq!(copy.branch_id, f.target_branch().unwrap().id);
        assert_eq!(copy.content, "body");
        assert!(copy.session_id.is_none());

        let copy = transfer_project_note(
            &f.store,
            &project_note.id,
            &f.target.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();
        assert_eq!(copy.project_id, f.target.id);
        assert!(copy.session_id.is_none());

        // The same sessions still refuse a move.
        for (id, label) in [(&note.id, "Findings"), (&project_note.id, "Plan")] {
            let err = if id == &note.id {
                transfer_branch_note(&f.store, id, &f.target.id, TransferMode::Move, never_called)
                    .await
                    .unwrap_err()
            } else {
                transfer_project_note(&f.store, id, &f.target.id, TransferMode::Move, never_called)
                    .await
                    .unwrap_err()
            };
            assert_eq!(
                err,
                format!(
                    "\"{label}\"'s session is running again; wait for it to finish before moving the note."
                )
            );
        }
    }

    #[tokio::test]
    async fn project_note_move_is_refused_for_a_child_on_a_remote_branch() {
        let f = fixture();
        let (parent, _, _) = f.family();
        let remote = Branch::new_remote(&f.source.id, "remote-feature", "origin/main", "ws-1")
            .with_project_repo(f.source_branch.project_repo_id.as_deref().unwrap());
        f.store.create_branch(&remote).unwrap();
        let (remote_child, _) = f.child_of(&parent, &remote.id, "Remote findings");

        let err = transfer_project_note(
            &f.store,
            &parent.id,
            &f.target.id,
            TransferMode::Move,
            never_called,
        )
        .await
        .unwrap_err();

        assert_eq!(
            err,
            "\"Plan\" has child note \"Remote findings\" on remote branch 'remote-feature'; notes on remote branches can't move between projects."
        );
        assert_eq!(
            f.store
                .get_project_note(&parent.id)
                .unwrap()
                .unwrap()
                .project_id,
            f.source.id
        );
        assert_eq!(
            f.store
                .get_note(&remote_child.id)
                .unwrap()
                .unwrap()
                .branch_id,
            remote.id
        );
    }

    /// A childless project note can still move to or from a remote project;
    /// one with children cannot, since their branches would have to.
    #[tokio::test]
    async fn project_note_move_into_a_remote_project_is_refused_only_with_children() {
        let f = fixture();
        let mut remote = Project::named("remote");
        remote.location = ProjectLocation::Remote;
        f.store.create_project(&remote).unwrap();
        let childless = ProjectNote::new(&f.source.id, "Childless", "body");
        f.store.create_project_note(&childless).unwrap();
        let (parent, child, session) = f.family();

        transfer_project_note(
            &f.store,
            &childless.id,
            &remote.id,
            TransferMode::Move,
            never_called,
        )
        .await
        .unwrap();
        let err = transfer_project_note(
            &f.store,
            &parent.id,
            &remote.id,
            TransferMode::Move,
            never_called,
        )
        .await
        .unwrap_err();

        assert_eq!(
            err,
            "Project 'remote' runs on a remote workspace; \"Plan\" has child notes, which can only move between local projects."
        );
        f.assert_family_unmoved(&parent, &child, &session);
    }

    #[tokio::test]
    async fn project_note_moves_directly_and_copies_duplicate() {
        let f = fixture();
        let note = ProjectNote::new(&f.source.id, "Plan", "body");
        f.store.create_project_note(&note).unwrap();

        let err = transfer_project_note(
            &f.store,
            &note.id,
            &f.source.id,
            TransferMode::Move,
            never_called,
        )
        .await
        .unwrap_err();
        assert_eq!(err, "\"Plan\" is already in source.");

        let copy = transfer_project_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();
        assert_ne!(copy.id, note.id);
        assert_eq!(copy.project_id, f.target.id);
        assert_eq!(copy.content, "body");
        assert!(copy.session_id.is_none());

        let moved = transfer_project_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Move,
            never_called,
        )
        .await
        .unwrap();
        assert_eq!(moved.id, note.id);
        assert_eq!(moved.project_id, f.target.id);
        assert!(f.store.list_project_notes(&f.source.id).unwrap().is_empty());
        assert_eq!(f.store.list_project_notes(&f.target.id).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_project_note_copy_takes_its_sources_read_state() {
        let f = fixture();
        let note = ProjectNote::new(&f.source.id, "Plan", "body");
        f.store.create_project_note(&note).unwrap();

        let unread_copy = transfer_project_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();
        f.store.set_project_note_read(&note.id, true).unwrap();
        let read_copy = transfer_project_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();

        assert!(!unread_copy.is_read());
        assert!(read_copy.is_read());
        let stored_read_at = |id: &str| f.store.get_project_note(id).unwrap().unwrap().read_at;
        assert_eq!(stored_read_at(&unread_copy.id), None);
        assert_eq!(stored_read_at(&read_copy.id), read_copy.read_at);
    }

    /// A copy is standalone: the children stay with the original, and so
    /// nothing needs adding to the target even when it lacks their repo.
    #[tokio::test]
    async fn project_note_copy_leaves_the_children_with_the_original() {
        let f = fixture();
        let (parent, child, _) = f.family();

        let copy = transfer_project_note(
            &f.store,
            &parent.id,
            &f.target.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();

        assert!(f.store.list_child_notes(&copy.id).unwrap().is_empty());
        assert_eq!(
            f.store
                .get_note(&child.id)
                .unwrap()
                .unwrap()
                .parent_project_note_id
                .as_deref(),
            Some(parent.id.as_str())
        );
    }

    #[tokio::test]
    async fn project_note_media_follows_the_copy() {
        let f = fixture();
        let session = Session::new_running("note", Path::new("/tmp/wt"));
        f.store.create_session(&session).unwrap();
        // A row from before the dimension columns: the copy reads the file.
        let retina = crate::note_media::dimensions::tests::PHYS_144DPI_PNG;
        let mut image = Image::new(
            None,
            &f.source.id,
            "shot.png",
            "image/png",
            retina.len() as i64,
            false,
        );
        image.session_id = Some(session.id.clone());
        let path = image_file_path(&f.source.id, &image.id, &image.filename).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, retina).unwrap();
        f.store.create_image(&image).unwrap();
        f.store.forget_image_dimensions(&image.id).unwrap();
        let note = ProjectNote::new(
            &f.source.id,
            "Plan",
            &format!("![a](staged-media://{}.png)", image.id),
        );
        f.store.create_project_note(&note).unwrap();

        let copy = transfer_project_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();

        let copied_id = refs::media_ids(&copy.content).into_iter().next().unwrap();
        let copied = f.store.get_image(&copied_id).unwrap().unwrap();
        assert_eq!(copied.project_id, f.target.id);
        assert!(copied.branch_id.is_none());
        assert!(image_file_path(&f.target.id, &copied.id, &copied.filename)
            .unwrap()
            .exists());
        assert_eq!(
            copied.dimensions,
            Some(crate::store::ImageDimensions {
                width: 8,
                height: 6,
                density: 2
            })
        );
    }

    #[tokio::test]
    async fn project_note_move_takes_its_media_along_with_its_id() {
        let f = fixture();
        let image = f.project_attachment();
        let content = format!("![a](staged-media://{}.png)", image.id);
        let note = ProjectNote::new(&f.source.id, "Plan", &content);
        f.store.create_project_note(&note).unwrap();

        let moved = transfer_project_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Move,
            never_called,
        )
        .await
        .unwrap();

        assert_eq!(moved.content, content);
        let rehomed = f.store.get_image(&image.id).unwrap().unwrap();
        assert_eq!(rehomed.project_id, f.target.id);
        assert!(rehomed.branch_id.is_none());
        assert_eq!(
            std::fs::read(image_file_path(&f.target.id, &image.id, &image.filename).unwrap())
                .unwrap(),
            PNG
        );
        assert!(!image_file_path(&f.source.id, &image.id, &image.filename)
            .unwrap()
            .exists());
    }

    /// A copy hands the file to the filesystem rather than reading it, and
    /// only an image is opened to read its size: a video's row keeps none.
    #[tokio::test]
    async fn a_copied_video_is_cloned_without_being_sized() {
        let f = fixture();
        let bytes = b"\0\0\0\x18ftypisom\0\0\0\0not really a movie";
        let session = Session::new_running("note", Path::new("/tmp/wt"));
        f.store.create_session(&session).unwrap();
        let mut video = Image::new(
            None,
            &f.source.id,
            "clip.mp4",
            "video/mp4",
            bytes.len() as i64,
            false,
        );
        video.session_id = Some(session.id.clone());
        let path = image_file_path(&f.source.id, &video.id, &video.filename).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        f.store.create_image(&video).unwrap();
        let note = ProjectNote::new(
            &f.source.id,
            "Plan",
            &format!("![v](staged-media://{}.mp4)", video.id),
        );
        f.store.create_project_note(&note).unwrap();

        let copy = transfer_project_note(
            &f.store,
            &note.id,
            &f.target.id,
            TransferMode::Copy,
            never_called,
        )
        .await
        .unwrap();

        let copied_id = refs::media_ids(&copy.content).into_iter().next().unwrap();
        assert_ne!(copied_id, video.id);
        let copied = f.store.get_image(&copied_id).unwrap().unwrap();
        assert_eq!(copied.mime_type, "video/mp4");
        assert!(copied.dimensions.is_none());
        assert!(!f.store.image_dimensions_recorded(&copied.id).unwrap());
        assert_eq!(
            std::fs::read(image_file_path(&f.target.id, &copied.id, &copied.filename).unwrap())
                .unwrap(),
            bytes
        );
        assert!(path.exists());
    }

    fn assert_resolves_in(store: &Store, content: &str, project_id: &str) {
        let refs = refs::extract_media_refs(content);
        assert!(!refs.is_empty(), "no media refs left in {content:?}");
        for reference in refs {
            let file = reference.target.strip_prefix("staged-media://").unwrap();
            let (path, mime) = crate::note_media::serving::lookup(store, file)
                .unwrap_or_else(|s| panic!("{file} does not resolve: {s}"));
            assert_eq!(mime, "image/png");
            assert_eq!(std::fs::read(&path).unwrap(), PNG, "{}", path.display());
            let (id, _) = refs::stored_ref(&reference.target).unwrap();
            assert_eq!(store.get_image(id).unwrap().unwrap().project_id, project_id);
        }
    }

    /// Every transfer leaves its media resolvable through the real lookup at
    /// the destination, and still does once the source project is deleted
    /// (taking its media rows with it).
    #[tokio::test]
    async fn every_transfer_leaves_media_resolvable_at_the_destination() {
        for mode in [TransferMode::Move, TransferMode::Copy] {
            let f = fixture();
            let (note, _) = f.media_note();
            let landed = transfer_branch_note(&f.store, &note.id, &f.target.id, mode, |key| {
                std::future::ready(f.attach_repo(key))
            })
            .await
            .unwrap();
            assert_resolves_in(&f.store, &landed.content, &f.target.id);
            f.store.delete_project(&f.source.id).unwrap();
            assert_resolves_in(&f.store, &landed.content, &f.target.id);
        }
        for mode in [TransferMode::Move, TransferMode::Copy] {
            let f = fixture();
            let image = f.project_attachment();
            let note = ProjectNote::new(
                &f.source.id,
                "Plan",
                &format!("![a](staged-media://{}.png)", image.id),
            );
            f.store.create_project_note(&note).unwrap();
            let landed =
                transfer_project_note(&f.store, &note.id, &f.target.id, mode, never_called)
                    .await
                    .unwrap();
            assert_resolves_in(&f.store, &landed.content, &f.target.id);
            f.store.delete_project(&f.source.id).unwrap();
            assert_resolves_in(&f.store, &landed.content, &f.target.id);
        }
    }
}
