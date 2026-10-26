use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tauri::Manager;

use super::kick::{kick, kick_all_with_queued_work, BranchQueueKicker, DrainFn, KickSource};
use super::retry::{
    queued_start_retry, QueuedStartAttempts, QueuedStartRetry, MAX_QUEUED_START_ATTEMPTS,
    QUEUED_START_RETRY_DELAYS,
};
use super::sweep::{
    end_observed_row, is_stranded, queue_health, reconcile_stranded_running_sessions, StrandedRow,
    STRANDED_SESSION_MESSAGE,
};
use crate::session_commands::claim_startable_queued_sessions;
use crate::session_runner::SessionRegistry;
use crate::store::{self, Session, SessionStatus, Store};

fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("failed to build mock app")
}

fn store_with_branch() -> (Arc<Store>, store::Branch) {
    let store = Arc::new(Store::in_memory().unwrap());
    let project = store::Project::new("test-owner/test-repo");
    store.create_project(&project).unwrap();
    let branch = store::Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    (store, branch)
}

/// A kicker whose drain is the real scheduling scan minus the runner: it
/// claims what the branch's rules would let the drain start.
fn claiming_kicker(store: &Arc<Store>) -> Arc<BranchQueueKicker> {
    let store = Arc::clone(store);
    let drain: DrainFn = Arc::new(move |branch_id: String| {
        let store = Arc::clone(&store);
        Box::pin(async move {
            claim_startable_queued_sessions(&store, &branch_id).map(|claimed| !claimed.is_empty())
        })
    });
    Arc::new(BranchQueueKicker::new(drain))
}

fn app_with_kicker(kicker: &Arc<BranchQueueKicker>) -> tauri::App<tauri::test::MockRuntime> {
    let app = mock_app();
    app.manage(Arc::clone(kicker));
    app
}

fn session(branch_id: &str, prompt: &str, status: SessionStatus) -> Session {
    match status {
        SessionStatus::Queued => Session::new_queued(prompt),
        SessionStatus::Running => Session::new_running(prompt, Path::new("/tmp")),
        other => panic!("unsupported test status: {}", other.as_str()),
    }
    .with_branch(branch_id)
}

fn note_session(store: &Store, branch_id: &str, status: SessionStatus) -> Session {
    let session = session(branch_id, "note", status);
    store.create_session(&session).unwrap();
    let note = store::Note::new(branch_id, "note", "").with_session(&session.id);
    store.create_note(&note).unwrap();
    session
}

fn review_session(store: &Store, branch_id: &str, status: SessionStatus) -> (Session, String) {
    let session = session(branch_id, "review", status);
    store.create_session(&session).unwrap();
    let review = store::Review::new(branch_id, "abc123", store::ReviewScope::Branch)
        .with_session(&session.id);
    store.create_review(&review).unwrap();
    (session, review.id)
}

fn commit_session(store: &Store, branch_id: &str, status: SessionStatus) -> (Session, String) {
    let session = session(branch_id, "commit", status);
    store.create_session(&session).unwrap();
    let commit = store::Commit::new_pending(branch_id).with_session(&session.id);
    store.create_commit(&commit).unwrap();
    (session, commit.id)
}

fn status_of(store: &Store, session_id: &str) -> Option<SessionStatus> {
    store.get_session(session_id).unwrap().map(|s| s.status)
}

/// Link an image to `session`, so the store's cleanup triggers leave the
/// session row in place when its artifact is deleted.
fn attach_image(store: &Store, branch: &store::Branch, session: &Session) {
    let image = store::Image::new(
        Some(&branch.id),
        &branch.project_id,
        "shot.png",
        "image/png",
        1,
        true,
    );
    store.create_image(&image).unwrap();
    store
        .set_images_session_id(std::slice::from_ref(&image.id), &session.id)
        .unwrap();
}

// ---------------------------------------------------------------------------
// Kick coalescing
// ---------------------------------------------------------------------------

/// Kicks that land while a branch's drain is in flight collapse into a single
/// rerun: the drain reads the queue fresh, so one more pass covers every event
/// that arrived during the first.
#[tokio::test]
async fn overlapping_kicks_run_one_more_drain_not_n() {
    let drains = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let drain: DrainFn = {
        let drains = Arc::clone(&drains);
        let gate = Arc::clone(&gate);
        Arc::new(move |_branch_id: String| {
            let drains = Arc::clone(&drains);
            let gate = Arc::clone(&gate);
            Box::pin(async move {
                drains.fetch_add(1, Ordering::SeqCst);
                gate.acquire().await.unwrap().forget();
                Ok(false)
            })
        })
    };
    let kicker = Arc::new(BranchQueueKicker::new(drain));

    for source in [
        KickSource::TerminalState,
        KickSource::ArtifactDeleted,
        KickSource::CancelFallback,
        KickSource::WorktreeCreated,
        KickSource::Startup,
    ] {
        kicker.kick("branch-1", source);
    }
    // A different branch drains independently.
    kicker.kick("branch-2", KickSource::Startup);

    gate.add_permits(16);
    kicker.wait_until_idle().await;

    // branch-1: the first drain plus exactly one rerun; branch-2: one.
    assert_eq!(drains.load(Ordering::SeqCst), 3);
    assert_eq!(kicker.kicked().len(), 6);
}

/// A drain that panics must not take its branch's `in_flight` entry down with
/// it: the next kick for the branch runs a fresh drain instead of being
/// "folded into" one that is no longer running — which would park the queue
/// for good, with no client drain left to paper over it.
#[tokio::test]
async fn a_panicking_drain_frees_its_branch_for_the_next_kick() {
    let drains = Arc::new(AtomicUsize::new(0));
    let drain: DrainFn = {
        let drains = Arc::clone(&drains);
        Arc::new(move |_branch_id: String| {
            let drains = Arc::clone(&drains);
            Box::pin(async move {
                if drains.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("malformed row");
                }
                Ok(false)
            })
        })
    };
    let kicker = Arc::new(BranchQueueKicker::new(drain));

    kicker.kick("branch-1", KickSource::Startup);
    kicker.wait_until_idle().await;
    assert_eq!(drains.load(Ordering::SeqCst), 1);

    kicker.kick("branch-1", KickSource::TerminalState);
    kicker.wait_until_idle().await;
    assert_eq!(
        drains.load(Ordering::SeqCst),
        2,
        "the kick after a panicked drain must run a drain of its own"
    );
}

/// A kick scheduled with a delay runs a drain once the delay has passed —
/// the drain's retry of a failed pre-claim start has no other event to ride.
#[tokio::test]
async fn a_scheduled_kick_fires_after_its_delay() {
    let drains = Arc::new(AtomicUsize::new(0));
    let drain: DrainFn = {
        let drains = Arc::clone(&drains);
        Arc::new(move |_branch_id: String| {
            let drains = Arc::clone(&drains);
            Box::pin(async move {
                drains.fetch_add(1, Ordering::SeqCst);
                Ok(false)
            })
        })
    };
    let kicker = Arc::new(BranchQueueKicker::new(drain));

    kicker.kick_after(
        "branch-1",
        std::time::Duration::from_millis(20),
        KickSource::StartRetry,
    );
    assert_eq!(
        kicker.scheduled(),
        vec![(
            "branch-1".to_string(),
            std::time::Duration::from_millis(20),
            KickSource::StartRetry
        )]
    );
    assert!(
        kicker.kicked().is_empty(),
        "nothing is kicked before the delay"
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while drains.load(Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "scheduled kick never ran"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    kicker.wait_until_idle().await;
    assert_eq!(drains.load(Ordering::SeqCst), 1);
    assert_eq!(
        kicker.kicked(),
        vec![("branch-1".to_string(), KickSource::StartRetry)]
    );
}

/// The retry bound is a pure function of how many starts have failed: one
/// delayed retry per entry in `QUEUED_START_RETRY_DELAYS`, then the row is
/// given up on. The tracker holds a row in backoff until its retry is due,
/// drops a row it gives up on, and forgets rows that left the queue.
#[test]
fn queued_start_attempts_retry_with_growing_delays_then_give_up() {
    assert_eq!(MAX_QUEUED_START_ATTEMPTS, 3);
    assert_eq!(
        queued_start_retry(1),
        QueuedStartRetry::After {
            failures: 1,
            delay: QUEUED_START_RETRY_DELAYS[0]
        }
    );
    assert_eq!(
        queued_start_retry(2),
        QueuedStartRetry::After {
            failures: 2,
            delay: QUEUED_START_RETRY_DELAYS[1]
        }
    );
    assert_eq!(
        queued_start_retry(3),
        QueuedStartRetry::GiveUp { failures: 3 }
    );
    assert!(
        QUEUED_START_RETRY_DELAYS.windows(2).all(|w| w[0] < w[1]),
        "each retry waits longer than the last"
    );

    let attempts = QueuedStartAttempts::default();
    let t0 = std::time::Instant::now();

    assert_eq!(
        attempts.record_failure("b", "s1", t0),
        QueuedStartRetry::After {
            failures: 1,
            delay: QUEUED_START_RETRY_DELAYS[0]
        }
    );
    assert!(attempts.in_backoff("b", "s1", t0));
    assert!(attempts.in_backoff(
        "b",
        "s1",
        t0 + QUEUED_START_RETRY_DELAYS[0] - std::time::Duration::from_millis(1)
    ));
    assert!(!attempts.in_backoff("b", "s1", t0 + QUEUED_START_RETRY_DELAYS[0]));
    assert!(
        !attempts.in_backoff("b", "s2", t0),
        "an untried row is not in backoff"
    );
    assert!(!attempts.in_backoff("other", "s1", t0));

    let t1 = t0 + QUEUED_START_RETRY_DELAYS[0];
    assert_eq!(
        attempts.record_failure("b", "s1", t1),
        QueuedStartRetry::After {
            failures: 2,
            delay: QUEUED_START_RETRY_DELAYS[1]
        }
    );
    assert_eq!(attempts.failures("b", "s1"), 2);
    assert!(attempts.in_backoff("b", "s1", t1 + QUEUED_START_RETRY_DELAYS[0]));
    assert!(!attempts.in_backoff("b", "s1", t1 + QUEUED_START_RETRY_DELAYS[1]));

    // Another row on the branch counts on its own.
    assert_eq!(
        attempts.record_failure("b", "s2", t1),
        QueuedStartRetry::After {
            failures: 1,
            delay: QUEUED_START_RETRY_DELAYS[0]
        }
    );

    // Giving up drops the entry; the row is about to be ended.
    let t2 = t1 + QUEUED_START_RETRY_DELAYS[1];
    assert_eq!(
        attempts.record_failure("b", "s1", t2),
        QueuedStartRetry::GiveUp { failures: 3 }
    );
    assert_eq!(attempts.failures("b", "s1"), 0);
    assert!(!attempts.in_backoff("b", "s1", t2));

    // A row that left the queue (stopped, forced, deleted) is forgotten on
    // the next drain's prune; the branch's map goes with its last row.
    attempts.retain_queued("b", |id| id == "s2");
    assert_eq!(attempts.failures("b", "s2"), 1);
    attempts.retain_queued("b", |_| false);
    assert!(attempts.is_empty());

    attempts.record_failure("gone", "s3", t2);
    attempts.forget_branch("gone");
    assert!(attempts.is_empty());
}

/// A handle with no kicker managed drops the kick rather than panicking: the
/// shared command bodies are driven by mock apps in tests that don't care
/// about the queue.
#[test]
fn kick_without_a_managed_kicker_is_a_no_op() {
    let app = mock_app();
    kick(app.handle(), "branch-1", KickSource::TerminalState);
}

// ---------------------------------------------------------------------------
// Unblock paths
// ---------------------------------------------------------------------------

/// A Stop on a `running` row with no live token — a stranded busy marker —
/// takes the store-write fallback, which used to end the row without moving
/// the branch. The queued note behind it now starts.
#[tokio::test]
async fn cancelling_an_unregistered_running_row_kicks_its_branch() {
    let (store, branch) = store_with_branch();
    let (commit, _) = commit_session(&store, &branch.id, SessionStatus::Running);
    let note = note_session(&store, &branch.id, SessionStatus::Queued);

    let kicker = claiming_kicker(&store);
    let app = app_with_kicker(&kicker);
    let registry = SessionRegistry::new();
    let slot = Mutex::new(Some(Arc::clone(&store)));
    crate::session_commands::cancel_session_impl(&registry, &slot, app.handle(), &commit.id)
        .unwrap();

    assert_eq!(
        kicker.kicked(),
        vec![(branch.id.clone(), KickSource::CancelFallback)]
    );
    kicker.wait_until_idle().await;
    assert_eq!(
        status_of(&store, &commit.id),
        Some(SessionStatus::Cancelled)
    );
    assert_eq!(status_of(&store, &note.id), Some(SessionStatus::Running));
}

/// A queued commit is the FIFO barrier in front of everything queued after
/// it. Deleting it used to leave the notes behind it waiting for a client's
/// follow-up drain; the backend now starts them itself.
#[tokio::test]
async fn deleting_a_queued_barrier_commit_starts_the_notes_behind_it() {
    let (store, branch) = store_with_branch();
    let (commit, commit_id) = commit_session(&store, &branch.id, SessionStatus::Queued);
    let note_1 = note_session(&store, &branch.id, SessionStatus::Queued);
    let note_2 = note_session(&store, &branch.id, SessionStatus::Queued);

    let kicker = claiming_kicker(&store);
    let app = app_with_kicker(&kicker);
    // Nothing moves while the barrier is in place.
    kicker.kick(&branch.id, KickSource::Startup);
    kicker.wait_until_idle().await;
    assert_eq!(status_of(&store, &note_1.id), Some(SessionStatus::Queued));

    let registry = SessionRegistry::new();
    crate::timeline::delete_pending_commit_impl(&store, &registry, app.handle(), &commit_id, true)
        .unwrap();

    assert_eq!(
        kicker.kicked().last(),
        Some(&(branch.id.clone(), KickSource::ArtifactDeleted))
    );
    kicker.wait_until_idle().await;
    assert_eq!(status_of(&store, &commit.id), None);
    assert_eq!(status_of(&store, &note_1.id), Some(SessionStatus::Running));
    assert_eq!(status_of(&store, &note_2.id), Some(SessionStatus::Running));
}

/// "Start now" on a queued commit claims it ahead of the drain. If the start
/// then fails between the claim and the handoff, `fail_claimed_queued_start`
/// ends the row — and, unlike the drain, the forced start has no loop that
/// goes on to the rows behind it, so the failure must kick the branch itself
/// or the note behind the commit waits for an unrelated terminal event.
///
/// The forced start needs the real runtime, so this performs what it does
/// after its precheck: the claim's own write, then the shared failure body.
#[tokio::test]
async fn a_forced_start_that_fails_after_its_claim_starts_the_rows_behind_it() {
    let (store, branch) = store_with_branch();
    let (commit, _) = commit_session(&store, &branch.id, SessionStatus::Queued);
    let note = note_session(&store, &branch.id, SessionStatus::Queued);

    let kicker = claiming_kicker(&store);
    let app = app_with_kicker(&kicker);

    // The forced start's claim: the commit is `running` under our pid and the
    // note is parked behind it.
    assert!(store.transition_queued_to_running(&commit.id).unwrap());
    assert_eq!(status_of(&store, &note.id), Some(SessionStatus::Queued));

    crate::session_commands::fail_claimed_queued_start(
        &store,
        app.handle(),
        &branch.id,
        &branch.project_id,
        &commit.id,
        "database is locked".to_string(),
    );

    // The failure's own kick is the only one; nothing else ended on the branch.
    assert_eq!(
        kicker.kicked(),
        vec![(branch.id.clone(), KickSource::ClaimedStartFailed)]
    );
    kicker.wait_until_idle().await;
    assert_eq!(status_of(&store, &commit.id), Some(SessionStatus::Error));
    assert_eq!(status_of(&store, &note.id), Some(SessionStatus::Running));
}

/// Same for a review in front of a note: one review runs at a time, and a
/// queued review's deletion is what lets a note behind it go.
#[tokio::test]
async fn deleting_a_queued_review_starts_the_note_behind_it() {
    let (store, branch) = store_with_branch();
    let (_, running_review_id) = review_session(&store, &branch.id, SessionStatus::Running);
    let (queued_review, queued_review_id) =
        review_session(&store, &branch.id, SessionStatus::Queued);
    let note = note_session(&store, &branch.id, SessionStatus::Queued);
    let _ = running_review_id;

    let kicker = claiming_kicker(&store);
    let app = app_with_kicker(&kicker);
    crate::timeline::delete_review_impl(&store, app.handle(), &queued_review_id, true).unwrap();

    assert_eq!(
        kicker.kicked(),
        vec![(branch.id.clone(), KickSource::ArtifactDeleted)]
    );
    kicker.wait_until_idle().await;
    assert_eq!(status_of(&store, &queued_review.id), None);
    assert_eq!(status_of(&store, &note.id), Some(SessionStatus::Running));
}

/// Deleting a running note's artifact (and its session) frees the one-review
/// slot's neighbour: a note the user removed mid-run no longer blocks the
/// exclusive commit queued behind it.
#[tokio::test]
async fn deleting_a_running_note_starts_the_commit_behind_it() {
    let (store, branch) = store_with_branch();
    let running_note = note_session(&store, &branch.id, SessionStatus::Running);
    let (commit, _) = commit_session(&store, &branch.id, SessionStatus::Queued);
    let note_id = store
        .get_note_by_session(&running_note.id)
        .unwrap()
        .unwrap()
        .id;

    let kicker = claiming_kicker(&store);
    let app = app_with_kicker(&kicker);
    crate::note_commands::delete_note_impl(&store, app.handle(), &note_id, true).unwrap();

    assert_eq!(
        kicker.kicked(),
        vec![(branch.id.clone(), KickSource::ArtifactDeleted)]
    );
    kicker.wait_until_idle().await;
    assert_eq!(status_of(&store, &running_note.id), None);
    assert_eq!(status_of(&store, &commit.id), Some(SessionStatus::Running));
}

/// Deleting a session row outright resolves its branch before the row — the
/// only link — is gone, and kicks it afterwards.
#[tokio::test]
async fn deleting_a_session_row_kicks_the_branch_it_belonged_to() {
    let (store, branch) = store_with_branch();
    let (commit, _) = commit_session(&store, &branch.id, SessionStatus::Running);
    let note = note_session(&store, &branch.id, SessionStatus::Queued);

    let kicker = claiming_kicker(&store);
    let app = app_with_kicker(&kicker);
    let registry = SessionRegistry::new();
    crate::session_commands::delete_session_impl(&registry, &store, app.handle(), &commit.id)
        .unwrap();

    assert_eq!(
        kicker.kicked(),
        vec![(branch.id.clone(), KickSource::SessionDeleted)]
    );
    kicker.wait_until_idle().await;
    assert_eq!(status_of(&store, &note.id), Some(SessionStatus::Running));
}

/// A queued row whose artifact is gone — deleted without its session, or the
/// session delete failed — keeps its `branch_id`, so the branch's queue still
/// lists it. It used to fail the resolver and abort every drain on the branch
/// for good; now it is stepped over and the rows behind it start.
#[tokio::test]
async fn an_orphaned_queued_row_is_stepped_over_not_a_barrier() {
    let (store, branch) = store_with_branch();
    let orphan = session(&branch.id, "orphan", SessionStatus::Queued);
    store.create_session(&orphan).unwrap();
    let note = note_session(&store, &branch.id, SessionStatus::Queued);

    let kicker = claiming_kicker(&store);
    kicker.kick(&branch.id, KickSource::Startup);
    kicker.wait_until_idle().await;

    assert_eq!(status_of(&store, &orphan.id), Some(SessionStatus::Queued));
    assert_eq!(status_of(&store, &note.id), Some(SessionStatus::Running));
}

/// Deleting an artifact but keeping its session (`deleteSession: false`)
/// could leave a `queued` row with nothing to say what it is. The store's
/// cleanup triggers remove a non-running session with its last artifact, so
/// the row survives only while something else still references it — an
/// attached image — and that is the row the shared delete bodies now cancel,
/// for a commit, a review and a note alike. A `running` row is left to the
/// thread that owns it. With the barriers gone, the note queued behind them
/// starts.
#[tokio::test]
async fn deleting_an_artifact_but_keeping_its_queued_session_cancels_the_row() {
    let (store, branch) = store_with_branch();
    let (review, review_id) = review_session(&store, &branch.id, SessionStatus::Queued);
    let (commit, commit_id) = commit_session(&store, &branch.id, SessionStatus::Queued);
    let queued_note = note_session(&store, &branch.id, SessionStatus::Queued);
    let running_note = note_session(&store, &branch.id, SessionStatus::Running);
    let trailing = note_session(&store, &branch.id, SessionStatus::Queued);
    for session in [&review, &commit, &queued_note] {
        attach_image(&store, &branch, session);
    }
    let note_id_of =
        |session: &Session| store.get_note_by_session(&session.id).unwrap().unwrap().id;
    let queued_note_id = note_id_of(&queued_note);
    let running_note_id = note_id_of(&running_note);

    // No kicker during the deletes: the drain each one kicks off would race
    // the next delete for the queued note, which it is free to start beside
    // the running one.
    let app = mock_app();
    let registry = SessionRegistry::new();
    // The review first: deleting the commit cleans up reviews created after it.
    crate::timeline::delete_review_impl(&store, app.handle(), &review_id, false).unwrap();
    crate::timeline::delete_pending_commit_impl(&store, &registry, app.handle(), &commit_id, false)
        .unwrap();
    crate::note_commands::delete_note_impl(&store, app.handle(), &queued_note_id, false).unwrap();
    crate::note_commands::delete_note_impl(&store, app.handle(), &running_note_id, false).unwrap();

    for (session, what) in [
        (&review, "review"),
        (&commit, "commit"),
        (&queued_note, "note"),
    ] {
        let row = store
            .get_session(&session.id)
            .unwrap()
            .unwrap_or_else(|| panic!("the {what}'s session row is gone"));
        assert_eq!(row.status, SessionStatus::Cancelled, "{what}");
        assert_eq!(
            row.completion_reason,
            Some(store::CompletionReason::Interrupted),
            "{what}"
        );
    }
    assert_eq!(
        status_of(&store, &running_note.id),
        Some(SessionStatus::Running),
        "a running row is its thread's to finish"
    );

    let kicker = claiming_kicker(&store);
    let app = app_with_kicker(&kicker);
    kick(app.handle(), &branch.id, KickSource::ArtifactDeleted);
    kicker.wait_until_idle().await;
    assert_eq!(
        status_of(&store, &trailing.id),
        Some(SessionStatus::Running),
        "cancelled rows are not barriers"
    );
}

/// The trigger path, for contrast: a queued session nothing else references
/// goes with its artifact, so there is no row left to cancel and nothing in
/// the queue either way.
#[test]
fn deleting_an_artifact_whose_queued_session_nothing_else_references_removes_the_row() {
    let (store, branch) = store_with_branch();
    let queued_note = note_session(&store, &branch.id, SessionStatus::Queued);
    let note_id = store
        .get_note_by_session(&queued_note.id)
        .unwrap()
        .unwrap()
        .id;
    let app = mock_app();

    crate::note_commands::delete_note_impl(&store, app.handle(), &note_id, false).unwrap();

    assert_eq!(status_of(&store, &queued_note.id), None);
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// With no client attached, startup still moves every branch that has queued
/// rows — and only those.
#[test]
fn startup_kicks_every_branch_with_queued_rows_without_a_client() {
    let (store, branch_a) = store_with_branch();
    let branch_b = store::Branch::new(&branch_a.project_id, "other", "main");
    store.create_branch(&branch_b).unwrap();
    let idle = store::Branch::new(&branch_a.project_id, "idle", "main");
    store.create_branch(&idle).unwrap();
    let busy_only = store::Branch::new(&branch_a.project_id, "busy", "main");
    store.create_branch(&busy_only).unwrap();

    note_session(&store, &branch_a.id, SessionStatus::Queued);
    note_session(&store, &branch_a.id, SessionStatus::Queued);
    commit_session(&store, &branch_b.id, SessionStatus::Queued);
    note_session(&store, &busy_only.id, SessionStatus::Running);

    let kicker = Arc::new(BranchQueueKicker::recording());
    let app = app_with_kicker(&kicker);
    kick_all_with_queued_work(app.handle(), &store, KickSource::Startup);

    let mut kicked = kicker.kicked_branches();
    kicked.sort();
    let mut expected = vec![branch_a.id.clone(), branch_b.id.clone()];
    expected.sort();
    assert_eq!(kicked, expected);
}

// ---------------------------------------------------------------------------
// Stranded sweep
// ---------------------------------------------------------------------------

#[test]
fn is_stranded_is_a_pure_function_of_liveness_and_ownership() {
    let me = std::process::id();
    let mut row = Session::new_running("work", Path::new("/tmp"));
    // The row's age plays no part: this is as old as a row can be.
    row.updated_at = 0;

    // Owned by us, nothing in the registry owns it: stranded, however fresh.
    assert!(is_stranded(&row, false, me));
    // Live in the registry means a thread or task owns the row, however old.
    assert!(!is_stranded(&row, true, me));
    // Another process's row is its own to finish.
    assert!(!is_stranded(&row, false, me + 1));
    // A row that is not running is never stranded, whatever else is true.
    row.status = SessionStatus::Queued;
    assert!(!is_stranded(&row, false, me));
}

/// The deadlock at the state level: a stranded `running` commit keeps a
/// queued note from starting, no drain can move it, and the sweep is what
/// ends the marker and lets the note go.
#[tokio::test]
async fn sweep_ends_an_owned_running_row_nothing_owns() {
    let (store, branch) = store_with_branch();
    let (stranded, _) = commit_session(&store, &branch.id, SessionStatus::Running);
    let note = note_session(&store, &branch.id, SessionStatus::Queued);

    let kicker = claiming_kicker(&store);
    let app = app_with_kicker(&kicker);
    let registry = SessionRegistry::new();

    kicker.kick(&branch.id, KickSource::Startup);
    kicker.wait_until_idle().await;
    assert_eq!(status_of(&store, &note.id), Some(SessionStatus::Queued));

    let ended = reconcile_stranded_running_sessions(&store, &registry, app.handle());
    assert_eq!(ended, vec![stranded.id.clone()]);

    let row = store.get_session(&stranded.id).unwrap().unwrap();
    assert_eq!(row.status, SessionStatus::Error);
    assert_eq!(row.error_message.as_deref(), Some(STRANDED_SESSION_MESSAGE));
    assert_eq!(
        row.completion_reason,
        Some(store::CompletionReason::Crashed)
    );

    assert_eq!(
        kicker.kicked().last(),
        Some(&(branch.id.clone(), KickSource::Sweep))
    );
    kicker.wait_until_idle().await;
    assert_eq!(status_of(&store, &note.id), Some(SessionStatus::Running));
}

/// The sweep has no clock, so a row something in this process owns is never
/// ended, whatever its age: a registered runner's row and a held row are
/// both older here than any session could be, and both survive. A row that
/// has lost its only owner is ended on the same pass — age is not a factor
/// in either direction.
#[tokio::test]
async fn a_live_row_is_never_swept_however_old() {
    let (store, branch) = store_with_branch();
    let mut registered = session(&branch.id, "registered", SessionStatus::Running);
    registered.updated_at = 0;
    store.create_session(&registered).unwrap();
    let mut held = session(&branch.id, "held", SessionStatus::Running);
    held.updated_at = 0;
    store.create_session(&held).unwrap();
    let mut theirs = session(&branch.id, "theirs", SessionStatus::Running);
    theirs.owner_pid = Some(std::process::id() + 1);
    theirs.updated_at = 0;
    store.create_session(&theirs).unwrap();
    let mut released = session(&branch.id, "released", SessionStatus::Running);
    released.updated_at = 0;
    store.create_session(&released).unwrap();

    let kicker = Arc::new(BranchQueueKicker::recording());
    let app = app_with_kicker(&kicker);
    let registry = Arc::new(SessionRegistry::new());
    let _token = registry.register_external(&registered.id);
    let _hold = registry.hold(&held.id);
    // Had a hold, and let it go without writing a terminal state: that is the
    // one way a row this process wrote can be stranded.
    drop(registry.hold(&released.id));

    let ended = reconcile_stranded_running_sessions(&store, &registry, app.handle());
    assert_eq!(ended, vec![released.id.clone()]);
    assert_eq!(
        kicker.kicked(),
        vec![(branch.id.clone(), KickSource::Sweep)]
    );
    for id in [&registered.id, &held.id, &theirs.id] {
        assert_eq!(status_of(&store, id), Some(SessionStatus::Running), "{id}");
    }
    assert_eq!(status_of(&store, &released.id), Some(SessionStatus::Error));
}

/// The one interleaving `status = 'running'` alone could not tell apart: the
/// sweep reads a row as `running`, its thread writes the terminal state and
/// deregisters, and a Resume makes the same id `running` again before the
/// sweep's write. The write is keyed on the `updated_at` the sweep saw, so
/// the resumed turn is left alone — and a row unchanged since it was read is
/// still ended.
#[test]
fn the_sweep_ends_only_the_running_incarnation_it_observed() {
    let (store, branch) = store_with_branch();
    let mut row = session(&branch.id, "turn", SessionStatus::Running);
    row.updated_at = 1;
    store.create_session(&row).unwrap();
    let observed = store.get_session(&row.id).unwrap().unwrap();

    // The thread finishes the turn the sweep saw...
    assert!(store
        .transition_from_running(
            &row.id,
            SessionStatus::Completed,
            None,
            Some(&store::CompletionReason::TurnComplete),
        )
        .unwrap());
    // ...and a Resume wins the row before the sweep writes.
    assert!(store.transition_to_running(&row.id).unwrap());

    assert!(
        !end_observed_row(&store, &observed),
        "the resumed turn is not the row the sweep observed"
    );
    let resumed = store.get_session(&row.id).unwrap().unwrap();
    assert_eq!(resumed.status, SessionStatus::Running);
    assert_eq!(resumed.error_message, None);

    // Read again and unchanged since: that is a stranded row, and it is ended.
    assert!(end_observed_row(&store, &resumed));
    let ended = store.get_session(&row.id).unwrap().unwrap();
    assert_eq!(ended.status, SessionStatus::Error);
    assert_eq!(
        ended.error_message.as_deref(),
        Some(STRANDED_SESSION_MESSAGE)
    );
}

/// The Doctor readout is the sweep's decision without the write: before the
/// sweep runs a row nothing owns is "stranded now", and after it runs the
/// same row is counted as swept instead.
#[tokio::test]
async fn queue_health_reports_what_the_sweep_would_do_and_has_done() {
    let (store, branch) = store_with_branch();
    let (stranded, _) = commit_session(&store, &branch.id, SessionStatus::Running);
    let registered = note_session(&store, &branch.id, SessionStatus::Running);
    let held = note_session(&store, &branch.id, SessionStatus::Running);
    let kicker = Arc::new(BranchQueueKicker::recording());
    let app = app_with_kicker(&kicker);
    let registry = Arc::new(SessionRegistry::new());
    let _token = registry.register_external(&registered.id);
    let _hold = registry.hold(&held.id);

    let before = queue_health(&store, &registry).unwrap();
    assert_eq!(before.owned_running, 3);
    assert_eq!(before.live, 2);
    assert_eq!(
        before.stranded,
        vec![StrandedRow {
            session_id: stranded.id.clone(),
            branch_id: Some(branch.id.clone()),
        }]
    );
    assert_eq!(before.swept_total, 0);

    reconcile_stranded_running_sessions(&store, &registry, app.handle());

    let after = queue_health(&store, &registry).unwrap();
    assert_eq!(after.owned_running, 2);
    assert_eq!(after.live, 2);
    assert!(after.stranded.is_empty());
    assert_eq!(after.swept_total, 1);
}

/// What deleting a branch does to its sessions before the cascade: a queued
/// row is cancelled, a running row with a token is signalled and waited for
/// until its thread has written `cancelled` and deregistered, a held row is
/// waited out, and a row nothing here owns is cancelled in the store. Every
/// row is terminal by the time the branch row is deleted.
#[test]
fn deleting_a_branch_cancels_its_queued_rows_and_signals_its_running_ones() {
    use crate::session_queue::teardown::{signal_branch_sessions, stop_branch_sessions};

    let (store, branch) = store_with_branch();
    let registered = note_session(&store, &branch.id, SessionStatus::Running);
    let held = note_session(&store, &branch.id, SessionStatus::Running);
    let orphan = note_session(&store, &branch.id, SessionStatus::Running);
    let queued = note_session(&store, &branch.id, SessionStatus::Queued);

    let app = mock_app();
    let registry = Arc::new(SessionRegistry::new());

    // The runner thread: waits on its token, records the cancel, deregisters.
    let registration = registry.register_external(&registered.id);
    let thread = std::thread::spawn({
        let store = Arc::clone(&store);
        let session_id = registered.id.clone();
        move || {
            while !registration.token().is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(store
                .transition_from_running(
                    &session_id,
                    SessionStatus::Cancelled,
                    None,
                    Some(&store::CompletionReason::Interrupted),
                )
                .unwrap());
            drop(registration);
        }
    });
    let hold = registry.hold(&held.id);

    let mut owned =
        signal_branch_sessions(&store, &registry, app.handle(), &[branch.clone()]).unwrap();
    owned.sort();
    let mut expected = vec![registered.id.clone(), held.id.clone()];
    expected.sort();
    assert_eq!(owned, expected);
    assert_eq!(
        status_of(&store, &queued.id),
        Some(SessionStatus::Cancelled)
    );
    assert_eq!(
        status_of(&store, &orphan.id),
        Some(SessionStatus::Cancelled)
    );

    // The held row's owner finishes it, then lets go.
    assert!(store
        .transition_from_running(&held.id, SessionStatus::Cancelled, None, None)
        .unwrap());
    drop(hold);

    tauri::async_runtime::block_on(stop_branch_sessions(
        &store,
        &registry,
        app.handle(),
        &[branch.clone()],
    ))
    .unwrap();
    thread.join().unwrap();
    assert!(!registry.is_live(&registered.id));
    for id in [&registered.id, &held.id, &orphan.id, &queued.id] {
        assert_eq!(
            status_of(&store, id),
            Some(SessionStatus::Cancelled),
            "{id}"
        );
    }

    store.delete_branch(&branch.id).unwrap();
    for id in [&registered.id, &held.id, &orphan.id, &queued.id] {
        assert_eq!(status_of(&store, id), None, "{id}");
    }
}

/// The hazard `stop_branch_sessions` exists for: `sessions.branch_id`
/// cascades, so a branch delete removes a `running` row out from under its
/// runner. The artifact cleanup triggers spare running rows; this FK does not.
#[test]
fn a_branch_delete_cascades_to_its_running_session_rows() {
    let (store, branch) = store_with_branch();
    let running = note_session(&store, &branch.id, SessionStatus::Running);

    store.delete_branch(&branch.id).unwrap();
    assert_eq!(status_of(&store, &running.id), None);
}
