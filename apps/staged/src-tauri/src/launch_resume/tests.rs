use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use super::plan::{
    action_for, plan_launch_resume, CandidateKind, LaunchAction, LaunchCandidate,
    PIPELINE_NOT_RESUMED_REASON, PROJECT_NEVER_STARTED_REASON, PROJECT_SESSION_RESUME_PROMPT,
};
use super::run::{run_project_phase, run_repo_phase, wait_until_settled, PhaseOutcome};
use super::{build_recovery, handled_through_from_preferences, LaunchResumeState};
use crate::session_commands::{
    claim_startable_queued_sessions, queued_start_kind, resume_pre_head_within, ClaimedResume,
    QueuedStartKind,
};
use crate::session_runner::SessionRegistry;
use crate::store::{self, CompletionReason, Session, SessionStatus, Store};

fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("failed to build mock app")
}

/// A project with one local branch that has a worktree, so the resume gate's
/// provisioning check passes and `/tmp` is the working directory.
fn store_with_branch() -> (Arc<Store>, store::Project, store::Branch) {
    let store = Arc::new(Store::in_memory().unwrap());
    let mut project = store::Project::new("owner/widgets");
    project.name = "Widgets".to_string();
    store.create_project(&project).unwrap();
    let branch = store::Branch::new(&project.id, "fix-login", "main");
    store.create_branch(&branch).unwrap();
    let workdir = store::Workdir::new(&project.id, "/tmp").with_branch(&branch.id);
    store.create_workdir(&workdir).unwrap();
    (store, project, branch)
}

fn end_with_app_quit(store: &Store, session_id: &str, status: SessionStatus) {
    assert!(store
        .transition_from_running(session_id, status, None, Some(&CompletionReason::AppQuit))
        .unwrap());
}

/// A commit session on `branch` the quit cancelled while running, created at
/// `created_at`, with or without an ACP conversation to load.
fn interrupted_commit(
    store: &Store,
    branch_id: &str,
    created_at: i64,
    agent_id: Option<&str>,
) -> Session {
    let mut session = Session::new_running("commit", Path::new("/tmp")).with_branch(branch_id);
    session.created_at = created_at;
    if let Some(agent_id) = agent_id {
        session = session.with_agent(agent_id);
    }
    store.create_session(&session).unwrap();
    let commit = store::Commit::new_pending(branch_id).with_session(&session.id);
    store.create_commit(&commit).unwrap();
    end_with_app_quit(store, &session.id, SessionStatus::Cancelled);
    session
}

/// A commit session the quit sweep cancelled while it was still queued: no
/// agent, no working dir, bare instructions.
fn interrupted_queued_commit(store: &Store, branch_id: &str, created_at: i64) -> Session {
    let mut session = Session::new_queued("commit").with_branch(branch_id);
    session.created_at = created_at;
    store.create_session(&session).unwrap();
    let commit = store::Commit::new_pending(branch_id).with_session(&session.id);
    store.create_commit(&commit).unwrap();
    assert!(store
        .transition_from_queued(
            &session.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::AppQuit),
        )
        .unwrap());
    session
}

fn interrupted_project_session(
    store: &Store,
    project_id: &str,
    created_at: i64,
    agent_id: Option<&str>,
) -> Session {
    let mut session = Session::new_running("plan", Path::new("/tmp"));
    session.created_at = created_at;
    if let Some(agent_id) = agent_id {
        session = session.with_agent(agent_id);
    }
    store.create_session(&session).unwrap();
    let note = store::ProjectNote::new(project_id, "Plan", "").with_session(&session.id);
    store.create_project_note(&note).unwrap();
    end_with_app_quit(store, &session.id, SessionStatus::Cancelled);
    session
}

fn status_of(store: &Store, session_id: &str) -> SessionStatus {
    store.get_session(session_id).unwrap().unwrap().status
}

fn complete(store: &Store, session_id: &str) {
    assert!(store
        .transition_from_running(
            session_id,
            SessionStatus::Completed,
            None,
            Some(&CompletionReason::TurnComplete),
        )
        .unwrap());
}

fn candidate(
    id: &str,
    branch: Option<&str>,
    created_at: i64,
    kind: CandidateKind,
    agent: bool,
) -> LaunchCandidate {
    LaunchCandidate {
        session_id: id.to_string(),
        branch_id: branch.map(str::to_string),
        created_at,
        kind,
        agent_resumable: agent,
    }
}

const REPO: CandidateKind = CandidateKind::Repo { pipeline: false };
const PIPELINE: CandidateKind = CandidateKind::Repo { pipeline: true };

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

#[test]
fn action_depends_on_kind_and_conversation() {
    assert_eq!(
        action_for(&candidate("a", Some("b"), 1, REPO, true)),
        LaunchAction::Resume
    );
    assert_eq!(
        action_for(&candidate("a", Some("b"), 1, REPO, false)),
        LaunchAction::Requeue
    );
    assert_eq!(
        action_for(&candidate("a", Some("b"), 1, PIPELINE, true)),
        LaunchAction::Skip {
            reason: PIPELINE_NOT_RESUMED_REASON
        }
    );
    assert_eq!(
        action_for(&candidate("a", None, 1, CandidateKind::Project, true)),
        LaunchAction::Resume
    );
    assert_eq!(
        action_for(&candidate("a", None, 1, CandidateKind::Project, false)),
        LaunchAction::Skip {
            reason: PROJECT_NEVER_STARTED_REASON
        }
    );
    assert!(matches!(
        action_for(&candidate("a", None, 1, CandidateKind::Internal, true)),
        LaunchAction::Skip { .. }
    ));
}

/// A resumed project session may continue an `app_quit` repo session only
/// toward that session's own goal, never to hand it the next piece of work.
#[test]
fn project_resume_prompt_scopes_continue_to_the_sessions_own_goal() {
    assert!(PROJECT_SESSION_RESUME_PROMPT
        .contains("with `continue_repo_session` to finish its own goal, or started again"));
}

/// Repo sessions come first, grouped by branch in `created_at` order; project
/// sessions follow in `created_at` order; internal rows are dropped.
#[test]
fn plan_orders_repo_by_branch_then_created_at_and_project_after() {
    let plan = plan_launch_resume(vec![
        candidate("p2", None, 50, CandidateKind::Project, true),
        candidate("b-late", Some("branch-b"), 40, REPO, false),
        candidate("a-late", Some("branch-a"), 30, REPO, true),
        candidate("pikchr", None, 5, CandidateKind::Internal, true),
        candidate("a-early", Some("branch-a"), 10, REPO, false),
        candidate("p1", None, 20, CandidateKind::Project, false),
        candidate("b-early", Some("branch-b"), 35, PIPELINE, true),
    ]);

    let repo: Vec<(&str, &LaunchAction)> = plan
        .repo
        .iter()
        .map(|p| (p.session_id.as_str(), &p.action))
        .collect();
    assert_eq!(
        repo,
        vec![
            ("a-early", &LaunchAction::Requeue),
            ("a-late", &LaunchAction::Resume),
            (
                "b-early",
                &LaunchAction::Skip {
                    reason: PIPELINE_NOT_RESUMED_REASON
                }
            ),
            ("b-late", &LaunchAction::Requeue),
        ]
    );
    let project: Vec<&str> = plan.project.iter().map(|p| p.session_id.as_str()).collect();
    assert_eq!(project, vec!["p1", "p2"]);
    assert_eq!(
        plan.project[0].action,
        LaunchAction::Skip {
            reason: PROJECT_NEVER_STARTED_REASON
        }
    );
    assert_eq!(plan.project[1].action, LaunchAction::Resume);
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

#[test]
fn build_recovery_partitions_labels_and_excludes_internal_rows() {
    let (store, project, branch) = store_with_branch();
    let repo = interrupted_commit(&store, &branch.id, 10, Some("acp-1"));
    let queued = interrupted_queued_commit(&store, &branch.id, 20);
    let project_session = interrupted_project_session(&store, &project.id, 30, Some("acp-2"));
    let project_never_started = interrupted_project_session(&store, &project.id, 40, None);

    // A pipeline row: listed, not resumable automatically.
    let mut push = Session::new_running("Push the current branch to the remote", Path::new("/tmp"))
        .with_branch(&branch.id)
        .with_agent("acp-3");
    push.pipeline = Some(store::PipelineExecution::from_steps(&[]));
    store.create_session(&push).unwrap();
    end_with_app_quit(&store, &push.id, SessionStatus::Cancelled);

    // A Pikchr child: no branch, no project note. Never offered.
    let pikchr = Session::new_running("diagram", Path::new("/tmp")).with_agent("acp-4");
    store.create_session(&pikchr).unwrap();
    end_with_app_quit(&store, &pikchr.id, SessionStatus::Cancelled);

    // Ended for another reason: not offered.
    let stopped = interrupted_commit(&store, &branch.id, 50, Some("acp-5"));
    store.transition_app_quit_to_queued(&stopped.id).unwrap();
    assert!(store.transition_queued_to_running(&stopped.id).unwrap());
    assert!(store
        .transition_from_running(
            &stopped.id,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::Interrupted),
        )
        .unwrap());

    let (recovery, newest) = build_recovery(&store, 0)
        .unwrap()
        .expect("something to offer");

    let repo_ids: Vec<&str> = recovery
        .repo_sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(
        repo_ids,
        vec![repo.id.as_str(), queued.id.as_str(), push.id.as_str()]
    );
    assert_eq!(recovery.repo_sessions[0].disposition, "resume");
    assert_eq!(recovery.repo_sessions[0].label, "commit on fix-login");
    assert_eq!(
        recovery.repo_sessions[0].branch_name.as_deref(),
        Some("fix-login")
    );
    assert_eq!(
        recovery.repo_sessions[0].project_name.as_deref(),
        Some("Widgets")
    );
    assert_eq!(recovery.repo_sessions[1].disposition, "restart");
    assert_eq!(recovery.repo_sessions[2].disposition, "manual");
    assert_eq!(
        recovery.repo_sessions[2].reason,
        Some(PIPELINE_NOT_RESUMED_REASON)
    );
    assert_eq!(
        recovery.repo_sessions[2].session_type.as_deref(),
        Some("push")
    );

    let project_ids: Vec<&str> = recovery
        .project_sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert_eq!(
        project_ids,
        vec![
            project_session.id.as_str(),
            project_never_started.id.as_str()
        ]
    );
    assert_eq!(recovery.project_sessions[0].disposition, "resume");
    assert_eq!(recovery.project_sessions[0].label, "note on Widgets");
    assert_eq!(recovery.project_sessions[1].disposition, "manual");
    assert_eq!(
        recovery.project_sessions[1].reason,
        Some(PROJECT_NEVER_STARTED_REASON)
    );

    assert!(!recovery
        .resumable_ids()
        .iter()
        .any(|id| id == &push.id || id == &project_never_started.id));
    assert_eq!(recovery.resumable_ids().len(), 3);

    // The marker is the newest row offered; nothing is offered past it.
    let max_updated = recovery
        .repo_sessions
        .iter()
        .chain(recovery.project_sessions.iter())
        .map(|s| s.interrupted_at)
        .max()
        .unwrap();
    assert_eq!(newest, max_updated);
    assert!(build_recovery(&store, newest).unwrap().is_none());
}

#[test]
fn handled_marker_reads_missing_or_malformed_as_zero() {
    assert_eq!(handled_through_from_preferences(&serde_json::json!({})), 0);
    assert_eq!(
        handled_through_from_preferences(
            &serde_json::json!({ "launch-resume-handled-through": "x" })
        ),
        0
    );
    assert_eq!(
        handled_through_from_preferences(
            &serde_json::json!({ "launch-resume-handled-through": 1234 })
        ),
        1234
    );
    assert_eq!(
        handled_through_from_preferences(
            &serde_json::json!({ "launch-resume-handled-through": 1234.0 })
        ),
        1234
    );
}

/// The offer stays until it is answered, so a main-window reload between
/// `load()` and the user's answer asks again and sees the same set. The
/// answer clears it and takes the marker exactly once; a second answer has
/// nothing to record.
#[test]
fn state_holds_the_offer_until_it_is_answered() {
    let app = mock_app();
    app.manage(LaunchResumeState::default());
    let (store, _project, branch) = store_with_branch();
    interrupted_commit(&store, &branch.id, 10, Some("acp-1"));

    super::detect_at_launch(app.handle(), &store);
    let first = super::get_interrupted_sessions(app.state()).expect("an offer");
    let after_reload =
        super::get_interrupted_sessions(app.state()).expect("the offer again after a reload");
    assert_eq!(first, after_reload);

    let state = app.state::<LaunchResumeState>();
    let marker = state.take_handled().expect("the marker, once");
    assert_eq!(marker, first.repo_sessions[0].interrupted_at);
    assert!(super::get_interrupted_sessions(app.state()).is_none());
    assert!(state.take_handled().is_none());
}

fn arm_deferred_push(store: &Store, branch_id: &str) {
    store
        .set_branch_deferred_push(
            branch_id,
            Some(store::DeferredPush {
                force: false,
                no_verify: false,
                requested_at: 1,
            }),
        )
        .unwrap();
}

/// The startup pass over armed deferred pushes skips a branch the offer may
/// put work back on — one with an interrupted session of its own, or in a
/// project with an interrupted project session — and kicks the rest. The
/// held branches are kicked when the offer is answered, whichever way.
#[test]
fn launch_holds_deferred_pushes_the_offer_names_and_kicks_the_rest() {
    let (store, project, branch) = store_with_branch();
    let sibling = store::Branch::new(&project.id, "sibling", "main");
    store.create_branch(&sibling).unwrap();
    let other_project = store::Project::new("other/repo");
    store.create_project(&other_project).unwrap();
    let elsewhere = store::Branch::new(&other_project.id, "elsewhere", "main");
    store.create_branch(&elsewhere).unwrap();
    for id in [&branch.id, &sibling.id, &elsewhere.id] {
        arm_deferred_push(&store, id);
    }
    interrupted_commit(&store, &branch.id, 10, Some("acp-1"));

    let kicker = Arc::new(crate::session_queue::BranchQueueKicker::recording());
    let app = mock_app();
    app.manage(Arc::clone(&kicker));
    app.manage(LaunchResumeState::default());
    super::detect_at_launch(app.handle(), &store);
    super::kick_deferred_pushes_at_launch(app.handle(), &store);

    // Only the interrupted commit's own branch waits; its sibling is free.
    let mut kicked = kicker.kicked_branches();
    kicked.sort();
    let mut expected = vec![sibling.id.clone(), elsewhere.id.clone()];
    expected.sort();
    assert_eq!(kicked, expected);
    let state = app.state::<LaunchResumeState>();
    assert_eq!(state.deferred_push_holds(), vec![branch.id.clone()]);
    assert_eq!(state.take_deferred_push_holds(), vec![branch.id.clone()]);
    assert!(state.take_deferred_push_holds().is_empty());

    // An interrupted project session holds every armed branch of its project.
    let (store, project, branch) = store_with_branch();
    let sibling = store::Branch::new(&project.id, "sibling", "main");
    store.create_branch(&sibling).unwrap();
    let other_project = store::Project::new("other/repo");
    store.create_project(&other_project).unwrap();
    let elsewhere = store::Branch::new(&other_project.id, "elsewhere", "main");
    store.create_branch(&elsewhere).unwrap();
    for id in [&branch.id, &sibling.id, &elsewhere.id] {
        arm_deferred_push(&store, id);
    }
    interrupted_project_session(&store, &project.id, 10, Some("acp-2"));

    let kicker = Arc::new(crate::session_queue::BranchQueueKicker::recording());
    let app = mock_app();
    app.manage(Arc::clone(&kicker));
    app.manage(LaunchResumeState::default());
    super::detect_at_launch(app.handle(), &store);
    super::kick_deferred_pushes_at_launch(app.handle(), &store);

    assert_eq!(kicker.kicked_branches(), vec![elsewhere.id.clone()]);
    let mut held = app.state::<LaunchResumeState>().deferred_push_holds();
    held.sort();
    let mut expected = vec![branch.id.clone(), sibling.id.clone()];
    expected.sort();
    assert_eq!(held, expected);
}

/// With nothing interrupted there is no offer to wait on: every armed branch
/// is kicked at startup and nothing is held.
#[test]
fn launch_kicks_every_deferred_push_when_nothing_was_interrupted() {
    let (store, _project, branch) = store_with_branch();
    arm_deferred_push(&store, &branch.id);

    let kicker = Arc::new(crate::session_queue::BranchQueueKicker::recording());
    let app = mock_app();
    app.manage(Arc::clone(&kicker));
    app.manage(LaunchResumeState::default());
    super::detect_at_launch(app.handle(), &store);
    super::kick_deferred_pushes_at_launch(app.handle(), &store);

    assert_eq!(
        kicker.kicked(),
        vec![(branch.id.clone(), crate::session_queue::KickSource::Startup)]
    );
    assert!(app
        .state::<LaunchResumeState>()
        .deferred_push_holds()
        .is_empty());
}

/// A launch with nothing interrupted offers nothing and has no marker.
#[test]
fn state_is_empty_when_nothing_was_interrupted() {
    let app = mock_app();
    app.manage(LaunchResumeState::default());
    let (store, _project, _branch) = store_with_branch();

    super::detect_at_launch(app.handle(), &store);
    assert!(super::get_interrupted_sessions(app.state()).is_none());
    assert!(app.state::<LaunchResumeState>().take_handled().is_none());
}

// ---------------------------------------------------------------------------
// Phase 1 ordering
// ---------------------------------------------------------------------------

async fn no_op_resume(_claim: ClaimedResume) -> Result<(), String> {
    Ok(())
}

/// Three rows the quit sweep cancelled while queued, behind a session with a
/// conversation to resume: the resumable one is claimed `running`, the three
/// go back on the queue in their original order, and the drain starts them in
/// that order once the running one finishes.
#[tokio::test]
async fn requeued_rows_drain_in_original_order_behind_a_resumed_session() {
    let (store, _project, branch) = store_with_branch();
    let registry = Arc::new(SessionRegistry::new());
    let app = mock_app();

    let resumable = interrupted_commit(&store, &branch.id, 100, Some("acp-1"));
    let first = interrupted_queued_commit(&store, &branch.id, 200);
    let second = interrupted_queued_commit(&store, &branch.id, 300);
    let third = interrupted_queued_commit(&store, &branch.id, 400);

    // Offered newest-first on purpose: the plan sorts.
    let candidates: Vec<LaunchCandidate> = [&third, &first, &resumable, &second]
        .iter()
        .map(|s| {
            super::plan::classify_candidate(
                &store,
                store.get_session(&s.id).unwrap().as_ref().unwrap(),
            )
        })
        .collect();
    let plan = plan_launch_resume(candidates);
    let order: Vec<&str> = plan.repo.iter().map(|p| p.session_id.as_str()).collect();
    assert_eq!(order, vec![&resumable.id, &first.id, &second.id, &third.id]);

    let outcome = run_repo_phase(&store, &registry, app.handle(), &plan.repo, no_op_resume).await;
    assert_eq!(outcome.resumed, vec![resumable.id.clone()]);
    assert_eq!(
        outcome.requeued,
        vec![first.id.clone(), second.id.clone(), third.id.clone()]
    );
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);

    assert_eq!(status_of(&store, &resumable.id), SessionStatus::Running);
    let queued: Vec<String> = store
        .get_queued_sessions_for_branch(&branch.id)
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(
        queued,
        vec![first.id.clone(), second.id.clone(), third.id.clone()]
    );
    for id in [&first.id, &second.id, &third.id] {
        let row = store.get_session(id).unwrap().unwrap();
        assert_eq!(row.completion_reason, None);
        assert_eq!(row.owner_pid, None);
    }

    // The drain: nothing starts while the resumed commit runs, then one at a
    // time in enqueue order.
    assert!(claim_startable_queued_sessions(&store, &branch.id)
        .unwrap()
        .is_empty());
    complete(&store, &resumable.id);
    assert_eq!(
        claim_startable_queued_sessions(&store, &branch.id).unwrap(),
        vec![first.id.clone()]
    );
    complete(&store, &first.id);
    assert_eq!(
        claim_startable_queued_sessions(&store, &branch.id).unwrap(),
        vec![second.id.clone()]
    );
    complete(&store, &second.id);
    assert_eq!(
        claim_startable_queued_sessions(&store, &branch.id).unwrap(),
        vec![third.id.clone()]
    );
}

/// A resumable session created *between* cancelled-at-quit rows is blocked
/// by the row re-queued ahead of it and falls back to the queue itself, so
/// the branch drains in the original order rather than jumping it ahead.
#[tokio::test]
async fn resumable_session_behind_requeued_rows_keeps_its_place() {
    let (store, _project, branch) = store_with_branch();
    let registry = Arc::new(SessionRegistry::new());
    let app = mock_app();

    let first = interrupted_queued_commit(&store, &branch.id, 100);
    let resumable = interrupted_commit(&store, &branch.id, 200, Some("acp-1"));
    let second = interrupted_queued_commit(&store, &branch.id, 300);
    let third = interrupted_queued_commit(&store, &branch.id, 400);

    let candidates: Vec<LaunchCandidate> = [&first, &resumable, &second, &third]
        .iter()
        .map(|s| {
            super::plan::classify_candidate(
                &store,
                store.get_session(&s.id).unwrap().as_ref().unwrap(),
            )
        })
        .collect();
    let plan = plan_launch_resume(candidates);

    let outcome = run_repo_phase(&store, &registry, app.handle(), &plan.repo, no_op_resume).await;
    assert!(outcome.resumed.is_empty());
    assert_eq!(
        outcome.requeued,
        vec![
            first.id.clone(),
            resumable.id.clone(),
            second.id.clone(),
            third.id.clone()
        ]
    );

    let queued: Vec<String> = store
        .get_queued_sessions_for_branch(&branch.id)
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(
        queued,
        vec![
            first.id.clone(),
            resumable.id.clone(),
            second.id.clone(),
            third.id.clone()
        ]
    );

    // The re-queue keeps the conversation: the row the dialog promised to
    // `resume` is started by the drain as a resume of `acp-1`, not from the
    // original prompt, while the rows that never ran start fresh.
    let requeued = store.get_session(&resumable.id).unwrap().unwrap();
    assert_eq!(requeued.status, SessionStatus::Queued);
    assert_eq!(requeued.agent_id.as_deref(), Some("acp-1"));
    assert_eq!(
        queued_start_kind(&requeued),
        QueuedStartKind::Resume {
            agent_session_id: "acp-1".to_string()
        }
    );
    for id in [&first.id, &second.id, &third.id] {
        let row = store.get_session(id).unwrap().unwrap();
        assert_eq!(queued_start_kind(&row), QueuedStartKind::Fresh);
    }

    assert_eq!(
        claim_startable_queued_sessions(&store, &branch.id).unwrap(),
        vec![first.id.clone()]
    );
    complete(&store, &first.id);
    assert_eq!(
        claim_startable_queued_sessions(&store, &branch.id).unwrap(),
        vec![resumable.id.clone()]
    );
    // The claim is the drain's `transition_queued_to_running`, which leaves
    // the agent id for `load_session`.
    let claimed = store.get_session(&resumable.id).unwrap().unwrap();
    assert_eq!(claimed.status, SessionStatus::Running);
    assert_eq!(claimed.agent_id.as_deref(), Some("acp-1"));
}

/// A resume whose pre-run HEAD read hangs is bounded by `RESUME_HEAD_READ_CAP`
/// inside `prepare_resume`, and the bound degrades rather than fails: the
/// read is dropped, the snapshot is `None` (the commit completion hook has
/// nothing to compare against for this run), and the resume goes ahead. The
/// row stays `running` from the claim with nothing recorded as an error, the
/// claim's liveness hold is released when the resume returns (in production
/// the runner's own hold takes over at `start_session`), and the phase moves
/// on to the sessions after it. The rows re-queued behind it on the same
/// branch wait on it as on any running exclusive session and are claimable in
/// order once it completes.
///
/// `resume_claimed_session_for_store` needs a concrete app runtime (the
/// launch takes the action executors and the Wry handle), so the test resume
/// mirrors the production path on a mock runtime: the same bounded read, the
/// claim dropped on return. It shows the bound degrading and the hold
/// releasing; what guards `resume_claimed_session_for_store` and
/// `prepare_resume` themselves is the source pin
/// `session_commands::tests::resume_preparation_bounds_only_the_head_read`.
#[tokio::test]
async fn a_hanging_head_read_costs_the_snapshot_and_the_resume_goes_ahead() {
    let (store, project, branch) = store_with_branch();
    let other_branch = store::Branch::new(&project.id, "fix-signup", "main");
    store.create_branch(&other_branch).unwrap();
    let workdir = store::Workdir::new(&project.id, "/private/tmp").with_branch(&other_branch.id);
    store.create_workdir(&workdir).unwrap();
    let registry = Arc::new(SessionRegistry::new());
    let app = mock_app();

    let slow = interrupted_commit(&store, &branch.id, 100, Some("acp-slow"));
    let behind_one = interrupted_queued_commit(&store, &branch.id, 200);
    let behind_two = interrupted_queued_commit(&store, &branch.id, 300);
    let elsewhere = interrupted_commit(&store, &other_branch.id, 400, Some("acp-ok"));

    let plan = plan_launch_resume(
        [&slow, &behind_one, &behind_two, &elsewhere]
            .iter()
            .map(|s| {
                super::plan::classify_candidate(
                    &store,
                    store.get_session(&s.id).unwrap().as_ref().unwrap(),
                )
            })
            .collect(),
    );
    // Branches are grouped in id order (unspecified here); within the branch
    // the slow row leads and the re-queued rows follow it.
    let position = |id: &str| plan.repo.iter().position(|p| p.session_id == id).unwrap();
    assert!(position(&slow.id) < position(&behind_one.id));
    assert!(position(&behind_one.id) < position(&behind_two.id));

    const CAP: Duration = Duration::from_millis(100);
    let slow_id = slow.id.clone();
    let registry_for_resume = Arc::clone(&registry);
    // What the slow resume's preparation produced for `pre_head_sha`, so the
    // test can assert the degradation and not only that the phase moved on.
    let snapshot = Arc::new(std::sync::Mutex::new(None::<Option<String>>));
    let snapshot_for_resume = Arc::clone(&snapshot);
    let started = std::time::Instant::now();
    let outcome = run_repo_phase(
        &store,
        &registry,
        app.handle(),
        &plan.repo,
        move |claim: ClaimedResume| {
            let registry = Arc::clone(&registry_for_resume);
            let snapshot = Arc::clone(&snapshot_for_resume);
            let slow = claim.session.id == slow_id;
            async move {
                if !slow {
                    return Ok(());
                }
                // `prepare_resume`'s HEAD read, minus the launch: the read
                // never finishes, the bound drops it, and the preparation
                // carries on with no snapshot. The claim (and with it the
                // hold the bound protects: live here, released on return) is
                // dropped as the production resume drops it, after the runner
                // would have taken the row.
                assert!(registry.is_live(&claim.session.id));
                let pre_head_sha =
                    resume_pre_head_within(CAP, &claim.session.id, std::future::pending()).await;
                *snapshot.lock().unwrap() = Some(pre_head_sha);
                drop(claim);
                Ok(())
            }
        },
    )
    .await;

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the cap, not the hang, ended the wait"
    );
    assert_eq!(*snapshot.lock().unwrap(), Some(None), "no pre-run snapshot");
    let mut resumed = outcome.resumed.clone();
    resumed.sort();
    let mut expected = vec![slow.id.clone(), elsewhere.id.clone()];
    expected.sort();
    assert_eq!(resumed, expected);
    assert_eq!(
        outcome.requeued,
        vec![behind_one.id.clone(), behind_two.id.clone()]
    );
    assert_eq!(outcome.failed, vec![]);

    // The slow row resumed: `running` from the claim, no error recorded, and
    // the claim's hold released now that the resume has returned.
    let resumed_row = store.get_session(&slow.id).unwrap().unwrap();
    assert_eq!(resumed_row.status, SessionStatus::Running);
    assert_eq!(resumed_row.error_message, None);
    assert_eq!(resumed_row.completion_reason, None);
    assert_eq!(resumed_row.agent_id.as_deref(), Some("acp-slow"));
    assert!(!registry.is_live(&slow.id));

    // The re-queued rows behind it wait on the resumed session like any
    // running exclusive session, and are claimable in order once it is done.
    assert_eq!(
        claim_startable_queued_sessions(&store, &branch.id).unwrap(),
        Vec::<String>::new()
    );
    complete(&store, &slow.id);
    assert_eq!(
        claim_startable_queued_sessions(&store, &branch.id).unwrap(),
        vec![behind_one.id.clone()]
    );
    complete(&store, &behind_one.id);
    assert_eq!(
        claim_startable_queued_sessions(&store, &branch.id).unwrap(),
        vec![behind_two.id.clone()]
    );

    assert_eq!(status_of(&store, &elsewhere.id), SessionStatus::Running);
}

/// A row that moved before the phase reached it (resumed by hand, or by
/// another instance) is left alone.
#[tokio::test]
async fn phase_skips_rows_that_are_no_longer_interrupted() {
    let (store, _project, branch) = store_with_branch();
    let registry = Arc::new(SessionRegistry::new());
    let app = mock_app();

    let session = interrupted_queued_commit(&store, &branch.id, 100);
    let candidates = vec![super::plan::classify_candidate(
        &store,
        store.get_session(&session.id).unwrap().as_ref().unwrap(),
    )];
    let plan = plan_launch_resume(candidates);

    assert!(store.transition_app_quit_to_queued(&session.id).unwrap());
    assert!(store.transition_queued_to_running(&session.id).unwrap());

    let outcome = run_repo_phase(&store, &registry, app.handle(), &plan.repo, no_op_resume).await;
    assert_eq!(outcome, PhaseOutcome::default());
    assert_eq!(status_of(&store, &session.id), SessionStatus::Running);
}

/// A pipeline row is listed but never touched by the phase.
#[tokio::test]
async fn phase_leaves_pipeline_rows_terminal() {
    let (store, _project, branch) = store_with_branch();
    let registry = Arc::new(SessionRegistry::new());
    let app = mock_app();

    let mut push = Session::new_running("Push the current branch to the remote", Path::new("/tmp"))
        .with_branch(&branch.id)
        .with_agent("acp-3");
    push.pipeline = Some(store::PipelineExecution::from_steps(&[]));
    store.create_session(&push).unwrap();
    end_with_app_quit(&store, &push.id, SessionStatus::Cancelled);

    let plan = plan_launch_resume(vec![super::plan::classify_candidate(
        &store,
        store.get_session(&push.id).unwrap().as_ref().unwrap(),
    )]);
    let outcome = run_repo_phase(&store, &registry, app.handle(), &plan.repo, no_op_resume).await;
    assert_eq!(
        outcome.skipped,
        vec![(push.id.clone(), PIPELINE_NOT_RESUMED_REASON.to_string())]
    );
    assert_eq!(status_of(&store, &push.id), SessionStatus::Cancelled);
}

// ---------------------------------------------------------------------------
// Settlement and phase 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn settlement_counts_queued_terminal_and_live_running_rows() {
    let (store, _project, branch) = store_with_branch();
    let registry = Arc::new(SessionRegistry::new());

    let queued = interrupted_queued_commit(&store, &branch.id, 100);
    assert!(store.transition_app_quit_to_queued(&queued.id).unwrap());
    let terminal = interrupted_commit(&store, &branch.id, 200, None);
    let running = interrupted_commit(&store, &branch.id, 300, Some("acp-1"));
    assert!(store.transition_to_running(&running.id).unwrap());

    let ids = vec![queued.id.clone(), terminal.id.clone(), running.id.clone()];
    // Running with nothing in the registry owning it: not settled.
    let pending = wait_until_settled(&store, &registry, &ids, Duration::from_millis(50)).await;
    assert_eq!(pending, vec![running.id.clone()]);

    // A liveness hold is what a claim or a registering runner provides.
    let _hold = registry.hold(&running.id);
    let pending = wait_until_settled(&store, &registry, &ids, Duration::from_millis(50)).await;
    assert!(pending.is_empty());
}

#[tokio::test]
async fn project_phase_resumes_only_sessions_with_a_conversation() {
    let (store, project, _branch) = store_with_branch();
    let registry = Arc::new(SessionRegistry::new());

    let resumable = interrupted_project_session(&store, &project.id, 100, Some("acp-1"));
    let never_started = interrupted_project_session(&store, &project.id, 200, None);

    let plan = plan_launch_resume(
        [&resumable, &never_started]
            .iter()
            .map(|s| {
                super::plan::classify_candidate(
                    &store,
                    store.get_session(&s.id).unwrap().as_ref().unwrap(),
                )
            })
            .collect(),
    );
    assert!(plan.repo.is_empty());

    let outcome = run_project_phase(&store, &registry, &plan.project, no_op_resume).await;
    assert_eq!(outcome.resumed, vec![resumable.id.clone()]);
    assert_eq!(
        outcome.skipped,
        vec![(
            never_started.id.clone(),
            PROJECT_NEVER_STARTED_REASON.to_string()
        )]
    );
    assert_eq!(status_of(&store, &resumable.id), SessionStatus::Running);
    assert_eq!(
        status_of(&store, &never_started.id),
        SessionStatus::Cancelled
    );
}
