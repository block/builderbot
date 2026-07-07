//! Tests for the store module.

use std::collections::HashSet;
use std::path::Path;

use super::models::*;
use super::Store;

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
fn test_transition_from_active_succeeds_when_queued() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_queued("queued");
    store.create_session(&session).unwrap();

    let transitioned = store
        .transition_from_active(&session.id, SessionStatus::Cancelled, None, None)
        .unwrap();
    assert!(transitioned);

    let final_state = store.get_session(&session.id).unwrap().unwrap();
    assert_eq!(final_state.status, SessionStatus::Cancelled);
}

#[test]
fn test_transition_from_active_does_not_overwrite_completed_session() {
    let store = Store::in_memory().unwrap();

    let session = Session::new_running("completed first", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    store
        .update_session_status(&session.id, SessionStatus::Completed, None, None)
        .unwrap();

    let transitioned = store
        .transition_from_active(&session.id, SessionStatus::Cancelled, None, None)
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
    let session = Session::new_queued("queued").with_acp_config_selection(selection.clone());
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

    let mut session = Session::new_queued("Rebase branch");
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
    let mut timeline = vec![("failed-note", failed_note_time), ("commit", commit_time)];
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

    let mut session = Session::new_running("Squash commits", Path::new("/tmp"));
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
fn test_completion_reason_round_trips() {
    let store = Store::in_memory().unwrap();

    for reason in [
        CompletionReason::TurnComplete,
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
    let since = store.get_session_messages_since(&session.id, id1).unwrap();
    assert_eq!(since.len(), 2);
    assert_eq!(since[0].id, id1);
    assert_eq!(since[1].id, id2);
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
fn test_acp_metadata_messages_query_includes_hidden_rows() {
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

    let metadata_rows = store
        .get_session_acp_metadata_messages(&session.id)
        .unwrap();
    assert_eq!(metadata_rows.len(), 2);
    assert_eq!(metadata_rows[0].role, MessageRole::User);
    assert_eq!(
        metadata_rows[0].acp.acp_message_id.as_deref(),
        Some("user-msg-1")
    );
    assert_eq!(metadata_rows[1].acp.acp_usage.as_ref().unwrap()["used"], 42);
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

    store.delete_repo_action(&action.id).unwrap();
    assert!(store.list_repo_actions(&context.id).unwrap().is_empty());
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
fn test_set_review_auto_restamps_completed_at_when_made_visible() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let review = Review::new(&branch.id, "abc123", ReviewScope::Branch).with_auto();
    store.create_review(&review).unwrap();
    store
        .update_review_title(&review.id, "Auto review")
        .unwrap();

    let auto_review = store.get_review(&review.id).unwrap().unwrap();
    let original_completed_at = auto_review.completed_at.unwrap();

    std::thread::sleep(std::time::Duration::from_millis(2));
    store.set_review_auto(&review.id, false).unwrap();

    let visible_review = store.get_review(&review.id).unwrap().unwrap();
    assert!(!visible_review.is_auto);
    assert!(visible_review.completed_at.unwrap() > original_completed_at);
}

#[test]
fn test_set_review_auto_leaves_incomplete_review_uncompleted() {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();

    let review = Review::new(&branch.id, "abc123", ReviewScope::Branch).with_auto();
    store.create_review(&review).unwrap();

    store.set_review_auto(&review.id, false).unwrap();

    let visible_review = store.get_review(&review.id).unwrap().unwrap();
    assert!(!visible_review.is_auto);
    assert!(visible_review.completed_at.is_none());
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
