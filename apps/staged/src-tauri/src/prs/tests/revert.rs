//! Revert: the pipeline step, its handoff prompt and SHA validation.

use super::*;

#[test]
fn revert_pipeline_uses_signoff_and_hands_off_to_ai() {
    let steps = build_revert_pipeline_steps("abc1234").unwrap();

    assert_eq!(steps.len(), 1);
    let (label, command, on_failure) = command_at(&steps, 0);
    assert_eq!(label, "Revert commit");
    assert_eq!(command, "git revert --signoff --no-edit abc1234");

    let FailureStrategy::HandoffToAi { prompt_template } = on_failure else {
        panic!("expected revert failure to hand off to AI");
    };
    assert!(prompt_template.contains("git revert --continue"));
    assert!(prompt_template.contains("preserving unrelated local changes"));
    assert!(prompt_template.contains("Do not push the branch"));
    assert!(prompt_template.contains("{step_outputs}"));
}

#[test]
fn revert_pipeline_rejects_unsafe_commit_sha() {
    assert!(build_revert_pipeline_steps("abc1234").is_ok());
    assert!(build_revert_pipeline_steps("abcdef").is_err());
    assert!(build_revert_pipeline_steps("abc1234; rm -rf /").is_err());
    assert!(build_revert_pipeline_steps("not-a-sha").is_err());
}

#[test]
fn inserting_a_running_revert_is_what_makes_the_branch_busy() {
    let (store, branch) = setup_branch_store();
    let registry = test_registry();
    let ctx = pipeline_context(&branch);
    let steps = build_revert_pipeline_steps("abc1234def").unwrap();

    let running = insert_running_revert_pipeline_session_if_idle(
        &store,
        &registry,
        &ctx,
        &steps,
        "abc1234def",
        Some("claude"),
    )
    .unwrap();

    // The runner, not the command, reads the pre-revert HEAD.
    assert!(running.capture_pre_head);
    assert_eq!(running.prompt, "Revert commit abc1234");
    assert!(registry.is_live(&running.session_id));
    let session = store.get_session(&running.session_id).unwrap().unwrap();
    assert_eq!(session.status, store::SessionStatus::Running);
    assert_eq!(session.branch_id.as_deref(), Some(branch.id.as_str()));
    assert_eq!(session.provider.as_deref(), Some("claude"));
    assert_eq!(session.pipeline.unwrap().kind, None);
    let pending = store
        .get_commit_by_session(&running.session_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.sha, None);

    // A second revert racing the first is refused rather than run, and
    // the queueable actions queue behind it.
    let err = insert_running_revert_pipeline_session_if_idle(
        &store,
        &registry,
        &ctx,
        &steps,
        "abc1234def",
        None,
    )
    .err()
    .unwrap();
    assert_eq!(err, REVERT_BRANCH_BUSY);
    assert!(
        queue_commit_pipeline_locked(&store, &branch.id, &PipelineKind::Rebase, None, None)
            .unwrap()
            .is_some()
    );
    assert!(queue_push(&store, &branch.id, PUSH).is_some());
}

#[test]
fn revert_on_a_busy_branch_inserts_nothing() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);
    let steps = build_revert_pipeline_steps("abc1234").unwrap();

    let err = insert_running_revert_pipeline_session_if_idle(
        &store,
        &test_registry(),
        &pipeline_context(&branch),
        &steps,
        "abc1234",
        None,
    )
    .err()
    .unwrap();

    assert_eq!(err, REVERT_BRANCH_BUSY);
    let running = store.get_running_sessions_for_branch(&branch.id).unwrap();
    assert_eq!(running.len(), 1, "only the note session is running");
    assert!(store
        .list_commits_for_branch(&branch.id)
        .unwrap()
        .is_empty());
}

/// Reset-to-origin and discard leave no session row, so the marker they
/// set is what refuses a revert started while one is moving HEAD.
#[test]
fn revert_during_a_worktree_operation_inserts_nothing() {
    let (store, branch) = setup_branch_store();
    let registry = test_registry();
    let ctx = pipeline_context(&branch);
    let steps = build_revert_pipeline_steps("abc1234").unwrap();
    let revert = || {
        insert_running_revert_pipeline_session_if_idle(
            &store, &registry, &ctx, &steps, "abc1234", None,
        )
    };

    let operation = crate::session_commands::BranchWorktreeOperationGuard::acquire(&branch.id);
    assert_eq!(revert().err().unwrap(), REVERT_WORKTREE_BUSY);
    assert!(store
        .get_running_sessions_for_branch(&branch.id)
        .unwrap()
        .is_empty());
    assert!(store
        .list_commits_for_branch(&branch.id)
        .unwrap()
        .is_empty());

    drop(operation);
    assert!(revert().is_ok());
}

/// A queued row whose artifact was deleted is stepped over by the drain,
/// so it must not count as work in flight: a revert would be refused on
/// the branch for good, and a rebase queued behind it alone would never
/// be drained.
#[test]
fn an_unlinked_queued_row_leaves_the_branch_idle() {
    let (store, branch) = setup_branch_store();
    let orphan = store::Session::new_queued("deleted note").with_branch(&branch.id);
    store.create_session(&orphan).unwrap();

    assert!(!branch_has_work_in_flight(&store, &branch.id).unwrap());
    assert_eq!(
        queue_commit_pipeline_locked(&store, &branch.id, &PipelineKind::Rebase, None, None)
            .unwrap(),
        None
    );
    let steps = build_revert_pipeline_steps("abc1234").unwrap();
    assert!(insert_running_revert_pipeline_session_if_idle(
        &store,
        &test_registry(),
        &pipeline_context(&branch),
        &steps,
        "abc1234",
        None,
    )
    .is_ok());
}
