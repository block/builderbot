//! Tests for the store module.

use std::collections::HashSet;
use std::path::Path;

use super::models::*;
use super::{
    ChildSessionInfo, SessionPollRequest, Store, ToolCallBody, TranscriptWindow,
    DEFERRED_TOOL_BODY_MIN_BYTES,
};

#[test]
fn test_create_and_get_project() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let fetched = store.get_project(&project.id).unwrap().unwrap();
    assert_eq!(fetched.github_repo.as_deref(), Some("test-owner/test-repo"));
    assert!(fetched.subpath.is_none());
}

#[test]
fn test_project_with_subpath() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/monorepo").with_subpath("packages/app".to_string());
    store.create_project(&project).unwrap();

    let fetched = store.get_project(&project.id).unwrap().unwrap();
    assert_eq!(fetched.subpath.as_deref(), Some("packages/app"));
}

#[test]
fn test_project_allows_same_repo_with_different_subpaths() {
    let store = Store::in_memory().unwrap();
    let p1 = Project::new("test-owner/test-repo").with_subpath("packages/app-a".to_string());
    let p2 = Project::new("test-owner/test-repo").with_subpath("packages/app-b".to_string());
    store.create_project(&p1).unwrap();
    store.create_project(&p2).unwrap();

    let projects = store.list_projects().unwrap();
    assert_eq!(projects.len(), 2);
}

#[test]
fn test_project_unique_repo_and_subpath() {
    let store = Store::in_memory().unwrap();
    let p1 = Project::new("test-owner/test-repo").with_subpath("packages/app".to_string());
    let p2 = Project::new("test-owner/test-repo").with_subpath("packages/app".to_string());
    store.create_project(&p1).unwrap();
    assert!(store.create_project(&p2).is_err());
}

#[test]
fn test_project_unique_repo_with_no_subpath() {
    let store = Store::in_memory().unwrap();
    let p1 = Project::new("test-owner/test-repo");
    let p2 = Project::new("test-owner/test-repo");
    store.create_project(&p1).unwrap();
    assert!(store.create_project(&p2).is_err());
}

#[test]
fn test_list_projects() {
    let store = Store::in_memory().unwrap();
    store
        .create_project(&Project::new("test-owner/repo-a"))
        .unwrap();
    store
        .create_project(&Project::new("test-owner/repo-b"))
        .unwrap();
    let projects = store.list_projects().unwrap();
    assert_eq!(projects.len(), 2);
}

#[test]
fn test_list_projects_returns_newest_first() {
    let store = Store::in_memory().unwrap();

    let mut oldest = Project::new("test-owner/repo-oldest");
    oldest.created_at = 1_000;
    let mut middle = Project::new("test-owner/repo-middle");
    middle.created_at = 2_000;
    let mut newest = Project::new("test-owner/repo-newest");
    newest.created_at = 3_000;

    // Insert out of timestamp order so the ordering can't come from insert order.
    store.create_project(&middle).unwrap();
    store.create_project(&newest).unwrap();
    store.create_project(&oldest).unwrap();

    let ids: Vec<String> = store
        .list_projects()
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, vec![newest.id, middle.id, oldest.id]);
}

#[test]
fn test_list_projects_breaks_created_at_ties_by_insert_order() {
    let store = Store::in_memory().unwrap();

    let mut first = Project::new("test-owner/repo-first");
    first.created_at = 5_000;
    let mut second = Project::new("test-owner/repo-second");
    second.created_at = 5_000;

    store.create_project(&first).unwrap();
    store.create_project(&second).unwrap();

    let ids: Vec<String> = store
        .list_projects()
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, vec![second.id, first.id]);
}

#[test]
fn test_rename_project_moves_only_the_name() {
    let store = Store::in_memory().unwrap();
    let mut project = Project::new("test-owner/test-repo");
    project.subpath = Some("apps/one".to_string());
    store.create_project(&project).unwrap();

    store.rename_project(&project.id, "Renamed").unwrap();

    let fetched = store.get_project(&project.id).unwrap().unwrap();
    assert_eq!(fetched.name, "Renamed");
    assert_eq!(fetched.github_repo, project.github_repo);
    assert_eq!(fetched.location, project.location);
    assert_eq!(fetched.subpath.as_deref(), Some("apps/one"));
    assert_eq!(fetched.created_at, project.created_at);
    assert!(fetched.updated_at >= project.updated_at);
    assert_eq!(store.list_projects().unwrap()[0].name, "Renamed");

    let err = store.rename_project("missing", "Nope").unwrap_err();
    assert!(err.to_string().contains("Project not found"), "{err}");
}

#[test]
fn test_project_note_sets_completed_at_when_created_with_content() {
    let note = ProjectNote::new("project-1", "Title", "Body");
    assert_eq!(note.completed_at, Some(note.created_at));
}

#[test]
fn test_project_note_completion_is_write_once() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let note = ProjectNote::new(&project.id, "", "");
    store.create_project_note(&note).unwrap();

    let before = store.get_project_note(&note.id).unwrap().unwrap();
    assert!(before.completed_at.is_none());

    store
        .update_project_note_title_and_content(&note.id, "First", "Initial content", None, None)
        .unwrap();
    let completed = store.get_project_note(&note.id).unwrap().unwrap();
    let first_completed_at = completed.completed_at.unwrap();

    std::thread::sleep(std::time::Duration::from_millis(2));
    store
        .update_project_note_title_and_content(&note.id, "Second", "Updated content", None, None)
        .unwrap();
    let updated = store.get_project_note(&note.id).unwrap().unwrap();

    assert_eq!(updated.completed_at, Some(first_completed_at));
    assert!(updated.updated_at >= completed.updated_at);
}

#[test]
fn test_update_project_note_title_and_content_is_noop_when_unchanged() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let note = ProjectNote::new(&project.id, "", "");
    store.create_project_note(&note).unwrap();

    store
        .update_project_note_title_and_content(
            &note.id,
            "Title",
            "Body",
            Some("commit-step"),
            Some("note-step"),
        )
        .unwrap();
    let after_first = store.get_project_note(&note.id).unwrap().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(2));

    // Same title/content/steps — must not bump updated_at.
    store
        .update_project_note_title_and_content(
            &note.id,
            "Title",
            "Body",
            Some("commit-step"),
            Some("note-step"),
        )
        .unwrap();
    let after_second = store.get_project_note(&note.id).unwrap().unwrap();
    assert_eq!(after_second.updated_at, after_first.updated_at);
    assert_eq!(after_second.completed_at, after_first.completed_at);

    // A real change still advances updated_at.
    std::thread::sleep(std::time::Duration::from_millis(2));
    store
        .update_project_note_title_and_content(
            &note.id,
            "Title",
            "New body",
            Some("commit-step"),
            Some("note-step"),
        )
        .unwrap();
    let after_third = store.get_project_note(&note.id).unwrap().unwrap();
    assert!(after_third.updated_at > after_first.updated_at);
}

#[test]
fn test_list_project_notes_orders_by_completion_time() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let older = ProjectNote::new(&project.id, "", "").with_session("session-older");
    store.create_project_note(&older).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));

    let newer = ProjectNote::new(&project.id, "", "").with_session("session-newer");
    store.create_project_note(&newer).unwrap();

    store
        .update_project_note_title_and_content(&newer.id, "Newer", "Completed first", None, None)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    store
        .update_project_note_title_and_content(&older.id, "Older", "Completed second", None, None)
        .unwrap();

    let notes = store.list_project_notes(&project.id).unwrap();
    let ordered_ids: Vec<_> = notes.iter().map(|note| note.id.as_str()).collect();
    assert_eq!(ordered_ids, vec![older.id.as_str(), newer.id.as_str()]);
}

#[test]
fn test_get_project_note_with_status_is_not_project_scoped() {
    let store = Store::in_memory().unwrap();
    let other_project = Project::new("test-owner/other-repo");
    store.create_project(&other_project).unwrap();

    let session = Session::new_running("write the project note", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    let note = ProjectNote::new(&other_project.id, "Foreign", "Body").with_session(&session.id);
    store.create_project_note(&note).unwrap();

    // A `#project-note:<id>` reference clicked from another project has no
    // project scope to search, so the lookup is by id alone.
    let fetched = store
        .get_project_note_with_status(&note.id)
        .unwrap()
        .unwrap();
    assert_eq!(fetched.project_id, other_project.id);
    assert_eq!(fetched.session_status.as_deref(), Some("running"));

    assert!(store
        .get_project_note_with_status("missing")
        .unwrap()
        .is_none());
}

#[test]
fn test_list_project_notes_with_status_fills_chat_activity() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let session = Session::new_running("write the project note", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    // Written well before its reply: a read only counts replies past the note.
    let session_note = ProjectNote {
        updated_at: 0,
        ..ProjectNote::new(&project.id, "Overview", "Body").with_session(&session.id)
    };
    store.create_project_note(&session_note).unwrap();
    let plain_note = ProjectNote::new(&project.id, "Pasted", "Body");
    store.create_project_note(&plain_note).unwrap();
    store
        .add_session_message(&session.id, MessageRole::Assistant, "a reply")
        .unwrap();
    store.mark_session_chat_read(&session.id).unwrap();
    let (last_reply_at, read_at) = store.session_chat_activity(&session.id).unwrap();
    assert!(last_reply_at.is_some() && read_at.is_some());

    let notes = store.list_project_notes_with_status(&project.id).unwrap();
    let chat_of = |id: &str| {
        let note = notes.iter().find(|note| note.id == id).unwrap();
        (note.last_chat_message_at, note.chat_read_at)
    };
    assert_eq!(chat_of(&session_note.id), (last_reply_at, read_at));
    assert_eq!(chat_of(&plain_note.id), (None, None));

    // The single-note refresh a finished session triggers carries them too.
    let refreshed = store
        .get_project_note_by_session_with_status(&session.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (refreshed.last_chat_message_at, refreshed.chat_read_at),
        (last_reply_at, read_at)
    );
}

#[test]
fn test_delete_project_cascades() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // Create a workdir assigned to this branch
    let workdir = Workdir::new(&project.id, "/tmp/wt").with_branch(&branch.id);
    store.create_workdir(&workdir).unwrap();

    let session = Session::new_running("do something", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    // Link the session to a commit on this branch
    let commit = Commit::new_with_sha(&branch.id, "abc123").with_session(&session.id);
    store.create_commit(&commit).unwrap();

    store.delete_project(&project.id).unwrap();

    // Branch cascades from project, commits cascade from branch
    assert!(store.get_branch(&branch.id).unwrap().is_none());
    assert!(store.get_commit(&commit.id).unwrap().is_none());
    // Workdir cascades from project
    assert!(store.get_workdir(&workdir.id).unwrap().is_none());
    // Session is still running, so the cleanup trigger leaves it alone
    assert!(store.get_session(&session.id).unwrap().is_some());
}

#[test]
fn test_branch_crud() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let branch = Branch::new(&project.id, "feature", "main").with_pr(42);
    store.create_branch(&branch).unwrap();

    let fetched = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(fetched.branch_name, "feature");
    assert_eq!(fetched.pr_number, Some(42));
    assert_eq!(fetched.branch_type, BranchType::Local);
    assert!(fetched.workspace_name.is_none());
    assert!(fetched.workspace_status.is_none());
    // No agent field — branch has workspace_name for remote tracking only

    store.update_branch_base(&branch.id, "develop").unwrap();
    let updated = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(updated.base_branch, "develop");
}

#[test]
fn test_remote_branch_crud() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let branch = Branch::new_remote(&project.id, "remote-feature", "main", "my-workspace");
    store.create_branch(&branch).unwrap();

    let fetched = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(fetched.branch_name, "remote-feature");
    assert_eq!(fetched.branch_type, BranchType::Remote);
    assert_eq!(fetched.workspace_name.as_deref(), Some("my-workspace"));
    assert_eq!(fetched.workspace_status, Some(WorkspaceStatus::Starting));
}

#[test]
fn test_update_workspace_status() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let branch = Branch::new_remote(&project.id, "remote-feature", "main", "my-workspace");
    store.create_branch(&branch).unwrap();

    // Starts as Starting
    let fetched = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(fetched.workspace_status, Some(WorkspaceStatus::Starting));

    // Transition to Running
    store
        .update_branch_workspace_status(&branch.id, &WorkspaceStatus::Running)
        .unwrap();
    let running = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(running.workspace_status, Some(WorkspaceStatus::Running));

    // Transition to Stopped
    store
        .update_branch_workspace_status(&branch.id, &WorkspaceStatus::Stopped)
        .unwrap();
    let stopped = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(stopped.workspace_status, Some(WorkspaceStatus::Stopped));

    // Transition to Error
    store
        .update_branch_workspace_status(&branch.id, &WorkspaceStatus::Error)
        .unwrap();
    let errored = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(errored.workspace_status, Some(WorkspaceStatus::Error));
}

#[test]
fn test_list_branches_includes_both_types() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let local = Branch::new(&project.id, "local-feature", "main");
    let remote = Branch::new_remote(&project.id, "remote-feature", "main", "ws-1");
    store.create_branch(&local).unwrap();
    store.create_branch(&remote).unwrap();

    let branches = store.list_branches_for_project(&project.id).unwrap();
    assert_eq!(branches.len(), 2);

    let local_branch = branches
        .iter()
        .find(|b| b.branch_name == "local-feature")
        .unwrap();
    let remote_branch = branches
        .iter()
        .find(|b| b.branch_name == "remote-feature")
        .unwrap();
    assert_eq!(local_branch.branch_type, BranchType::Local);
    assert_eq!(remote_branch.branch_type, BranchType::Remote);
    assert_eq!(remote_branch.workspace_name.as_deref(), Some("ws-1"));
}

#[test]
fn test_list_branch_ids() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let local = Branch::new(&project.id, "local-feature", "main");
    let remote = Branch::new_remote(&project.id, "remote-feature", "main", "ws-1");
    store.create_branch(&local).unwrap();
    store.create_branch(&remote).unwrap();

    let branch_ids: HashSet<String> = store.list_branch_ids().unwrap().into_iter().collect();
    assert_eq!(
        branch_ids,
        HashSet::from([local.id.clone(), remote.id.clone()])
    );
}

#[test]
fn test_session_lifecycle() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("fix the bug", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    // Running
    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(fetched.status, SessionStatus::Running);

    // Complete
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    let completed = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(completed.status, SessionStatus::Completed);
    assert!(completed.error_message.is_none());
}

#[test]
fn test_session_error() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("do stuff", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    store
        .update_session_status(&session.id, SessionStatus::Error, Some("boom"), None)
        .unwrap();
    let failed = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(failed.status, SessionStatus::Error);
    assert_eq!(failed.error_message.as_deref(), Some("boom"));
}

#[test]
fn test_session_error_message_ignored_for_non_error() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("do stuff", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    // Even if error_message is passed, it's ignored for non-Error status
    store
        .update_session_status(
            &session.id,
            SessionStatus::Completed,
            Some("should be ignored"),
            None,
        )
        .unwrap();
    let completed = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(completed.status, SessionStatus::Completed);
    assert!(completed.error_message.is_none());
}

#[test]
fn test_transition_from_running() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("race me", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    // Simulate: cancel_session sets status to cancelled via direct update
    store
        .update_session_status(&session.id, SessionStatus::Cancelled, None, None)
        .unwrap();
    let after_cancel = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(after_cancel.status, SessionStatus::Cancelled);

    // Simulate: background thread tries to set completed — should be a no-op
    let transitioned = store
        .transition_from_running(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    assert!(!transitioned);

    // Status is still cancelled, not overwritten
    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Cancelled);
}

#[test]
fn test_transition_from_running_succeeds_when_running() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("happy path", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    // No concurrent cancel — transition should succeed
    let transitioned = store
        .transition_from_running(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    assert!(transitioned);

    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Completed);
}

#[test]
fn test_transition_from_running_keeps_message_for_cancelled() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("timed out", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    // A cancelled run may carry an explanation (e.g. a Pikchr sub-session
    // killed by its tool-call timeout); completed runs still clear it.
    let transitioned = store
        .transition_from_running(
            &session.id,
            SessionStatus::Cancelled,
            Some("the call timed out"),
            Some(&CompletionReason::Interrupted),
        )
        .unwrap();
    assert!(transitioned);

    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Cancelled);
    assert_eq!(
        final_state.error_message.as_deref(),
        Some("the call timed out")
    );

    let completed = Session::new_running("clean finish", Path::new("/tmp"));
    store.create_session(&completed).unwrap();
    store
        .transition_from_running(
            &completed.id,
            SessionStatus::Completed,
            Some("should be dropped"),
            None,
        )
        .unwrap();
    let completed_state = store.get_session(&completed.id).unwrap().unwrap();
    assert!(completed_state.error_message.is_none());
}

#[test]
fn test_transition_from_owned_active_succeeds_when_queued() {
    let store = Store::in_memory().unwrap();

    // Queued rows carry no owner, and count as the caller's by convention.
    let session = Session::new_queued("queued");
    store.create_session(&session).unwrap();

    let transitioned = store
        .transition_from_owned_active(
            &session.id,
            SessionStatus::Cancelled,
            None,
            None,
            std::process::id(),
        )
        .unwrap();
    assert!(transitioned);

    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Cancelled);
}

#[test]
fn test_transition_from_owned_active_succeeds_for_our_running_session() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("ours", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let transitioned = store
        .transition_from_owned_active(
            &session.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::AppQuit),
            std::process::id(),
        )
        .unwrap();
    assert!(transitioned);

    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Cancelled);
    assert_eq!(
        final_state.completion_reason,
        Some(CompletionReason::AppQuit)
    );
}

/// The race this guard exists for: between the quit sweep's snapshot and its
/// write, another instance can claim a queued row — one statement that flips it
/// to `running` and stamps *its* pid. Liveness alone still matches, so an
/// ownership-blind CAS would cancel that instance's live session.
#[test]
fn test_transition_from_owned_active_leaves_another_instances_session_alone() {
    let store = Store::in_memory().unwrap();

    // What that claim leaves behind: running, with someone else's pid on it.
    let mut theirs = Session::new_running("theirs", Path::new("/tmp"));
    theirs.owner_pid = Some(std::process::id().wrapping_add(1));
    store.create_session(&theirs).unwrap();

    let transitioned = store
        .transition_from_owned_active(
            &theirs.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::AppQuit),
            std::process::id(),
        )
        .unwrap();
    assert!(!transitioned);

    let final_state = store.get_session(&theirs.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Running);
    assert_eq!(final_state.completion_reason, None);
}

#[test]
fn test_transition_from_owned_active_does_not_overwrite_completed_session() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("completed first", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();

    let transitioned = store
        .transition_from_owned_active(
            &session.id,
            SessionStatus::Cancelled,
            None,
            None,
            std::process::id(),
        )
        .unwrap();
    assert!(!transitioned);

    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Completed);
}

/// A remote session a dead instance left running is taken over by whichever
/// instance sweeps it first; the next sweep (here or elsewhere) finds it owned.
#[test]
fn test_adopt_running_session_takes_a_dead_owners_row_once() {
    let store = Store::in_memory().unwrap();
    let dead_pid = std::process::id().wrapping_add(1);

    let mut orphaned = Session::new_running("orphaned", Path::new("/tmp"));
    orphaned.owner_pid = Some(dead_pid);
    store.create_session(&orphaned).unwrap();

    assert!(store
        .adopt_running_session(&orphaned.id, Some(dead_pid))
        .unwrap());
    let adopted = store.get_session(&orphaned.id).unwrap().unwrap();
    assert_eq!(adopted.status, SessionStatus::Running);
    assert_eq!(adopted.owner_pid, Some(std::process::id()));

    assert!(
        !store
            .adopt_running_session(&orphaned.id, Some(dead_pid))
            .unwrap(),
        "a row already taken over is not taken again"
    );

    let mut finished = Session::new_running("finished", Path::new("/tmp"));
    finished.owner_pid = None;
    store.create_session(&finished).unwrap();
    store
        .update_session_status(&finished.id, SessionStatus::Completed, None, None)
        .unwrap();
    assert!(!store.adopt_running_session(&finished.id, None).unwrap());
}

#[test]
fn test_transition_queued_to_running_claims_queued_session() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_queued("queued");
    store.create_session(&session).unwrap();

    let transitioned = store.transition_queued_to_running(&session.id).unwrap();
    assert!(transitioned);

    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Running);
    assert_eq!(final_state.owner_pid, Some(std::process::id()));
}

#[test]
fn test_transition_queued_to_running_does_not_overwrite_cancelled_session() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_queued("cancelled before claim");
    store.create_session(&session).unwrap();
    store
        .update_session_status(
            &session.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::Interrupted),
        )
        .unwrap();

    let transitioned = store.transition_queued_to_running(&session.id).unwrap();
    assert!(!transitioned);

    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Cancelled);
    assert_eq!(
        final_state.completion_reason,
        Some(CompletionReason::Interrupted)
    );
    assert_eq!(final_state.owner_pid, None);
}

#[test]
fn test_transition_to_running_can_clear_a_matching_temporary_agent_id() {
    let store = Store::in_memory().unwrap();

    let mut session = Session::new_running("cancelled startup", Path::new("/tmp"));
    session.agent_id = Some("pending-branch-start:test-token".to_string());
    store.create_session(&session).unwrap();
    store
        .update_session_status(
            &session.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::Interrupted),
        )
        .unwrap();

    assert!(store
        .transition_to_running_clearing_agent_id(
            &session.id,
            Some("pending-branch-start:test-token")
        )
        .unwrap());

    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Running);
    assert_eq!(final_state.agent_id, None);
    assert_eq!(final_state.completion_reason, None);
}

#[test]
fn test_transition_to_running_preserves_a_non_matching_agent_id() {
    let store = Store::in_memory().unwrap();

    let mut session = Session::new_running("completed turn", Path::new("/tmp"));
    session.agent_id = Some("real-agent-session".to_string());
    store.create_session(&session).unwrap();
    store
        .update_session_status(
            &session.id,
            SessionStatus::Completed,
            None,
            Some(&CompletionReason::TurnComplete),
        )
        .unwrap();

    assert!(store
        .transition_to_running_clearing_agent_id(
            &session.id,
            Some("pending-branch-start:test-token")
        )
        .unwrap());

    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Running);
    assert_eq!(final_state.agent_id.as_deref(), Some("real-agent-session"));
    assert_eq!(final_state.completion_reason, None);
}

#[test]
fn test_get_active_sessions_returns_running_queued_and_disconnected_only() {
    let store = Store::in_memory().unwrap();

    let running = Session::new_running("running", Path::new("/tmp"));
    store.create_session(&running).unwrap();
    let queued = Session::new_queued("queued");
    store.create_session(&queued).unwrap();
    // A disconnected row still holds its branch, so clients hydrate it.
    let disconnected = Session::new_running("disconnected", Path::new("/tmp"));
    store.create_session(&disconnected).unwrap();
    store
        .update_session_status(&disconnected.id, SessionStatus::Disconnected, None, None)
        .unwrap();

    for status in [
        SessionStatus::Completed,
        SessionStatus::Error,
        SessionStatus::Cancelled,
    ] {
        let terminal = Session::new_running("terminal", Path::new("/tmp"));
        store.create_session(&terminal).unwrap();
        store
            .update_session_status(&terminal.id, status, None, None)
            .unwrap();
    }

    let active = store.get_active_sessions().unwrap();
    assert_eq!(active.len(), 3);
    assert!(active.iter().any(|s| s.id == running.id));
    assert!(active.iter().any(|s| s.id == queued.id));
    assert!(active.iter().any(|s| s.id == disconnected.id));
}

/// Make `session` a `disconnected` row, as the runner leaves one, and return
/// the row as a reconcile would read it.
fn disconnected_row(store: &Store, session: &Session) -> Session {
    store.create_session(session).unwrap();
    store
        .transition_from_running(&session.id, SessionStatus::Disconnected, None, None)
        .unwrap();
    store.get_session(&session.id).unwrap().unwrap()
}

#[test]
fn test_disconnected_rows_settle_only_from_the_row_that_was_read() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("remote", Path::new("/tmp"));
    let row = disconnected_row(&store, &session);
    assert_eq!(store.get_disconnected_sessions().unwrap().len(), 1);

    // A Stop's fallback restamps the row first: the reconcile's CAS loses.
    std::thread::sleep(std::time::Duration::from_millis(2));
    store
        .update_session_status(&session.id, SessionStatus::Disconnected, None, None)
        .unwrap();
    assert!(!store
        .transition_from_disconnected_observed_at(
            &session.id,
            row.updated_at,
            SessionStatus::Error,
            Some("gone"),
            Some(&CompletionReason::Crashed),
        )
        .unwrap());
    assert!(!store
        .claim_disconnected_session(&session.id, row.updated_at)
        .unwrap());

    // The fresh read wins, keeps the message, and a second write of the same
    // snapshot is a no-op.
    let fresh = store.get_session(&session.id).unwrap().unwrap();
    assert!(store
        .transition_from_disconnected_observed_at(
            &session.id,
            fresh.updated_at,
            SessionStatus::Error,
            Some("gone"),
            Some(&CompletionReason::Crashed),
        )
        .unwrap());
    assert!(!store
        .transition_from_disconnected_observed_at(
            &session.id,
            fresh.updated_at,
            SessionStatus::Completed,
            None,
            None,
        )
        .unwrap());
    let ended = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(ended.status, SessionStatus::Error);
    assert_eq!(ended.error_message.as_deref(), Some("gone"));
    assert!(store.get_disconnected_sessions().unwrap().is_empty());
}

#[test]
fn test_claiming_a_disconnected_row_makes_it_ours_and_running() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("remote", Path::new("/tmp"));
    let row = disconnected_row(&store, &session);
    assert_eq!(row.owner_pid, None);

    assert!(store
        .claim_disconnected_session(&session.id, row.updated_at)
        .unwrap());
    let claimed = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(claimed.status, SessionStatus::Running);
    assert_eq!(claimed.owner_pid, Some(std::process::id()));
    // Idempotent: the claimed row is no longer the one that was read.
    assert!(!store
        .claim_disconnected_session(&session.id, row.updated_at)
        .unwrap());
}

#[test]
fn test_blox_command_pin_survives_bookkeeping_and_clears_on_a_new_turn() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("remote", Path::new("/tmp"));
    let row = disconnected_row(&store, &session);

    store
        .set_session_blox_command_id(&session.id, Some("27305746"))
        .unwrap();
    let pinned = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(pinned.blox_command_id.as_deref(), Some("27305746"));
    // Bookkeeping only: the reconcile's CAS on `updated_at` still holds.
    assert_eq!(pinned.updated_at, row.updated_at);

    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    assert!(store.transition_to_running(&session.id).unwrap());
    let resumed = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(resumed.blox_command_id, None);
}

#[test]
fn test_latest_user_message_id_marks_where_the_last_turn_starts() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("remote", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    assert_eq!(store.latest_user_message_id(&session.id).unwrap(), None);

    store
        .add_session_message(&session.id, MessageRole::User, "first")
        .unwrap();
    store
        .add_session_message(&session.id, MessageRole::Assistant, "one")
        .unwrap();
    let second = store
        .add_session_message(&session.id, MessageRole::User, "second")
        .unwrap();
    store
        .add_session_message(&session.id, MessageRole::Assistant, "two")
        .unwrap();
    assert_eq!(
        store.latest_user_message_id(&session.id).unwrap(),
        Some(second)
    );
}

#[test]
fn test_session_round_trips_parent_session_id() {
    let store = Store::in_memory().unwrap();

    let parent = Session::new_running("project session", Path::new("/tmp"));
    store.create_session(&parent).unwrap();
    let child = Session::new_queued("repo work").with_parent_session(&parent.id);
    store.create_session(&child).unwrap();

    let fetched = store.get_session(&child.id).unwrap().unwrap();
    assert_eq!(
        fetched.parent_session_id.as_deref(),
        Some(parent.id.as_str())
    );
    // Sessions a user started directly have no parent.
    let fetched_parent = store.get_session(&parent.id).unwrap().unwrap();
    assert!(fetched_parent.parent_session_id.is_none());
}

#[test]
fn test_session_started_by_round_trips_and_defaults_to_none() {
    let store = Store::in_memory().unwrap();

    let by_session = Session::new_queued("queued by an agent").with_started_by("session:s1");
    store.create_session(&by_session).unwrap();
    let by_app =
        Session::new_running("started by an app", Path::new("/tmp")).with_started_by("app:grant-1");
    store.create_session(&by_app).unwrap();
    let by_user = Session::new_running("started in the UI", Path::new("/tmp"));
    store.create_session(&by_user).unwrap();

    let fetch = |id: &str| store.get_session(id).unwrap().unwrap().started_by;
    assert_eq!(fetch(&by_session.id).as_deref(), Some("session:s1"));
    assert_eq!(fetch(&by_app.id).as_deref(), Some("app:grant-1"));
    assert_eq!(fetch(&by_user.id), None, "UI starts record no actor");
}

/// Fixture for the child-session badge query: a project with one repo, one
/// branch on it, and a parent project session.
fn child_session_fixture(store: &Store) -> (Project, Branch, ProjectRepo, Session) {
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let repo = ProjectRepo::new(&project.id, "test-owner/test-repo", "main", None);
    store.create_project_repo(&repo).unwrap();
    let branch = Branch::new(&project.id, "feature", "main").with_project_repo(&repo.id);
    store.create_branch(&branch).unwrap();
    let parent = Session::new_running("project session", Path::new("/tmp"));
    store.create_session(&parent).unwrap();
    (project, branch, repo, parent)
}

/// Enqueue a child session of `parent` and give it the artifact that makes it
/// a commit / note / review child, mirroring `start_repo_session`.
///
/// Every child is stamped with the same `created_at`, which is what real
/// back-to-back `start_repo_session` calls produce at `now_timestamp`'s
/// millisecond precision: ordering has to fall through to the tie-break.
fn child_session(store: &Store, parent_id: &str, branch_id: &str, kind: &str) -> Session {
    let mut session = Session::new_queued(kind).with_parent_session(parent_id);
    session.created_at = 1_000;
    store.create_session(&session).unwrap();
    match kind {
        "commit" => store
            .create_commit(&Commit::new_pending(branch_id).with_session(&session.id))
            .unwrap(),
        "note" => store
            .create_note(&Note::new(branch_id, "research", "").with_session(&session.id))
            .unwrap(),
        "review" => store
            .create_review(
                &Review::new(branch_id, "", ReviewScope::Branch).with_session(&session.id),
            )
            .unwrap(),
        other => panic!("unknown child kind {other}"),
    }
    session
}

#[test]
fn test_list_active_child_sessions_orders_children_by_start_and_links_their_branch() {
    let store = Store::in_memory().unwrap();
    let (_project, branch, _repo, parent) = child_session_fixture(&store);

    for kind in ["commit", "note", "review"] {
        let child = child_session(&store, &parent.id, &branch.id, kind);
        store.transition_queued_to_running(&child.id).unwrap();
    }

    let children = store.list_active_child_sessions(&parent.id).unwrap();
    let kinds: Vec<&str> = children.iter().map(|c| c.session_type.as_str()).collect();
    // Ordered by start order, which is the order they were enqueued: these
    // three share a `created_at`, so the row order is the tie-break's doing and
    // badge positions stay put across refreshes.
    assert_eq!(kinds, vec!["commit", "note", "review"]);
    for child in &children {
        assert_eq!(child.status, "running");
        assert_eq!(child.branch_id, branch.id);
    }
}

/// Every `start_repo_session` outcome — hidden child note, detached note,
/// commit, code review — is a dependency of the parent and must badge.
#[test]
fn test_list_active_child_sessions_covers_every_start_repo_session_outcome() {
    let store = Store::in_memory().unwrap();
    let (project, branch, _repo, parent) = child_session_fixture(&store);
    let project_note = ProjectNote::new(&project.id, "Parent", "");
    store.create_project_note(&project_note).unwrap();

    // `child_note`: aggregated under the project note and hidden from the
    // repo's visible timeline (`list_notes` filters it out), but still a note
    // as far as the badge is concerned — there is no visibility filter here.
    let hidden = Session::new_queued("investigate").with_parent_session(&parent.id);
    store.create_session(&hidden).unwrap();
    store
        .create_note(
            &Note::new(&branch.id, "investigate", "")
                .with_session(&hidden.id)
                .with_parent_project_note(&project_note.id),
        )
        .unwrap();
    // `note_in_repo`, `commit`, `code_review`.
    let detached = child_session(&store, &parent.id, &branch.id, "note");
    let commit = child_session(&store, &parent.id, &branch.id, "commit");
    let review = child_session(&store, &parent.id, &branch.id, "review");
    for child in [&hidden, &detached, &commit, &review] {
        store.transition_queued_to_running(&child.id).unwrap();
    }
    assert!(
        store
            .list_notes_for_branch(&branch.id)
            .unwrap()
            .iter()
            .all(|n| n.session_id.as_deref() != Some(hidden.id.as_str())),
        "fixture: the child note should be hidden from the branch timeline"
    );

    let children = store.list_active_child_sessions(&parent.id).unwrap();
    let mut ids: Vec<&str> = children.iter().map(|c| c.session_id.as_str()).collect();
    ids.sort();
    let mut expected = vec![
        hidden.id.as_str(),
        detached.id.as_str(),
        commit.id.as_str(),
        review.id.as_str(),
    ];
    expected.sort();
    assert_eq!(ids, expected);
    let kind_of = |id: &str| {
        children
            .iter()
            .find(|c| c.session_id == id)
            .map(|c| c.session_type.as_str())
            .unwrap()
    };
    assert_eq!(kind_of(&hidden.id), "note");
    assert_eq!(kind_of(&detached.id), "note");
    assert_eq!(kind_of(&commit.id), "commit");
    assert_eq!(kind_of(&review.id), "review");
}

/// The branch queue runs one session per branch at a time, so a parent that
/// fires several `start_repo_session` calls at one repo has most of them
/// waiting. They are dependencies all the same and must appear, flagged
/// `queued` so the UI can draw them differently from running work.
#[test]
fn test_list_active_child_sessions_includes_queued_children() {
    let store = Store::in_memory().unwrap();
    let (project, branch, _repo, parent) = child_session_fixture(&store);
    let project_note = ProjectNote::new(&project.id, "Parent", "");
    store.create_project_note(&project_note).unwrap();

    let running = child_session(&store, &parent.id, &branch.id, "commit");
    store.transition_queued_to_running(&running.id).unwrap();

    // One queued child of every outcome, hidden child note included.
    let queued_hidden = Session::new_queued("investigate").with_parent_session(&parent.id);
    store.create_session(&queued_hidden).unwrap();
    store
        .create_note(
            &Note::new(&branch.id, "investigate", "")
                .with_session(&queued_hidden.id)
                .with_parent_project_note(&project_note.id),
        )
        .unwrap();
    let queued_note = child_session(&store, &parent.id, &branch.id, "note");
    let queued_commit = child_session(&store, &parent.id, &branch.id, "commit");
    let queued_review = child_session(&store, &parent.id, &branch.id, "review");

    let children = store.list_active_child_sessions(&parent.id).unwrap();
    assert_eq!(children.len(), 5);
    let status_of = |id: &str| {
        children
            .iter()
            .find(|c| c.session_id == id)
            .map(|c| c.status.as_str())
            .unwrap_or_else(|| panic!("child {id} missing from the badge rows"))
    };
    assert_eq!(status_of(&running.id), "running");
    for queued in [&queued_hidden, &queued_note, &queued_commit, &queued_review] {
        assert_eq!(status_of(&queued.id), "queued");
    }
}

/// Each badge carries the title the branch timeline would give the same
/// session: a commit's live ACP title else its prompt, a note's stored title
/// (ACP titles never apply to notes), and a review's stored title else — only
/// while running — its ACP title.
#[test]
fn test_list_active_child_sessions_titles_follow_the_branch_timeline_rules() {
    let store = Store::in_memory().unwrap();
    let (_project, branch, _repo, parent) = child_session_fixture(&store);

    let commit = child_session(&store, &parent.id, &branch.id, "commit");
    let note = child_session(&store, &parent.id, &branch.id, "note");
    let review = child_session(&store, &parent.id, &branch.id, "review");
    let title_of = |children: &[ChildSessionInfo], id: &str| {
        children
            .iter()
            .find(|c| c.session_id == id)
            .unwrap()
            .title
            .clone()
    };

    // Queued, nothing pushed yet: the commit shows the start of its prompt,
    // the note its stub title, the review nothing.
    let children = store.list_active_child_sessions(&parent.id).unwrap();
    assert_eq!(title_of(&children, &commit.id).as_deref(), Some("commit"));
    assert_eq!(title_of(&children, &note.id).as_deref(), Some("research"));
    assert_eq!(title_of(&children, &review.id), None);

    // The agent pushes an ACP title while each is still queued: only the
    // commit's label changes. The review's does not — the branch timeline
    // applies an ACP title to a review only once it is running.
    for child in [&commit, &note, &review] {
        store
            .update_session_acp_title(&child.id, Some("Live title"))
            .unwrap();
    }
    let children = store.list_active_child_sessions(&parent.id).unwrap();
    assert_eq!(
        title_of(&children, &commit.id).as_deref(),
        Some("Live title")
    );
    assert_eq!(title_of(&children, &note.id).as_deref(), Some("research"));
    assert_eq!(title_of(&children, &review.id), None);

    // Running: the review picks the ACP title up too.
    for child in [&commit, &note, &review] {
        store.transition_queued_to_running(&child.id).unwrap();
    }
    let children = store.list_active_child_sessions(&parent.id).unwrap();
    assert_eq!(
        title_of(&children, &review.id).as_deref(),
        Some("Live title")
    );

    // A stored review title beats the ACP title.
    let review_row = store
        .list_reviews_for_branch(&branch.id)
        .unwrap()
        .into_iter()
        .find(|r| r.session_id.as_deref() == Some(review.id.as_str()))
        .unwrap();
    store
        .update_review_title(&review_row.id, "Stored review title")
        .unwrap();
    let children = store.list_active_child_sessions(&parent.id).unwrap();
    assert_eq!(
        title_of(&children, &review.id).as_deref(),
        Some("Stored review title")
    );
}

#[test]
fn test_list_active_child_sessions_yields_one_row_per_multi_artifact_child() {
    let store = Store::in_memory().unwrap();
    let (_project, branch, _repo, parent) = child_session_fixture(&store);

    // Nothing in the schema stops a session from owning several artifact rows;
    // the badge query must still yield one row per child, because the frontend
    // keys badges by session_id.
    let child = child_session(&store, &parent.id, &branch.id, "note");
    store
        .create_note(&Note::new(&branch.id, "second note", "").with_session(&child.id))
        .unwrap();
    store.transition_queued_to_running(&child.id).unwrap();

    let children = store.list_active_child_sessions(&parent.id).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].session_id, child.id);
    assert_eq!(children[0].session_type, "note");
}

#[test]
fn test_list_active_child_sessions_excludes_finished_children_and_other_parents() {
    let store = Store::in_memory().unwrap();
    let (_project, branch, _repo, parent) = child_session_fixture(&store);

    let running = child_session(&store, &parent.id, &branch.id, "commit");
    store.transition_queued_to_running(&running.id).unwrap();

    // Terminal children have already landed on the branch timeline.
    for status in [
        SessionStatus::Completed,
        SessionStatus::Error,
        SessionStatus::Cancelled,
    ] {
        let finished = child_session(&store, &parent.id, &branch.id, "review");
        store
            .update_session_status(&finished.id, status, None, None)
            .unwrap();
    }

    // Another project session's running child.
    let other_parent = Session::new_running("other project session", Path::new("/tmp"));
    store.create_session(&other_parent).unwrap();
    let other_child = child_session(&store, &other_parent.id, &branch.id, "commit");
    store.transition_queued_to_running(&other_child.id).unwrap();

    let children = store.list_active_child_sessions(&parent.id).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].session_id, running.id);
}

#[test]
fn test_queued_session_messages_order_and_image_ids() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let first_images = vec!["img-1".to_string(), "img-2".to_string()];
    let first = store
        .add_queued_session_message(&session.id, "first", &first_images, None)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let second = store
        .add_queued_session_message(&session.id, "second", &[], None)
        .unwrap();

    let queued = store.list_queued_session_messages(&session.id).unwrap();
    assert_eq!(
        queued
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec![first.id.as_str(), second.id.as_str()]
    );
    assert_eq!(queued[0].image_ids, first_images);
    assert!(queued[1].image_ids.is_empty());
}

#[test]
fn test_queued_session_message_claims_pending_images() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    let image = Image::new(None, &project.id, "screenshot.png", "image/png", 42, true);
    store.create_image(&image).unwrap();

    store
        .add_queued_session_message(
            &session.id,
            "with image",
            std::slice::from_ref(&image.id),
            None,
        )
        .unwrap();

    let claimed = store.get_image(&image.id).unwrap().unwrap();
    assert_eq!(claimed.session_id.as_deref(), Some(session.id.as_str()));
}

#[test]
fn test_session_acp_config_selection_round_trips() {
    let store = Store::in_memory().unwrap();
    let selection = AcpConfigSelection {
        model: Some(AcpConfigValueSelection {
            config_id: "model".to_string(),
            value_id: "gpt-5".to_string(),
            label: Some("GPT-5".to_string()),
        }),
        effort: Some(AcpConfigValueSelection {
            config_id: "reasoning_effort".to_string(),
            value_id: "high".to_string(),
            label: None,
        }),
    };

    let session = Session::new_running("configured", Path::new("/tmp"))
        .with_acp_config_selection(selection.clone());
    store.create_session(&session).unwrap();

    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(fetched.acp_config_selection, Some(selection.clone()));

    let replacement = AcpConfigSelection {
        model: Some(AcpConfigValueSelection {
            config_id: "model".to_string(),
            value_id: "gpt-5-mini".to_string(),
            label: Some("GPT-5 mini".to_string()),
        }),
        effort: None,
    };
    store
        .set_session_acp_config_selection(&session.id, Some(&replacement))
        .unwrap();
    let updated = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(updated.acp_config_selection, Some(replacement));

    store
        .set_session_acp_config_selection(&session.id, None)
        .unwrap();
    let cleared = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(cleared.acp_config_selection, None);
}

#[test]
fn test_session_acp_title_round_trips() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(fetched.acp_title, None);

    store
        .update_session_acp_title(&session.id, Some("Refactor the widget pipeline"))
        .unwrap();
    let updated = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(
        updated.acp_title.as_deref(),
        Some("Refactor the widget pipeline")
    );

    store
        .update_session_acp_title(&session.id, Some("Refactor and test the widget pipeline"))
        .unwrap();
    let replaced = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(
        replaced.acp_title.as_deref(),
        Some("Refactor and test the widget pipeline")
    );

    store.update_session_acp_title(&session.id, None).unwrap();
    let cleared = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(cleared.acp_title, None);
}

#[test]
fn test_resolve_session_status_exposes_acp_title() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let resolved = store.resolve_session_status(Some(&session.id));
    assert_eq!(resolved.acp_title, None);

    store
        .update_session_acp_title(&session.id, Some("Fix the login flow"))
        .unwrap();
    let resolved = store.resolve_session_status(Some(&session.id));
    assert_eq!(resolved.session_id.as_deref(), Some(session.id.as_str()));
    assert_eq!(resolved.status.as_deref(), Some("running"));
    assert_eq!(resolved.acp_title.as_deref(), Some("Fix the login flow"));

    let unresolved = store.resolve_session_status(None);
    assert_eq!(unresolved.acp_title, None);
}

#[test]
fn test_queued_session_acp_config_selection_round_trips() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let selection = AcpConfigSelection {
        model: Some(AcpConfigValueSelection {
            config_id: "model".to_string(),
            value_id: "gpt-5".to_string(),
            label: Some("GPT-5".to_string()),
        }),
        effort: Some(AcpConfigValueSelection {
            config_id: "thought_level".to_string(),
            value_id: "medium".to_string(),
            label: Some("Medium".to_string()),
        }),
    };
    let session = Session::new_queued("queued")
        .with_branch(&branch.id)
        .with_acp_config_selection(selection.clone());
    store.create_session(&session).unwrap();
    let note = Note::new(&branch.id, "queued", "").with_session(&session.id);
    store.create_note(&note).unwrap();

    let queued = store.get_queued_sessions_for_branch(&branch.id).unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].acp_config_selection, Some(selection));
}

#[test]
fn test_delete_queued_session_message_releases_claimed_images() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    let image = Image::new(
        Some(&branch.id),
        &project.id,
        "screenshot.png",
        "image/png",
        42,
        true,
    );
    store.create_image(&image).unwrap();
    let queued = store
        .add_queued_session_message(
            &session.id,
            "with image",
            std::slice::from_ref(&image.id),
            Some(&branch.id),
        )
        .unwrap();

    assert!(store.delete_queued_session_message(&queued.id).unwrap());

    let released = store.get_image(&image.id).unwrap().unwrap();
    assert_eq!(released.session_id, None);
    let branch_images = store.list_images_for_branch(&branch.id).unwrap();
    assert_eq!(
        branch_images
            .iter()
            .map(|image| image.id.as_str())
            .collect::<Vec<_>>(),
        vec![image.id.as_str()]
    );
}

/// A reply to a session whose project is being transferred is held whatever
/// the session's status, and is what a drain on the branch finds — unless
/// the session is running, when its own turn's end drains it.
#[test]
fn test_held_queued_session_messages_are_listed_per_branch_for_sessions_not_running() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let other = Branch::new(&project.id, "other", "main");
    store.create_branch(&other).unwrap();

    let finished = Session::new_running("done", Path::new("/tmp")).with_branch(&branch.id);
    store.create_session(&finished).unwrap();
    store
        .update_session_status(&finished.id, SessionStatus::Completed, None, None)
        .unwrap();
    let running = Session::new_running("busy", Path::new("/tmp")).with_branch(&branch.id);
    store.create_session(&running).unwrap();
    let elsewhere = Session::new_running("elsewhere", Path::new("/tmp")).with_branch(&other.id);
    store.create_session(&elsewhere).unwrap();
    store
        .update_session_status(&elsewhere.id, SessionStatus::Completed, None, None)
        .unwrap();

    let held = store
        .add_queued_session_message_held_for_transfer(&finished.id, "later", &[], Some(&branch.id))
        .unwrap();
    assert_eq!(held.status, QueuedSessionMessageStatus::Queued);
    store
        .add_queued_session_message(&running.id, "after this turn", &[], Some(&branch.id))
        .unwrap();
    store
        .add_queued_session_message_held_for_transfer(&elsewhere.id, "other branch", &[], None)
        .unwrap();
    assert_eq!(
        store
            .add_queued_session_message_held_for_transfer("missing", "x", &[], None)
            .unwrap_err()
            .to_string(),
        "Session not found: missing"
    );

    assert_eq!(
        store
            .session_ids_with_queued_messages_for_branch(&branch.id)
            .unwrap(),
        vec![finished.id.clone()],
        "the running session's follow-up waits for its turn"
    );
    assert_eq!(
        store
            .count_queued_session_messages_for_branch(&branch.id)
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .count_queued_session_messages_for_branch(&other.id)
            .unwrap(),
        1
    );

    // Drained like any other: claimed oldest-first, then sent.
    let claimed = store
        .claim_oldest_queued_session_message(&finished.id)
        .unwrap()
        .expect("claimable");
    assert_eq!(claimed.id, held.id);
    assert!(store
        .session_ids_with_queued_messages_for_branch(&branch.id)
        .unwrap()
        .is_empty());
}

#[test]
fn test_add_queued_session_message_requires_running_session() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();

    let err = store
        .add_queued_session_message(&session.id, "too late", &[], None)
        .unwrap_err();
    assert_eq!(err.to_string(), "Session is not running");
    assert!(store
        .list_queued_session_messages(&session.id)
        .unwrap()
        .is_empty());

    let err = store
        .add_queued_session_message("missing-session", "not found", &[], None)
        .unwrap_err();
    assert_eq!(err.to_string(), "Session not found: missing-session");
}

#[test]
fn test_claim_selected_queued_session_message_and_release() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let oldest = store
        .add_queued_session_message(&session.id, "oldest", &[], None)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let selected = store
        .add_queued_session_message(&session.id, "selected", &[], None)
        .unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();

    let claimed = store
        .claim_queued_session_message(&selected.id)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, selected.id);
    assert_eq!(claimed.status, QueuedSessionMessageStatus::Sending);
    assert_eq!(claimed.owner_pid, Some(std::process::id()));

    store
        .release_queued_session_message(&selected.id, Some("try again"))
        .unwrap();
    let queued = store.list_queued_session_messages(&session.id).unwrap();
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0].id, oldest.id);
    assert_eq!(queued[1].id, selected.id);
    assert_eq!(queued[1].last_error.as_deref(), Some("try again"));
}

#[test]
fn test_claim_oldest_queued_session_message_skips_running_session() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .add_queued_session_message(&session.id, "queued", &[], None)
        .unwrap();

    let claimed = store
        .claim_oldest_queued_session_message(&session.id)
        .unwrap();
    assert!(claimed.is_none());

    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    let claimed = store
        .claim_oldest_queued_session_message(&session.id)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.content, "queued");
}

#[test]
fn test_queued_session_message_marks_sent_with_transcript_message() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    let queued = store
        .add_queued_session_message(&session.id, "follow up", &[], None)
        .unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();
    store
        .claim_queued_session_message(&queued.id)
        .unwrap()
        .unwrap();

    let message_id = store
        .add_session_message_with_images_from_queue(
            &session.id,
            MessageRole::User,
            "follow up",
            &[],
            &queued.id,
        )
        .unwrap();

    assert!(store
        .list_queued_session_messages(&session.id)
        .unwrap()
        .is_empty());
    let messages = store.get_session_messages(&session.id).unwrap();
    assert_eq!(messages[0].id, message_id);
    assert_eq!(messages[0].content, "follow up");
}

#[test]
fn test_queued_session_messages_cascade_when_session_deleted() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .add_queued_session_message(&session.id, "follow up", &[], None)
        .unwrap();

    store.delete_session(&session.id).unwrap();

    assert!(store
        .list_queued_session_messages(&session.id)
        .unwrap()
        .is_empty());
}

#[test]
fn test_queued_project_session_cancellation_records_project_session_interrupted() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_queued("cancelled by project session");
    store.create_session(&session).unwrap();

    let transitioned = store
        .transition_from_queued(
            &session.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::ProjectSessionInterrupted),
        )
        .unwrap();
    assert!(transitioned);

    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Cancelled);
    assert_eq!(
        final_state.completion_reason,
        Some(CompletionReason::ProjectSessionInterrupted)
    );
}

#[test]
fn test_queued_pipeline_commit_owns_branch_queue_position() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let mut session = Session::new_queued("Rebase branch").with_branch(&branch.id);
    session.pipeline = Some(PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase));
    store.create_session(&session).unwrap();
    let commit = Commit::new_pending(&branch.id).with_session(&session.id);
    store.create_commit(&commit).unwrap();

    let queued = store.get_queued_sessions_for_branch(&branch.id).unwrap();

    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].id, session.id);
    assert_eq!(
        queued[0].pipeline.as_ref().and_then(|p| p.kind.as_ref()),
        Some(&PipelineKind::Rebase)
    );
}

#[test]
fn test_queued_push_pipeline_is_found_through_session_branch_link() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let other_branch = Branch::new(&project.id, "other", "main");
    store.create_branch(&other_branch).unwrap();

    // No commit/note/review artifact: the branch link is the only way in.
    let mut session = Session::new_queued("Push the current branch").with_branch(&branch.id);
    session.pipeline = Some(
        PipelineExecution::from_steps(&[])
            .with_kind(PipelineKind::Push)
            .with_push_force(true),
    );
    store.create_session(&session).unwrap();

    let queued = store.get_queued_sessions_for_branch(&branch.id).unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].id, session.id);
    assert_eq!(queued[0].branch_id.as_deref(), Some(branch.id.as_str()));
    assert_eq!(
        queued[0].pipeline.as_ref().and_then(|p| p.kind.as_ref()),
        Some(&PipelineKind::Push)
    );
    assert!(queued[0].pipeline.as_ref().unwrap().push_force);

    assert!(store
        .get_queued_sessions_for_branch(&other_branch.id)
        .unwrap()
        .is_empty());
}

#[test]
fn test_running_push_pipeline_marks_branch_busy_and_resolves_branch() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let mut session =
        Session::new_running("Push the current branch", Path::new("/tmp")).with_branch(&branch.id);
    session.pipeline = Some(PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Push));
    store.create_session(&session).unwrap();

    assert!(store.has_running_session_for_branch(&branch.id).unwrap());
    assert_eq!(
        store
            .get_branch_id_for_session(&session.id)
            .unwrap()
            .as_deref(),
        Some(branch.id.as_str())
    );
    assert_eq!(
        store
            .get_project_id_for_session(&session.id)
            .unwrap()
            .as_deref(),
        Some(project.id.as_str())
    );
    // The drain scan resolves schedules from `get_running_sessions`, so that
    // query has to carry the branch link too.
    let running = store.get_running_sessions().unwrap();
    assert_eq!(
        running
            .iter()
            .find(|running| running.id == session.id)
            .and_then(|running| running.branch_id.as_deref()),
        Some(branch.id.as_str())
    );
}

#[test]
fn test_mark_session_artifact_started_restamps_empty_note_and_preserves_session_created_at() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let mut session = Session::new_queued("queued note");
    session.created_at = 1_000;
    session.updated_at = 1_000;
    store.create_session(&session).unwrap();

    let mut note = Note::new(&branch.id, "queued note", "").with_session(&session.id);
    note.created_at = 1_000;
    note.updated_at = 1_000;
    store.create_note(&note).unwrap();

    assert!(store.transition_to_running(&session.id).unwrap());
    store.mark_session_artifact_started(&session.id).unwrap();

    let started_note = store.get_note(&note.id).unwrap().unwrap();
    assert!(started_note.created_at > note.created_at);
    assert_eq!(started_note.updated_at, started_note.created_at);
    assert!(started_note.completed_at.is_none());

    let started_session = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(started_session.status, SessionStatus::Running);
    assert_eq!(started_session.created_at, session.created_at);
}

#[test]
fn test_mark_session_artifact_started_restamps_incomplete_review() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let mut session = Session::new_queued("queued review");
    session.created_at = 1_000;
    session.updated_at = 1_000;
    store.create_session(&session).unwrap();

    let mut review = Review::new(&branch.id, "", ReviewScope::Branch).with_session(&session.id);
    review.created_at = 1_000;
    review.updated_at = 1_000;
    store.create_review(&review).unwrap();

    assert!(store.transition_to_running(&session.id).unwrap());
    store.mark_session_artifact_started(&session.id).unwrap();

    let started_review = store.get_review(&review.id).unwrap().unwrap();
    assert!(started_review.created_at > review.created_at);
    assert_eq!(started_review.updated_at, started_review.created_at);
    assert!(started_review.completed_at.is_none());
}

#[test]
fn test_mark_session_artifact_started_restamps_queued_pipeline_pending_commit() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let mut session = Session::new_queued("Rebase branch");
    session.created_at = 1_000;
    session.updated_at = 1_000;
    session.pipeline = Some(PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase));
    store.create_session(&session).unwrap();

    let mut commit = Commit::new_pending(&branch.id).with_session(&session.id);
    commit.created_at = 1_000;
    commit.updated_at = 1_000;
    store.create_commit(&commit).unwrap();

    assert!(store.transition_to_running(&session.id).unwrap());
    store.mark_session_artifact_started(&session.id).unwrap();

    let started_commit = store.get_commit(&commit.id).unwrap().unwrap();
    assert!(started_commit.created_at > commit.created_at);
    assert_eq!(started_commit.updated_at, started_commit.created_at);
    assert!(started_commit.sha.is_none());
}

#[test]
fn test_failed_empty_note_orders_after_commit_completed_before_note_started() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let mut session = Session::new_queued("queued note");
    session.created_at = 1_000;
    session.updated_at = 1_000;
    store.create_session(&session).unwrap();

    let mut note = Note::new(&branch.id, "queued note", "").with_session(&session.id);
    note.created_at = 1_000;
    note.updated_at = 1_000;
    store.create_note(&note).unwrap();

    let mut commit = Commit::new_with_sha(&branch.id, "abc123");
    commit.created_at = 2_000;
    commit.updated_at = 2_000;
    store.create_commit(&commit).unwrap();

    assert!(store.transition_to_running(&session.id).unwrap());
    store.mark_session_artifact_started(&session.id).unwrap();
    assert!(store
        .transition_from_running(
            &session.id,
            SessionStatus::Error,
            Some("app quit"),
            Some(&CompletionReason::AppQuit),
        )
        .unwrap());

    let failed_note = store.get_note(&note.id).unwrap().unwrap();
    let failed_note_time = failed_note.completed_at.unwrap_or(failed_note.created_at);
    let commit_time = commit.created_at;
    let mut timeline = [("failed-note", failed_note_time), ("commit", commit_time)];
    timeline.sort_by_key(|(_, timestamp)| *timestamp);

    assert_eq!(timeline[0].0, "commit");
    assert_eq!(timeline[1].0, "failed-note");
}

#[test]
fn test_running_pipeline_commit_marks_branch_busy_and_resolves_branch() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let mut session =
        Session::new_running("Squash commits", Path::new("/tmp")).with_branch(&branch.id);
    session.pipeline = Some(PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Squash));
    store.create_session(&session).unwrap();
    let commit = Commit::new_pending(&branch.id).with_session(&session.id);
    store.create_commit(&commit).unwrap();

    assert!(store.has_running_session_for_branch(&branch.id).unwrap());
    assert_eq!(
        store
            .get_branch_id_for_session(&session.id)
            .unwrap()
            .as_deref(),
        Some(branch.id.as_str())
    );
}

#[test]
fn test_session_branch_id_round_trips_and_resolves_branch_and_project() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // A pr/push pipeline session: no artifact row, branch carried on the
    // session row itself.
    let session = Session::new_running(
        "Create a pull request for the current branch",
        Path::new("/tmp"),
    )
    .with_branch(&branch.id);
    store.create_session(&session).unwrap();

    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(fetched.branch_id.as_deref(), Some(branch.id.as_str()));

    assert_eq!(
        store
            .get_branch_id_for_session(&session.id)
            .unwrap()
            .as_deref(),
        Some(branch.id.as_str())
    );
    assert_eq!(
        store
            .get_project_id_for_session(&session.id)
            .unwrap()
            .as_deref(),
        Some(project.id.as_str())
    );
}

#[test]
fn test_session_completion_effects_marker_round_trips() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("Push the current branch to the remote", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(fetched.completion_effects_at, None);

    store.mark_completion_effects_ran(&session.id).unwrap();
    let marked = store.get_session(&session.id).unwrap().unwrap();
    let marked_at = marked
        .completion_effects_at
        .expect("marker should be set after delivering outcomes");
    assert!(marked_at >= session.created_at);
    assert_eq!(marked.updated_at, marked_at);
}

/// `branch_session_ids_ended_since` is the deferred push's "did anything end
/// after that push" question: every terminal status on the branch counts, a
/// row written in the same millisecond counts (a tie is "after"), the push
/// itself is excluded, and running or queued rows and other branches' rows
/// are not ends on this branch.
#[test]
fn test_branch_session_ids_ended_since_lists_other_terminal_rows_including_ties() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let sibling = Branch::new(&project.id, "other", "main");
    store.create_branch(&sibling).unwrap();

    let on_branch = |prompt: &str| {
        let session = Session::new_running(prompt, Path::new("/tmp")).with_branch(&branch.id);
        store.create_session(&session).unwrap();
        session.id
    };
    let push = on_branch("Push the current branch to the remote");
    let completed = on_branch("completed");
    let errored = on_branch("errored");
    let cancelled = on_branch("cancelled");
    let _still_running = on_branch("running");
    let elsewhere = Session::new_running("elsewhere", Path::new("/tmp")).with_branch(&sibling.id);
    store.create_session(&elsewhere).unwrap();

    store
        .update_session_status(&push, SessionStatus::Completed, None, None)
        .unwrap();
    store
        .update_session_status(&completed, SessionStatus::Completed, None, None)
        .unwrap();
    store
        .update_session_status(&errored, SessionStatus::Error, Some("boom"), None)
        .unwrap();
    store
        .update_session_status(&cancelled, SessionStatus::Cancelled, None, None)
        .unwrap();
    store
        .update_session_status(&elsewhere.id, SessionStatus::Completed, None, None)
        .unwrap();
    let push_ended_at = store.get_session(&push).unwrap().unwrap().updated_at;

    // Everything above was written within the same few milliseconds; asking
    // from the push's own end time catches the ties and anything later.
    assert_eq!(
        store
            .branch_session_ids_ended_since(&branch.id, push_ended_at, &push)
            .unwrap(),
        vec![completed.clone(), errored.clone(), cancelled.clone()]
    );
    assert!(
        store
            .branch_session_ids_ended_since(&branch.id, push_ended_at, &completed)
            .unwrap()
            .contains(&push),
        "the exclusion is by id, not by kind"
    );

    // Nothing ended after a point in the future.
    assert!(store
        .branch_session_ids_ended_since(&branch.id, push_ended_at + 1_000, &push)
        .unwrap()
        .is_empty());
}

#[test]
fn test_session_without_branch_or_artifact_resolves_to_none() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("unattributed", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    assert_eq!(
        store.get_session(&session.id).unwrap().unwrap().branch_id,
        None
    );
    assert_eq!(store.get_branch_id_for_session(&session.id).unwrap(), None);
    assert_eq!(store.get_project_id_for_session(&session.id).unwrap(), None);
}

#[test]
fn test_completion_reason_round_trips() {
    let store = Store::in_memory().unwrap();

    for reason in [
        CompletionReason::TurnComplete,
        CompletionReason::HeldUntilCap,
        CompletionReason::HoldStopped,
        CompletionReason::Interrupted,
        CompletionReason::ProjectSessionInterrupted,
        CompletionReason::Crashed,
        CompletionReason::RemoteAgentLost,
        CompletionReason::RateLimited,
        CompletionReason::AppQuit,
        CompletionReason::Unknown,
    ] {
        let session = Session::new_running("test reason", Path::new("/tmp"));
        store.create_session(&session).unwrap();
        store
            .update_session_status(&session.id, SessionStatus::Completed, None, Some(&reason))
            .unwrap();
        let fetched = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(
            fetched.completion_reason.as_ref(),
            Some(&reason),
            "round-trip failed for {:?}",
            reason
        );
    }
}

#[test]
fn test_completion_reason_resumability() {
    // Anything that cut a session's work short can be nudged; a turn the agent
    // said it finished cannot. Mirrored by RESUMABLE_REASONS in
    // `src/lib/types.ts`, which drives the Resume affordance.
    for reason in [
        CompletionReason::HeldUntilCap,
        CompletionReason::HoldStopped,
        CompletionReason::Interrupted,
        CompletionReason::ProjectSessionInterrupted,
        CompletionReason::Crashed,
        CompletionReason::RemoteAgentLost,
        CompletionReason::RateLimited,
        CompletionReason::AppQuit,
    ] {
        assert!(reason.is_resumable(), "{reason:?} should be resumable");
    }
    for reason in [CompletionReason::TurnComplete, CompletionReason::Unknown] {
        assert!(!reason.is_resumable(), "{reason:?} should not be resumable");
    }
}

#[test]
fn test_session_provider_metadata_round_trips() {
    let store = Store::in_memory().unwrap();

    let mut session = Session::new_running("test metadata", Path::new("/tmp"));
    session.provider_metadata = Some(serde_json::json!({
        "acp": {
            "promptResponse": {
                "stopReason": "end_turn"
            }
        }
    }));
    store.create_session(&session).unwrap();

    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(
        fetched
            .provider_metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/acp/promptResponse/stopReason"))
            .and_then(|value| value.as_str()),
        Some("end_turn")
    );
}

#[test]
fn test_session_provider_metadata_merges_protocol_fragments() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("merge metadata", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    store
        .merge_session_provider_metadata(
            &session.id,
            &serde_json::json!({
                "acp": {
                    "usageUpdate": {
                        "used": 100,
                        "_meta": {
                            "_claude/rateLimit": {
                                "status": "rejected",
                                "resetsAt": "2026-06-24T15:45:00Z"
                            }
                        }
                    }
                }
            }),
        )
        .unwrap();
    store
        .merge_session_provider_metadata(
            &session.id,
            &serde_json::json!({
                "acp": {
                    "promptError": {
                        "code": -32603,
                        "message": "rate limited",
                        "data": {
                            "errorKind": "rate_limit"
                        }
                    }
                }
            }),
        )
        .unwrap();

    let fetched = store.get_session(&session.id).unwrap().unwrap();
    let metadata = fetched.provider_metadata.as_ref().unwrap();
    assert_eq!(
        metadata
            .pointer("/acp/usageUpdate/_meta")
            .and_then(|meta| meta.get("_claude/rateLimit"))
            .and_then(|rate_limit| rate_limit.get("resetsAt"))
            .and_then(|value| value.as_str()),
        Some("2026-06-24T15:45:00Z")
    );
    assert_eq!(
        metadata
            .pointer("/acp/promptError/data/errorKind")
            .and_then(|value| value.as_str()),
        Some("rate_limit")
    );
}

#[test]
fn test_transition_running_to_rate_limited_pauses_with_recovery_metadata() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("rate limited", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    assert!(store
        .transition_running_to_rate_limited(
            &session.id,
            Some("rate limit"),
            Some("2026-06-24T15:45:00Z"),
            Some("claude_pro")
        )
        .unwrap());

    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(fetched.status, SessionStatus::Paused);
    assert_eq!(fetched.error_message.as_deref(), Some("rate limit"));
    assert_eq!(
        fetched.completion_reason,
        Some(CompletionReason::RateLimited)
    );
    assert_eq!(
        fetched.rate_limit_resets_at.as_deref(),
        Some("2026-06-24T15:45:00Z")
    );
    assert_eq!(fetched.rate_limit_kind.as_deref(), Some("claude_pro"));
}

#[test]
fn test_transition_running_to_rate_limited_merges_provider_metadata_in_the_same_write() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("rate limited", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .merge_session_provider_metadata(
            &session.id,
            &serde_json::json!({ "acp": { "promptError": { "data": { "errorKind": "rate_limit" } } } }),
        )
        .unwrap();

    assert!(store
        .transition_running_to_rate_limited_with_provider_metadata(
            &session.id,
            Some("rate limit"),
            Some("2026-06-24T15:45:00Z"),
            Some("claude_pro"),
            Some(&serde_json::json!({ "staged": { "rateLimitAutoRecovery": { "attempt": 1 } } })),
        )
        .unwrap());

    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(fetched.status, SessionStatus::Paused);
    assert_eq!(
        fetched.completion_reason,
        Some(CompletionReason::RateLimited)
    );
    let metadata = fetched.provider_metadata.expect("metadata persisted");
    assert_eq!(
        metadata.pointer("/staged/rateLimitAutoRecovery/attempt"),
        Some(&serde_json::json!(1))
    );
    // The merge keeps unrelated provider metadata.
    assert_eq!(
        metadata.pointer("/acp/promptError/data/errorKind"),
        Some(&serde_json::json!("rate_limit"))
    );

    // The guard still applies: a row that is no longer running takes neither
    // the status nor the metadata.
    assert!(!store
        .transition_running_to_rate_limited_with_provider_metadata(
            &session.id,
            Some("again"),
            None,
            None,
            Some(&serde_json::json!({ "staged": { "rateLimitAutoRecovery": { "attempt": 2 } } })),
        )
        .unwrap());
    let unchanged = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(unchanged.error_message.as_deref(), Some("rate limit"));
    assert_eq!(
        unchanged
            .provider_metadata
            .unwrap()
            .pointer("/staged/rateLimitAutoRecovery/attempt"),
        Some(&serde_json::json!(1))
    );
}

#[test]
fn test_transition_to_running_clears_stale_recovery_metadata() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("retry metadata", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .merge_session_provider_metadata(
            &session.id,
            &serde_json::json!({
                "acp": {
                    "promptError": {
                        "data": {
                            "errorKind": "rate_limit"
                        }
                    }
                }
            }),
        )
        .unwrap();
    store
        .transition_running_to_rate_limited(
            &session.id,
            Some("rate limit"),
            Some("2026-06-24T15:45:00Z"),
            Some("rate_limit"),
        )
        .unwrap();

    assert!(store.transition_to_running(&session.id).unwrap());

    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(fetched.status, SessionStatus::Running);
    assert!(fetched.error_message.is_none());
    assert!(fetched.completion_reason.is_none());
    assert!(fetched.provider_metadata.is_none());
    assert!(fetched.rate_limit_resets_at.is_none());
    assert!(fetched.rate_limit_kind.is_none());
}

#[test]
fn test_transition_to_running_can_preserve_selected_provider_metadata() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("retry metadata", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .merge_session_provider_metadata(
            &session.id,
            &serde_json::json!({
                "acp": {
                    "promptError": {
                        "data": {
                            "errorKind": "rate_limit"
                        }
                    }
                }
            }),
        )
        .unwrap();
    store
        .transition_running_to_rate_limited(
            &session.id,
            Some("rate limit"),
            Some("2026-06-24T15:45:00Z"),
            Some("rate_limit"),
        )
        .unwrap();

    assert!(store
        .transition_to_running_with_provider_metadata(
            &session.id,
            Some(&serde_json::json!({
                "staged": {
                    "rateLimitAutoRecovery": {
                        "attempt": 1
                    }
                }
            })),
        )
        .unwrap());

    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(fetched.status, SessionStatus::Running);
    let metadata = fetched.provider_metadata.as_ref().unwrap();
    assert!(metadata.pointer("/acp/promptError").is_none());
    assert_eq!(
        metadata
            .pointer("/staged/rateLimitAutoRecovery/attempt")
            .and_then(|value| value.as_i64()),
        Some(1)
    );
    assert!(fetched.rate_limit_resets_at.is_none());
    assert!(fetched.rate_limit_kind.is_none());
}

#[test]
fn test_auto_resume_claim_requires_rate_limited_paused_session() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("claim race", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    // Still running: the auto-resume claim must not match.
    assert!(!store
        .transition_to_running_with_provider_metadata(&session.id, None)
        .unwrap());

    // Completed between the scheduler's scan and the claim: must not match
    // either, or the claim would resurrect a finished session.
    store
        .update_session_status(
            &session.id,
            SessionStatus::Completed,
            None,
            Some(&CompletionReason::TurnComplete),
        )
        .unwrap();
    assert!(!store
        .transition_to_running_with_provider_metadata(&session.id, None)
        .unwrap());
    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(fetched.status, SessionStatus::Completed);

    // Only the paused/rate-limited state is claimable.
    store
        .update_session_status(&session.id, SessionStatus::Running, None, None)
        .unwrap();
    store
        .transition_running_to_rate_limited(&session.id, Some("rate limit"), None, None)
        .unwrap();
    assert!(store
        .transition_to_running_with_provider_metadata(&session.id, None)
        .unwrap());
    let fetched = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(fetched.status, SessionStatus::Running);
}

#[test]
fn test_session_messages() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let id1 = store
        .add_session_message(&session.id, MessageRole::User, "hello")
        .unwrap();
    let id2 = store
        .add_session_message(&session.id, MessageRole::Assistant, "hi there")
        .unwrap();

    let all = store.get_session_messages(&session.id).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].role, MessageRole::User);
    assert_eq!(all[1].role, MessageRole::Assistant);

    // Test since (inclusive — re-fetches id1 plus anything after it)
    let since = store
        .get_session_messages_since(&session.id, id1, &[])
        .unwrap();
    assert_eq!(since.len(), 2);
    assert_eq!(since[0].id, id1);
    assert_eq!(since[1].id, id2);
}

#[test]
fn test_session_messages_since_refetches_open_assistant_row() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let assistant_id = store
        .add_session_message(&session.id, MessageRole::Assistant, "The")
        .unwrap();
    let tool_call_id = store
        .add_session_message(&session.id, MessageRole::ToolCall, "Read file")
        .unwrap();
    let tool_result_id = store
        .add_session_message(&session.id, MessageRole::ToolResult, "contents")
        .unwrap();
    let hidden_id = store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("session_info_update".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    // The writer keeps extending the assistant row behind the tool rows.
    store
        .update_message_content(assistant_id, "The search is back")
        .unwrap();

    let tail_only = store
        .get_session_messages_since(&session.id, tool_call_id, &[])
        .unwrap();
    assert_eq!(
        tail_only.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![tool_call_id, tool_result_id]
    );

    let with_refetch = store
        .get_session_messages_since(&session.id, tool_call_id, &[assistant_id, hidden_id])
        .unwrap();
    assert_eq!(
        with_refetch.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![assistant_id, tool_call_id, tool_result_id]
    );
    assert_eq!(with_refetch[0].content, "The search is back");
}

#[test]
fn test_acp_metadata_rows_are_hidden_from_legacy_session_messages() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let visible_id = store
        .add_session_message(&session.id, MessageRole::Assistant, "visible")
        .unwrap();
    let metadata_id = store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("session_info_update".to_string()),
                acp_session_info: Some(serde_json::json!({"title": "ACP title"})),
                ..Default::default()
            },
        )
        .unwrap();

    let all = store.get_session_messages(&session.id).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, visible_id);
    assert_eq!(all[0].content, "visible");

    let count = store
        .count_assistant_messages_after(&session.id, 0)
        .unwrap();
    assert_eq!(count, 1);

    let conn = store.conn.lock().unwrap();
    let raw_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_messages WHERE session_id = ?1",
            [&session.id],
            |row| row.get(0),
        )
        .unwrap();
    let stored_info: String = conn
        .query_row(
            "SELECT acp_session_info FROM session_messages WHERE id = ?1",
            [metadata_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw_count, 2);
    assert_eq!(stored_info, r#"{"title":"ACP title"}"#);
}

#[test]
fn test_acp_metadata_archive_keeps_rows_the_transcript_projection_drops() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    store
        .add_acp_metadata_message_with_role(
            &session.id,
            MessageRole::User,
            &AcpMessageMetadata {
                acp_event_kind: Some("user_message_chunk".to_string()),
                acp_message_id: Some("user-msg-1".to_string()),
                acp_content: Some(serde_json::json!({
                    "content": {"type": "text", "text": "hello"}
                })),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("usage_update".to_string()),
                acp_usage: Some(serde_json::json!({
                    "used": 42,
                    "size": 1000
                })),
                ..Default::default()
            },
        )
        .unwrap();

    assert!(
        store.get_session_messages(&session.id).unwrap().is_empty(),
        "ACP metadata rows must stay hidden from legacy transcript reads"
    );
    assert!(
        store
            .get_session_acp_metadata_messages(&session.id)
            .unwrap()
            .is_empty(),
        "chunk and usage rows are replay-only and never shipped to the transcript"
    );

    let archive_rows = store.get_session_acp_metadata_archive(&session.id).unwrap();
    assert_eq!(archive_rows.len(), 2);
    assert_eq!(archive_rows[0].role, MessageRole::User);
    assert_eq!(
        archive_rows[0].acp.acp_message_id.as_deref(),
        Some("user-msg-1")
    );
    assert_eq!(archive_rows[1].acp.acp_usage.as_ref().unwrap()["used"], 42);
}

/// Six visible rows with a hidden metadata row between each pair, so a
/// window's `limit` is shown to count visible rows only.
fn seed_windowed_transcript(store: &Store) -> (Session, Vec<i64>) {
    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    let mut visible_ids = Vec::new();
    for turn in 0..3 {
        visible_ids.push(
            store
                .add_session_message(&session.id, MessageRole::User, &format!("ask {turn}"))
                .unwrap(),
        );
        store
            .add_acp_metadata_message(
                &session.id,
                &AcpMessageMetadata {
                    acp_event_kind: Some("agent_message_chunk".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        visible_ids.push(
            store
                .add_session_message(
                    &session.id,
                    MessageRole::Assistant,
                    &format!("answer {turn}"),
                )
                .unwrap(),
        );
        store
            .add_acp_metadata_message(
                &session.id,
                &AcpMessageMetadata {
                    acp_event_kind: Some("usage_update".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    (session, visible_ids)
}

fn ids(messages: &[SessionMessage]) -> Vec<i64> {
    messages.iter().map(|m| m.id).collect()
}

#[test]
fn test_session_messages_window_returns_newest_visible_rows_ascending() {
    let store = Store::in_memory().unwrap();
    let (session, visible) = seed_windowed_transcript(&store);

    let window = store
        .get_session_messages_window(
            &session.id,
            TranscriptWindow {
                limit: Some(3),
                before_id: None,
            },
        )
        .unwrap();
    assert_eq!(ids(&window), visible[3..].to_vec());
    assert_eq!(window[0].content, "answer 1");
    assert_eq!(window[2].content, "answer 2");
}

#[test]
fn test_session_messages_window_pages_backwards_with_before_id() {
    let store = Store::in_memory().unwrap();
    let (session, visible) = seed_windowed_transcript(&store);

    let page = |before_id: i64| {
        ids(&store
            .get_session_messages_window(
                &session.id,
                TranscriptWindow {
                    limit: Some(2),
                    before_id: Some(before_id),
                },
            )
            .unwrap())
    };
    assert_eq!(page(visible[4]), visible[2..4].to_vec());
    assert_eq!(page(visible[2]), visible[0..2].to_vec());
    assert_eq!(page(visible[0]), Vec::<i64>::new());
    // A short final page: fewer rows than asked for is how the caller learns
    // it has reached the start (it requests one more than it will show).
    assert_eq!(
        ids(&store
            .get_session_messages_window(
                &session.id,
                TranscriptWindow {
                    limit: Some(5),
                    before_id: Some(visible[3]),
                },
            )
            .unwrap()),
        visible[0..3].to_vec()
    );
}

#[test]
fn test_session_messages_window_limit_larger_than_transcript_returns_everything() {
    let store = Store::in_memory().unwrap();
    let (session, visible) = seed_windowed_transcript(&store);

    let window = store
        .get_session_messages_window(
            &session.id,
            TranscriptWindow {
                limit: Some(visible.len() as u32 + 1),
                before_id: None,
            },
        )
        .unwrap();
    assert_eq!(ids(&window), visible);
    // Exactly the transcript's length is indistinguishable from "more exist"
    // on the server side, which is why the client over-fetches by one.
    let exact = store
        .get_session_messages_window(
            &session.id,
            TranscriptWindow {
                limit: Some(visible.len() as u32),
                before_id: None,
            },
        )
        .unwrap();
    assert_eq!(ids(&exact), visible);
}

#[test]
fn test_session_messages_window_without_params_matches_full_query() {
    let store = Store::in_memory().unwrap();
    let (session, visible) = seed_windowed_transcript(&store);

    let full = store.get_session_messages(&session.id).unwrap();
    assert_eq!(ids(&full), visible);
    let all = store
        .get_session_messages_window(&session.id, TranscriptWindow::ALL)
        .unwrap();
    assert_eq!(
        serde_json::to_string(&all).unwrap(),
        serde_json::to_string(&full).unwrap()
    );
    // `before_id` alone is the whole prefix, still oldest first.
    let prefix = store
        .get_session_messages_window(
            &session.id,
            TranscriptWindow {
                limit: None,
                before_id: Some(visible[4]),
            },
        )
        .unwrap();
    assert_eq!(ids(&prefix), visible[..4].to_vec());
}

/// The windowed read must stay on the session index and must not sort through
/// a temp B-tree: the index yields rowid order in either direction, and the
/// store reverses the (at most `limit`) rows itself.
#[test]
fn test_session_messages_window_query_plan() {
    let store = Store::in_memory().unwrap();
    let session_id = "s";
    for window in [
        TranscriptWindow::ALL,
        TranscriptWindow {
            limit: Some(10),
            before_id: None,
        },
        TranscriptWindow {
            limit: Some(10),
            before_id: Some(5),
        },
        TranscriptWindow {
            limit: None,
            before_id: Some(5),
        },
    ] {
        let conn = store.conn.lock().unwrap();
        let sql = format!(
            "EXPLAIN QUERY PLAN {}",
            super::messages::session_messages_window_sql(&window)
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let plan: Vec<String> = stmt
            .query_map(
                &super::messages::session_messages_window_params(&session_id, &window)[..],
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let plan = plan.join("\n");
        assert!(
            plan.contains("USING INDEX idx_session_messages_session"),
            "{window:?}: {plan}"
        );
        assert!(!plan.contains("TEMP B-TREE"), "{window:?}: {plan}");
    }
}

#[test]
fn test_acp_metadata_messages_query_returns_only_transcript_rows() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let tool_metadata = |status: &str| AcpMessageMetadata {
        acp_event_kind: Some("tool_call".to_string()),
        acp_tool_call_id: Some(format!("tc-{status}")),
        acp_tool_status: Some(status.to_string()),
        ..Default::default()
    };
    // Visible, settled: already in `get_session_messages`, so sending it
    // again would ship the (large) raw output twice.
    let settled_id = store
        .add_session_message(&session.id, MessageRole::ToolCall, "Read file")
        .unwrap();
    store
        .update_message_acp_metadata(settled_id, &tool_metadata("completed"))
        .unwrap();
    // Visible, in flight: the poll must learn about it to re-read it by id.
    let in_flight_id = store
        .add_session_message(&session.id, MessageRole::ToolCall, "Run tests")
        .unwrap();
    store
        .update_message_acp_metadata(in_flight_id, &tool_metadata("in_progress"))
        .unwrap();
    // Hidden rows: one of each side of the allowlist.
    let plan_id = store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("plan_update".to_string()),
                acp_content: Some(serde_json::json!({"entries": []})),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("agent_message_chunk".to_string()),
                acp_content: Some(serde_json::json!({"content": {"type": "text", "text": "hi"}})),
                ..Default::default()
            },
        )
        .unwrap();
    let hidden_tool_id = store
        .add_acp_metadata_message(&session.id, &tool_metadata("completed"))
        .unwrap();

    let ids: Vec<i64> = store
        .get_session_acp_metadata_messages(&session.id)
        .unwrap()
        .iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, vec![in_flight_id, plan_id, hidden_tool_id]);
    assert_eq!(
        store
            .get_session_acp_metadata_archive(&session.id)
            .unwrap()
            .len(),
        5,
        "filtering is read-side only; every row stays in the table"
    );
}

#[test]
fn test_acp_metadata_messages_since_returns_new_and_refetched_rows() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    // A visible tool_call row whose ACP metadata is later mutated in place —
    // the pattern the writer uses for tool status updates.
    let tool_id = store
        .add_session_message(&session.id, MessageRole::ToolCall, "Run tests")
        .unwrap();
    store
        .update_message_acp_metadata(
            tool_id,
            &AcpMessageMetadata {
                acp_event_kind: Some("tool_call".to_string()),
                acp_tool_call_id: Some("tc-1".to_string()),
                acp_tool_status: Some("in_progress".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let plan_id = store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("plan_update".to_string()),
                acp_content: Some(serde_json::json!({"entries": []})),
                ..Default::default()
            },
        )
        .unwrap();
    // A visible row without ACP metadata never shows up in the metadata query,
    // and neither does a replay-only hidden kind.
    store
        .add_session_message(&session.id, MessageRole::Assistant, "text")
        .unwrap();
    store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("usage_update".to_string()),
                acp_usage: Some(serde_json::json!({"used": 1})),
                ..Default::default()
            },
        )
        .unwrap();

    // Cursor past everything, no refetch ids: nothing to return.
    let none = store
        .get_session_acp_metadata_messages_since(&session.id, plan_id, &[])
        .unwrap();
    assert!(none.is_empty());

    // Only rows after the cursor are returned.
    let after_tool = store
        .get_session_acp_metadata_messages_since(&session.id, tool_id, &[])
        .unwrap();
    assert_eq!(after_tool.len(), 1);
    assert_eq!(after_tool[0].id, plan_id);

    // An in-place status update on a row behind the cursor is only observable
    // when the caller re-requests that row explicitly. The settled row comes
    // back even though it no longer passes the transcript filter, so the
    // caller observes the terminal status and stops re-requesting it.
    store
        .update_message_acp_metadata(
            tool_id,
            &AcpMessageMetadata {
                acp_tool_status: Some("completed".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    let refetched = store
        .get_session_acp_metadata_messages_since(&session.id, plan_id, &[tool_id])
        .unwrap();
    assert_eq!(refetched.len(), 1);
    assert_eq!(refetched[0].id, tool_id);
    assert_eq!(
        refetched[0].acp.acp_tool_status.as_deref(),
        Some("completed")
    );
    assert!(
        store
            .get_session_acp_metadata_messages_since(&session.id, 0, &[])
            .unwrap()
            .iter()
            .all(|m| m.id != tool_id),
        "a settled visible tool row is only ever delivered by explicit refetch"
    );

    // The full metadata query and the since-query with a zero cursor agree.
    let full = store
        .get_session_acp_metadata_messages(&session.id)
        .unwrap();
    let since_zero = store
        .get_session_acp_metadata_messages_since(&session.id, 0, &[])
        .unwrap();
    assert_eq!(
        full.iter().map(|m| m.id).collect::<Vec<_>>(),
        since_zero.iter().map(|m| m.id).collect::<Vec<_>>()
    );
}

#[test]
fn test_transcript_reads_project_tool_images_to_refs_and_keep_raw_bytes() {
    use super::ToolImageField;
    use base64::Engine;

    let store = Store::in_memory().unwrap();
    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let png: &[u8] = b"\x89PNG\r\n\x1a\nfixture";
    let jpeg: &[u8] = b"\xff\xd8\xff\xe0jpeg";
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    // Claude's raw result nests the payload in `source`; ACP content wraps
    // the block in a `content` envelope and puts `data`/`mimeType` on it.
    let raw_output = serde_json::json!([
        {"type": "text", "text": "Read 1 image"},
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": encode(png)}},
    ]);
    let content = serde_json::json!([
        {"type": "content", "content": {"type": "image", "mimeType": "image/jpeg", "data": encode(jpeg), "annotations": {"audience": ["user"]}}},
    ]);
    let metadata = |status: &str| AcpMessageMetadata {
        acp_event_kind: Some("tool_call".to_string()),
        acp_tool_call_id: Some("tc-1".to_string()),
        acp_tool_status: Some(status.to_string()),
        acp_raw_output: Some(raw_output.clone()),
        acp_content: Some(content.clone()),
        ..Default::default()
    };
    let id = store
        .add_session_message(&session.id, MessageRole::ToolCall, "Read screenshot.png")
        .unwrap();
    store
        .update_message_acp_metadata(id, &metadata("in_progress"))
        .unwrap();

    // In flight: a later update may still replace or reorder the block list,
    // and a ref is cached as immutable, so the base64 ships inline from every
    // transcript read and the ref lookup refuses the row.
    for message in [
        &store.get_session_messages(&session.id).unwrap()[0],
        &store
            .get_session_messages_since(&session.id, id, &[])
            .unwrap()[0],
        // In flight, so the metadata query ships it too.
        &store
            .get_session_acp_metadata_messages(&session.id)
            .unwrap()[0],
        &store
            .get_session_acp_metadata_messages_since(&session.id, 0, &[id])
            .unwrap()[0],
    ] {
        assert_eq!(message.acp.acp_raw_output, Some(raw_output.clone()));
        assert_eq!(message.acp.acp_content, Some(content.clone()));
    }
    assert_eq!(
        store.get_tool_image(id, ToolImageField::Output, 1).unwrap(),
        None
    );
    assert_eq!(
        store
            .get_tool_image(id, ToolImageField::Content, 0)
            .unwrap(),
        None
    );

    // Settled: the block list is final, so every transcript read projects it.
    store
        .update_message_acp_metadata(id, &metadata("completed"))
        .unwrap();
    let expected_output = serde_json::json!([
        {"type": "text", "text": "Read 1 image"},
        {"type": "image", "ref": format!("{id}/output/1"), "mimeType": "image/png"},
    ]);
    let expected_content = serde_json::json!([
        {"type": "content", "content": {
            "type": "image", "ref": format!("{id}/content/0"), "mimeType": "image/jpeg",
            "annotations": {"audience": ["user"]},
        }},
    ]);
    for message in [
        &store.get_session_messages(&session.id).unwrap()[0],
        &store
            .get_session_messages_since(&session.id, id, &[])
            .unwrap()[0],
        // Settled visible rows leave the metadata query, but a refetch by id
        // still delivers the row so the poll sees it settle.
        &store
            .get_session_acp_metadata_messages_since(&session.id, 0, &[id])
            .unwrap()[0],
    ] {
        assert_eq!(message.acp.acp_raw_output, Some(expected_output.clone()));
        assert_eq!(message.acp.acp_content, Some(expected_content.clone()));
    }

    // Replay and archive readers still see the base64.
    let replay = store.get_session_replay_messages(&session.id).unwrap();
    assert_eq!(replay[0].acp.acp_raw_output, Some(raw_output));
    assert_eq!(replay[0].acp.acp_content, Some(content));
    assert_eq!(
        store.get_session_acp_metadata_archive(&session.id).unwrap()[0]
            .acp
            .acp_content
            .as_ref()
            .unwrap()[0]["content"]["data"],
        encode(jpeg)
    );

    // The refs resolve to the decoded bytes…
    let image = store
        .get_tool_image(id, ToolImageField::Output, 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        (image.mime_type.as_str(), image.bytes.as_slice()),
        ("image/png", png)
    );
    let image = store
        .get_tool_image(id, ToolImageField::Content, 0)
        .unwrap()
        .unwrap();
    assert_eq!(
        (image.mime_type.as_str(), image.bytes.as_slice()),
        ("image/jpeg", jpeg)
    );
    // …and anything that is not a base64 image block is `None`.
    assert_eq!(
        store.get_tool_image(id, ToolImageField::Output, 0).unwrap(),
        None
    );
    assert_eq!(
        store.get_tool_image(id, ToolImageField::Output, 2).unwrap(),
        None
    );
    assert_eq!(
        store
            .get_tool_image(id + 1, ToolImageField::Output, 1)
            .unwrap(),
        None
    );
}

#[test]
fn test_poll_session_matches_the_four_standalone_reads() {
    use base64::Engine;

    let store = Store::in_memory().unwrap();
    let session = Session::new_running("poll me", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .add_queued_session_message(&session.id, "first follow-up", &[], None)
        .unwrap();
    // Queued ids are UUIDs, so rows sharing a millisecond `created_at` sort
    // randomly; keep the two apart so the order assertion below is stable.
    std::thread::sleep(std::time::Duration::from_millis(2));
    store
        .add_queued_session_message(&session.id, "second follow-up", &[], None)
        .unwrap();

    // What the pane has loaded: a prompt, an assistant row a tool call then
    // interrupted, and that tool call (in flight, with a screenshot result
    // that ships inline until the row settles).
    store
        .add_session_message(&session.id, MessageRole::User, "look at this")
        .unwrap();
    let assistant_id = store
        .add_session_message(&session.id, MessageRole::Assistant, "Taking a")
        .unwrap();
    let tool_id = store
        .add_session_message(&session.id, MessageRole::ToolCall, "Read screenshot.png")
        .unwrap();
    let png: &[u8] = b"\x89PNG\r\n\x1a\nfixture";
    let inline_output = serde_json::json!([{
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": "image/png",
            "data": base64::engine::general_purpose::STANDARD.encode(png),
        },
    }]);
    store
        .update_message_acp_metadata(
            tool_id,
            &AcpMessageMetadata {
                acp_event_kind: Some("tool_call".to_string()),
                acp_tool_call_id: Some("tc-1".to_string()),
                acp_tool_status: Some("in_progress".to_string()),
                acp_raw_output: Some(inline_output.clone()),
                ..Default::default()
            },
        )
        .unwrap();

    // Since then: the interrupted row grew, a new tail row and a plan arrived,
    // and a replay-only row the transcript never ships.
    store
        .update_message_content(assistant_id, "Taking a look")
        .unwrap();
    let tail_id = store
        .add_session_message(&session.id, MessageRole::Assistant, "Done.")
        .unwrap();
    let plan_id = store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("plan_update".to_string()),
                acp_content: Some(serde_json::json!({"entries": []})),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("usage_update".to_string()),
                acp_usage: Some(serde_json::json!({"used": 1})),
                ..Default::default()
            },
        )
        .unwrap();

    let request = SessionPollRequest {
        since_id: tool_id,
        refetch_ids: vec![assistant_id],
        metadata_since_id: tool_id,
        metadata_refetch_ids: vec![tool_id],
    };
    let poll = store.poll_session(&session.id, &request).unwrap();

    // Field for field what the standalone reads return for the same cursors.
    fn json(value: &impl serde::Serialize) -> serde_json::Value {
        serde_json::to_value(value).unwrap()
    }
    assert_eq!(
        json(&poll.session),
        json(&store.get_session(&session.id).unwrap())
    );
    assert_eq!(
        json(&poll.queued_messages),
        json(&store.list_queued_session_messages(&session.id).unwrap())
    );
    assert_eq!(
        json(&poll.messages),
        json(
            &store
                .get_session_messages_since(&session.id, tool_id, &[assistant_id])
                .unwrap()
        )
    );
    assert_eq!(
        json(&poll.acp_metadata_messages),
        json(
            &store
                .get_session_acp_metadata_messages_since(&session.id, tool_id, &[tool_id])
                .unwrap()
        )
    );

    // And those are the rows the pane needs.
    assert_eq!(poll.session.unwrap().status, SessionStatus::Running);
    assert_eq!(
        poll.queued_messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>(),
        ["first follow-up", "second follow-up"]
    );
    assert_eq!(
        poll.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
        [assistant_id, tool_id, tail_id]
    );
    assert_eq!(poll.messages[0].content, "Taking a look");
    assert_eq!(
        poll.acp_metadata_messages
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        [tool_id, plan_id]
    );
    assert_eq!(
        poll.messages[1].acp.acp_raw_output,
        Some(inline_output.clone())
    );
    assert_eq!(
        poll.acp_metadata_messages[0].acp.acp_raw_output,
        Some(inline_output.clone())
    );

    // Once the tool settles, the refetch delivers the row with the image
    // projected to a ref, through the poll exactly as through the standalone
    // read.
    store
        .update_message_acp_metadata(
            tool_id,
            &AcpMessageMetadata {
                acp_event_kind: Some("tool_call".to_string()),
                acp_tool_call_id: Some("tc-1".to_string()),
                acp_tool_status: Some("completed".to_string()),
                acp_raw_output: Some(inline_output),
                ..Default::default()
            },
        )
        .unwrap();
    let settled = store.poll_session(&session.id, &request).unwrap();
    let projected = serde_json::json!([
        {"type": "image", "ref": format!("{tool_id}/output/0"), "mimeType": "image/png"},
    ]);
    assert_eq!(
        settled.acp_metadata_messages[0].acp.acp_raw_output,
        Some(projected.clone())
    );
    assert_eq!(
        json(&settled.acp_metadata_messages),
        json(
            &store
                .get_session_acp_metadata_messages_since(&session.id, tool_id, &[tool_id])
                .unwrap()
        )
    );
}

#[test]
fn test_poll_session_from_an_empty_transcript_returns_every_visible_row() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("poll me", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    for (role, content) in [
        (MessageRole::User, "hi"),
        (MessageRole::Assistant, "hello"),
        (MessageRole::User, "again"),
    ] {
        store
            .add_session_message(&session.id, role, content)
            .unwrap();
    }
    store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("agent_message_chunk".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

    let poll = store
        .poll_session(&session.id, &SessionPollRequest::default())
        .unwrap();
    assert_eq!(
        poll.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
        store
            .get_session_messages(&session.id)
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(poll.messages.len(), 3);
    assert!(poll.queued_messages.is_empty());
    assert!(poll.acp_metadata_messages.is_empty());

    // An unknown session is an empty poll, not an error, as `get_session` is.
    let missing = store
        .poll_session("no-such-session", &SessionPollRequest::default())
        .unwrap();
    assert!(missing.session.is_none());
    assert!(missing.messages.is_empty());
}

/// A body one byte past the deferral threshold: `{"text":"xxxx…"}`.
fn oversized_tool_body(key: &str) -> serde_json::Value {
    let envelope = format!("{{\"{key}\":\"\"}}").len();
    serde_json::json!({ key: "x".repeat(DEFERRED_TOOL_BODY_MIN_BYTES + 1 - envelope) })
}

fn deferred_marker(id: i64, field: &str, body: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"$deferred": {
        "messageId": id,
        "field": field,
        "bytes": serde_json::to_vec(body).unwrap().len(),
    }})
}

#[test]
fn test_transcript_reads_defer_large_bodies_of_settled_tool_rows_only() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let big_input = oversized_tool_body("content");
    let big_output = oversized_tool_body("stdout");
    let small_content = serde_json::json!([
        {"type": "content", "content": {"type": "text", "text": "ok"}},
    ]);
    let metadata = |status: &str| AcpMessageMetadata {
        acp_event_kind: Some("tool_call".to_string()),
        acp_tool_call_id: Some("tc-1".to_string()),
        acp_tool_status: Some(status.to_string()),
        acp_raw_input: Some(big_input.clone()),
        acp_raw_output: Some(big_output.clone()),
        acp_content: Some(small_content.clone()),
        ..Default::default()
    };
    let id = store
        .add_session_message(&session.id, MessageRole::ToolCall, "Write notes.md")
        .unwrap();
    store
        .update_message_acp_metadata(id, &metadata("in_progress"))
        .unwrap();

    // In flight: the writer still mutates the row and the poll re-reads it,
    // so every body ships whole.
    let in_flight = &store.get_session_messages(&session.id).unwrap()[0];
    assert_eq!(in_flight.acp.acp_raw_input, Some(big_input.clone()));
    assert_eq!(in_flight.acp.acp_raw_output, Some(big_output.clone()));
    assert_eq!(in_flight.acp.acp_content, Some(small_content.clone()));
    // No marker exists yet, and the body lookup refuses the row so the
    // `immutable` body route cannot cache bodies the writer may still replace.
    assert_eq!(store.get_tool_call_body(id).unwrap(), None);

    // Settled: the two large bodies become markers, the small one stays.
    store
        .update_message_acp_metadata(id, &metadata("completed"))
        .unwrap();
    let expected_input = deferred_marker(id, "input", &big_input);
    let expected_output = deferred_marker(id, "output", &big_output);
    assert!(
        expected_input["$deferred"]["bytes"].as_u64().unwrap()
            == DEFERRED_TOOL_BODY_MIN_BYTES as u64 + 1,
        "fixture should sit one byte past the threshold"
    );
    for message in [
        &store.get_session_messages(&session.id).unwrap()[0],
        &store
            .get_session_messages_since(&session.id, id, &[])
            .unwrap()[0],
        // Settled visible rows leave the metadata query, but a refetch by id
        // still delivers the row (now projected) so the poll sees it settle.
        &store
            .get_session_acp_metadata_messages_since(&session.id, 0, &[id])
            .unwrap()[0],
    ] {
        assert_eq!(message.acp.acp_raw_input, Some(expected_input.clone()));
        assert_eq!(message.acp.acp_raw_output, Some(expected_output.clone()));
        assert_eq!(message.acp.acp_content, Some(small_content.clone()));
        assert_eq!(message.acp.acp_tool_status.as_deref(), Some("completed"));
    }

    // Replay and archive readers still see the bodies, and the table does too.
    let replay = store.get_session_replay_messages(&session.id).unwrap();
    assert_eq!(replay[0].acp.acp_raw_input, Some(big_input.clone()));
    assert_eq!(replay[0].acp.acp_raw_output, Some(big_output.clone()));
    assert_eq!(
        store.get_session_acp_metadata_archive(&session.id).unwrap()[0]
            .acp
            .acp_raw_output,
        Some(big_output.clone())
    );

    // The marker resolves to the row's bodies…
    let body = store.get_tool_call_body(id).unwrap().unwrap();
    assert_eq!(
        body,
        ToolCallBody {
            message_id: id,
            acp_raw_input: Some(big_input),
            acp_raw_output: Some(big_output),
            acp_content: Some(small_content),
        }
    );
    let wire = serde_json::to_value(&body).unwrap();
    assert_eq!(wire["messageId"], serde_json::json!(id));
    for key in ["acpRawInput", "acpRawOutput", "acpContent"] {
        assert!(
            wire.get(key).is_some(),
            "wire field names follow SessionMessage: {key}"
        );
    }
    // …and an unknown row is `None`.
    assert_eq!(store.get_tool_call_body(id + 1).unwrap(), None);
}

#[test]
fn test_tool_body_deferral_judges_the_projected_size_and_keeps_pikchr_hints() {
    use base64::Engine;

    let store = Store::in_memory().unwrap();
    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    // A screenshot result: far past the threshold as stored, tiny once the
    // image is a ref — so it ships inline, and the body route projects the
    // same ref.
    let png = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 4096]);
    let image_output = serde_json::json!([
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": png}},
    ]);
    let image_id = store
        .add_session_message(&session.id, MessageRole::ToolCall, "Read shot.png")
        .unwrap();
    store
        .update_message_acp_metadata(
            image_id,
            &AcpMessageMetadata {
                acp_event_kind: Some("tool_call".to_string()),
                acp_tool_call_id: Some("tc-img".to_string()),
                acp_tool_status: Some("completed".to_string()),
                acp_raw_output: Some(image_output),
                ..Default::default()
            },
        )
        .unwrap();
    let projected = serde_json::json!([
        {"type": "image", "ref": format!("{image_id}/output/0"), "mimeType": "image/png"},
    ]);
    assert_eq!(
        store.get_session_messages(&session.id).unwrap()[0]
            .acp
            .acp_raw_output,
        Some(projected.clone())
    );
    assert_eq!(
        store
            .get_tool_call_body(image_id)
            .unwrap()
            .unwrap()
            .acp_raw_output,
        Some(projected)
    );

    // A `render_pikchr` call keeps its source in the marker (the collapsed
    // card draws it), and a `generate_pikchr` result keeps the child session
    // id it names however deeply.
    let pikchr_source = format!("box \"{}\"", "x".repeat(DEFERRED_TOOL_BODY_MIN_BYTES));
    let pikchr_input = serde_json::json!({"pikchr": pikchr_source, "description": "big"});
    let pikchr_output = serde_json::json!({
        "content": [{"type": "text", "text": "x".repeat(DEFERRED_TOOL_BODY_MIN_BYTES)}],
        "result": {"structured_content": {"inner_session_id": "child-1"}},
    });
    let pikchr_id = store
        .add_session_message(&session.id, MessageRole::ToolCall, "render_pikchr")
        .unwrap();
    store
        .update_message_acp_metadata(
            pikchr_id,
            &AcpMessageMetadata {
                acp_event_kind: Some("tool_call".to_string()),
                acp_tool_call_id: Some("tc-pikchr".to_string()),
                acp_tool_status: Some("completed".to_string()),
                acp_raw_input: Some(pikchr_input.clone()),
                acp_raw_output: Some(pikchr_output.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    let row = &store.get_session_messages(&session.id).unwrap()[1];
    let mut expected_input = deferred_marker(pikchr_id, "input", &pikchr_input);
    expected_input["$deferred"]["pikchr"] = serde_json::Value::String(pikchr_source);
    assert_eq!(row.acp.acp_raw_input, Some(expected_input));
    let mut expected_output = deferred_marker(pikchr_id, "output", &pikchr_output);
    expected_output["$deferred"]["innerSessionId"] = serde_json::Value::String("child-1".into());
    assert_eq!(row.acp.acp_raw_output, Some(expected_output));
}

#[test]
fn test_acp_initialization_metadata_is_queryable() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("initialize".to_string()),
                acp_protocol_version: Some("1".to_string()),
                acp_agent_capabilities: Some(serde_json::json!({
                    "loadSession": true,
                    "promptCapabilities": {"image": true},
                    "mcpCapabilities": {"http": true, "sse": false}
                })),
                acp_auth_methods: Some(serde_json::json!([
                    {"id": "default", "name": "Default"}
                ])),
                acp_agent_info: Some(serde_json::json!({
                    "name": "goose",
                    "version": "1.2.3"
                })),
                ..Default::default()
            },
        )
        .unwrap();

    assert!(
        store.get_session_messages(&session.id).unwrap().is_empty(),
        "initialization metadata must remain hidden from legacy transcript reads"
    );

    let metadata = store
        .get_session_acp_initialization(&session.id)
        .unwrap()
        .expect("initialization metadata should be returned");
    assert_eq!(metadata.acp_event_kind.as_deref(), Some("initialize"));
    assert_eq!(metadata.acp_protocol_version.as_deref(), Some("1"));
    assert_eq!(
        metadata.acp_agent_capabilities.as_ref().unwrap()["promptCapabilities"]["image"],
        true
    );
    assert_eq!(
        metadata.acp_auth_methods.as_ref().unwrap()[0]["id"],
        "default"
    );
    assert_eq!(
        metadata.acp_agent_info.as_ref().unwrap()["version"],
        "1.2.3"
    );
}

#[test]
fn test_count_assistant_messages_after() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    // Add messages with different roles — timestamps are auto-set via now_timestamp()
    // so we use a timestamp of 0 to count all assistant messages.
    store
        .add_session_message(&session.id, MessageRole::User, "hello")
        .unwrap();
    store
        .add_session_message(&session.id, MessageRole::Assistant, "hi there")
        .unwrap();
    store
        .add_session_message(&session.id, MessageRole::User, "more")
        .unwrap();
    store
        .add_session_message(&session.id, MessageRole::Assistant, "reply")
        .unwrap();

    // All assistant messages are after timestamp 0
    let count = store
        .count_assistant_messages_after(&session.id, 0)
        .unwrap();
    assert_eq!(count, 2);

    // No assistant messages after a far-future timestamp
    let count = store
        .count_assistant_messages_after(&session.id, i64::MAX)
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_session_chat_activity_tracks_the_latest_visible_reply() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    let set_created_at = |id: i64, created_at: i64| {
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE session_messages SET created_at = ?1 WHERE id = ?2",
                rusqlite::params![created_at, id],
            )
            .unwrap();
    };

    assert_eq!(
        store.session_chat_activity(&session.id).unwrap(),
        (None, None)
    );

    let reply = store
        .add_session_message(&session.id, MessageRole::Assistant, "reply")
        .unwrap();
    set_created_at(reply, 1_000);
    // Rows the chat doesn't show as replies never count, however late.
    for role in [
        MessageRole::User,
        MessageRole::ToolCall,
        MessageRole::ToolResult,
    ] {
        let id = store
            .add_session_message(&session.id, role, "not a reply")
            .unwrap();
        set_created_at(id, 5_000);
    }
    let metadata = store
        .add_acp_metadata_message(
            &session.id,
            &AcpMessageMetadata {
                acp_event_kind: Some("session_info_update".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    set_created_at(metadata, 5_000);

    assert_eq!(
        store.session_chat_activity(&session.id).unwrap(),
        (Some(1_000), None)
    );
}

#[test]
fn test_mark_session_chat_read_only_writes_for_new_replies() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    let read_at = || store.session_chat_activity(&session.id).unwrap().1;

    // A chat with no replies has nothing to read.
    store.mark_session_chat_read(&session.id).unwrap();
    assert_eq!(read_at(), None);

    store
        .add_session_message(&session.id, MessageRole::Assistant, "reply")
        .unwrap();
    store.mark_session_chat_read(&session.id).unwrap();
    let first_read = read_at().unwrap();
    let reply_at = store.session_chat_activity(&session.id).unwrap().0;
    assert!(reply_at.is_some_and(|at| at <= first_read));

    // Reopening with nothing new leaves the read time alone.
    std::thread::sleep(std::time::Duration::from_millis(2));
    store.mark_session_chat_read(&session.id).unwrap();
    assert_eq!(read_at(), Some(first_read));

    // A reply after the read moves it on at the next mark.
    let later = store
        .add_session_message(&session.id, MessageRole::Assistant, "later reply")
        .unwrap();
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE session_messages SET created_at = ?1 WHERE id = ?2",
            rusqlite::params![first_read + 1, later],
        )
        .unwrap();
    store.mark_session_chat_read(&session.id).unwrap();
    assert!(read_at().unwrap() > first_read);
}

#[test]
fn test_set_note_read_marks_landed_notes_only() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // A session's note is a stub until its content lands; opening it then
    // marks nothing, so it still reads as unread once it lands.
    let note = Note::new(&branch.id, "", "").with_session("session-1");
    store.create_note(&note).unwrap();
    let opened = store.set_note_read(&note.id, true).unwrap();
    assert_eq!((opened.completed_at, opened.read_at), (None, None));

    store
        .update_note_title_and_content(&note.id, "Plan", "body", None, None)
        .unwrap();
    let landed = store.get_note(&note.id).unwrap().unwrap();
    assert!(landed.completed_at.is_some());
    assert_eq!(landed.read_at, None);

    let read = store.set_note_read(&note.id, true).unwrap();
    let read_at = read.read_at.unwrap();
    assert!(read_at >= landed.completed_at.unwrap());

    // Reopening a read note leaves the read time alone.
    std::thread::sleep(std::time::Duration::from_millis(2));
    assert_eq!(
        store.set_note_read(&note.id, true).unwrap().read_at,
        Some(read_at)
    );
    // The session re-emitting the same note changes nothing, so it stays read.
    store
        .update_note_title_and_content(&note.id, "Plan", "body", None, None)
        .unwrap();
    assert_eq!(
        store.get_note(&note.id).unwrap().unwrap().read_at,
        Some(read_at)
    );
    // New suggested next steps alone are no new content to read: the write
    // lands, but the note stays read.
    store
        .update_note_title_and_content(&note.id, "Plan", "body", Some("Commit it"), None)
        .unwrap();
    let stepped = store.get_note(&note.id).unwrap().unwrap();
    assert_eq!(
        stepped.suggested_next_commit_step.as_deref(),
        Some("Commit it")
    );
    assert!(stepped.updated_at > landed.updated_at);
    assert_eq!(stepped.read_at, Some(read_at));
    // A revision is new content the user hasn't seen: it reads as unread
    // again, and opening it marks it read again.
    store
        .update_note_title_and_content(&note.id, "Plan", "revised body", None, None)
        .unwrap();
    let revised = store.get_note(&note.id).unwrap().unwrap();
    assert_eq!(revised.completed_at, landed.completed_at);
    assert_eq!(revised.read_at, None);
    assert!(store.set_note_read(&note.id, true).unwrap().is_read());

    let unread = store.set_note_read(&note.id, false).unwrap();
    assert_eq!(unread.read_at, None);
    assert_eq!(unread.content, "revised body");

    assert!(store.set_note_read("missing", true).is_err());
}

#[test]
fn test_set_note_read_survives_a_clock_behind_completion() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let future = super::now_timestamp() + 60_000;
    let note = Note {
        completed_at: Some(future),
        ..Note::new(&branch.id, "Plan", "body")
    };
    store.create_note(&note).unwrap();
    let project_note = ProjectNote {
        completed_at: Some(future),
        ..ProjectNote::new(&project.id, "Overview", "body")
    };
    store.create_project_note(&project_note).unwrap();

    // Stamped "now" alone, the read would predate the note and leave it unread.
    assert_eq!(
        store.set_note_read(&note.id, true).unwrap().read_at,
        Some(future)
    );
    assert_eq!(
        store
            .set_project_note_read(&project_note.id, true)
            .unwrap()
            .read_at,
        Some(future)
    );
}

#[test]
fn test_set_project_note_read_round_trips_and_lists() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let note = ProjectNote::new(&project.id, "Overview", "body");
    store.create_project_note(&note).unwrap();
    let read_at_in_list = || store.list_project_notes_with_status(&project.id).unwrap()[0].read_at;

    // New notes start unread.
    assert_eq!(read_at_in_list(), None);

    let read = store.set_project_note_read(&note.id, true).unwrap();
    assert_eq!(read.project_id, project.id);
    assert!(read.read_at.is_some());
    assert_eq!(read_at_in_list(), read.read_at);

    store.set_project_note_read(&note.id, false).unwrap();
    assert_eq!(read_at_in_list(), None);

    // An agent revision makes a read project note unread again.
    store.set_project_note_read(&note.id, true).unwrap();
    store
        .update_project_note_title_and_content(&note.id, "Overview", "body", None, None)
        .unwrap();
    assert!(read_at_in_list().is_some());
    // New suggested next steps alone leave it read.
    store
        .update_project_note_title_and_content(
            &note.id,
            "Overview",
            "body",
            None,
            Some("Write the plan"),
        )
        .unwrap();
    let stepped = store.get_project_note(&note.id).unwrap().unwrap();
    assert_eq!(
        stepped.suggested_next_note_step.as_deref(),
        Some("Write the plan")
    );
    assert!(read_at_in_list().is_some());
    store
        .update_project_note_title_and_content(&note.id, "Overview", "revised", None, None)
        .unwrap();
    assert_eq!(read_at_in_list(), None);

    assert!(store.set_project_note_read("missing", false).is_err());
}

// =============================================================================
// Workdirs
// =============================================================================

#[test]
fn test_workdir_crud() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let workdir = Workdir::new(&project.id, "/tmp/wt/feature");
    store.create_workdir(&workdir).unwrap();

    let fetched = store.get_workdir(&workdir.id).unwrap().unwrap();
    assert_eq!(fetched.path, "/tmp/wt/feature");
    assert!(fetched.branch_id.is_none());
}

#[test]
fn test_workdir_assign_and_release() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let workdir = Workdir::new(&project.id, "/tmp/wt/feature");
    store.create_workdir(&workdir).unwrap();

    // Assign
    store.assign_workdir(&workdir.id, &branch.id).unwrap();
    let assigned = store.get_workdir(&workdir.id).unwrap().unwrap();
    assert_eq!(assigned.branch_id.as_deref(), Some(branch.id.as_str()));

    // Look up by branch
    let by_branch = store.get_workdir_for_branch(&branch.id).unwrap().unwrap();
    assert_eq!(by_branch.id, workdir.id);

    // Release
    store.release_workdir(&workdir.id).unwrap();
    let released = store.get_workdir(&workdir.id).unwrap().unwrap();
    assert!(released.branch_id.is_none());

    // No longer found by branch
    assert!(store.get_workdir_for_branch(&branch.id).unwrap().is_none());
}

#[test]
fn test_workdir_find_available() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let w1 = Workdir::new(&project.id, "/tmp/wt/1").with_branch(&branch.id);
    let w2 = Workdir::new(&project.id, "/tmp/wt/2");
    store.create_workdir(&w1).unwrap();
    store.create_workdir(&w2).unwrap();

    // w1 is occupied, w2 is available
    let available = store.find_available_workdir(&project.id).unwrap().unwrap();
    assert_eq!(available.id, w2.id);
}

#[test]
fn test_workdir_find_available_none() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let w1 = Workdir::new(&project.id, "/tmp/wt/1").with_branch(&branch.id);
    store.create_workdir(&w1).unwrap();

    // All occupied
    assert!(store.find_available_workdir(&project.id).unwrap().is_none());
}

#[test]
fn test_workdir_unique_path_per_project() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let w1 = Workdir::new(&project.id, "/tmp/wt/1");
    let w2 = Workdir::new(&project.id, "/tmp/wt/1");
    store.create_workdir(&w1).unwrap();
    assert!(store.create_workdir(&w2).is_err());
}

#[test]
fn test_workdir_branch_id_nulled_on_branch_delete() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let workdir = Workdir::new(&project.id, "/tmp/wt").with_branch(&branch.id);
    store.create_workdir(&workdir).unwrap();

    // Delete branch — workdir should remain but with branch_id = NULL
    store.delete_branch(&branch.id).unwrap();
    let after = store.get_workdir(&workdir.id).unwrap().unwrap();
    assert!(after.branch_id.is_none());
}

#[test]
fn test_list_workdirs_for_project() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();

    let w1 = Workdir::new(&project.id, "/tmp/wt/1");
    let w2 = Workdir::new(&project.id, "/tmp/wt/2");
    store.create_workdir(&w1).unwrap();
    store.create_workdir(&w2).unwrap();

    let workdirs = store.list_workdirs_for_project(&project.id).unwrap();
    assert_eq!(workdirs.len(), 2);
}

// =============================================================================
// Commits
// =============================================================================

#[test]
fn test_commit_with_sha() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let session = Session::new_running("first commit", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let commit = Commit::new_with_sha(&branch.id, "aaa111").with_session(&session.id);
    store.create_commit(&commit).unwrap();

    // Look up by id
    let fetched = store.get_commit(&commit.id).unwrap().unwrap();
    assert_eq!(fetched.sha.as_deref(), Some("aaa111"));
    assert_eq!(fetched.session_id.as_deref(), Some(session.id.as_str()));

    // Look up by sha
    let by_sha = store
        .get_commit_by_sha(&branch.id, "aaa111")
        .unwrap()
        .unwrap();
    assert_eq!(by_sha.id, commit.id);

    // Unknown sha returns None
    let missing = store.get_commit_by_sha(&branch.id, "zzz999").unwrap();
    assert!(missing.is_none());
}

#[test]
fn test_commit_pending_then_landed() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let session = Session::new_running("working on it", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    // Create pending commit (no SHA yet)
    let commit = Commit::new_pending(&branch.id).with_session(&session.id);
    store.create_commit(&commit).unwrap();

    let pending = store.get_commit(&commit.id).unwrap().unwrap();
    assert!(pending.sha.is_none());
    assert_eq!(pending.session_id.as_deref(), Some(session.id.as_str()));

    // Listed in branch commits
    let commits = store.list_commits_for_branch(&branch.id).unwrap();
    assert_eq!(commits.len(), 1);
    assert!(commits[0].sha.is_none());

    // Commit lands in git — update SHA
    store.update_commit_sha(&commit.id, "bbb222").unwrap();

    let landed = store.get_commit(&commit.id).unwrap().unwrap();
    assert_eq!(landed.sha.as_deref(), Some("bbb222"));

    // Can now look up by sha
    let by_sha = store
        .get_commit_by_sha(&branch.id, "bbb222")
        .unwrap()
        .unwrap();
    assert_eq!(by_sha.id, commit.id);
}

#[test]
fn test_commit_unique_sha_per_branch() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let c1 = Commit::new_with_sha(&branch.id, "aaa111");
    let c2 = Commit::new_with_sha(&branch.id, "aaa111");
    store.create_commit(&c1).unwrap();
    assert!(store.create_commit(&c2).is_err());

    // Multiple pending commits (sha = NULL) are allowed
    let p1 = Commit::new_pending(&branch.id);
    let p2 = Commit::new_pending(&branch.id);
    store.create_commit(&p1).unwrap();
    store.create_commit(&p2).unwrap();
}

#[test]
fn test_complete_pending_commit_sha_resolves_duplicate_without_unique_error() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let existing = Commit::new_with_sha(&branch.id, "aaa111");
    let pending = Commit::new_pending(&branch.id);
    store.create_commit(&existing).unwrap();
    store.create_commit(&pending).unwrap();

    let updated = store
        .complete_pending_commit_sha(&pending.id, &branch.id, "aaa111")
        .unwrap();

    assert!(!updated);
    assert!(store.get_commit(&existing.id).unwrap().is_some());
    assert!(store.get_commit(&pending.id).unwrap().is_none());
}

#[test]
fn test_complete_pending_commit_sha_updates_pending_row() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let pending = Commit::new_pending(&branch.id);
    store.create_commit(&pending).unwrap();

    let updated = store
        .complete_pending_commit_sha(&pending.id, &branch.id, "bbb222")
        .unwrap();

    assert!(updated);
    let commit = store.get_commit(&pending.id).unwrap().unwrap();
    assert_eq!(commit.sha.as_deref(), Some("bbb222"));
}

#[test]
fn test_remap_commit_shas_moves_rows_and_their_reviews() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let first = Commit::new_with_sha(&branch.id, "old111");
    let second = Commit::new_with_sha(&branch.id, "old222");
    store.create_commit(&first).unwrap();
    store.create_commit(&second).unwrap();
    let review = Review::new(&branch.id, "old222", ReviewScope::Commit);
    store.create_review(&review).unwrap();

    let remapped = store
        .remap_commit_shas(
            &branch.id,
            &[
                (first.id.as_str(), "old111", "new111"),
                (second.id.as_str(), "old222", "new222"),
            ],
        )
        .unwrap();

    assert_eq!(remapped, 2);
    let first = store.get_commit(&first.id).unwrap().unwrap();
    assert_eq!(first.sha.as_deref(), Some("new111"));
    let second = store.get_commit(&second.id).unwrap().unwrap();
    assert_eq!(second.sha.as_deref(), Some("new222"));
    let review = store.get_review(&review.id).unwrap().unwrap();
    assert_eq!(review.commit_sha, "new222");
}

/// The `(branch_id, sha)` unique index is re-checked inside the transaction,
/// so a target SHA another row already owns is skipped rather than blowing up
/// the whole remap.
#[test]
fn test_remap_commit_shas_skips_target_owned_by_another_row() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let orphan = Commit::new_with_sha(&branch.id, "old111");
    let owner = Commit::new_with_sha(&branch.id, "new111");
    let movable = Commit::new_with_sha(&branch.id, "old222");
    store.create_commit(&orphan).unwrap();
    store.create_commit(&owner).unwrap();
    store.create_commit(&movable).unwrap();

    let remapped = store
        .remap_commit_shas(
            &branch.id,
            &[
                (orphan.id.as_str(), "old111", "new111"),
                (movable.id.as_str(), "old222", "new222"),
            ],
        )
        .unwrap();

    assert_eq!(remapped, 1);
    let orphan = store.get_commit(&orphan.id).unwrap().unwrap();
    assert_eq!(orphan.sha.as_deref(), Some("old111"));
    let movable = store.get_commit(&movable.id).unwrap().unwrap();
    assert_eq!(movable.sha.as_deref(), Some("new222"));
}

#[test]
fn test_delete_branch_cascades_commits() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let session = Session::new_running("test", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .add_session_message(&session.id, MessageRole::User, "hi")
        .unwrap();

    let commit = Commit::new_with_sha(&branch.id, "abc").with_session(&session.id);
    store.create_commit(&commit).unwrap();

    let note = Note::new(&branch.id, "Note", "some content");
    store.create_note(&note).unwrap();

    store.delete_branch(&branch.id).unwrap();

    // Session is still running, so the cleanup trigger leaves it alone
    assert!(store.get_session(&session.id).unwrap().is_some());
    assert!(!store.get_session_messages(&session.id).unwrap().is_empty());
    // Commits and notes cascade from branch
    assert!(store.get_commit(&commit.id).unwrap().is_none());
    assert!(store.get_note(&note.id).unwrap().is_none());
}

// =============================================================================
// Session cleanup triggers
// =============================================================================

#[test]
fn test_completed_session_cleaned_up_on_branch_delete() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // Completed session linked to a commit
    let session = Session::new_running("make changes", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();

    let commit = Commit::new_with_sha(&branch.id, "abc").with_session(&session.id);
    store.create_commit(&commit).unwrap();

    store.delete_branch(&branch.id).unwrap();

    // Commit cascaded, trigger cleaned up the session + its messages
    assert!(store.get_commit(&commit.id).unwrap().is_none());
    assert!(store.get_session(&session.id).unwrap().is_none());
}

#[test]
fn test_session_not_cleaned_up_if_still_referenced() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch_a = Branch::new(&project.id, "feature-a", "main");
    let branch_b = Branch::new(&project.id, "feature-b", "main");
    store.create_branch(&branch_a).unwrap();
    store.create_branch(&branch_b).unwrap();

    // Same session referenced by commits on two branches
    let session = Session::new_running("shared work", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();

    let commit_a = Commit::new_with_sha(&branch_a.id, "aaa").with_session(&session.id);
    let commit_b = Commit::new_with_sha(&branch_b.id, "bbb").with_session(&session.id);
    store.create_commit(&commit_a).unwrap();
    store.create_commit(&commit_b).unwrap();

    // Delete branch_a — session still referenced by branch_b's commit
    store.delete_branch(&branch_a.id).unwrap();
    assert!(store.get_session(&session.id).unwrap().is_some());

    // Delete branch_b — now the session is unreferenced and gets cleaned up
    store.delete_branch(&branch_b.id).unwrap();
    assert!(store.get_session(&session.id).unwrap().is_none());
}

#[test]
fn test_running_session_not_cleaned_up() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // Running session linked to a commit
    let session = Session::new_running("still working", Path::new("/tmp"));
    store.create_session(&session).unwrap();

    let commit = Commit::new_with_sha(&branch.id, "abc").with_session(&session.id);
    store.create_commit(&commit).unwrap();

    store.delete_branch(&branch.id).unwrap();

    // Session is still running — trigger leaves it alone
    assert!(store.get_session(&session.id).unwrap().is_some());
}

/// A Blox session whose turn may still be running on its workstation keeps
/// its row when its artifacts go, so reconcile can still settle it.
#[test]
fn test_disconnected_session_not_cleaned_up() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let session = Session::new_running("working remotely", Path::new("/tmp"));
    disconnected_row(&store, &session);
    let commit = Commit::new_with_sha(&branch.id, "abc").with_session(&session.id);
    store.create_commit(&commit).unwrap();
    let note = Note::new(&branch.id, "Design", "content").with_session(&session.id);
    store.create_note(&note).unwrap();

    store.delete_note(&note.id).unwrap();
    store.delete_branch(&branch.id).unwrap();

    let row = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(row.status, SessionStatus::Disconnected);
}

#[test]
fn test_session_cleaned_up_via_note_delete() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let session = Session::new_running("write notes", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();

    let note = Note::new(&branch.id, "Design", "content").with_session(&session.id);
    store.create_note(&note).unwrap();

    store.delete_branch(&branch.id).unwrap();

    // Note cascaded, trigger cleaned up the session
    assert!(store.get_session(&session.id).unwrap().is_none());
}

#[test]
fn test_session_cleaned_up_via_review_delete() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let session = Session::new_running("review code", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();

    let review = Review::new(&branch.id, "abc123", ReviewScope::Commit).with_session(&session.id);
    store.create_review(&review).unwrap();

    store.delete_branch(&branch.id).unwrap();

    // Review cascaded, trigger cleaned up the session
    assert!(store.get_session(&session.id).unwrap().is_none());
}

#[test]
fn test_session_messages_cascade_from_session_cleanup() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let session = Session::new_running("chat", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .add_session_message(&session.id, MessageRole::User, "hello")
        .unwrap();
    store
        .add_session_message(&session.id, MessageRole::Assistant, "hi")
        .unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();

    let commit = Commit::new_with_sha(&branch.id, "abc").with_session(&session.id);
    store.create_commit(&commit).unwrap();

    store.delete_branch(&branch.id).unwrap();

    // Session and its messages are both gone
    assert!(store.get_session(&session.id).unwrap().is_none());
    assert!(store.get_session_messages(&session.id).unwrap().is_empty());
}

// =============================================================================
// Notes
// =============================================================================

#[test]
fn test_notes() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let note = Note::new(&branch.id, "Design Doc", "# Design\n\nHere is the design.");
    store.create_note(&note).unwrap();

    // Listed in branch notes
    let notes = store.list_notes_for_branch(&branch.id).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].content.contains("Design"));
}

#[test]
fn test_list_notes_for_branch_orders_by_completion_time() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let older = Note::new(&branch.id, "", "").with_session("session-older");
    store.create_note(&older).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));

    let newer = Note::new(&branch.id, "", "").with_session("session-newer");
    store.create_note(&newer).unwrap();

    store
        .update_note_title_and_content(&newer.id, "Newer", "Completed first", None, None)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    store
        .update_note_title_and_content(&older.id, "Older", "Completed second", None, None)
        .unwrap();

    let notes = store.list_notes_for_branch(&branch.id).unwrap();
    let ordered_ids: Vec<_> = notes.iter().map(|note| note.id.as_str()).collect();
    assert_eq!(ordered_ids, vec![older.id.as_str(), newer.id.as_str()]);
}

#[test]
fn test_update_note_title_and_content_is_noop_when_unchanged() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let note = Note::new(&branch.id, "", "").with_session("session-1");
    store.create_note(&note).unwrap();

    store
        .update_note_title_and_content(
            &note.id,
            "Title",
            "Body",
            Some("commit-step"),
            Some("note-step"),
        )
        .unwrap();
    let after_first = store.get_note(&note.id).unwrap().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(2));

    // Same title/content/steps — must not bump updated_at.
    store
        .update_note_title_and_content(
            &note.id,
            "Title",
            "Body",
            Some("commit-step"),
            Some("note-step"),
        )
        .unwrap();
    let after_second = store.get_note(&note.id).unwrap().unwrap();
    assert_eq!(after_second.updated_at, after_first.updated_at);
    assert_eq!(after_second.completed_at, after_first.completed_at);

    // A real change still advances updated_at.
    std::thread::sleep(std::time::Duration::from_millis(2));
    store
        .update_note_title_and_content(
            &note.id,
            "Title",
            "New body",
            Some("commit-step"),
            Some("note-step"),
        )
        .unwrap();
    let after_third = store.get_note(&note.id).unwrap().unwrap();
    assert!(after_third.updated_at > after_first.updated_at);
}

#[test]
fn test_create_note_with_unique_title_appends_increment_on_collision() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let mut first = Note::new(&branch.id, "Build log", "first");
    store.create_note_with_unique_title(&mut first).unwrap();
    assert_eq!(first.title, "Build log");

    let mut second = Note::new(&branch.id, "Build log", "second");
    store.create_note_with_unique_title(&mut second).unwrap();
    assert_eq!(second.title, "Build log (2)");

    let mut third = Note::new(&branch.id, "Build log", "third");
    store.create_note_with_unique_title(&mut third).unwrap();
    assert_eq!(third.title, "Build log (3)");

    // Each insert actually landed, with the disambiguated titles persisted.
    let titles: Vec<_> = store
        .list_notes_for_branch(&branch.id)
        .unwrap()
        .into_iter()
        .map(|n| n.title)
        .collect();
    assert!(titles.contains(&"Build log".to_string()));
    assert!(titles.contains(&"Build log (2)".to_string()));
    assert!(titles.contains(&"Build log (3)".to_string()));
}

#[test]
fn test_create_note_with_unique_title_picks_max_plus_one_with_gaps() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // Seed a non-contiguous set: "Build log" and "Build log (5)" exist, but
    // (2)–(4) do not. macOS-Finder convention says the next pick is (6).
    let mut base = Note::new(&branch.id, "Build log", "");
    store.create_note_with_unique_title(&mut base).unwrap();
    let manual = Note::new(&branch.id, "Build log (5)", "");
    store.create_note(&manual).unwrap();

    let mut next = Note::new(&branch.id, "Build log", "");
    store.create_note_with_unique_title(&mut next).unwrap();
    assert_eq!(next.title, "Build log (6)");
}

#[test]
fn test_create_note_with_unique_title_is_scoped_to_branch() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch_a = Branch::new(&project.id, "feature-a", "main");
    let branch_b = Branch::new(&project.id, "feature-b", "main");
    store.create_branch(&branch_a).unwrap();
    store.create_branch(&branch_b).unwrap();

    let mut on_a = Note::new(&branch_a.id, "Build log", "");
    store.create_note_with_unique_title(&mut on_a).unwrap();

    // Same title on a different branch must not trigger a suffix.
    let mut on_b = Note::new(&branch_b.id, "Build log", "");
    store.create_note_with_unique_title(&mut on_b).unwrap();
    assert_eq!(on_b.title, "Build log");
}

#[test]
fn test_create_note_with_unique_title_skips_empty_titles() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // Empty-titled session stubs must stay empty — the runner fills them in
    // later via update_note_title_and_content.
    let mut first = Note::new(&branch.id, "", "").with_session("session-1");
    store.create_note_with_unique_title(&mut first).unwrap();
    let mut second = Note::new(&branch.id, "", "").with_session("session-2");
    store.create_note_with_unique_title(&mut second).unwrap();
    assert_eq!(first.title, "");
    assert_eq!(second.title, "");
}

#[test]
fn test_written_note_round_trips_its_subtype() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let mut written = Note::new(&branch.id, "Design sketch", "# Design sketch")
        .with_subtype(Note::SUBTYPE_WRITTEN);
    store.create_note_with_unique_title(&mut written).unwrap();
    let agent = Note::new(&branch.id, "Agent note", "body").with_session("session-1");
    store.create_note(&agent).unwrap();

    assert!(store.get_note(&written.id).unwrap().unwrap().is_written());
    assert!(!store.get_note(&agent.id).unwrap().unwrap().is_written());
}

#[test]
fn test_update_written_note_rewrites_title_and_content() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let mut note = Note::new(&branch.id, "Draft", "# Draft").with_subtype(Note::SUBTYPE_WRITTEN);
    note.set_initial_read(true);
    store.create_note_with_unique_title(&mut note).unwrap();
    let created_completed_at = note.completed_at;

    let updated = store
        .update_written_note(&note.id, "Final", "# Final\n\nbody")
        .unwrap();
    assert_eq!(updated.title, "Final");
    assert_eq!(updated.content, "# Final\n\nbody");
    // Completion is write-once: the note has been readable since it was saved.
    assert_eq!(updated.completed_at, created_completed_at);
    // The user's own edit is nothing new to them, so the note stays read.
    assert_eq!(updated.read_at, note.read_at);

    let stored = store.get_note(&note.id).unwrap().unwrap();
    assert_eq!(stored.title, "Final");
    assert_eq!(stored.content, "# Final\n\nbody");
    assert!(stored.is_written());
    assert!(stored.is_read());
}

/// A written note edited down to a bare title has no body left, which used to
/// leave `completed_at` NULL — the fingerprint of a session note still
/// generating, and enough for branch history and the `#note:` picker to drop it.
#[test]
fn test_update_written_note_completes_a_note_with_an_empty_body() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // Saved without a body, the way a one-line note arrives from the editor.
    let mut note =
        Note::new_standalone(&branch.id, "Reminder", "").with_subtype(Note::SUBTYPE_WRITTEN);
    assert!(note.completed_at.is_some(), "no session will fill it in");
    note.completed_at = None; // a row predating the 0031 backfill
    store.create_note_with_unique_title(&mut note).unwrap();

    let updated = store
        .update_written_note(&note.id, "Reminder, trimmed", "")
        .unwrap();
    assert_eq!(updated.content, "");
    assert!(updated.completed_at.is_some());

    let stored = store.get_note(&note.id).unwrap().unwrap();
    assert_eq!(stored.completed_at, updated.completed_at);
}

#[test]
fn test_update_written_note_keeps_its_own_title_but_avoids_others() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let mut note = Note::new(&branch.id, "Notes", "body").with_subtype(Note::SUBTYPE_WRITTEN);
    store.create_note_with_unique_title(&mut note).unwrap();
    let other = Note::new(&branch.id, "Other", "body");
    store.create_note(&other).unwrap();

    // Re-saving under the same title must not accumulate " (2)" suffixes.
    let resaved = store
        .update_written_note(&note.id, "Notes", "body v2")
        .unwrap();
    assert_eq!(resaved.title, "Notes");

    // Renaming onto another note's title still disambiguates.
    let renamed = store
        .update_written_note(&note.id, "Other", "body v3")
        .unwrap();
    assert_eq!(renamed.title, "Other (2)");
}

#[test]
fn test_update_written_note_rejects_session_produced_notes() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // An agent owns this note's content — its session would overwrite any edit
    // on the next turn, so the store refuses rather than silently losing it.
    let agent = Note::new(&branch.id, "Agent note", "body").with_session("session-1");
    store.create_note(&agent).unwrap();

    assert!(store
        .update_written_note(&agent.id, "Hijacked", "mine now")
        .is_err());
    let unchanged = store.get_note(&agent.id).unwrap().unwrap();
    assert_eq!(unchanged.content, "body");

    assert!(store
        .update_written_note("missing", "Title", "body")
        .is_err());
}

#[test]
fn test_list_child_notes_returns_children_and_excludes_them_from_branch_timeline() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let parent = ProjectNote::new(&project.id, "Parent", "aggregated");
    store.create_project_note(&parent).unwrap();

    // A standalone branch note plus two children aggregated under the parent.
    let standalone = Note::new(&branch.id, "Standalone", "top-level");
    store.create_note(&standalone).unwrap();
    let child_a = Note::new(&branch.id, "Child A", "a").with_parent_project_note(&parent.id);
    store.create_note(&child_a).unwrap();
    let child_b = Note::new(&branch.id, "Child B", "b").with_parent_project_note(&parent.id);
    store.create_note(&child_b).unwrap();

    // Children are returned by the dedicated parent-note query.
    let children = store.list_child_notes(&parent.id).unwrap();
    let child_ids: Vec<_> = children.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(children.len(), 2);
    assert!(child_ids.contains(&child_a.id.as_str()));
    assert!(child_ids.contains(&child_b.id.as_str()));
    assert_eq!(
        children[0].parent_project_note_id.as_deref(),
        Some(parent.id.as_str())
    );

    // ...but excluded from the branch timeline, which only shows the standalone note.
    let timeline = store.list_notes_for_branch(&branch.id).unwrap();
    let timeline_ids: Vec<_> = timeline.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(timeline_ids, vec![standalone.id.as_str()]);
}

#[test]
fn test_list_all_notes_for_branch_includes_children_in_order() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let parent = ProjectNote::new(&project.id, "Parent", "aggregated");
    store.create_project_note(&parent).unwrap();

    let standalone = Note::new(&branch.id, "Standalone", "top-level");
    store.create_note(&standalone).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let child = Note::new(&branch.id, "Child", "c").with_parent_project_note(&parent.id);
    store.create_note(&child).unwrap();

    // Branch history sees both, newest first — children interleave like any note.
    let all = store.list_all_notes_for_branch(&branch.id).unwrap();
    let all_ids: Vec<_> = all.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(all_ids, vec![child.id.as_str(), standalone.id.as_str()]);

    // The UI timeline still hides the child.
    let timeline = store.list_notes_for_branch(&branch.id).unwrap();
    let timeline_ids: Vec<_> = timeline.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(timeline_ids, vec![standalone.id.as_str()]);
}

#[test]
fn test_delete_project_note_cascades_to_child_notes() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let parent = ProjectNote::new(&project.id, "Parent", "aggregated");
    store.create_project_note(&parent).unwrap();

    let child = Note::new(&branch.id, "Child", "c").with_parent_project_note(&parent.id);
    store.create_note(&child).unwrap();
    let unrelated = Note::new(&branch.id, "Unrelated", "u");
    store.create_note(&unrelated).unwrap();

    store.delete_project_note(&parent.id).unwrap();

    // The child note is gone, but the unrelated branch note survives.
    assert!(store.get_note(&child.id).unwrap().is_none());
    assert!(store.list_child_notes(&parent.id).unwrap().is_empty());
    assert!(store.get_note(&unrelated.id).unwrap().is_some());
}

#[test]
fn test_delete_project_note_cleans_up_child_note_sessions() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let parent = ProjectNote::new(&project.id, "Parent", "aggregated");
    store.create_project_note(&parent).unwrap();

    let session = Session::new_running("write child note", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();

    let child = Note::new(&branch.id, "Child", "c")
        .with_session(&session.id)
        .with_parent_project_note(&parent.id);
    store.create_note(&child).unwrap();

    let orphaned = store.delete_project_note(&parent.id).unwrap();

    // Deleting the child fired the note-delete trigger, cleaning up its session.
    assert!(store.get_note(&child.id).unwrap().is_none());
    assert!(store.get_session(&session.id).unwrap().is_none());
    // The child session is still reported so the command layer can cancel it.
    assert_eq!(orphaned.child_session_ids, vec![session.id.clone()]);
    assert_eq!(orphaned.project_note_session_id, None);
}

/// A child session that is still running survives the note-delete trigger (it
/// skips `running` sessions), so `delete_project_note` must report it for the
/// command layer to cancel and delete.
#[test]
fn test_delete_project_note_reports_running_child_sessions() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let parent_session = Session::new_running("write project note", Path::new("/tmp"));
    store.create_session(&parent_session).unwrap();
    let parent =
        ProjectNote::new(&project.id, "Parent", "aggregated").with_session(&parent_session.id);
    store.create_project_note(&parent).unwrap();

    let child_session = Session::new_running("write child note", Path::new("/tmp"));
    store.create_session(&child_session).unwrap();
    let child = Note::new(&branch.id, "Child", "c")
        .with_session(&child_session.id)
        .with_parent_project_note(&parent.id);
    store.create_note(&child).unwrap();

    let orphaned = store.delete_project_note(&parent.id).unwrap();

    assert_eq!(orphaned.child_session_ids, vec![child_session.id.clone()]);
    assert_eq!(
        orphaned.project_note_session_id,
        Some(parent_session.id.clone())
    );
    // The trigger left the running session behind; the caller cleans it up.
    assert!(store.get_session(&child_session.id).unwrap().is_some());
    assert_eq!(
        orphaned.all_session_ids(),
        vec![child_session.id, parent_session.id]
    );
}

// =============================================================================
// Repo Actions
// =============================================================================

#[test]
fn test_repo_actions() {
    let store = Store::in_memory().unwrap();
    let context = store
        .get_or_create_action_context("test-owner/test-repo", None)
        .unwrap();

    let action = RepoAction::new(
        context.id.clone(),
        "Build".to_string(),
        "cargo build".to_string(),
        ActionType::Build,
        0,
    );
    store.create_repo_action(&action).unwrap();

    let actions = store.list_repo_actions(&context.id).unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].name, "Build");
    // Header presence is opt-in, and no icon means "the default for the type".
    assert!(!actions[0].pinned);
    assert_eq!(actions[0].icon, None);

    store.delete_repo_action(&action.id).unwrap();
    assert!(store.list_repo_actions(&context.id).unwrap().is_empty());
}

#[test]
fn test_repo_action_round_trips_pinned_and_icon() {
    let store = Store::in_memory().unwrap();
    let context = store
        .get_or_create_action_context("test-owner/test-repo", None)
        .unwrap();

    let action = RepoAction::new(
        context.id.clone(),
        "Dev".to_string(),
        "just dev".to_string(),
        ActionType::Run,
        0,
    )
    .with_pinned(true)
    .with_icon(Some("rocket".to_string()));
    store.create_repo_action(&action).unwrap();

    let stored = store.get_repo_action(&action.id).unwrap().unwrap();
    assert!(stored.pinned);
    assert_eq!(stored.icon.as_deref(), Some("rocket"));

    // Unpinning and clearing the icon both have to survive the update path —
    // a NULL icon is what sends the button back to its action-type default.
    let updated = RepoAction {
        pinned: false,
        icon: None,
        ..stored
    };
    store.update_repo_action(&updated).unwrap();

    let reread = store.get_repo_action(&action.id).unwrap().unwrap();
    assert!(!reread.pinned);
    assert_eq!(reread.icon, None);

    // The bulk read builds actions from its own SELECT list, so it needs the
    // columns too.
    let grouped = store.list_all_repo_actions().unwrap();
    let bulk = &grouped[0].actions[0];
    assert!(!bulk.pinned);
    assert_eq!(bulk.icon, None);
}

#[test]
fn test_list_all_repo_actions_groups_by_context() {
    let store = Store::in_memory().unwrap();
    let root = store
        .get_or_create_action_context("test-owner/monorepo", None)
        .unwrap();
    let sub = store
        .get_or_create_action_context("test-owner/monorepo", Some("apps/staged"))
        .unwrap();
    // A context with no actions is absent from the result — callers read that
    // as an empty action list.
    store
        .get_or_create_action_context("test-owner/actionless", None)
        .unwrap();

    // Inserted out of sort order to prove the query orders within a context.
    for (context_id, name, command, action_type, sort_order) in [
        (&root.id, "Test", "cargo test", ActionType::Test, 1),
        (&root.id, "Build", "cargo build", ActionType::Build, 0),
        (&sub.id, "Dev", "just dev", ActionType::Run, 0),
    ] {
        store
            .create_repo_action(&RepoAction::new(
                context_id.clone(),
                name.to_string(),
                command.to_string(),
                action_type,
                sort_order,
            ))
            .unwrap();
    }

    let grouped = store.list_all_repo_actions().unwrap();
    assert_eq!(grouped.len(), 2);
    assert!(!grouped
        .iter()
        .any(|g| g.github_repo == "test-owner/actionless"));

    let root_group = grouped.iter().find(|g| g.subpath.is_none()).unwrap();
    assert_eq!(root_group.github_repo, "test-owner/monorepo");
    assert_eq!(
        root_group
            .actions
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Build", "Test"]
    );

    let sub_group = grouped
        .iter()
        .find(|g| g.subpath.as_deref() == Some("apps/staged"))
        .unwrap();
    assert_eq!(sub_group.github_repo, "test-owner/monorepo");
    assert_eq!(sub_group.actions.len(), 1);
    assert_eq!(sub_group.actions[0].name, "Dev");
    assert_eq!(sub_group.actions[0].context_id, sub.id);
}

#[test]
fn test_reorder_actions() {
    let store = Store::in_memory().unwrap();
    let context = store
        .get_or_create_action_context("test-owner/test-repo", None)
        .unwrap();

    let a1 = RepoAction::new(
        context.id.clone(),
        "A".to_string(),
        "a".to_string(),
        ActionType::Build,
        0,
    );
    let a2 = RepoAction::new(
        context.id.clone(),
        "B".to_string(),
        "b".to_string(),
        ActionType::Test,
        1,
    );
    store.create_repo_action(&a1).unwrap();
    store.create_repo_action(&a2).unwrap();

    // Reverse order
    store
        .reorder_repo_actions(&[a2.id.clone(), a1.id.clone()])
        .unwrap();

    let actions = store.list_repo_actions(&context.id).unwrap();
    assert_eq!(actions[0].name, "B");
    assert_eq!(actions[1].name, "A");
}

#[test]
fn test_claim_action_context_detection_records_the_owner_pid() {
    let store = Store::in_memory().unwrap();
    let context = store
        .get_or_create_action_context("test-owner/test-repo", None)
        .unwrap();

    assert!(store
        .claim_action_context_detection(&context.id, 4242)
        .unwrap());
    assert_eq!(
        store.list_detecting_action_contexts().unwrap(),
        vec![(context.id.clone(), Some(4242))]
    );

    // Closing the window drops the owner with the flag: a pid outliving its
    // window would read as a live claim the day that pid is reused.
    store.mark_action_context_detected(&context.id).unwrap();
    assert!(store.list_detecting_action_contexts().unwrap().is_empty());
    store
        .claim_action_context_detection(&context.id, 4242)
        .unwrap();
    store.clear_action_context_detection(&context.id).unwrap();
    assert!(store.list_detecting_action_contexts().unwrap().is_empty());
}

#[test]
fn test_release_detection_claim_only_matches_the_expected_owner() {
    let store = Store::in_memory().unwrap();
    let context = store
        .get_or_create_action_context("test-owner/test-repo", None)
        .unwrap();
    assert!(store
        .claim_action_context_detection(&context.id, 4242)
        .unwrap());

    // The claim changed hands between a sweep's read and its write: the
    // release matches nothing rather than clobbering the live window.
    assert!(!store
        .release_detection_claim(&context.id, Some(9999))
        .unwrap());
    assert!(!store.release_detection_claim(&context.id, None).unwrap());
    assert!(!store
        .claim_action_context_detection(&context.id, 1)
        .unwrap());

    assert!(store
        .release_detection_claim(&context.id, Some(4242))
        .unwrap());
    assert!(store.list_detecting_action_contexts().unwrap().is_empty());
    assert!(store
        .claim_action_context_detection(&context.id, 1)
        .unwrap());
}

#[test]
fn test_release_detection_claim_matches_a_null_owner() {
    let store = Store::in_memory().unwrap();
    let context = store
        .get_or_create_action_context("test-owner/test-repo", None)
        .unwrap();
    // A row claimed before `detecting_pid` existed.
    store
        .claim_action_context_detection_without_owner(&context.id)
        .unwrap();
    assert_eq!(
        store.list_detecting_action_contexts().unwrap(),
        vec![(context.id.clone(), None)]
    );

    assert!(store.release_detection_claim(&context.id, None).unwrap());
    assert!(store.list_detecting_action_contexts().unwrap().is_empty());
}

#[test]
fn test_take_over_detection_claim_swaps_the_owner_without_dropping_the_flag() {
    let store = Store::in_memory().unwrap();
    let context = store
        .get_or_create_action_context("test-owner/test-repo", None)
        .unwrap();
    assert!(store
        .claim_action_context_detection(&context.id, 4242)
        .unwrap());

    assert!(store
        .take_over_detection_claim(&context.id, Some(4242), 7)
        .unwrap());
    // The whole point of doing this in one statement: the window is never
    // observably closed, so a concurrent reader can't take the context's
    // undetected action list for final, and nothing can claim in a gap and
    // reject the caller that just won the window.
    assert_eq!(
        store.list_detecting_action_contexts().unwrap(),
        vec![(context.id.clone(), Some(7))]
    );
    assert!(!store
        .claim_action_context_detection(&context.id, 1)
        .unwrap());

    // And the new owner closes it like any other.
    store.mark_action_context_detected(&context.id).unwrap();
    assert!(store.list_detecting_action_contexts().unwrap().is_empty());
}

#[test]
fn test_take_over_detection_claim_only_matches_the_expected_owner() {
    let store = Store::in_memory().unwrap();
    let context = store
        .get_or_create_action_context("test-owner/test-repo", None)
        .unwrap();
    // A row claimed before `detecting_pid` existed: the null owner is a real
    // expectation, not a wildcard.
    store
        .claim_action_context_detection_without_owner(&context.id)
        .unwrap();
    assert!(!store
        .take_over_detection_claim(&context.id, Some(4242), 7)
        .unwrap());
    assert!(store
        .take_over_detection_claim(&context.id, None, 7)
        .unwrap());

    // The claim changed hands between a waiter's read and its write: the
    // takeover matches nothing rather than stealing it from its new owner.
    assert!(!store
        .take_over_detection_claim(&context.id, Some(4242), 9)
        .unwrap());
    assert_eq!(
        store.list_detecting_action_contexts().unwrap(),
        vec![(context.id.clone(), Some(7))]
    );

    // A closed window has no claim to take over, however it is addressed.
    store.mark_action_context_detected(&context.id).unwrap();
    assert!(!store
        .take_over_detection_claim(&context.id, Some(7), 9)
        .unwrap());
    assert!(!store
        .take_over_detection_claim(&context.id, None, 9)
        .unwrap());
    assert!(store.list_detecting_action_contexts().unwrap().is_empty());
}

// =============================================================================
// Reviews
// =============================================================================

#[test]
fn test_review_crud() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let review = Review::new(&branch.id, "abc123", ReviewScope::Commit);
    store.create_review(&review).unwrap();

    let fetched = store.get_review(&review.id).unwrap().unwrap();
    assert_eq!(fetched.branch_id, branch.id);
    assert_eq!(fetched.commit_sha, "abc123");
    assert_eq!(fetched.scope, ReviewScope::Commit);
    assert!(fetched.reviewed.is_empty());
    assert!(fetched.comments.is_empty());
    assert!(fetched.reference_files.is_empty());
}

#[test]
fn test_review_duplicates_allowed_and_latest_is_returned() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let r1 = Review::new(&branch.id, "abc123", ReviewScope::Commit);
    let mut r2 = Review::new(&branch.id, "abc123", ReviewScope::Commit);
    r2.created_at = r1.created_at + 1;
    r2.updated_at = r2.created_at;
    store.create_review(&r1).unwrap();
    store.create_review(&r2).unwrap();

    let latest = store
        .find_review(&branch.id, "abc123", ReviewScope::Commit)
        .unwrap()
        .unwrap();
    assert_eq!(latest.id, r2.id);

    // Different scope on same commit is also fine.
    let r3 = Review::new(&branch.id, "abc123", ReviewScope::Branch);
    store.create_review(&r3).unwrap();
}

#[test]
fn test_review_with_comments_and_files() {
    use crate::git::Span;

    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let review = Review::new(&branch.id, "abc123", ReviewScope::Branch);
    store.create_review(&review).unwrap();

    // Mark files reviewed
    store.mark_reviewed(&review.id, "src/main.rs").unwrap();
    store.mark_reviewed(&review.id, "src/lib.rs").unwrap();

    // Add comments
    let c1 = Comment::new(
        "src/main.rs",
        Span::new(10, 15),
        "Consider error handling here",
    );
    let c2 = Comment::new("src/lib.rs", Span::new(5, 6), "Typo in doc comment");
    store.add_comment(&review.id, &c1).unwrap();
    store.add_comment(&review.id, &c2).unwrap();

    // Add reference files
    store.add_reference_file(&review.id, "README.md").unwrap();

    // Fetch and verify everything loaded
    let fetched = store.get_review(&review.id).unwrap().unwrap();
    assert_eq!(fetched.reviewed.len(), 2);
    assert_eq!(fetched.comments.len(), 2);
    assert_eq!(fetched.reference_files.len(), 1);
    assert_eq!(fetched.reference_files[0], "README.md");

    // Update a comment
    store.update_comment(&c1.id, "Updated comment").unwrap();
    let updated = store.get_review(&review.id).unwrap().unwrap();
    let updated_comment = updated.comments.iter().find(|c| c.id == c1.id).unwrap();
    assert_eq!(updated_comment.content, "Updated comment");

    // Unmark a file
    store.unmark_reviewed(&review.id, "src/lib.rs").unwrap();
    let after_unmark = store.get_review(&review.id).unwrap().unwrap();
    assert_eq!(after_unmark.reviewed.len(), 1);

    // Delete a comment
    store.delete_comment(&c2.id).unwrap();
    let after_delete = store.get_review(&review.id).unwrap().unwrap();
    assert_eq!(after_delete.comments.len(), 1);

    // Remove reference file
    store
        .remove_reference_file(&review.id, "README.md")
        .unwrap();
    let after_remove = store.get_review(&review.id).unwrap().unwrap();
    assert!(after_remove.reference_files.is_empty());
}

#[test]
fn test_set_comment_session_round_trips() {
    use crate::git::Span;

    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let review = Review::new(&branch.id, "abc123", ReviewScope::Branch);
    store.create_review(&review).unwrap();

    let comment = Comment::new("src/main.rs", Span::new(10, 15), "Needs a follow-up");
    store.add_comment(&review.id, &comment).unwrap();

    // Freshly added comments load with no linked sessions.
    let loaded = store.get_review(&review.id).unwrap().unwrap();
    let loaded_comment = loaded.comments.iter().find(|c| c.id == comment.id).unwrap();
    assert_eq!(loaded_comment.note_session_id, None);
    assert_eq!(loaded_comment.commit_session_id, None);

    // The note and commit links are independent and coexist on one comment.
    store
        .set_comment_session(&comment.id, "note", "note-session-1")
        .unwrap();
    store
        .set_comment_session(&comment.id, "commit", "commit-session-1")
        .unwrap();

    let linked = store.get_review(&review.id).unwrap().unwrap();
    let linked_comment = linked.comments.iter().find(|c| c.id == comment.id).unwrap();
    assert_eq!(
        linked_comment.note_session_id.as_deref(),
        Some("note-session-1")
    );
    assert_eq!(
        linked_comment.commit_session_id.as_deref(),
        Some("commit-session-1")
    );

    // Re-linking the same type is last-write-wins.
    store
        .set_comment_session(&comment.id, "note", "note-session-2")
        .unwrap();
    let relinked = store.get_review(&review.id).unwrap().unwrap();
    let relinked_comment = relinked
        .comments
        .iter()
        .find(|c| c.id == comment.id)
        .unwrap();
    assert_eq!(
        relinked_comment.note_session_id.as_deref(),
        Some("note-session-2")
    );
    assert_eq!(
        relinked_comment.commit_session_id.as_deref(),
        Some("commit-session-1")
    );

    // An unknown session type is rejected.
    assert!(store
        .set_comment_session(&comment.id, "bogus", "x")
        .is_err());
}

#[test]
fn test_list_reviews_for_branch() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let r1 = Review::new(&branch.id, "aaa", ReviewScope::Commit);
    let r2 = Review::new(&branch.id, "bbb", ReviewScope::Commit);
    let r3 = Review::new(&branch.id, "bbb", ReviewScope::Branch);
    store.create_review(&r1).unwrap();
    store.create_review(&r2).unwrap();
    store.create_review(&r3).unwrap();

    let reviews = store.list_reviews_for_branch(&branch.id).unwrap();
    assert_eq!(reviews.len(), 3);
}

#[test]
fn test_delete_review_cascades() {
    use crate::git::Span;

    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let review = Review::new(&branch.id, "abc123", ReviewScope::Commit);
    store.create_review(&review).unwrap();
    store.mark_reviewed(&review.id, "file.rs").unwrap();
    store
        .add_comment(
            &review.id,
            &Comment::new("file.rs", Span::new(1, 2), "note"),
        )
        .unwrap();
    store.add_reference_file(&review.id, "other.rs").unwrap();

    store.delete_review(&review.id).unwrap();
    assert!(store.get_review(&review.id).unwrap().is_none());
}

#[test]
fn test_delete_branch_cascades_reviews() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let review = Review::new(&branch.id, "abc123", ReviewScope::Commit);
    store.create_review(&review).unwrap();

    store.delete_branch(&branch.id).unwrap();
    assert!(store.get_review(&review.id).unwrap().is_none());
}

// =============================================================================
// Change feed
// =============================================================================

#[test]
fn change_feed_publishes_domain_changes_for_mutations() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);

    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Project {
            project_id: Some(project.id.clone())
        }
    );

    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Branch {
            branch_id: branch.id.clone(),
            project_id: Some(project.id.clone())
        }
    );

    // Methods that only hold the aggregate id enrich the secondary id by lookup.
    store.update_branch_name(&branch.id, "feature-2").unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Branch {
            branch_id: branch.id.clone(),
            project_id: Some(project.id.clone())
        }
    );

    // Deletes resolve enrichment before the row disappears.
    store.delete_branch(&branch.id).unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Branch {
            branch_id: branch.id.clone(),
            project_id: Some(project.id.clone())
        }
    );
}

#[test]
fn change_feed_written_note_edits_publish_like_other_note_mutations() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);

    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let mut note = Note::new(&branch.id, "Draft", "# Draft").with_subtype(Note::SUBTYPE_WRITTEN);
    store.create_note_with_unique_title(&mut note).unwrap();
    while rx.try_recv().is_ok() {}

    // Without this, a note edited in one window leaves every other view showing
    // the old title and content until some unrelated event refreshes it.
    store
        .update_written_note(&note.id, "Final", "# Final")
        .unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Notes {
            branch_id: Some(branch.id.clone()),
            project_id: None
        }
    );

    // A rejected edit writes nothing, so it announces nothing.
    let agent = Note::new(&branch.id, "Agent note", "body").with_session("session-1");
    store.create_note(&agent).unwrap();
    while rx.try_recv().is_ok() {}
    assert!(store
        .update_written_note(&agent.id, "Hijacked", "mine now")
        .is_err());
    assert!(rx.try_recv().is_err());
}

#[test]
fn change_feed_marking_a_chat_read_publishes_its_note_only_when_it_writes() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);

    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let branch_session = Session::new_running("branch note", Path::new("/tmp"));
    store.create_session(&branch_session).unwrap();
    // Both notes are written well before their replies: a read only counts
    // replies past the note.
    let note = Note {
        updated_at: 0,
        ..Note::new(&branch.id, "Plan", "body").with_session(&branch_session.id)
    };
    store.create_note(&note).unwrap();
    let project_session = Session::new_running("project note", Path::new("/tmp"));
    store.create_session(&project_session).unwrap();
    let project_note = ProjectNote {
        updated_at: 0,
        ..ProjectNote::new(&project.id, "Overview", "body").with_session(&project_session.id)
    };
    store.create_project_note(&project_note).unwrap();
    for session_id in [&branch_session.id, &project_session.id] {
        store
            .add_session_message(session_id, MessageRole::Assistant, "a reply")
            .unwrap();
    }
    while rx.try_recv().is_ok() {}

    // Note rows show the read state, so their lists reload in every window.
    store.mark_session_chat_read(&branch_session.id).unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Notes {
            branch_id: Some(branch.id.clone()),
            project_id: None
        }
    );
    store.mark_session_chat_read(&project_session.id).unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Notes {
            branch_id: None,
            project_id: Some(project.id.clone())
        }
    );

    // Reopening a chat with nothing new writes nothing, so it reloads nothing.
    store.mark_session_chat_read(&branch_session.id).unwrap();
    assert!(rx.try_recv().is_err());
}

#[test]
fn change_feed_marking_a_chat_read_skips_replies_its_note_already_covers() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);

    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let branch_session = Session::new_running("branch note", Path::new("/tmp"));
    store.create_session(&branch_session).unwrap();
    let project_session = Session::new_running("project note", Path::new("/tmp"));
    store.create_session(&project_session).unwrap();
    let sessions = [&branch_session.id, &project_session.id];
    let add_reply_at = |session_id: &str, created_at: i64| {
        let id = store
            .add_session_message(session_id, MessageRole::Assistant, "a reply")
            .unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE session_messages SET created_at = ?1 WHERE id = ?2",
                rusqlite::params![created_at, id],
            )
            .unwrap();
    };
    let read_at = |session_id: &str| store.session_chat_activity(session_id).unwrap().1;

    // Each session replies, then writes its note after that reply.
    for session_id in sessions {
        add_reply_at(session_id, 1_000);
    }
    store
        .create_note(&Note {
            updated_at: 2_000,
            ..Note::new(&branch.id, "Plan", "body").with_session(&branch_session.id)
        })
        .unwrap();
    store
        .create_project_note(&ProjectNote {
            updated_at: 2_000,
            ..ProjectNote::new(&project.id, "Overview", "body").with_session(&project_session.id)
        })
        .unwrap();
    while rx.try_recv().is_ok() {}

    // The rows show no chat activity, so opening either chat writes nothing
    // and reloads nothing, even though neither has been read.
    for session_id in sessions {
        store.mark_session_chat_read(session_id).unwrap();
        assert_eq!(read_at(session_id), None);
    }
    assert!(rx.try_recv().is_err());

    // A reply past the note is what the next open marks read.
    for session_id in sessions {
        add_reply_at(session_id, 3_000);
        store.mark_session_chat_read(session_id).unwrap();
        assert!(read_at(session_id).is_some_and(|at| at >= 3_000));
    }
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Notes {
            branch_id: Some(branch.id.clone()),
            project_id: None
        }
    );
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Notes {
            branch_id: None,
            project_id: Some(project.id.clone())
        }
    );
}

#[test]
fn change_feed_note_read_state_publishes_only_when_it_changes() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);

    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let note = Note::new(&branch.id, "Plan", "body");
    store.create_note(&note).unwrap();
    let project_note = ProjectNote::new(&project.id, "Overview", "body");
    store.create_project_note(&project_note).unwrap();
    while rx.try_recv().is_ok() {}

    let branch_change = super::StoreChange::Notes {
        branch_id: Some(branch.id.clone()),
        project_id: None,
    };
    let project_change = super::StoreChange::Notes {
        branch_id: None,
        project_id: Some(project.id.clone()),
    };

    // Rows show the read state, so their lists reload in every window.
    store.set_note_read(&note.id, true).unwrap();
    assert_eq!(rx.try_recv().unwrap(), branch_change);
    store.set_project_note_read(&project_note.id, true).unwrap();
    assert_eq!(rx.try_recv().unwrap(), project_change);

    // Reopening a read note writes nothing, so it reloads nothing.
    store.set_note_read(&note.id, true).unwrap();
    store.set_project_note_read(&project_note.id, true).unwrap();
    assert!(rx.try_recv().is_err());

    store.set_note_read(&note.id, false).unwrap();
    assert_eq!(rx.try_recv().unwrap(), branch_change);
    store
        .set_project_note_read(&project_note.id, false)
        .unwrap();
    assert_eq!(rx.try_recv().unwrap(), project_change);

    // Marking an unread note unread again is a no-op too.
    store.set_note_read(&note.id, false).unwrap();
    store
        .set_project_note_read(&project_note.id, false)
        .unwrap();
    assert!(rx.try_recv().is_err());
}

#[test]
fn change_feed_ensure_review_publishes_only_on_create() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);

    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    while rx.try_recv().is_ok() {}

    let review = store
        .ensure_review(&branch.id, "abc123", ReviewScope::Commit)
        .unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Review {
            review_id: review.id.clone(),
            branch_id: Some(branch.id.clone())
        }
    );

    // Second ensure finds the existing review — no write, no event.
    let again = store
        .ensure_review(&branch.id, "abc123", ReviewScope::Commit)
        .unwrap();
    assert_eq!(again.id, review.id);
    assert!(rx.try_recv().is_err());
}

#[test]
fn change_feed_pr_status_publishes_only_on_domain_change() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);

    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main").with_pr(1);
    store.create_branch(&branch).unwrap();
    while rx.try_recv().is_ok() {}

    let expected = super::StoreChange::Branch {
        branch_id: branch.id.clone(),
        project_id: Some(project.id.clone()),
    };
    let poll = |checks: &str| {
        store
            .update_branch_pr_status(
                &branch.id,
                Some(1),
                Some("OPEN".to_string()),
                Some(checks.to_string()),
                Some("APPROVED".to_string()),
                Some(true),
                Some(false),
                Some("https://github.com/test-owner/test-repo/pull/1".to_string()),
                Some(1_700_000_000),
                Some("abc123".to_string()),
                Some("Fix login redirect".to_string()),
            )
            .unwrap();
    };

    // First fetch moves the domain fields off their defaults.
    poll("PENDING");
    assert_eq!(rx.try_recv().unwrap(), expected);

    // The steady state: the poller refetches identical PR state. Only
    // pr_fetched_at / updated_at move, so the feed stays silent.
    poll("PENDING");
    assert!(rx.try_recv().is_err());

    // A genuine flip publishes exactly one event.
    poll("SUCCESS");
    assert_eq!(rx.try_recv().unwrap(), expected);
    assert!(rx.try_recv().is_err());

    // The clear path (PR gone) nulls the fields — also a real change.
    store
        .update_branch_pr_status(
            &branch.id, None, None, None, None, None, None, None, None, None, None,
        )
        .unwrap();
    assert_eq!(rx.try_recv().unwrap(), expected);

    // Clearing an already-cleared branch is a no-op poll echo.
    store
        .update_branch_pr_status(
            &branch.id, None, None, None, None, None, None, None, None, None, None,
        )
        .unwrap();
    assert!(rx.try_recv().is_err());

    // A branch that no longer exists publishes nothing.
    store.delete_branch(&branch.id).unwrap();
    while rx.try_recv().is_ok() {}
    poll("SUCCESS");
    assert!(rx.try_recv().is_err());
}

/// A PR status fetch with everything but the title held fixed.
fn poll_pr_title(store: &Store, branch_id: &str, title: Option<&str>) {
    poll_pr(store, branch_id, 1, title);
}

/// A PR status fetch for PR `pr_number`, with everything but the title held
/// fixed. Returns whether the write landed.
fn poll_pr(store: &Store, branch_id: &str, pr_number: u64, title: Option<&str>) -> bool {
    store
        .update_branch_pr_status(
            branch_id,
            Some(pr_number),
            Some("OPEN".to_string()),
            Some("SUCCESS".to_string()),
            None,
            Some(true),
            Some(false),
            None,
            None,
            Some("abc123".to_string()),
            title.map(str::to_string),
        )
        .unwrap()
}

fn stored_pr_title(store: &Store, branch_id: &str) -> Option<String> {
    store.get_branch(branch_id).unwrap().unwrap().pr_title
}

#[test]
fn change_feed_pr_title_change_publishes_and_a_repeat_poll_stays_silent() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);

    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main").with_pr(1);
    store.create_branch(&branch).unwrap();
    poll_pr_title(&store, &branch.id, Some("Fix login"));
    assert_eq!(
        stored_pr_title(&store, &branch.id).as_deref(),
        Some("Fix login")
    );
    while rx.try_recv().is_ok() {}

    let expected = super::StoreChange::Branch {
        branch_id: branch.id.clone(),
        project_id: Some(project.id.clone()),
    };

    // Only pr_fetched_at / updated_at move: silent.
    poll_pr_title(&store, &branch.id, Some("Fix login"));
    assert!(rx.try_recv().is_err());

    // A title edit on GitHub is a domain change: the pr-status-changed event
    // doesn't carry the title, so only this tells the frontend.
    poll_pr_title(&store, &branch.id, Some("Fix login redirect"));
    assert_eq!(rx.try_recv().unwrap(), expected);
    assert!(rx.try_recv().is_err());
    assert_eq!(
        stored_pr_title(&store, &branch.id).as_deref(),
        Some("Fix login redirect")
    );

    // No title in hand keeps the stored one, and that is no change.
    poll_pr_title(&store, &branch.id, None);
    assert!(rx.try_recv().is_err());
    assert_eq!(
        stored_pr_title(&store, &branch.id).as_deref(),
        Some("Fix login redirect")
    );
}

#[test]
fn clearing_pr_status_after_a_push_keeps_the_pr_title() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main").with_pr(1);
    store.create_branch(&branch).unwrap();
    poll_pr_title(&store, &branch.id, Some("Fix login"));

    store
        .update_branch_pr_status(
            &branch.id, None, None, None, None, None, None, None, None, None, None,
        )
        .unwrap();

    let cleared = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(cleared.pr_state, None);
    assert_eq!(cleared.pr_head_sha, None);
    assert_eq!(cleared.pr_title.as_deref(), Some("Fix login"));
}

#[test]
fn update_branch_pr_number_sets_keeps_or_drops_the_pr_title() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // Linking with the title from the PR list sets it straight away.
    store
        .update_branch_pr_number(&branch.id, Some(7), Some("Fix login"))
        .unwrap();
    assert_eq!(
        stored_pr_title(&store, &branch.id).as_deref(),
        Some("Fix login")
    );

    // Re-recording the same PR without a title keeps it.
    store
        .update_branch_pr_number(&branch.id, Some(7), None)
        .unwrap();
    assert_eq!(
        stored_pr_title(&store, &branch.id).as_deref(),
        Some("Fix login")
    );

    // A different PR without a title must not show PR 7's.
    store
        .update_branch_pr_number(&branch.id, Some(8), None)
        .unwrap();
    assert_eq!(stored_pr_title(&store, &branch.id), None);

    // A title in hand wins even for the same PR.
    store
        .update_branch_pr_number(&branch.id, Some(8), Some("Add metrics"))
        .unwrap();
    assert_eq!(
        stored_pr_title(&store, &branch.id).as_deref(),
        Some("Add metrics")
    );

    // Unlinking drops it.
    store
        .update_branch_pr_number(&branch.id, None, None)
        .unwrap();
    let unlinked = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(unlinked.pr_number, None);
    assert_eq!(unlinked.pr_title, None);
}

#[test]
fn pr_status_poll_for_a_relinked_or_unlinked_pr_writes_nothing() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main").with_pr(7);
    store.create_branch(&branch).unwrap();
    assert!(poll_pr(&store, &branch.id, 7, Some("Fix login")));

    // A poll reads PR 7, and while `gh` answers, the branch is relinked to PR
    // 8 with its own title. The late answer about PR 7 must not land.
    store
        .update_branch_pr_number(&branch.id, Some(8), Some("Add metrics"))
        .unwrap();
    while rx.try_recv().is_ok() {}
    let before = store.get_branch(&branch.id).unwrap().unwrap();
    let written = store
        .update_branch_pr_status(
            &branch.id,
            Some(7),
            Some("MERGED".to_string()),
            Some("FAILURE".to_string()),
            Some("APPROVED".to_string()),
            Some(true),
            Some(false),
            None,
            None,
            Some("def456".to_string()),
            Some("Fix login".to_string()),
        )
        .unwrap();
    assert!(!written);
    let after = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(after.pr_number, Some(8));
    assert_eq!(after.pr_title.as_deref(), Some("Add metrics"));
    assert_eq!(after.pr_state, before.pr_state);
    assert_eq!(after.pr_checks_status, before.pr_checks_status);
    assert_eq!(after.pr_head_sha, before.pr_head_sha);
    assert_eq!(after.updated_at, before.updated_at);
    assert!(rx.try_recv().is_err());

    // A poll for the PR the branch now links lands as usual.
    assert!(poll_pr(&store, &branch.id, 8, Some("Add metrics v2")));
    assert_eq!(
        stored_pr_title(&store, &branch.id).as_deref(),
        Some("Add metrics v2")
    );

    // Unlinked during the fetch: still nothing written.
    store
        .update_branch_pr_number(&branch.id, None, None)
        .unwrap();
    let before = store.get_branch(&branch.id).unwrap().unwrap();
    assert!(!poll_pr(&store, &branch.id, 8, Some("Add metrics v3")));
    let unlinked = store.get_branch(&branch.id).unwrap().unwrap();
    assert_eq!(unlinked.pr_title, None);
    assert_eq!(unlinked.updated_at, before.updated_at);

    // The clear after a push names no PR, so it writes regardless.
    assert!(store
        .update_branch_pr_status(
            &branch.id, None, None, None, None, None, None, None, None, None, None,
        )
        .unwrap());
}

#[test]
fn pr_title_round_trips_and_serializes_as_camel_case() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let mut branch = Branch::new(&project.id, "feature", "main").with_pr(3);
    branch.pr_title = Some("Fix login".to_string());
    store.create_branch(&branch).unwrap();

    let listed = store.list_branches_for_project(&project.id).unwrap();
    assert_eq!(listed[0].pr_title.as_deref(), Some("Fix login"));

    let wire = serde_json::to_value(&listed[0]).unwrap();
    assert_eq!(wire["prTitle"], "Fix login");
}

#[test]
fn change_feed_workspace_status_publishes_only_on_domain_change() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);

    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    // A local branch starts with a NULL workspace_status, so the first real
    // status exercises the null-safe half of the `IS NOT` guard.
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    while rx.try_recv().is_ok() {}

    let expected = super::StoreChange::Branch {
        branch_id: branch.id.clone(),
        project_id: Some(project.id.clone()),
    };
    // Returns the stored status, so every assertion below pins the write as
    // well as the publish.
    let poll = |status: WorkspaceStatus| {
        store
            .update_branch_workspace_status(&branch.id, &status)
            .unwrap();
        store
            .get_branch(&branch.id)
            .unwrap()
            .unwrap()
            .workspace_status
    };

    // NULL -> Starting is a real change.
    assert_eq!(
        poll(WorkspaceStatus::Starting),
        Some(WorkspaceStatus::Starting)
    );
    assert_eq!(rx.try_recv().unwrap(), expected);

    // The steady state: the Blox poller rewrites the status it already sees.
    // The status is still stored, so the silence is "nothing moved", not "a
    // real change went missing".
    assert_eq!(
        poll(WorkspaceStatus::Starting),
        Some(WorkspaceStatus::Starting)
    );
    assert!(rx.try_recv().is_err());

    // A genuine transition publishes exactly one event.
    assert_eq!(
        poll(WorkspaceStatus::Running),
        Some(WorkspaceStatus::Running)
    );
    assert_eq!(rx.try_recv().unwrap(), expected);
    assert!(rx.try_recv().is_err());

    // ...and its own poll echo is silent again.
    assert_eq!(
        poll(WorkspaceStatus::Running),
        Some(WorkspaceStatus::Running)
    );
    assert!(rx.try_recv().is_err());

    // A branch that no longer exists matches no row, so it publishes nothing.
    store.delete_branch(&branch.id).unwrap();
    while rx.try_recv().is_ok() {}
    store
        .update_branch_workspace_status(&branch.id, &WorkspaceStatus::Stopped)
        .unwrap();
    assert!(rx.try_recv().is_err());
}

#[test]
fn change_feed_workspace_status_by_name_publishes_only_for_moved_branches() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);

    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let already = Branch::new_remote(&project.id, "peer-a", "main", "shared-ws");
    let moving = Branch::new_remote(&project.id, "peer-b", "main", "shared-ws");
    store.create_branch(&already).unwrap();
    store.create_branch(&moving).unwrap();
    // Both are created Starting, so park one elsewhere to give it somewhere to
    // move back from.
    store
        .update_branch_workspace_status(&moving.id, &WorkspaceStatus::Suspended)
        .unwrap();
    while rx.try_recv().is_ok() {}

    let ids = store
        .update_workspace_status_by_workspace_name("shared-ws", &WorkspaceStatus::Starting)
        .unwrap();

    // The return value still covers every branch on the workspace, moved or
    // not — `resume_workspace` reads an empty vec as "no such workspace" and
    // turns it into a user-visible error.
    let mut returned = ids.clone();
    returned.sort();
    let mut all_ids = vec![already.id.clone(), moving.id.clone()];
    all_ids.sort();
    assert_eq!(returned, all_ids);

    // Only the branch whose status actually moved publishes.
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Branch {
            branch_id: moving.id.clone(),
            project_id: Some(project.id.clone()),
        }
    );
    assert!(rx.try_recv().is_err());

    // Both rows hold the new status, so the silence was a no-op and not a
    // skipped write.
    for id in &ids {
        assert_eq!(
            store.get_branch(id).unwrap().unwrap().workspace_status,
            Some(WorkspaceStatus::Starting)
        );
    }
}

#[test]
fn change_feed_deletes_publish_only_when_a_row_is_removed() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let store = Store::in_memory().unwrap().with_change_sender(tx);

    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let repo = ProjectRepo::new(&project.id, "test-owner/test-repo", "main", None).primary();
    store.create_project_repo(&repo).unwrap();
    while rx.try_recv().is_ok() {}

    // The delete that lands carries the enrichment resolved before the row went.
    store.delete_branch(&branch.id).unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Branch {
            branch_id: branch.id.clone(),
            project_id: Some(project.id.clone()),
        }
    );

    // Two windows racing to delete the same branch: the loser matches no row.
    // Publishing here would carry `project_id: None`, the frontend's widest
    // tier — every cached branch list dropped and refetched in every window.
    store.delete_branch(&branch.id).unwrap();
    assert!(rx.try_recv().is_err());

    store.delete_project_repo(&repo.id).unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        super::StoreChange::Project {
            project_id: Some(project.id.clone()),
        }
    );

    store.delete_project_repo(&repo.id).unwrap();
    assert!(rx.try_recv().is_err());
}

/// The rows a run writes, in order: its prompt, then its connection's
/// `initialize` and setup, then the agent's answer. A reattachment anchors on
/// that prompt, asks whether the agent answered it, and takes when that
/// connection initialized for when the turn's agent process was up.
#[test]
fn a_runs_prompt_is_the_user_row_its_connection_followed() {
    let store = Store::in_memory().unwrap();
    let session = Session::new_running("go", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    let event = |kind: &str, origin: Option<&str>| {
        store
            .add_acp_metadata_message(
                &session.id,
                &AcpMessageMetadata {
                    acp_event_kind: Some(kind.to_string()),
                    acp_origin: origin.map(str::to_string),
                    ..Default::default()
                },
            )
            .unwrap()
    };
    let message =
        |role: MessageRole, text: &str| store.add_session_message(&session.id, role, text).unwrap();
    let prompt_id = || {
        store
            .get_turn_prompt_message(&session.id)
            .unwrap()
            .map(|message| message.id)
    };
    let answered = |id: i64| store.agent_answered_message(&session.id, id).unwrap();
    let initialize_id = |prompt: i64| {
        store
            .get_turn_initialize_message(&session.id, prompt)
            .unwrap()
            .map(|message| message.id)
    };

    assert_eq!(prompt_id(), None);

    // Still connecting: nothing follows the prompt yet.
    let first = message(MessageRole::User, "go");
    assert_eq!(prompt_id(), Some(first));
    assert!(!answered(first));
    assert_eq!(initialize_id(first), None);
    // The connection's setup rows don't answer it.
    let first_initialize = event("initialize", None);
    event("config_options_update", None);
    event("session_mode_state", None);
    assert_eq!(prompt_id(), Some(first));
    assert!(!answered(first));
    assert_eq!(initialize_id(first), Some(first_initialize));
    message(MessageRole::Assistant, "Looking into it");
    assert!(answered(first));

    // A steer the turn's output follows is not the run's prompt.
    message(MessageRole::User, "also check the tests");
    message(MessageRole::ToolCall, "Run the tests");
    assert_eq!(prompt_id(), Some(first));

    // A reattachment's connection initializes after the turn's output; the
    // turn's own connection is still the one that initialized first.
    event("initialize", None);
    message(MessageRole::Assistant, "The tests pass");
    assert_eq!(prompt_id(), Some(first));
    assert_eq!(initialize_id(first), Some(first_initialize));

    // Background output from the earlier turn, as the writer records it: a
    // plain assistant row, with a hidden `agent_message_chunk` row beside it
    // that carries the origin. It comes before the follow-up run's prompt,
    // as it does for a remote session, whose next run starts once the
    // earlier connection has settled.
    message(MessageRole::Assistant, "The background build passed");
    event("agent_message_chunk", Some("background"));

    // A follow-up run.
    let second = message(MessageRole::User, "now push");
    assert_eq!(prompt_id(), Some(second));
    assert!(!answered(second));
    let second_initialize = event("initialize", None);
    assert_eq!(prompt_id(), Some(second));
    assert!(!answered(second));
    assert_eq!(initialize_id(second), Some(second_initialize));
    message(MessageRole::ToolCall, "git push");
    assert!(answered(second));
}

// =============================================================================
// Launch resume: re-queue CAS and app_quit candidates
// =============================================================================

fn app_quit_branch_session(store: &Store, branch_id: &str, created_at: i64) -> Session {
    let mut session = Session::new_running("work", Path::new("/tmp")).with_branch(branch_id);
    session.created_at = created_at;
    session.updated_at = created_at;
    store.create_session(&session).unwrap();
    store
        .transition_from_running(
            &session.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::AppQuit),
        )
        .unwrap();
    session
}

#[test]
fn transition_app_quit_to_queued_requeues_and_keeps_created_at() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("owner/repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let session = app_quit_branch_session(&store, &branch.id, 1_000);
    let before = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(before.status, SessionStatus::Cancelled);
    assert_eq!(before.completion_reason, Some(CompletionReason::AppQuit));

    assert!(store.transition_app_quit_to_queued(&session.id).unwrap());

    let after = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(after.status, SessionStatus::Queued);
    assert_eq!(after.completion_reason, None);
    assert_eq!(after.error_message, None);
    assert_eq!(after.owner_pid, None);
    assert_eq!(
        after.created_at, 1_000,
        "the drain's ordering key is untouched"
    );
    assert!(after.updated_at >= before.updated_at);

    // Already queued: the CAS is a no-op the second time.
    assert!(!store.transition_app_quit_to_queued(&session.id).unwrap());
}

#[test]
fn transition_app_quit_to_queued_takes_crash_recovered_error_rows() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("owner/repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let session = Session::new_running("work", Path::new("/tmp")).with_branch(&branch.id);
    store.create_session(&session).unwrap();
    store
        .transition_from_running(
            &session.id,
            SessionStatus::Error,
            None,
            Some(&CompletionReason::AppQuit),
        )
        .unwrap();

    assert!(store.transition_app_quit_to_queued(&session.id).unwrap());
    assert_eq!(
        store.get_session(&session.id).unwrap().unwrap().status,
        SessionStatus::Queued
    );
}

#[test]
fn transition_app_quit_to_queued_refuses_other_rows() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("owner/repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // Cancelled for another reason.
    let interrupted = Session::new_running("work", Path::new("/tmp")).with_branch(&branch.id);
    store.create_session(&interrupted).unwrap();
    store
        .transition_from_running(
            &interrupted.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::Interrupted),
        )
        .unwrap();
    assert!(!store
        .transition_app_quit_to_queued(&interrupted.id)
        .unwrap());

    // Completed with app_quit would be contradictory, but a completed row is
    // never taken regardless of its reason.
    let completed = Session::new_running("work", Path::new("/tmp")).with_branch(&branch.id);
    store.create_session(&completed).unwrap();
    store
        .transition_from_running(
            &completed.id,
            SessionStatus::Completed,
            None,
            Some(&CompletionReason::AppQuit),
        )
        .unwrap();
    assert!(!store.transition_app_quit_to_queued(&completed.id).unwrap());

    // A project session (no branch) is never queued.
    let project_session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&project_session).unwrap();
    store
        .transition_from_running(
            &project_session.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::AppQuit),
        )
        .unwrap();
    assert!(!store
        .transition_app_quit_to_queued(&project_session.id)
        .unwrap());
    assert_eq!(
        store
            .get_session(&project_session.id)
            .unwrap()
            .unwrap()
            .status,
        SessionStatus::Cancelled
    );

    // Still running: not terminal, not taken.
    let running = Session::new_running("work", Path::new("/tmp")).with_branch(&branch.id);
    store.create_session(&running).unwrap();
    assert!(!store.transition_app_quit_to_queued(&running.id).unwrap());
}

#[test]
fn get_app_quit_sessions_since_filters_by_marker_and_orders_by_created_at() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("owner/repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // Created out of order on purpose; the query sorts by created_at.
    let second = app_quit_branch_session(&store, &branch.id, 2_000);
    let first = app_quit_branch_session(&store, &branch.id, 1_000);
    let third = app_quit_branch_session(&store, &branch.id, 3_000);

    // A row ended for another reason is not a candidate.
    let interrupted = Session::new_running("work", Path::new("/tmp")).with_branch(&branch.id);
    store.create_session(&interrupted).unwrap();
    store
        .transition_from_running(
            &interrupted.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::Interrupted),
        )
        .unwrap();

    // A project session with app_quit is a candidate too (the caller
    // partitions repo from project rows).
    let project_session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&project_session).unwrap();
    store
        .transition_from_running(
            &project_session.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::AppQuit),
        )
        .unwrap();

    let ids: Vec<String> = store
        .get_app_quit_sessions_since(0)
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(
        ids[..3],
        [first.id.clone(), second.id.clone(), third.id.clone()]
    );
    assert!(ids.contains(&project_session.id));
    assert!(!ids.contains(&interrupted.id));

    // The marker excludes everything at or before it. Every transition above
    // restamped updated_at to "now", so a marker of now hides them all.
    let newest = store
        .get_app_quit_sessions_since(0)
        .unwrap()
        .iter()
        .map(|s| s.updated_at)
        .max()
        .unwrap();
    assert!(store
        .get_app_quit_sessions_since(newest)
        .unwrap()
        .is_empty());

    // Re-queued rows drop out: they are no longer terminal.
    assert!(store.transition_app_quit_to_queued(&first.id).unwrap());
    let ids: Vec<String> = store
        .get_app_quit_sessions_since(0)
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert!(!ids.contains(&first.id));
}

/// The launch-resume detection query hands back an interrupted row whole.
/// It once listed its own 16 columns while `row_to_session` read a 17th
/// (`parent_session_id`, added on main), so every matching row failed the
/// query with `InvalidColumnIndex` and launch detection only logged a
/// warning — the dialog never appeared.
#[test]
fn get_app_quit_sessions_since_returns_the_interrupted_session() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("owner/repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    // The parent is still running, so it is not a candidate itself.
    let parent = Session::new_running("project session", Path::new("/tmp"));
    store.create_session(&parent).unwrap();
    let mut child = Session::new_running("repo work", Path::new("/tmp"))
        .with_branch(&branch.id)
        .with_parent_session(&parent.id);
    child.agent_id = Some("acp-session-1".to_string());
    store.create_session(&child).unwrap();
    store
        .transition_from_running(
            &child.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::AppQuit),
        )
        .unwrap();

    let found = store.get_app_quit_sessions_since(0).unwrap();
    assert_eq!(found.len(), 1, "exactly the interrupted child: {found:?}");
    let found = &found[0];
    assert_eq!(found.id, child.id);
    assert_eq!(found.status, SessionStatus::Cancelled);
    assert_eq!(found.completion_reason, Some(CompletionReason::AppQuit));
    assert_eq!(found.branch_id.as_deref(), Some(branch.id.as_str()));
    assert_eq!(found.agent_id.as_deref(), Some("acp-session-1"));
    assert_eq!(found.parent_session_id.as_deref(), Some(parent.id.as_str()));
}

/// Every query that maps rows through `row_to_session` reads a row exactly
/// as `get_session` does, `parent_session_id` included. They all select
/// `SESSION_COLUMNS`; this catches one that goes back to listing its own.
#[test]
fn session_list_queries_read_rows_like_get_session() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("owner/repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let parent = Session::new_running("project session", Path::new("/tmp"));
    store.create_session(&parent).unwrap();
    let child = |session: Session| {
        let session = session
            .with_branch(&branch.id)
            .with_parent_session(&parent.id);
        store.create_session(&session).unwrap();
        session.id
    };
    let queued = child(Session::new_queued("queued work"));
    let running = child(Session::new_running("running work", Path::new("/tmp")));
    let quit = child(Session::new_running("interrupted work", Path::new("/tmp")));
    store
        .transition_from_running(
            &quit,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::AppQuit),
        )
        .unwrap();

    for id in [&queued, &running, &quit] {
        let session = store.get_session(id).unwrap().unwrap();
        assert_eq!(
            session.parent_session_id.as_deref(),
            Some(parent.id.as_str())
        );
    }

    let as_json = |session: &Session| serde_json::to_value(session).unwrap();
    let check = |query: &str, rows: Vec<Session>, expected: &[&String]| {
        let mut ids: Vec<&str> = rows.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        let mut expected: Vec<&str> = expected.iter().map(|id| id.as_str()).collect();
        expected.sort_unstable();
        assert_eq!(ids, expected, "{query} returned the wrong rows");
        for row in &rows {
            let canonical = store.get_session(&row.id).unwrap().unwrap();
            assert_eq!(
                as_json(row),
                as_json(&canonical),
                "{query} read {} differently from get_session",
                row.id
            );
        }
    };

    check(
        "get_app_quit_sessions_since",
        store.get_app_quit_sessions_since(0).unwrap(),
        &[&quit],
    );
    check(
        "get_running_sessions",
        store.get_running_sessions().unwrap(),
        &[&parent.id, &running],
    );
    check(
        "get_active_sessions",
        store.get_active_sessions().unwrap(),
        &[&parent.id, &queued, &running],
    );
    check(
        "get_queued_sessions_for_branch",
        store.get_queued_sessions_for_branch(&branch.id).unwrap(),
        &[&queued],
    );
    check(
        "get_running_sessions_for_branch",
        store.get_running_sessions_for_branch(&branch.id).unwrap(),
        &[&running],
    );
}

/// A `disconnected` Blox session may still be working on its workstation:
/// it has no owner, holds its branch like a running one, and neither a resume
/// nor a queued follow-up may start a second turn beside it.
#[test]
fn test_disconnected_session_is_unowned_busy_and_not_resumable() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let session = Session::new_running("work", Path::new("/tmp")).with_branch(&branch.id);
    store.create_session(&session).unwrap();
    store
        .add_queued_session_message(&session.id, "queued", &[], None)
        .unwrap();

    assert!(store
        .transition_from_running(&session.id, SessionStatus::Disconnected, None, None)
        .unwrap());
    let row = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(row.status, SessionStatus::Disconnected);
    assert_eq!(row.owner_pid, None);
    assert_eq!(row.completion_reason, None);

    assert!(store.has_running_session_for_branch(&branch.id).unwrap());
    let busy = store.get_busy_sessions_for_branch(&branch.id).unwrap();
    assert_eq!(busy.len(), 1);
    assert_eq!(busy[0].id, session.id);
    assert!(store
        .get_running_sessions_for_branch(&branch.id)
        .unwrap()
        .is_empty());
    // The dead-owner and stranded sweeps read only `running` rows.
    assert!(store.get_running_sessions().unwrap().is_empty());

    assert!(!store.transition_to_running(&session.id).unwrap());
    assert!(store
        .claim_oldest_queued_session_message(&session.id)
        .unwrap()
        .is_none());

    // Once stopped, the session is an ordinary finished one again.
    store
        .update_session_status(&session.id, SessionStatus::Cancelled, None, None)
        .unwrap();
    assert!(!store.has_running_session_for_branch(&branch.id).unwrap());
    assert_eq!(
        store
            .claim_oldest_queued_session_message(&session.id)
            .unwrap()
            .unwrap()
            .content,
        "queued"
    );
}

#[test]
fn test_session_pre_head_and_blox_command_round_trip() {
    let store = Store::in_memory().unwrap();
    let mut session = Session::new_running("work", Path::new("/tmp"));
    session.blox_command_id = Some("cmd-1".to_string());
    store.create_session(&session).unwrap();

    let row = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(row.pre_head_sha, None);
    assert_eq!(row.blox_command_id.as_deref(), Some("cmd-1"));

    // Bookkeeping, not a lifecycle change: the stranded sweep's CAS on
    // `updated_at` must not lose to it.
    store
        .set_session_pre_head_sha(&session.id, Some("abc123"))
        .unwrap();
    let after = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(after.pre_head_sha.as_deref(), Some("abc123"));
    assert_eq!(after.updated_at, row.updated_at);

    store.set_session_pre_head_sha(&session.id, None).unwrap();
    let cleared = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(cleared.pre_head_sha, None);
}
