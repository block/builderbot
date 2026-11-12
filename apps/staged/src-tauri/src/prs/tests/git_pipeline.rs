//! Rebuilding a queued push or pull on dequeue.

use super::*;

/// A no-verify push that waited behind other work must still skip the
/// hook once the queue drains it, and the running row must still say so.
#[test]
fn queued_no_verify_push_is_rebuilt_with_the_flag_on_dequeue() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    for variant in [NO_VERIFY_PUSH, NO_VERIFY_FORCE_PUSH, PUSH] {
        let id = queue_push(&store, &branch.id, variant).unwrap();
        let queued = store.get_session(&id).unwrap().unwrap();

        let rebuilt = rebuild_queued_git_pipeline(
            &id,
            queued.pipeline.as_ref().unwrap(),
            "feature-renamed",
            "feature",
        )
        .unwrap();

        assert_eq!(rebuilt.session_type, "push");
        assert_eq!(rebuilt.prompt, queued.prompt);
        assert_eq!(PushVariant::of(&rebuilt.pipeline), variant);
        let (_, command, _) = command_at(&rebuilt.steps, 0);
        assert!(
            command.contains("origin feature-renamed:feature"),
            "{command}"
        );
        assert_eq!(
            command.contains("--no-verify"),
            variant.no_verify,
            "{command}"
        );
        assert_eq!(
            command.contains("--force-with-lease"),
            variant.force,
            "{command}"
        );
    }
}

#[test]
fn rebuilt_pipeline_keeps_the_push_dependency_and_rejects_commit_kinds() {
    let queued = PipelineExecution::from_steps(&[])
        .with_kind(PipelineKind::Push)
        .with_push_force(true)
        .with_depends_on_session_id(Some("rebase-1".to_string()));
    let rebuilt = rebuild_queued_git_pipeline("push-1", &queued, "feature", "feature").unwrap();
    assert_eq!(
        rebuilt.pipeline.depends_on_session_id.as_deref(),
        Some("rebase-1")
    );
    assert_eq!(PushVariant::of(&rebuilt.pipeline), FORCE_PUSH);

    let pull = PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Pull);
    let rebuilt = rebuild_queued_git_pipeline("pull-1", &pull, "feature", "feature").unwrap();
    assert_eq!(rebuilt.session_type, "pull");
    assert_eq!(rebuilt.prompt, PULL_PROMPT);

    let rebase = PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase);
    assert!(rebuild_queued_git_pipeline("rebase-1", &rebase, "feature", "feature").is_err());
}
