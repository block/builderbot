//! Rebase and squash: queueing, dedupe, "Rebase and force push", and the
//! pipeline steps and prompts.

use super::*;

#[test]
fn idle_branch_runs_pipeline_immediately_instead_of_queueing() {
    let (store, branch) = setup_branch_store();

    assert_eq!(
        queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None),
        None
    );
    assert!(store
        .list_commits_for_branch(&branch.id)
        .unwrap()
        .is_empty());
}

#[test]
fn busy_branch_queues_pipeline_with_label_and_pending_commit() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let session_id = queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None)
        .expect("rebase should queue behind the running note session");

    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, store::SessionStatus::Queued);
    // The timeline renders queued pipeline rows from this prompt, so it has
    // to read as the git action rather than a bare "Pending commit".
    assert_eq!(session.prompt, "Rebase branch");
    let pipeline = session.pipeline.unwrap();
    assert_eq!(pipeline.kind, Some(PipelineKind::Rebase));
    assert_eq!(pipeline.rebase_target, None);

    let pending: Vec<_> = store
        .list_commits_for_branch(&branch.id)
        .unwrap()
        .into_iter()
        .filter(|c| c.sha.is_none())
        .collect();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].session_id.as_deref(), Some(session_id.as_str()));
}

#[test]
fn queued_squash_uses_squash_label() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let session_id = queue_pipeline(&store, &branch.id, PipelineKind::Squash, None).unwrap();

    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.prompt, "Squash commits");
    assert_eq!(session.pipeline.unwrap().kind, Some(PipelineKind::Squash));
}

#[test]
fn repeated_click_reuses_the_already_queued_pipeline() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let first = queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();
    let second = queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();

    assert_eq!(first, second);
    assert_eq!(
        store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn different_pipeline_kinds_queue_separately() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let rebase = queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();
    let squash = queue_pipeline(&store, &branch.id, PipelineKind::Squash, None).unwrap();

    assert_ne!(rebase, squash);
    assert_eq!(
        store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn rebase_onto_origin_is_not_deduped_against_rebase_onto_base() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let onto_base = queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();
    let onto_origin =
        queue_pipeline(&store, &branch.id, PipelineKind::Rebase, Some("origin")).unwrap();

    assert_ne!(onto_base, onto_origin);
    let origin_session = store.get_session(&onto_origin).unwrap().unwrap();
    assert_eq!(
        origin_session.pipeline.unwrap().rebase_target.as_deref(),
        Some("feature")
    );

    // Clicking the diverged row's rebase again still dedupes.
    assert_eq!(
        queue_pipeline(&store, &branch.id, PipelineKind::Rebase, Some("origin")),
        Some(onto_origin)
    );
}

#[test]
fn queued_pipeline_alone_keeps_the_branch_busy() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);
    queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();

    // The note session finishes, but the still-queued rebase must keep a
    // newly requested squash on the queue rather than running it now.
    for session in store.get_running_sessions().unwrap() {
        store
            .update_session_status(&session.id, store::SessionStatus::Completed, None, None)
            .unwrap();
    }

    assert!(queue_pipeline(&store, &branch.id, PipelineKind::Squash, None).is_some());
}

fn queue_rebase_then_force_push(store: &Arc<Store>, branch_id: &str) -> BranchPipelineResponse {
    queue_commit_pipeline_if_branch_busy(store, branch_id, &PipelineKind::Rebase, None, None, true)
        .unwrap()
        .expect("a busy branch queues the rebase")
}

#[test]
fn rebase_and_force_push_queues_a_push_gated_on_the_rebase() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let response = queue_rebase_then_force_push(&store, &branch.id);
    let push_id = response
        .follow_up_session_id
        .clone()
        .expect("the force push is queued alongside the rebase");

    let rebase = store.get_session(&response.session_id).unwrap().unwrap();
    let push = store.get_session(&push_id).unwrap().unwrap();
    assert_eq!(push.status, store::SessionStatus::Queued);
    assert_eq!(push.prompt, FORCE_PUSH_PROMPT);
    assert_eq!(push.branch_id.as_deref(), Some(branch.id.as_str()));
    let pipeline = push.pipeline.unwrap();
    assert_eq!(pipeline.kind, Some(PipelineKind::Push));
    assert!(pipeline.push_force);
    assert_eq!(
        pipeline.depends_on_session_id.as_deref(),
        Some(rebase.id.as_str())
    );
    // Strictly after the rebase, so a shared millisecond can't put the
    // push first in the queue or the timeline.
    assert!(push.created_at > rebase.created_at);
    let queued: Vec<_> = store
        .get_queued_sessions_for_branch(&branch.id)
        .unwrap()
        .into_iter()
        .map(|session| session.id)
        .collect();
    assert_eq!(queued, vec![rebase.id, push_id]);
}

#[test]
fn repeated_rebase_and_force_push_reuses_both_queued_rows() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let first = queue_rebase_then_force_push(&store, &branch.id);
    let second = queue_rebase_then_force_push(&store, &branch.id);

    assert_eq!(first, second);
    assert_eq!(
        store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn gated_and_unconditional_force_pushes_do_not_dedupe_into_each_other() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let gated = queue_rebase_then_force_push(&store, &branch.id)
        .follow_up_session_id
        .unwrap();
    let plain = queue_push(&store, &branch.id, FORCE_PUSH).unwrap();

    assert_ne!(gated, plain);
    let plain = store.get_session(&plain).unwrap().unwrap();
    assert_eq!(plain.pipeline.unwrap().depends_on_session_id, None);
    // And a plain rebase still dedupes onto the queued one, without a push.
    let rebase_again = queue_commit_pipeline_if_branch_busy(
        &store,
        &branch.id,
        &PipelineKind::Rebase,
        None,
        None,
        false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(rebase_again.follow_up_session_id, None);
    assert_eq!(
        store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .len(),
        3
    );
}

/// On an idle branch the rebase runs now, but its push still queues: it is
/// inserted in the same locked section as the running rebase row, before
/// the runner could finish (and drain) it.
#[test]
fn running_rebase_still_gets_a_queued_push() {
    let (store, branch) = setup_branch_store();
    let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();
    let running = insert_running_commit_pipeline_session(
        &store,
        &test_registry(),
        &pipeline_context(&branch),
        PipelineKind::Rebase,
        &steps,
        None,
        None,
    )
    .unwrap();

    let response = with_follow_up_force_push_locked(
        &store,
        &branch.id,
        None,
        BranchPipelineResponse::running(running.session_id.clone()),
        true,
    )
    .unwrap();

    assert_eq!(response.session_status, BranchSessionLaunchStatus::Running);
    let push = store
        .get_session(&response.follow_up_session_id.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(push.status, store::SessionStatus::Queued);
    assert_eq!(
        push.pipeline.unwrap().depends_on_session_id,
        Some(running.session_id)
    );
}

/// A mock app whose web broadcast channel is managed, so a test can read
/// back exactly what `emit_to_all` sent to browser clients.
fn mock_app_with_web_events() -> (
    tauri::App<tauri::test::MockRuntime>,
    tokio::sync::broadcast::Receiver<crate::web_server::WebEvent>,
) {
    use tauri::Manager;
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("failed to build mock app");
    let (tx, rx) = tokio::sync::broadcast::channel::<crate::web_server::WebEvent>(8);
    app.manage(tx);
    (app, rx)
}

/// The running rebase row goes in before its push, so a push insert that
/// fails must not leave a `running` rebase nobody is running — and every
/// client must hear about the failure, since the requesting one only
/// toasts the error and nobody else would otherwise refresh the timeline.
#[test]
fn running_rebase_is_failed_when_its_push_cannot_be_queued() {
    let (store, branch) = setup_branch_store();
    let (app, mut web_events) = mock_app_with_web_events();
    let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();
    let running = insert_running_commit_pipeline_session(
        &store,
        &test_registry(),
        &pipeline_context(&branch),
        PipelineKind::Rebase,
        &steps,
        None,
        None,
    )
    .unwrap();

    // The push insert looks the branch up by id, so an unknown id is a
    // store failure after the rebase row already exists.
    let err = with_follow_up_force_push_or_fail_rebase_locked(
        &store,
        app.handle(),
        "missing-branch",
        &branch.project_id,
        None,
        &running.session_id,
        true,
    )
    .unwrap_err();
    assert!(err.contains("Branch not found"), "{err}");

    let rebase = store.get_session(&running.session_id).unwrap().unwrap();
    assert_eq!(rebase.status, store::SessionStatus::Error);
    assert_eq!(rebase.error_message.as_deref(), Some(err.as_str()));
    assert!(store
        .get_queued_sessions_for_branch(&branch.id)
        .unwrap()
        .is_empty());

    // The same terminal event the dead-session sweep would have emitted,
    // so timelines invalidate now rather than on the next refresh.
    let event = web_events.try_recv().expect("the failure is announced");
    assert_eq!(event.event_name, "session-status-changed");
    let payload: serde_json::Value = serde_json::from_str(&event.payload).unwrap();
    assert_eq!(payload["payload"]["sessionId"], running.session_id);
    assert_eq!(payload["payload"]["status"], "error");
    assert_eq!(payload["payload"]["errorMessage"], err);
    assert_eq!(payload["payload"]["branchId"], "missing-branch");
    assert_eq!(payload["payload"]["projectId"], branch.project_id);

    // Without a push to queue there is nothing that can fail, and the
    // rebase is left running for the runner, with nothing announced.
    let running = insert_running_commit_pipeline_session(
        &store,
        &test_registry(),
        &pipeline_context(&branch),
        PipelineKind::Rebase,
        &steps,
        None,
        None,
    )
    .unwrap();
    with_follow_up_force_push_or_fail_rebase_locked(
        &store,
        app.handle(),
        "missing-branch",
        &branch.project_id,
        None,
        &running.session_id,
        false,
    )
    .unwrap();
    assert_eq!(
        store
            .get_session(&running.session_id)
            .unwrap()
            .unwrap()
            .status,
        store::SessionStatus::Running
    );
    assert!(web_events.try_recv().is_err());
}

#[test]
fn follow_up_session_id_is_omitted_from_plain_responses() {
    let json = serde_json::to_string(&BranchPipelineResponse::queued("s".into())).unwrap();
    assert!(!json.contains("followUpSessionId"), "{json}");
}

#[test]
fn inserting_a_running_pipeline_is_what_makes_the_branch_busy() {
    let (store, branch) = setup_branch_store();
    let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();

    // The pre-flight check sees an idle branch and lets the rebase run now.
    assert_eq!(
        queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None),
        None
    );
    insert_running_commit_pipeline_session(
        &store,
        &test_registry(),
        &pipeline_context(&branch),
        PipelineKind::Rebase,
        &steps,
        None,
        None,
    )
    .unwrap();

    // The rows that insert wrote are what the re-check at insert time reads, so
    // an action that raced it queues instead of also starting.
    assert!(
        queue_commit_pipeline_locked(&store, &branch.id, &PipelineKind::Squash, None, None)
            .unwrap()
            .is_some()
    );
    assert!(queue_push(&store, &branch.id, PUSH).is_some());
}

#[test]
fn rebase_onto_origin_targets_the_branch_the_checkout_pushes_to() {
    let (store, branch) = setup_branch_store();
    // No worktree yet: the branch resolves to its own name.
    let remote_branch = crate::branches::resolve_remote_branch_sync(&store, &branch);
    assert_eq!(remote_branch, branch.branch_name);
    assert_eq!(
        rebase_ref_for_target(&branch, &remote_branch, Some("origin")),
        branch.branch_name
    );

    // A de-duplicated PR import whose checkout tracks the PR head:
    // "rebase onto origin" targets the PR head, not origin/feature-x-2.
    let deduplicated = store::Branch::new(&branch.project_id, "feature-x-2", "main").with_pr(7);
    assert_eq!(
        rebase_ref_for_target(&deduplicated, "feature-x", Some("origin")),
        "feature-x"
    );
    assert_eq!(
        rebase_ref_for_target(&deduplicated, "feature-x", None),
        "main"
    );

    let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "feature-x").unwrap();
    let (_, rebase_command, _) = command_at(&steps, 1);
    assert!(rebase_command.contains("origin/feature-x"));
    assert!(!rebase_command.contains("feature-x-2"));
}

/// A queued "rebase onto origin" marks its row with the branch's local
/// name even when the checkout tracks a differently named PR head: the
/// branch on origin is resolved when the row is dequeued, never replayed
/// from the marker.
#[test]
fn queued_rebase_onto_origin_marks_the_local_name_not_the_resolved_remote() {
    use crate::test_utils::{run_git_in, TempDir, TempGitRepo};

    let seed = TempGitRepo::new();
    seed.write_file("file.txt", "base\n");
    seed.commit("base");
    seed.run_git(&["branch", "feature-x"]);
    let origin = TempDir::new("origin");
    let origin_str = origin.path().to_str().unwrap();
    run_git_in(seed.path(), &["clone", "--bare", ".", origin_str]);
    let clone = TempDir::new("clone");
    run_git_in(
        origin.path(),
        &["clone", origin_str, clone.path().to_str().unwrap()],
    );
    run_git_in(
        clone.path(),
        &[
            "checkout",
            "--no-track",
            "-b",
            "feature-x-2",
            "origin/feature-x",
        ],
    );
    run_git_in(
        clone.path(),
        &["branch", "-u", "origin/feature-x", "feature-x-2"],
    );

    let store = Arc::new(Store::in_memory().unwrap());
    let project = store::Project::new("acme/widgets");
    store.create_project(&project).unwrap();
    let branch = store::Branch::new(&project.id, "feature-x-2", "main").with_pr(7);
    store.create_branch(&branch).unwrap();
    store
        .create_workdir(
            &store::Workdir::new(&project.id, clone.path().to_str().unwrap())
                .with_branch(&branch.id),
        )
        .unwrap();
    assert_eq!(
        crate::branches::resolve_remote_branch_sync(&store, &branch),
        "feature-x"
    );

    start_running_note_session(&store, &branch.id);
    let queued = queue_pipeline(&store, &branch.id, PipelineKind::Rebase, Some("origin")).unwrap();
    let session = store.get_session(&queued).unwrap().unwrap();
    assert_eq!(
        session.pipeline.unwrap().rebase_target.as_deref(),
        Some("feature-x-2")
    );
    // The marker still tells "onto origin" apart from "onto base".
    assert_eq!(
        queue_pipeline(&store, &branch.id, PipelineKind::Rebase, Some("origin")),
        Some(queued.clone())
    );
    assert_ne!(
        queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None),
        Some(queued)
    );
}

#[test]
fn commit_pipeline_steps_reject_the_git_pipeline_kinds() {
    let push_err = build_commit_pipeline_steps(&PipelineKind::Push, "main", "main").unwrap_err();
    let pull_err = build_commit_pipeline_steps(&PipelineKind::Pull, "main", "main").unwrap_err();

    assert_eq!(push_err, "Push is not a commit pipeline");
    assert_eq!(pull_err, "Pull is not a commit pipeline");
}

#[test]
fn rebase_pipeline_uses_signoff() {
    let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();

    let (label, command, _) = command_at(&steps, 0);
    assert_eq!(label, "Fetch latest base");
    assert_eq!(
        command,
        "if ! git fetch origin main; then git -c 'url.https://github.com/.insteadOf=git@github.com:' fetch origin main; fi"
    );

    let (label, command, _) = command_at(&steps, 1);
    assert_eq!(label, "Rebase onto base");
    assert_eq!(
        command,
        git::rebase_preserving_merge_edits_command("origin/main")
    );
    assert!(command.contains("--signoff"));
    assert!(command.contains("onto='origin/main'"));
}

/// The rebase step must never be a bare `git rebase`: that is exactly the
/// command that silently drops hand edits carried by merge commits. The
/// step also travels to remote workspaces as a single argv element, so the
/// command must be one line.
#[test]
fn rebase_pipeline_routes_through_the_merge_edit_safeguard() {
    for target in ["main", "feature-branch"] {
        let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", target).unwrap();
        let (_, command, on_failure) = command_at(&steps, 1);
        assert!(command.contains("git merge-tree --write-tree"), "{command}");
        assert!(!command.contains("--rebase-merges"), "{command}");
        assert!(!command.starts_with("git rebase"), "{command}");
        assert!(!command.contains('\n'), "{command}");

        let FailureStrategy::HandoffToAi { prompt_template } = on_failure else {
            panic!("expected rebase step to have HandoffToAi failure");
        };
        assert!(prompt_template.contains(git::HAND_EDIT_COMMIT_SUBJECT_PREFIX));
        assert!(prompt_template.contains("never drop or skip them"));
    }
}

/// After a conflict the only correct move is `git rebase --continue`. The
/// prompt must not send the agent back to the generated command (a long
/// script it cannot reproduce byte for byte) and must leave aborting to
/// the user.
#[test]
fn rebase_handoff_prompt_continues_never_restarts_and_leaves_abort_to_the_user() {
    let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();
    let (_, _, on_failure) = command_at(&steps, 1);
    let FailureStrategy::HandoffToAi { prompt_template } = on_failure else {
        panic!("expected rebase step to have HandoffToAi failure");
    };
    assert!(
        prompt_template.contains("run `git rebase --continue`"),
        "{prompt_template}"
    );
    assert!(
        prompt_template.contains("Never restart the rebase and never run a plain `git rebase`"),
        "{prompt_template}"
    );
    assert!(
        prompt_template.contains("`git rebase --abort` is the user's decision, not yours"),
        "{prompt_template}"
    );
    assert!(
        prompt_template.contains("refused before it started"),
        "{prompt_template}"
    );
    assert!(
        !prompt_template.contains("re-run the exact rebase command"),
        "{prompt_template}"
    );
    assert!(
        !prompt_template.contains("if you must restart"),
        "{prompt_template}"
    );
}

#[test]
fn rebase_pipeline_targets_origin_branch_when_target_differs() {
    let steps =
        build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "feature-branch").unwrap();

    let (label, command, _) = command_at(&steps, 0);
    assert_eq!(label, "Fetch origin/feature-branch");
    assert_eq!(
        command,
        "if ! git fetch origin feature-branch; then git -c 'url.https://github.com/.insteadOf=git@github.com:' fetch origin feature-branch; fi"
    );

    let (label, command, _) = command_at(&steps, 1);
    assert_eq!(label, "Rebase onto origin/feature-branch");
    assert_eq!(
        command,
        git::rebase_preserving_merge_edits_command("origin/feature-branch")
    );
    assert!(command.contains("onto='origin/feature-branch'"));
}

#[test]
fn rebase_pipeline_prompt_mentions_base_when_target_differs() {
    let steps =
        build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "feature-branch").unwrap();

    let fetch_failure = match &steps[0] {
        PipelineStep::Command {
            on_failure: FailureStrategy::HandoffToAi { prompt_template },
            ..
        } => prompt_template.clone(),
        _ => panic!("expected fetch step to have HandoffToAi failure"),
    };
    assert!(fetch_failure.contains("origin/feature-branch"));
    assert!(fetch_failure.contains("origin/main"));
    assert!(fetch_failure.contains("the rebase target may be wrong"));

    let rebase_failure = match &steps[1] {
        PipelineStep::Command {
            on_failure: FailureStrategy::HandoffToAi { prompt_template },
            ..
        } => prompt_template.clone(),
        _ => panic!("expected rebase step to have HandoffToAi failure"),
    };
    assert!(rebase_failure.contains("origin/feature-branch"));
    assert!(rebase_failure.contains("origin/main"));
    assert!(rebase_failure.contains("the rebase target may be wrong"));
}

#[test]
fn rebase_pipeline_prompt_omits_target_note_when_target_matches_base() {
    let steps = build_commit_pipeline_steps(&PipelineKind::Rebase, "main", "main").unwrap();

    for step in &steps {
        if let PipelineStep::Command {
            on_failure: FailureStrategy::HandoffToAi { prompt_template },
            ..
        } = step
        {
            assert!(!prompt_template.contains("the rebase target may be wrong"));
            assert!(!prompt_template.contains("is different"));
        }
    }
}

#[test]
fn squash_pipeline_prompt_requires_signoff() {
    let steps = build_commit_pipeline_steps(&PipelineKind::Squash, "main", "main").unwrap();

    let (_, prompt) = ai_prompt_at(&steps, 3);
    assert!(prompt.contains("Use the user's global git identity"));
    assert!(prompt.contains("git commit --signoff"));
}
