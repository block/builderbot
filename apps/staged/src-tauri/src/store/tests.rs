//! Tests for the store module.

use std::collections::HashSet;
use std::path::Path;

use super::models::*;
use super::{
    SessionPollRequest, Store, ToolCallBody, TranscriptWindow, DEFERRED_TOOL_BODY_MIN_BYTES,
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
fn test_project_status_override_round_trips() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    assert!(store
        .get_project(&project.id)
        .unwrap()
        .unwrap()
        .status_override
        .is_none());

    store
        .set_project_status_override(&project.id, Some("blocked"))
        .unwrap();
    let fetched = store.get_project(&project.id).unwrap().unwrap();
    assert_eq!(fetched.status_override.as_deref(), Some("blocked"));
    let listed = store.list_projects().unwrap();
    assert_eq!(listed[0].status_override.as_deref(), Some("blocked"));

    // Rewriting the primary repo leaves the status alone.
    store
        .update_project(
            &project.id,
            &fetched.name,
            Some("test-owner/other-repo"),
            &fetched.location,
            None,
        )
        .unwrap();
    let fetched = store.get_project(&project.id).unwrap().unwrap();
    assert_eq!(fetched.status_override.as_deref(), Some("blocked"));

    store
        .set_project_status_override(&project.id, None)
        .unwrap();
    let fetched = store.get_project(&project.id).unwrap().unwrap();
    assert!(fetched.status_override.is_none());
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
fn test_get_active_sessions_returns_running_and_queued_only() {
    let store = Store::in_memory().unwrap();

    let running = Session::new_running("running", Path::new("/tmp"));
    store.create_session(&running).unwrap();
    let queued = Session::new_queued("queued");
    store.create_session(&queued).unwrap();

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
    assert_eq!(active.len(), 2);
    assert!(active.iter().any(|s| s.id == running.id));
    assert!(active.iter().any(|s| s.id == queued.id));
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
fn test_list_running_child_sessions_projects_each_kind_to_its_repo() {
    let store = Store::in_memory().unwrap();
    let (_project, branch, repo, parent) = child_session_fixture(&store);

    for kind in ["commit", "note", "review"] {
        let child = child_session(&store, &parent.id, &branch.id, kind);
        store.transition_queued_to_running(&child.id).unwrap();
    }

    let children = store.list_running_child_sessions(&parent.id).unwrap();
    let kinds: Vec<&str> = children.iter().map(|c| c.session_type.as_str()).collect();
    // Ordered by start order, which is the order they were enqueued: these
    // three share a `created_at`, so the row order is the tie-break's doing and
    // badge positions stay put across refreshes.
    assert_eq!(kinds, vec!["commit", "note", "review"]);
    for child in &children {
        assert_eq!(child.branch_id, branch.id);
        assert_eq!(
            child.github_repo.as_deref(),
            Some(repo.github_repo.as_str())
        );
        assert!(child.subpath.is_none());
    }
}

#[test]
fn test_list_running_child_sessions_maps_a_child_note_to_note() {
    let store = Store::in_memory().unwrap();
    let (project, branch, _repo, parent) = child_session_fixture(&store);
    let project_note = ProjectNote::new(&project.id, "Parent", "");
    store.create_project_note(&project_note).unwrap();

    // A `child_note` outcome: the note is aggregated under the project note and
    // hidden from the repo timeline, but it is still a note as far as the badge
    // is concerned.
    let child = Session::new_queued("investigate").with_parent_session(&parent.id);
    store.create_session(&child).unwrap();
    store
        .create_note(
            &Note::new(&branch.id, "investigate", "")
                .with_session(&child.id)
                .with_parent_project_note(&project_note.id),
        )
        .unwrap();
    store.transition_queued_to_running(&child.id).unwrap();

    let children = store.list_running_child_sessions(&parent.id).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].session_type, "note");
    assert_eq!(children[0].session_id, child.id);
}

#[test]
fn test_list_running_child_sessions_yields_one_row_per_multi_artifact_child() {
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

    let children = store.list_running_child_sessions(&parent.id).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].session_id, child.id);
    assert_eq!(children[0].session_type, "note");
}

#[test]
fn test_list_running_child_sessions_excludes_inactive_children_and_other_parents() {
    let store = Store::in_memory().unwrap();
    let (_project, branch, _repo, parent) = child_session_fixture(&store);

    let running = child_session(&store, &parent.id, &branch.id, "commit");
    store.transition_queued_to_running(&running.id).unwrap();

    // Queued: nothing is happening for it yet, so a spinner would misreport.
    child_session(&store, &parent.id, &branch.id, "note");

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

    let children = store.list_running_child_sessions(&parent.id).unwrap();
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
        CompletionReason::AppQuit,
    ] {
        assert!(reason.is_resumable(), "{reason:?} should be resumable");
    }
    for reason in [CompletionReason::TurnComplete, CompletionReason::Unknown] {
        assert!(!reason.is_resumable(), "{reason:?} should not be resumable");
    }
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
    store.create_note_with_unique_title(&mut note).unwrap();
    let created_completed_at = note.completed_at;

    let updated = store
        .update_written_note(&note.id, "Final", "# Final\n\nbody")
        .unwrap();
    assert_eq!(updated.title, "Final");
    assert_eq!(updated.content, "# Final\n\nbody");
    // Completion is write-once: the note has been readable since it was saved.
    assert_eq!(updated.completed_at, created_completed_at);

    let stored = store.get_note(&note.id).unwrap().unwrap();
    assert_eq!(stored.title, "Final");
    assert_eq!(stored.content, "# Final\n\nbody");
    assert!(stored.is_written());
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
    let branch = Branch::new(&project.id, "feature", "main");
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
                Some("OPEN".to_string()),
                Some(checks.to_string()),
                Some("APPROVED".to_string()),
                Some(true),
                Some(false),
                Some("https://github.com/test-owner/test-repo/pull/1".to_string()),
                Some(1_700_000_000),
                Some("abc123".to_string()),
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
        .update_branch_pr_status(&branch.id, None, None, None, None, None, None, None, None)
        .unwrap();
    assert_eq!(rx.try_recv().unwrap(), expected);

    // Clearing an already-cleared branch is a no-op poll echo.
    store
        .update_branch_pr_status(&branch.id, None, None, None, None, None, None, None, None)
        .unwrap();
    assert!(rx.try_recv().is_err());

    // A branch that no longer exists publishes nothing.
    store.delete_branch(&branch.id).unwrap();
    while rx.try_recv().is_ok() {}
    poll("SUCCESS");
    assert!(rx.try_recv().is_err());
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
