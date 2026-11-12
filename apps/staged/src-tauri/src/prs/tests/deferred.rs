//! Deferred push ("Push after current work"): what holds it, what fires it,
//! and when a completed push stands in for it.

use super::*;

/// A running project-note session in `project_id`: the project-level
/// work a deferred push waits for, invisible to the branch queries.
fn start_running_project_note_session(store: &Arc<Store>, project_id: &str) -> String {
    let session = store::Session::new_running("plan", Path::new("/tmp/staged-test"));
    store.create_session(&session).unwrap();
    let note = store::ProjectNote::new(project_id, "Plan", "").with_session(&session.id);
    store.create_project_note(&note).unwrap();
    session.id
}

fn end_session(store: &Arc<Store>, session_id: &str) {
    store
        .update_session_status(session_id, store::SessionStatus::Completed, None, None)
        .unwrap();
}

fn deferred_push_of(store: &Arc<Store>, branch_id: &str) -> Option<store::DeferredPush> {
    store.get_branch(branch_id).unwrap().unwrap().deferred_push
}

/// Arm `variant` with an immediate push that must not run.
async fn arm(store: &Arc<Store>, branch_id: &str, variant: PushVariant) {
    let response = defer_branch_push_with(store, branch_id, variant, |variant| {
        std::future::ready(Err(format!(
            "the branch was busy; nothing should push now ({variant:?})"
        )))
    })
    .await
    .unwrap();
    assert_eq!(
        response,
        DeferredPushResponse {
            armed: true,
            push: None
        }
    );
}

/// Run the drain tail's firing step with a push that only records the
/// variant it was asked for.
async fn fire(store: &Arc<Store>, branch_id: &str) -> (bool, Vec<PushVariant>) {
    let fired = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&fired);
    let started = fire_deferred_push_with(store, branch_id, move |variant| {
        recorder.lock().unwrap().push(variant);
        std::future::ready(Ok(BranchPipelineResponse::running(format!(
            "push-{}",
            recorder.lock().unwrap().len()
        ))))
    })
    .await
    .unwrap();
    let fired = fired.lock().unwrap().clone();
    (started, fired)
}

/// (a) A deferred push does not fire while a branch session is running:
/// the flag stays armed, and the branch going idle is what fires it.
#[tokio::test]
async fn deferred_push_waits_for_the_branchs_running_session() {
    let (store, branch) = setup_branch_store();
    let note_session = {
        start_running_note_session(&store, &branch.id);
        store.get_running_sessions_for_branch(&branch.id).unwrap()[0]
            .id
            .clone()
    };

    arm(&store, &branch.id, PUSH).await;
    let armed = deferred_push_of(&store, &branch.id).expect("armed");
    assert_eq!(PushVariant::of_deferred(&armed), PUSH);
    assert!(armed.requested_at > 0);

    assert_eq!(fire(&store, &branch.id).await, (false, vec![]));
    assert!(
        deferred_push_of(&store, &branch.id).is_some(),
        "still armed"
    );

    end_session(&store, &note_session);
    assert_eq!(fire(&store, &branch.id).await, (true, vec![PUSH]));
    assert_eq!(
        deferred_push_of(&store, &branch.id),
        None,
        "cleared once fired"
    );
    // The push's own terminal state kicks the branch again: nothing left.
    assert_eq!(fire(&store, &branch.id).await, (false, vec![]));
}

/// (b) A running project-note session holds the push too — the branch
/// queries cannot see it — and each of the four variants fires as armed,
/// exactly once, when the project goes idle.
#[tokio::test]
async fn deferred_push_waits_for_the_projects_own_sessions_and_fires_each_variant_once() {
    for variant in [PUSH, FORCE_PUSH, NO_VERIFY_PUSH, NO_VERIFY_FORCE_PUSH] {
        let (store, branch) = setup_branch_store();
        let project_session = start_running_project_note_session(&store, &branch.project_id);
        assert!(!store.has_running_session_for_branch(&branch.id).unwrap());

        arm(&store, &branch.id, variant).await;
        assert_eq!(
            deferred_push_of(&store, &branch.id).map(|d| d.as_column_name()),
            Some(match (variant.force, variant.no_verify) {
                (false, false) => "push",
                (true, false) => "force",
                (false, true) => "push-no-verify",
                (true, true) => "force-no-verify",
            })
        );
        assert_eq!(
            fire(&store, &branch.id).await,
            (false, vec![]),
            "{variant:?}"
        );

        end_session(&store, &project_session);
        assert_eq!(
            fire(&store, &branch.id).await,
            (true, vec![variant]),
            "{variant:?}"
        );
        assert_eq!(deferred_push_of(&store, &branch.id), None);
        assert_eq!(
            fire(&store, &branch.id).await,
            (false, vec![]),
            "{variant:?}"
        );
    }
}

/// A project session that is only queued, or disconnected, holds the push
/// like a running one.
#[tokio::test]
async fn deferred_push_waits_for_queued_and_disconnected_project_sessions() {
    for status in [
        store::SessionStatus::Queued,
        store::SessionStatus::Disconnected,
    ] {
        let (store, branch) = setup_branch_store();
        let project_session = start_running_project_note_session(&store, &branch.project_id);
        store
            .update_session_status(&project_session, status.clone(), None, None)
            .unwrap();

        arm(&store, &branch.id, PUSH).await;
        assert_eq!(
            fire(&store, &branch.id).await,
            (false, vec![]),
            "{status:?}"
        );
    }
}

/// (c) Arming on an idle branch pushes right away instead: the window
/// between the menu rendering and the click would otherwise leave a flag
/// nothing kicks.
#[tokio::test]
async fn deferring_on_an_idle_branch_pushes_immediately_without_arming() {
    let (store, branch) = setup_branch_store();
    let pushed = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&pushed);
    let response = defer_branch_push_with(&store, &branch.id, NO_VERIFY_FORCE_PUSH, {
        move |variant| {
            recorder.lock().unwrap().push(variant);
            std::future::ready(Ok(BranchPipelineResponse::running("push-now".to_string())))
        }
    })
    .await
    .unwrap();

    assert_eq!(
        response,
        DeferredPushResponse {
            armed: false,
            push: Some(BranchPipelineResponse::running("push-now".to_string())),
        }
    );
    assert_eq!(*pushed.lock().unwrap(), vec![NO_VERIFY_FORCE_PUSH]);
    assert_eq!(deferred_push_of(&store, &branch.id), None);
}

/// Run the queued push `session_id` to a deterministic success, as the
/// pipeline runner records one: every step succeeded with a finish time,
/// no agent needed.
fn complete_push_successfully(store: &Arc<Store>, session_id: &str) {
    let session = store.get_session(session_id).unwrap().unwrap();
    let mut pipeline = session.pipeline.expect("a push pipeline");
    for step in &mut pipeline.steps {
        step.status = StepStatus::Succeeded;
        step.completed_at = Some(store::now_timestamp());
    }
    pipeline.completed_without_ai = true;
    store
        .update_session_pipeline(session_id, &pipeline)
        .unwrap();
    end_session(store, session_id);
}

/// Let the millisecond clock move on, so the next write is strictly later
/// than the last: the checks under test compare timestamps and treat a
/// tie as "after".
fn next_millisecond() {
    std::thread::sleep(std::time::Duration::from_millis(2));
}

/// Every push pipeline session on the branch, whatever its state.
fn push_sessions_on(store: &Arc<Store>, branch_id: &str) -> Vec<String> {
    let mut sessions = store.get_busy_sessions_for_branch(branch_id).unwrap();
    sessions.extend(store.get_queued_sessions_for_branch(branch_id).unwrap());
    sessions.extend(
        store
            .get_completed_sessions_for_branch_since(branch_id, 0)
            .unwrap(),
    );
    sessions
        .into_iter()
        .filter(|session| {
            session
                .pipeline
                .as_ref()
                .is_some_and(|pipeline| pipeline.kind.as_ref() == Some(&PipelineKind::Push))
        })
        .map(|session| session.id)
        .collect()
}

/// (d) A queued push of the same variant is work in flight, so the
/// deferred push waits behind it rather than stacking a second push; the
/// queue's own dedupe still folds repeat requests onto the queued row; and
/// once that push has succeeded the deferred one has nothing left to do:
/// it is cleared instead of fired, so one intent ends as one push session.
#[tokio::test]
async fn deferred_push_waits_behind_a_queued_push_instead_of_stacking() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);
    let queued = queue_push(&store, &branch.id, PUSH).expect("queued behind the note");

    arm(&store, &branch.id, PUSH).await;
    assert_eq!(queue_push(&store, &branch.id, PUSH), Some(queued.clone()));

    // The note ends; the queued push is still ahead.
    for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
        end_session(&store, &session.id);
    }
    assert_eq!(fire(&store, &branch.id).await, (false, vec![]));
    assert!(deferred_push_of(&store, &branch.id).is_some());
    assert_eq!(
        store
            .get_queued_sessions_for_branch(&branch.id)
            .unwrap()
            .len(),
        1,
        "no second push row"
    );

    // The queued push runs and succeeds — after the note ended, as the
    // queue guarantees — so the branch is idle and the push the user
    // wanted has happened.
    next_millisecond();
    complete_push_successfully(&store, &queued);
    assert_eq!(fire(&store, &branch.id).await, (false, vec![]));
    assert_eq!(
        deferred_push_of(&store, &branch.id),
        None,
        "cleared by the completed push"
    );
    assert_eq!(
        push_sessions_on(&store, &branch.id),
        vec![queued],
        "exactly one push session"
    );
}

/// A completed push of a *different* variant does not stand in for the
/// deferred one: the user asked for something the queued push did not do.
#[tokio::test]
async fn deferred_push_is_not_satisfied_by_a_completed_push_of_another_variant() {
    for (queued_variant, deferred_variant) in [
        (FORCE_PUSH, PUSH),
        (PUSH, NO_VERIFY_PUSH),
        (NO_VERIFY_PUSH, NO_VERIFY_FORCE_PUSH),
    ] {
        let (store, branch) = setup_branch_store();
        start_running_note_session(&store, &branch.id);
        let queued = queue_push(&store, &branch.id, queued_variant).expect("queued");
        arm(&store, &branch.id, deferred_variant).await;

        for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
            end_session(&store, &session.id);
        }
        complete_push_successfully(&store, &queued);

        assert_eq!(
            fire(&store, &branch.id).await,
            (true, vec![deferred_variant]),
            "{queued_variant:?} completed, {deferred_variant:?} deferred"
        );
        assert_eq!(deferred_push_of(&store, &branch.id), None);
    }
}

/// A same-variant push that did not succeed — cancelled while queued, or
/// completed before the deferred push was armed — leaves the deferred push
/// to fire.
#[tokio::test]
async fn deferred_push_fires_when_the_same_variant_push_was_cancelled_or_predates_it() {
    // Cancelled while queued.
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);
    let queued = queue_push(&store, &branch.id, PUSH).expect("queued");
    arm(&store, &branch.id, PUSH).await;
    for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
        end_session(&store, &session.id);
    }
    store
        .update_session_status(&queued, store::SessionStatus::Cancelled, None, None)
        .unwrap();
    assert_eq!(fire(&store, &branch.id).await, (true, vec![PUSH]));

    // Completed before the arm: the push the user asked for comes after it.
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);
    let earlier = queue_push(&store, &branch.id, PUSH).expect("queued");
    complete_push_successfully(&store, &earlier);
    // Timestamps are in ms: let one pass so the arm is strictly later.
    std::thread::sleep(std::time::Duration::from_millis(2));
    arm(&store, &branch.id, PUSH).await;
    for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
        end_session(&store, &session.id);
    }
    assert_eq!(fire(&store, &branch.id).await, (true, vec![PUSH]));
}

/// The review's counterexample: a same-variant push completes after the
/// arm, then a session queued behind it commits and completes. That push
/// cannot have carried the later commits, so it does not stand in for the
/// deferred push, which fires exactly once when the branch goes idle.
#[tokio::test]
async fn deferred_push_fires_when_a_session_ended_after_the_completed_push() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);
    let queued = queue_push(&store, &branch.id, PUSH).expect("queued behind the note");
    arm(&store, &branch.id, PUSH).await;

    // The first note ends; the queued push runs and succeeds.
    for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
        end_session(&store, &session.id);
    }
    next_millisecond();
    complete_push_successfully(&store, &queued);

    // Before the drain tail looks, another session takes the branch (the
    // commit session that was queued behind the push, say): still blocked.
    start_running_note_session(&store, &branch.id);
    assert_eq!(fire(&store, &branch.id).await, (false, vec![]));
    assert!(deferred_push_of(&store, &branch.id).is_some());

    // It completes after the push did. Its commits are unpushed.
    next_millisecond();
    for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
        end_session(&store, &session.id);
    }
    assert_eq!(
        fire(&store, &branch.id).await,
        (true, vec![PUSH]),
        "the completed push does not cover work that ended after it"
    );
    assert_eq!(deferred_push_of(&store, &branch.id), None);
    assert_eq!(
        fire(&store, &branch.id).await,
        (false, vec![]),
        "fired once: the flag is cleared before the push starts"
    );
}

/// A same-variant push from before the arm whose *row* was rewritten after
/// it — `project_transfer` and `branch_move` bump `updated_at` on every
/// session of a branch — is still an old push: the pipeline's own finish
/// time is what counts, and it predates the arm.
#[tokio::test]
async fn deferred_push_is_not_satisfied_by_an_old_push_whose_row_was_rewritten() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);
    let old_push = queue_push(&store, &branch.id, PUSH).expect("queued");
    complete_push_successfully(&store, &old_push);
    next_millisecond();
    arm(&store, &branch.id, PUSH).await;
    next_millisecond();
    for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
        end_session(&store, &session.id);
    }

    // The transfer's per-row write: `agent_id` and `updated_at` change,
    // the pipeline does not. The old push now looks newer than the arm
    // and than the note that just ended, if judged by `updated_at`.
    next_millisecond();
    store
        .set_agent_session_id(&old_push, "agent-after-transfer")
        .unwrap();
    let rewritten = store.get_session(&old_push).unwrap().unwrap();
    let armed_at = deferred_push_of(&store, &branch.id).unwrap().requested_at;
    assert!(
        rewritten.updated_at > armed_at,
        "the row now postdates the arm"
    );
    assert!(
        rewritten.pipeline.unwrap().finished_at().unwrap() < armed_at,
        "but the pipeline finished before it"
    );

    assert_eq!(fire(&store, &branch.id).await, (true, vec![PUSH]));
    assert_eq!(deferred_push_of(&store, &branch.id), None);
}

/// A push that fails to start leaves the flag armed for the next kick —
/// unless the user re-armed something else meanwhile, or the branch is gone.
#[tokio::test]
async fn deferred_push_that_fails_to_start_stays_armed() {
    let (store, branch) = setup_branch_store();
    let project_session = start_running_project_note_session(&store, &branch.project_id);
    arm(&store, &branch.id, FORCE_PUSH).await;
    end_session(&store, &project_session);

    let err = fire_deferred_push_with(&store, &branch.id, |_| {
        std::future::ready(Err::<BranchPipelineResponse, _>("no remote".to_string()))
    })
    .await
    .unwrap_err();
    assert_eq!(err, "no remote");
    assert_eq!(
        deferred_push_of(&store, &branch.id).map(|d| PushVariant::of_deferred(&d)),
        Some(FORCE_PUSH)
    );

    // Re-armed with another variant while the start was failing: kept.
    let err = fire_deferred_push_with(&store, &branch.id, |_| async {
        store
            .set_branch_deferred_push(
                &branch.id,
                Some(store::DeferredPush {
                    force: false,
                    no_verify: true,
                    requested_at: 1,
                }),
            )
            .unwrap();
        Err::<BranchPipelineResponse, _>("no remote".to_string())
    })
    .await
    .unwrap_err();
    assert_eq!(err, "no remote");
    assert_eq!(
        deferred_push_of(&store, &branch.id).map(|d| PushVariant::of_deferred(&d)),
        Some(NO_VERIFY_PUSH)
    );
}

/// Arming again replaces the variant; cancelling clears it; a branch with
/// nothing armed cancels to nothing.
#[tokio::test]
async fn deferred_push_is_replaced_by_rearming_and_cleared_by_cancel() {
    let (store, branch) = setup_branch_store();
    start_running_note_session(&store, &branch.id);

    arm(&store, &branch.id, PUSH).await;
    arm(&store, &branch.id, NO_VERIFY_FORCE_PUSH).await;
    assert_eq!(
        deferred_push_of(&store, &branch.id).map(|d| PushVariant::of_deferred(&d)),
        Some(NO_VERIFY_FORCE_PUSH)
    );

    cancel_deferred_push_for_branch(&store, &branch.id).unwrap();
    assert_eq!(deferred_push_of(&store, &branch.id), None);
    cancel_deferred_push_for_branch(&store, &branch.id).unwrap();

    // Cancelled: a kick finds nothing to fire even once idle.
    for session in store.get_running_sessions_for_branch(&branch.id).unwrap() {
        end_session(&store, &session.id);
    }
    assert_eq!(fire(&store, &branch.id).await, (false, vec![]));
}

/// The flag round-trips through the store, on create and on update, and
/// lists per project.
#[test]
fn deferred_push_round_trips_through_the_branch_row() {
    let (store, branch) = setup_branch_store();
    let other_project = store::Project::new("other/repo");
    store.create_project(&other_project).unwrap();
    let mut created_armed = store::Branch::new(&other_project.id, "armed-on-create", "main");
    created_armed.deferred_push = Some(store::DeferredPush {
        force: true,
        no_verify: true,
        requested_at: 1234,
    });
    store.create_branch(&created_armed).unwrap();

    assert_eq!(
        store
            .get_branch(&created_armed.id)
            .unwrap()
            .unwrap()
            .deferred_push,
        created_armed.deferred_push
    );
    assert_eq!(deferred_push_of(&store, &branch.id), None);

    let armed = store::DeferredPush {
        force: false,
        no_verify: true,
        requested_at: 99,
    };
    store
        .set_branch_deferred_push(&branch.id, Some(armed))
        .unwrap();
    assert_eq!(deferred_push_of(&store, &branch.id), Some(armed));
    assert_eq!(store.branch_ids_with_deferred_push(None).unwrap().len(), 2);
    assert_eq!(
        store
            .branch_ids_with_deferred_push(Some(&branch.project_id))
            .unwrap(),
        vec![branch.id.clone()]
    );

    store.set_branch_deferred_push(&branch.id, None).unwrap();
    assert_eq!(deferred_push_of(&store, &branch.id), None);
    assert_eq!(
        store
            .branch_ids_with_deferred_push(Some(&branch.project_id))
            .unwrap(),
        Vec::<String>::new()
    );

    for name in ["push", "force", "push-no-verify", "force-no-verify"] {
        let parsed = store::DeferredPush::parse(name, 7).unwrap();
        assert_eq!(parsed.as_column_name(), name);
        assert_eq!(parsed.requested_at, 7);
    }
    assert_eq!(store::DeferredPush::parse("later", 7), None);
}
