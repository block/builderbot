//! Push: queueing and per-variant dedupe, the run-now row, and the steps.

use super::*;

#[test]
fn idle_branch_pushes_immediately_instead_of_queueing() {
    let (store, branch) = setup_branch_store();

    assert_eq!(queue_push(&store, &branch.id, PUSH), None);
    assert!(store
        .get_queued_sessions_for_branch(&branch.id)
        .unwrap()
        .is_empty());
}

#[test]
fn busy_branch_queues_push_with_branch_link_and_no_artifact() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let session_id = queue_push(&store, &branch.id, PUSH)
        .expect("push should queue behind the running note session");

    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, store::SessionStatus::Queued);
    assert_eq!(session.prompt, PUSH_PROMPT);
    // The branch link is what keeps an artifact-less push on the queue.
    assert_eq!(session.branch_id.as_deref(), Some(branch.id.as_str()));
    let pipeline = session.pipeline.unwrap();
    assert_eq!(pipeline.kind, Some(PipelineKind::Push));
    assert!(!pipeline.push_force);

    // A pending commit would render as a failed commit once the push ends.
    assert!(store
        .list_commits_for_branch(&branch.id)
        .unwrap()
        .is_empty());
}

#[test]
fn repeated_push_click_reuses_the_already_queued_push() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let first = queue_push(&store, &branch.id, PUSH).unwrap();
    let second = queue_push(&store, &branch.id, PUSH).unwrap();

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
fn force_push_is_not_deduped_against_a_queued_normal_push() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let push = queue_push(&store, &branch.id, PUSH).unwrap();
    let force_push = queue_push(&store, &branch.id, FORCE_PUSH).unwrap();

    assert_ne!(push, force_push);
    let forced = store.get_session(&force_push).unwrap().unwrap();
    assert_eq!(forced.prompt, FORCE_PUSH_PROMPT);
    assert!(forced.pipeline.unwrap().push_force);
    assert_eq!(
        store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .len(),
        2
    );

    // Clicking force push again still dedupes.
    assert_eq!(queue_push(&store, &branch.id, FORCE_PUSH), Some(force_push));
}

#[test]
fn no_verify_and_verifying_pushes_do_not_dedupe_into_each_other() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let variants = [PUSH, NO_VERIFY_PUSH, FORCE_PUSH, NO_VERIFY_FORCE_PUSH];
    let ids: Vec<String> = variants
        .iter()
        .map(|&variant| queue_push(&store, &branch.id, variant).unwrap())
        .collect();

    let mut distinct = ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), variants.len(), "{ids:?}");
    for (variant, id) in variants.iter().zip(&ids) {
        let session = store.get_session(id).unwrap().unwrap();
        assert_eq!(
            session.prompt,
            pipeline_prompt(&PipelineKind::Push, *variant)
        );
        assert_eq!(PushVariant::of(&session.pipeline.unwrap()), *variant);
        // A repeat request for the same variant still dedupes.
        assert_eq!(queue_push(&store, &branch.id, *variant).as_ref(), Some(id));
    }
    assert_eq!(
        store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .len(),
        variants.len()
    );
}

/// The prompt is how a push session is recognised once it ends:
/// `session_completion` keys the `push-completed` event (and clearing the
/// stale PR status) on it.
#[test]
fn every_push_prompt_is_recognised_as_a_push() {
    for variant in [PUSH, NO_VERIFY_PUSH, FORCE_PUSH, NO_VERIFY_FORCE_PUSH] {
        let prompt = pipeline_prompt(&PipelineKind::Push, variant);
        assert_eq!(
            crate::session_commands::infer_branch_resume_session_type(prompt),
            Some("push"),
            "{variant:?}: {prompt}"
        );
    }
    assert!(NO_VERIFY_PUSH_PROMPT.contains("--no-verify"));
    assert!(NO_VERIFY_PUSH_PROMPT.contains("Do not force push."));
    assert!(NO_VERIFY_FORCE_PUSH_PROMPT.contains("--no-verify"));
}

#[test]
fn queued_push_alone_keeps_the_branch_busy() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);
    queue_push(&store, &branch.id, PUSH).unwrap();

    // The note session finishes, but the still-queued push must keep a newly
    // requested squash on the queue rather than running it now.
    for session in store.get_running_sessions().unwrap() {
        store
            .update_session_status(&session.id, store::SessionStatus::Completed, None, None)
            .unwrap();
    }

    assert!(queue_pipeline(&store, &branch.id, PipelineKind::Squash, None).is_some());
}

#[test]
fn queued_rebase_keeps_a_later_push_on_the_queue() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);
    queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).unwrap();

    for session in store.get_running_sessions().unwrap() {
        store
            .update_session_status(&session.id, store::SessionStatus::Completed, None, None)
            .unwrap();
    }

    assert!(queue_push(&store, &branch.id, PUSH).is_some());
}

#[test]
fn inserting_a_running_push_is_what_makes_the_branch_busy() {
    let (store, branch) = setup_branch_store();
    let steps = build_push_pipeline_steps(&branch.branch_name, &branch.branch_name, PUSH);

    insert_running_push_pipeline_session(
        &store,
        &test_registry(),
        &pipeline_context(&branch),
        PUSH,
        &steps,
        None,
    )
    .unwrap();

    // A push has no artifact, so `branch_id` is what the re-check keys on.
    assert!(
        queue_push_pipeline_locked(&store, &branch.id, None, FORCE_PUSH)
            .unwrap()
            .is_some()
    );
    assert!(
        queue_commit_pipeline_locked(&store, &branch.id, &PipelineKind::Rebase, None, None)
            .unwrap()
            .is_some()
    );
}

#[test]
fn push_steps_are_rebuilt_from_the_current_branch_name_and_force_flag() {
    // Same local and remote name: the refspec is still explicit.
    let normal = build_push_pipeline_steps("feature", "feature", PUSH);
    let (label, command, on_failure) = command_at(&normal, 0);
    assert_eq!(label, "Push to remote");
    assert!(command.contains("-u origin feature:feature"));
    assert!(!command.contains("--force-with-lease"));
    assert!(matches!(
        on_failure,
        FailureStrategy::Abort { marker } if marker.as_deref() == Some("non-fast-forward")
    ));

    // The drain path re-derives the refspec, so a branch renamed while the
    // push waited pushes the new local name rather than the queued one.
    let forced = build_push_pipeline_steps("feature-renamed", "feature", FORCE_PUSH);
    let (_, forced_command, forced_on_failure) = command_at(&forced, 0);
    assert!(forced_command.contains("-u origin feature-renamed:feature --force-with-lease"));
    // The handoff spells the refspec out so an agent doesn't retry with
    // `git push origin feature-renamed` and create a stray remote branch.
    let FailureStrategy::HandoffToAi { prompt_template } = forced_on_failure else {
        panic!("expected force push to hand off to an agent");
    };
    assert!(
        prompt_template.contains("`git push -u origin feature-renamed:feature --force-with-lease`"),
        "{prompt_template}"
    );
    assert!(
        prompt_template.contains("{step_outputs}"),
        "{prompt_template}"
    );
}

/// The bug this guards against: a PR import whose head branch was already
/// checked out elsewhere gets a de-duplicated local name (`feature-x-2`).
/// Pushing must still land on the PR's branch, not create
/// `origin/feature-x-2`.
#[test]
fn no_verify_push_steps_skip_the_hook_in_both_fallback_halves() {
    let plain = build_push_pipeline_steps("feature", "feature", PUSH);
    let (_, command, _) = command_at(&plain, 0);
    assert!(!command.contains("--no-verify"), "{command}");

    let no_verify = build_push_pipeline_steps("feature", "feature", NO_VERIFY_PUSH);
    let (_, command, on_failure) = command_at(&no_verify, 0);
    // Once for the SSH attempt, once for the HTTPS retry.
    assert_eq!(
        command
            .matches("push --no-verify -u origin feature:feature")
            .count(),
        2,
        "{command}"
    );
    assert!(!command.contains("--force-with-lease"), "{command}");
    // Still aborts on a rejection, so the force-push dialog can offer the
    // no-verify force push.
    assert!(matches!(
        on_failure,
        FailureStrategy::Abort { marker } if marker.as_deref() == Some("non-fast-forward")
    ));

    let forced = build_push_pipeline_steps("feature-renamed", "feature", NO_VERIFY_FORCE_PUSH);
    let (_, command, on_failure) = command_at(&forced, 0);
    assert_eq!(
        command
            .matches("push --no-verify -u origin feature-renamed:feature --force-with-lease")
            .count(),
        2,
        "{command}"
    );
    // The agent's retry must skip the hook too.
    let FailureStrategy::HandoffToAi { prompt_template } = on_failure else {
        panic!("expected force push to hand off to an agent");
    };
    assert!(
        prompt_template.contains(
            "`git push --no-verify -u origin feature-renamed:feature --force-with-lease`"
        ),
        "{prompt_template}"
    );
}

#[test]
fn deduplicated_pr_branch_pushes_to_the_pr_head_not_its_local_name() {
    let steps = build_push_pipeline_steps("feature-x-2", "feature-x", PUSH);
    let (_, command, _) = command_at(&steps, 0);
    assert!(command.contains("git push -u origin feature-x-2:feature-x"));
    assert!(!command.contains("origin feature-x-2 "));
    assert!(!command.contains("feature-x-2:feature-x-2"));
}
