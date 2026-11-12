//! Fast-forward pull: the immediate marker session, queueing, and the steps.

use super::*;

/// The stale-lock reproduction. In web mode the request future owns the
/// pull: hyper drops it the moment the client goes away (its HTTP/1
/// half-close default closes the connection on a mid-request EOF), and a
/// `running` Pull row with no runner behind it is what used to be left.
/// That row is an exclusive kind, so every later note, commit, review and
/// push on the branch queued behind it with nothing to ever release it.
///
/// Poll the request once — far enough to insert the marker — drop it, let
/// the pull finish, and the marker must still complete and the branch must
/// still take a Note without queueing it.
#[tokio::test]
async fn dropping_the_request_future_mid_pull_no_longer_strands_the_marker() {
    let (store, branch) = setup_branch_store();
    let workdir =
        store::Workdir::new(&branch.project_id, "/tmp/staged-test-pull").with_branch(&branch.id);
    store.create_workdir(&workdir).unwrap();

    let registry = test_registry();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let request = pull_or_queue_branch_with(
        Arc::clone(&store),
        Arc::clone(&registry),
        branch.id.clone(),
        move || {
            released.recv().map_err(|e| e.to_string())?;
            Ok(())
        },
        || {},
    );
    {
        let mut request = std::pin::pin!(request);
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(request.as_mut(), cx).is_pending(),
                "the pull must still be in flight after one poll"
            );
            std::task::Poll::Ready(())
        })
        .await;
        // The client is gone.
    }

    let marker = store
        .get_running_sessions()
        .unwrap()
        .into_iter()
        .find(|session| session.branch_id.as_deref() == Some(branch.id.as_str()))
        .expect("the marker row is inserted before the first await");
    // The marker never goes through the runner, so its liveness hold is
    // what keeps the stranded-session sweep off it for as long as the
    // pull takes — and the hold belongs to the pull task, not the
    // request future that was just dropped.
    assert!(
        registry.is_live(&marker.id),
        "the immediate pull's marker must be live in the registry while the pull runs"
    );
    release.send(()).unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        let status = store.get_session(&marker.id).unwrap().unwrap().status;
        if status != store::SessionStatus::Running || std::time::Instant::now() > deadline {
            break status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    assert_eq!(
        status,
        store::SessionStatus::Completed,
        "the marker must be released by the pull, not by the request future"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while registry.is_live(&marker.id) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        !registry.is_live(&marker.id),
        "the hold is released once the marker has its terminal state"
    );
    assert!(
        !crate::session_commands::should_queue_branch_session_start(
            &store,
            &branch.id,
            &crate::session_commands::BranchSessionType::Note,
        )
        .unwrap(),
        "a note on the branch must start, not queue behind a stale pull marker"
    );
}

/// The queued session id, or `None` when the branch was idle and the pull
/// claimed it to run now.
fn queue_pull(store: &Arc<Store>, branch_id: &str) -> Option<String> {
    match claim_or_queue_pull_for_branch(store, &test_registry(), branch_id).unwrap() {
        PullDisposition::Queued(session_id) => Some(session_id),
        PullDisposition::RunningNow { .. } => None,
    }
}

fn claim_pull(store: &Arc<Store>, branch_id: &str) -> String {
    match claim_or_queue_pull_for_branch(store, &test_registry(), branch_id).unwrap() {
        PullDisposition::RunningNow { session_id, .. } => session_id,
        PullDisposition::Queued(session_id) => {
            panic!("expected an immediate pull, got queued session {session_id}")
        }
    }
}

#[test]
fn idle_branch_pulls_immediately_instead_of_queueing() {
    let (store, branch) = setup_branch_store();

    assert_eq!(queue_pull(&store, &branch.id), None);
    assert!(store
        .get_queued_sessions_for_branch(&branch.id)
        .unwrap()
        .is_empty());
}

#[test]
fn immediate_pull_claims_the_branch_with_a_running_marker_session() {
    let (store, branch) = setup_branch_store();

    let session_id = claim_pull(&store, &branch.id);

    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, store::SessionStatus::Running);
    // The branch link and pipeline kind are what make the pull visible to the
    // queue; it creates no artifact, exactly like a push.
    assert_eq!(session.branch_id.as_deref(), Some(branch.id.as_str()));
    assert_eq!(
        session.pipeline.unwrap().kind,
        Some(store::PipelineKind::Pull)
    );
    assert!(store
        .list_commits_for_branch(&branch.id)
        .unwrap()
        .is_empty());
    assert!(store.has_running_session_for_branch(&branch.id).unwrap());
}

#[test]
fn a_pull_in_flight_keeps_other_branch_work_off_the_worktree() {
    let (store, branch) = setup_branch_store();
    claim_pull(&store, &branch.id);

    // A commit pipeline or a second git action requested mid-pull has to wait
    // rather than race the fast-forward in the same worktree.
    assert!(queue_pipeline(&store, &branch.id, PipelineKind::Rebase, None).is_some());
    assert!(queue_push(&store, &branch.id, PUSH).is_some());
    assert!(queue_pull(&store, &branch.id).is_some());
}

#[test]
fn finishing_an_immediate_pull_frees_the_branch_and_records_the_failure() {
    let (store, branch) = setup_branch_store();

    let ok = claim_pull(&store, &branch.id);
    finish_immediate_pull_session(&store, &ok, None);
    let completed = store.get_session(&ok).unwrap().unwrap();
    assert_eq!(completed.status, store::SessionStatus::Completed);
    assert!(!store.has_running_session_for_branch(&branch.id).unwrap());

    let failed_id = claim_pull(&store, &branch.id);
    finish_immediate_pull_session(
        &store,
        &failed_id,
        Some("Cannot pull with uncommitted changes"),
    );
    let failed = store.get_session(&failed_id).unwrap().unwrap();
    assert_eq!(failed.status, store::SessionStatus::Error);
    assert_eq!(
        failed.error_message.as_deref(),
        Some("Cannot pull with uncommitted changes")
    );
    assert!(!store.has_running_session_for_branch(&branch.id).unwrap());
}

#[test]
fn busy_branch_queues_pull_with_branch_link_and_no_artifact() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let session_id =
        queue_pull(&store, &branch.id).expect("pull should queue behind the running note");

    let session = store.get_session(&session_id).unwrap().unwrap();
    assert_eq!(session.status, store::SessionStatus::Queued);
    assert_eq!(session.prompt, PULL_PROMPT);
    // Like a queued push, the branch link is what keeps an artifact-less
    // pull on the queue.
    assert_eq!(session.branch_id.as_deref(), Some(branch.id.as_str()));
    assert_eq!(
        session.pipeline.unwrap().kind,
        Some(store::PipelineKind::Pull)
    );
    assert!(store
        .list_commits_for_branch(&branch.id)
        .unwrap()
        .is_empty());
}

#[test]
fn repeated_pull_click_reuses_the_already_queued_pull() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let first = queue_pull(&store, &branch.id).unwrap();
    let second = queue_pull(&store, &branch.id).unwrap();

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
fn pull_and_push_queue_separately() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    let pull = queue_pull(&store, &branch.id).unwrap();
    let push = queue_push(&store, &branch.id, PUSH).unwrap();

    assert_ne!(pull, push);
    assert_eq!(
        store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn queued_pull_alone_keeps_the_branch_busy() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);
    queue_pull(&store, &branch.id).unwrap();

    // The note session finishes, but the still-queued pull must keep a newly
    // requested push on the queue rather than running it now.
    for session in store.get_running_sessions().unwrap() {
        store
            .update_session_status(&session.id, store::SessionStatus::Completed, None, None)
            .unwrap();
    }

    assert!(queue_push(&store, &branch.id, PUSH).is_some());
}

#[test]
fn pull_steps_fast_forward_to_the_pinned_remote_branch() {
    // A de-duplicated PR import pulls from the PR head, never from the
    // (non-existent) origin/<local name>.
    let steps = build_pull_pipeline_steps("feature-x");
    let (_, fetch_command, _) = command_at(&steps, 0);
    assert!(fetch_command.contains("git fetch origin feature-x"));
    let (_, merge_command, _) = command_at(&steps, 1);
    assert_eq!(merge_command, "git merge --ff-only origin/feature-x");
}

#[test]
fn pull_steps_fetch_then_fast_forward_and_never_hand_off() {
    // The drain path re-derives the ref, so a branch renamed while the pull
    // waited fast-forwards the new name rather than the queued one.
    let steps = build_pull_pipeline_steps("feature-renamed");

    let (fetch_label, fetch_command, fetch_on_failure) = command_at(&steps, 0);
    assert_eq!(fetch_label, "Fetch origin/feature-renamed");
    assert!(fetch_command.contains("git fetch origin feature-renamed"));
    assert!(matches!(
        fetch_on_failure,
        FailureStrategy::Abort { marker: None }
    ));

    let (merge_label, merge_command, merge_on_failure) = command_at(&steps, 1);
    assert_eq!(merge_label, "Fast-forward to origin/feature-renamed");
    assert_eq!(merge_command, "git merge --ff-only origin/feature-renamed");
    // A diverged branch is the user's call to make, not an agent's.
    assert!(matches!(
        merge_on_failure,
        FailureStrategy::Abort { marker: None }
    ));
    assert_eq!(steps.len(), 2);
}
