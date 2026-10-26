use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tauri::Manager;

use super::kick::{kick, kick_all_with_queued_work, BranchQueueKicker, DrainFn, KickSource};
use super::sweep::{
    is_stranded, queue_health, reconcile_stranded_running_sessions, Liveness, StrandedRow,
    STRANDED_GRACE_MS, STRANDED_SESSION_MESSAGE,
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

fn unregistered() -> Liveness {
    Liveness {
        registered: false,
        deregistered_at: None,
    }
}

#[test]
fn is_stranded_is_a_pure_function_of_now_and_grace() {
    let me = std::process::id();
    let mut row = Session::new_running("work", Path::new("/tmp"));
    row.updated_at = 1_000_000;
    let grace = 600_000;

    // Quiet past the grace, owned by us, nothing registered: stranded.
    assert!(is_stranded(&row, unregistered(), me, 1_600_001, grace));
    // Exactly at the grace is not past it.
    assert!(!is_stranded(&row, unregistered(), me, 1_600_000, grace));
    // A registered token means a thread or task owns the row.
    assert!(!is_stranded(
        &row,
        Liveness {
            registered: true,
            deregistered_at: None
        },
        me,
        9_999_999,
        grace
    ));
    // Deregistered recently: the session thread is in its post-completion
    // hooks, about to write the terminal state itself.
    assert!(!is_stranded(
        &row,
        Liveness {
            registered: false,
            deregistered_at: Some(1_500_000)
        },
        me,
        1_600_001,
        grace
    ));
    assert!(is_stranded(
        &row,
        Liveness {
            registered: false,
            deregistered_at: Some(1_500_000)
        },
        me,
        2_100_001,
        grace
    ));
    // Another process's row is its own to finish.
    assert!(!is_stranded(&row, unregistered(), me + 1, 9_999_999, grace));
    // A row that is not running is never stranded, whatever else is true.
    row.status = SessionStatus::Queued;
    assert!(!is_stranded(&row, unregistered(), me, 9_999_999, grace));
}

/// The deadlock at the state level: a stranded `running` commit keeps a
/// queued note from starting, no drain can move it, and the sweep is what
/// ends the marker and lets the note go.
#[tokio::test]
async fn sweep_ends_an_owned_running_row_with_no_token_after_grace() {
    let (store, branch) = store_with_branch();
    let (stranded, _) = commit_session(&store, &branch.id, SessionStatus::Running);
    let note = note_session(&store, &branch.id, SessionStatus::Queued);

    let kicker = claiming_kicker(&store);
    let app = app_with_kicker(&kicker);
    let registry = SessionRegistry::new();

    kicker.kick(&branch.id, KickSource::Startup);
    kicker.wait_until_idle().await;
    assert_eq!(status_of(&store, &note.id), Some(SessionStatus::Queued));

    let now = stranded.updated_at + STRANDED_GRACE_MS + 1;
    let ended = reconcile_stranded_running_sessions(
        &store,
        &registry,
        app.handle(),
        now,
        STRANDED_GRACE_MS,
    );
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

#[tokio::test]
async fn sweep_leaves_registered_rows_and_other_pids_alone() {
    let (store, branch) = store_with_branch();
    // Every row is long past the grace by its own `updated_at`; what keeps
    // each one alive is something other than the clock.
    let mut registered = session(&branch.id, "registered", SessionStatus::Running);
    registered.updated_at = 0;
    store.create_session(&registered).unwrap();
    let mut theirs = session(&branch.id, "theirs", SessionStatus::Running);
    theirs.owner_pid = Some(std::process::id() + 1);
    theirs.updated_at = 0;
    store.create_session(&theirs).unwrap();
    let mut finishing = session(&branch.id, "finishing", SessionStatus::Running);
    finishing.updated_at = 0;
    store.create_session(&finishing).unwrap();

    let kicker = Arc::new(BranchQueueKicker::recording());
    let app = app_with_kicker(&kicker);
    let registry = Arc::new(SessionRegistry::new());
    let _token = registry.register_external(&registered.id);
    // Deregistered just now: the session thread is between its deregister and
    // its terminal write, however old the row's own `updated_at` is.
    drop(registry.register_external(&finishing.id));

    let now = store::now_timestamp() + 1;
    let ended = reconcile_stranded_running_sessions(
        &store,
        &registry,
        app.handle(),
        now,
        STRANDED_GRACE_MS,
    );
    assert_eq!(ended, Vec::<String>::new());
    assert!(kicker.kicked().is_empty());
    for id in [&registered.id, &theirs.id, &finishing.id] {
        assert_eq!(status_of(&store, id), Some(SessionStatus::Running), "{id}");
    }
}

/// The immediate start's second phase holds an unregistered `running` row
/// while it builds context; the grace is what keeps the sweep off it.
#[tokio::test]
async fn sweep_respects_grace_for_a_fresh_immediate_start() {
    let (store, branch) = store_with_branch();
    let (fresh, _) = commit_session(&store, &branch.id, SessionStatus::Running);

    let kicker = Arc::new(BranchQueueKicker::recording());
    let app = app_with_kicker(&kicker);
    let registry = SessionRegistry::new();

    let now = fresh.updated_at + STRANDED_GRACE_MS - 1;
    let ended = reconcile_stranded_running_sessions(
        &store,
        &registry,
        app.handle(),
        now,
        STRANDED_GRACE_MS,
    );
    assert!(ended.is_empty());
    assert!(kicker.kicked().is_empty());
    assert_eq!(status_of(&store, &fresh.id), Some(SessionStatus::Running));
}

/// The sweep prunes the registry's deregistration stamps it no longer needs,
/// so the map stays bounded over a long-running process.
/// The Doctor readout is the sweep's decision without the write: before the
/// sweep runs a quiet unregistered row is "stranded now", and after it runs
/// the same row is counted as swept instead.
#[tokio::test]
async fn queue_health_reports_what_the_sweep_would_do_and_has_done() {
    let (store, branch) = store_with_branch();
    let (stranded, _) = commit_session(&store, &branch.id, SessionStatus::Running);
    let registered = note_session(&store, &branch.id, SessionStatus::Running);
    let kicker = Arc::new(BranchQueueKicker::recording());
    let app = app_with_kicker(&kicker);
    let registry = Arc::new(SessionRegistry::new());
    let _token = registry.register_external(&registered.id);

    let now = stranded.updated_at + STRANDED_GRACE_MS + 1;
    let before = queue_health(&store, &registry, now, STRANDED_GRACE_MS).unwrap();
    assert_eq!(before.owned_running, 2);
    assert_eq!(before.registered, 1);
    assert_eq!(
        before.stranded,
        vec![StrandedRow {
            session_id: stranded.id.clone(),
            branch_id: Some(branch.id.clone()),
            quiet_secs: (STRANDED_GRACE_MS + 1) / 1000,
        }]
    );
    assert_eq!(before.swept_total, 0);

    reconcile_stranded_running_sessions(&store, &registry, app.handle(), now, STRANDED_GRACE_MS);

    let after = queue_health(&store, &registry, now, STRANDED_GRACE_MS).unwrap();
    assert_eq!(after.owned_running, 1);
    assert_eq!(after.registered, 1);
    assert!(after.stranded.is_empty());
    assert_eq!(after.swept_total, 1);
}

#[test]
fn sweep_prunes_stale_deregistration_stamps() {
    let (store, _branch) = store_with_branch();
    let registry = Arc::new(SessionRegistry::new());
    drop(registry.register_external("gone"));
    assert!(registry.deregistered_at("gone").is_some());

    let app = mock_app();
    let now = store::now_timestamp() + STRANDED_GRACE_MS + 1;
    reconcile_stranded_running_sessions(&store, &registry, app.handle(), now, STRANDED_GRACE_MS);
    assert!(registry.deregistered_at("gone").is_none());
}
