use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_client::{
    AttachOutcome, CancelOutcome, LatestPrompt, RecordedTurn, RemoteProcess, RemoteProcessStatus,
    StopReason, TurnResult,
};
use tauri::Manager;

use super::blox::probe_from_ws_info;
use super::row::{BACKGROUND_WORK_LIVE, PROMPT_NOT_RECEIVED, TURN_UNDETERMINED, WORKSTATION_GONE};
use super::stop::MAY_STILL_RUN;
use super::*;
use crate::session_queue::{BranchQueueKicker, KickSource};
use crate::session_runner::SessionStatusEvent;
use crate::store::{self, CompletionReason, MessageRole, Session, SessionStatus};
use crate::test_utils::TempGitRepo;

// =============================================================================
// Fakes
// =============================================================================

type AttachFn = dyn Fn(&ReconcileTarget, &Arc<Store>, &AttachControl) -> Result<AttachOutcome, String>
    + Send
    + Sync;

/// A scripted Blox. Every call is logged, in order.
struct FakeBlox {
    workspace: Mutex<WorkspaceProbe>,
    /// Runs inside each `ws info`, for a write that races the attempt.
    during_workspace: Mutex<Option<Box<dyn FnMut() + Send>>>,
    process: Mutex<Result<RemoteProcess, String>>,
    recorded: Mutex<Result<RecordedTurn, String>>,
    attach: Mutex<Arc<AttachFn>>,
    /// Whether an attach catches up and reports itself connected before
    /// running the scripted attach, as one to a live process does.
    connects: std::sync::atomic::AtomicBool,
    cancel: Mutex<Result<CancelOutcome, String>>,
    /// Runs inside a cancel that backfills, as its catch-up writes.
    backfill: Mutex<Option<BackfillHook>>,
    calls: Mutex<Vec<String>>,
}

type BackfillHook = Box<dyn Fn(&ReconcileTarget, &Arc<Store>) + Send>;

impl FakeBlox {
    fn new() -> Self {
        Self {
            workspace: Mutex::new(WorkspaceProbe::Running),
            during_workspace: Mutex::new(None),
            process: Mutex::new(Ok(process(RemoteProcessStatus::Idle, Some("cmd-1")))),
            recorded: Mutex::new(Ok(RecordedTurn::Unknown)),
            attach: Mutex::new(Arc::new(|_, _, _| Ok(ended_turn()))),
            connects: std::sync::atomic::AtomicBool::new(true),
            cancel: Mutex::new(Ok(CancelOutcome::Stopped)),
            backfill: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn set_workspace(&self, probe: WorkspaceProbe) {
        *self.workspace.lock().unwrap() = probe;
    }

    fn set_process(&self, status: RemoteProcessStatus, command_id: Option<&str>) {
        *self.process.lock().unwrap() = Ok(process(status, command_id));
    }

    fn set_recorded(&self, recorded: RecordedTurn) {
        *self.recorded.lock().unwrap() = Ok(recorded);
    }

    fn on_attach(
        &self,
        attach: impl Fn(&ReconcileTarget, &Arc<Store>, &AttachControl) -> Result<AttachOutcome, String>
            + Send
            + Sync
            + 'static,
    ) {
        *self.attach.lock().unwrap() = Arc::new(attach);
    }

    /// Attaches fail before they catch up: `on_connected` is never called.
    fn never_connects(&self) {
        self.connects
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn during_workspace(&self, hook: impl FnMut() + Send + 'static) {
        *self.during_workspace.lock().unwrap() = Some(Box::new(hook));
    }

    fn set_cancel(&self, outcome: Result<CancelOutcome, String>) {
        *self.cancel.lock().unwrap() = outcome;
    }

    fn on_backfill(&self, hook: impl Fn(&ReconcileTarget, &Arc<Store>) + Send + 'static) {
        *self.backfill.lock().unwrap() = Some(Box::new(hook));
    }

    fn log(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn called(&self, prefix: &str) -> bool {
        self.calls().iter().any(|call| call.starts_with(prefix))
    }
}

fn process(status: RemoteProcessStatus, command_id: Option<&str>) -> RemoteProcess {
    RemoteProcess {
        status,
        command_id: command_id.map(str::to_string),
    }
}

/// An attach that saw the newest prompt's turn end normally.
fn ended_turn() -> AttachOutcome {
    AttachOutcome::Ended {
        result: Some(TurnResult::Stopped(StopReason::EndTurn)),
        latest_prompt: LatestPrompt::Replayed,
        background_work_live: false,
    }
}

/// An attach that found the process gone, after a replay holding the newest
/// prompt.
fn process_gone(message: Option<String>) -> AttachOutcome {
    AttachOutcome::ProcessGone {
        message,
        latest_prompt: LatestPrompt::Replayed,
    }
}

impl ReconcileBlox for FakeBlox {
    fn workspace(&self, workspace_name: &str) -> WorkspaceProbe {
        self.log(format!("workspace:{workspace_name}"));
        if let Some(hook) = self.during_workspace.lock().unwrap().as_mut() {
            hook();
        }
        self.workspace.lock().unwrap().clone()
    }

    fn remote_working_dir(&self, _: &str, subpath: &str) -> Result<std::path::PathBuf, String> {
        Ok(subpath.into())
    }

    fn process(&self, target: &ReconcileTarget) -> Result<RemoteProcess, String> {
        self.log(format!("process:{:?}", target.command_id));
        self.process.lock().unwrap().clone()
    }

    fn recorded_turn(&self, _: &ReconcileTarget, command_id: &str) -> Result<RecordedTurn, String> {
        self.log(format!("recorded:{command_id}"));
        self.recorded.lock().unwrap().clone()
    }

    fn attach(
        &self,
        target: &ReconcileTarget,
        store: &Arc<Store>,
        control: &AttachControl,
    ) -> Result<AttachOutcome, String> {
        self.log(format!("attach:{:?}", target.command_id));
        if self.connects.load(std::sync::atomic::Ordering::SeqCst) {
            (control.on_connected)();
        }
        let attach = Arc::clone(&self.attach.lock().unwrap());
        attach(target, store, control)
    }

    /// Logged `cancel+backfill:` when it backfills.
    fn cancel(
        &self,
        target: &ReconcileTarget,
        backfill: Option<&Arc<Store>>,
        _: Duration,
    ) -> Result<CancelOutcome, String> {
        let Some(store) = backfill else {
            self.log(format!("cancel:{}", target.agent_session_id));
            return self.cancel.lock().unwrap().clone();
        };
        self.log(format!("cancel+backfill:{}", target.agent_session_id));
        if let Some(hook) = self.backfill.lock().unwrap().as_ref() {
            hook(target, store);
        }
        self.cancel.lock().unwrap().clone()
    }

    fn checkout_workspace<'a>(&self, _: &'a ReconcileTarget) -> Option<&'a str> {
        // The test checkout is local.
        None
    }
}

type QuitCheckHook = Box<dyn FnMut(usize) + Send>;

/// Records what reconcile asks of the app. With a kicker, the queue kick is
/// real.
#[derive(Default)]
struct RecordingHost {
    statuses: Mutex<Vec<SessionStatusEvent>>,
    reconcile_events: Mutex<Vec<SessionReconcileEvent>>,
    side_effects: Mutex<Vec<(String, String)>>,
    drains: Mutex<Vec<(String, Option<String>, bool)>>,
    skipped_dependents: Mutex<Vec<String>>,
    restops: Mutex<Vec<String>>,
    quitting: AtomicBool,
    /// Runs inside each quit check, with how many came before it.
    during_quit_check: Mutex<Option<QuitCheckHook>>,
    quit_checks: std::sync::atomic::AtomicUsize,
    app: Option<tauri::AppHandle<tauri::test::MockRuntime>>,
}

impl RecordingHost {
    fn statuses(&self) -> Vec<String> {
        self.statuses
            .lock()
            .unwrap()
            .iter()
            .map(|event| event.status.clone())
            .collect()
    }

    fn drains(&self) -> Vec<(String, Option<String>, bool)> {
        self.drains.lock().unwrap().clone()
    }

    fn last_reconcile_event(&self) -> Option<SessionReconcileEvent> {
        self.reconcile_events.lock().unwrap().last().cloned()
    }
}

impl ReconcileHost for RecordingHost {
    fn emit_status(&self, event: SessionStatusEvent) {
        self.statuses.lock().unwrap().push(event);
    }

    fn emit_reconcile(&self, event: &SessionReconcileEvent) {
        self.reconcile_events.lock().unwrap().push(event.clone());
    }

    fn completion_side_effects(&self, session_id: &str, _: Option<&str>, status: &str) {
        self.side_effects
            .lock()
            .unwrap()
            .push((session_id.to_string(), status.to_string()));
    }

    fn drain_and_kick(&self, session_id: &str, branch_id: Option<&str>, drain_message: bool) {
        self.drains.lock().unwrap().push((
            session_id.to_string(),
            branch_id.map(str::to_string),
            drain_message,
        ));
        if let (Some(app), Some(branch_id)) = (&self.app, branch_id) {
            crate::session_queue::kick(app, branch_id, KickSource::Reconciled);
        }
    }

    fn skip_dependents_of_cancelled(&self, session_id: &str) {
        self.skipped_dependents
            .lock()
            .unwrap()
            .push(session_id.to_string());
    }

    fn restop(&self, session_id: &str) {
        self.restops.lock().unwrap().push(session_id.to_string());
    }

    fn is_quitting(&self) -> bool {
        let checks = self.quit_checks.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = self.during_quit_check.lock().unwrap().as_mut() {
            hook(checks);
        }
        self.quitting.load(Ordering::SeqCst)
    }
}

// =============================================================================
// Fixture
// =============================================================================

struct Fixture {
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    blox: Arc<FakeBlox>,
    host: Arc<RecordingHost>,
    reconciler: Arc<Reconciler>,
    branch: store::Branch,
    repo: TempGitRepo,
    /// HEAD when the turn started.
    base: String,
}

impl Fixture {
    fn new() -> Self {
        Self::with(
            Arc::new(Store::in_memory().unwrap()),
            RecordingHost::default(),
        )
    }

    fn with(store: Arc<Store>, host: RecordingHost) -> Self {
        let project = store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = store::Branch::new_remote(&project.id, "remote", "main", "ws-1");
        store.create_branch(&branch).unwrap();
        let repo = TempGitRepo::new();
        repo.write_file("README.md", "base\n");
        let base = repo.commit("base");

        let registry = Arc::new(SessionRegistry::new());
        let blox = Arc::new(FakeBlox::new());
        let host = Arc::new(host);
        let reconciler = Arc::new(Reconciler::new(
            Arc::clone(&store),
            Arc::clone(&registry),
            blox.clone(),
            host.clone(),
        ));
        Self {
            store,
            registry,
            blox,
            host,
            reconciler,
            branch,
            repo,
            base,
        }
    }

    /// A commit session on the Blox branch, left `disconnected` mid-turn:
    /// its prompt is stored, its pending commit waits for the turn's
    /// commit, and its base is the repo's HEAD.
    fn disconnected_commit_session(&self) -> Session {
        let mut session = Session::new_running("commit the change", self.repo.path())
            .with_branch(&self.branch.id);
        session.agent_id = Some("acp-1".to_string());
        session.provider = Some("claude".to_string());
        self.store.create_session(&session).unwrap();
        let commit = store::Commit::new_pending(&self.branch.id).with_session(&session.id);
        self.store.create_commit(&commit).unwrap();
        self.store
            .add_session_message(&session.id, MessageRole::User, "commit the change")
            .unwrap();
        self.store
            .set_session_pre_head_sha(&session.id, Some(&self.base))
            .unwrap();
        self.store
            .transition_from_running(&session.id, SessionStatus::Disconnected, None, None)
            .unwrap();
        self.row(&session.id)
    }

    fn row(&self, session_id: &str) -> Session {
        self.store.get_session(session_id).unwrap().unwrap()
    }

    fn reconcile(&self, session: &Session) -> bool {
        self.reconciler
            .reconcile_blocking(&session.id, Trigger::CheckNow)
    }

    fn assert_settled(
        &self,
        session: &Session,
        status: SessionStatus,
        message: Option<&str>,
        reason: CompletionReason,
    ) {
        let row = self.row(&session.id);
        assert_eq!(row.status, status);
        assert_eq!(row.error_message.as_deref(), message);
        assert_eq!(row.completion_reason, Some(reason));
        assert!(
            !self.registry.is_live(&session.id),
            "a settled row keeps no registry entry"
        );
    }

    /// The row is untouched: still `disconnected`, as it was read.
    fn assert_untouched(&self, before: &Session) {
        let row = self.row(&before.id);
        assert_eq!(row.status, SessionStatus::Disconnected);
        assert_eq!(row.updated_at, before.updated_at);
        assert_eq!(row.owner_pid, None);
    }
}

// =============================================================================
// The decision
// =============================================================================

#[test]
fn a_deleted_workstation_ends_the_session_as_failed() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_workspace(WorkspaceProbe::Deleted);

    assert!(fx.reconcile(&session));

    fx.assert_settled(
        &session,
        SessionStatus::Error,
        Some(WORKSTATION_GONE),
        CompletionReason::Crashed,
    );
    assert!(!fx.blox.called("attach"));
    assert_eq!(fx.host.statuses(), vec!["error"]);
    // The row held the branch; ending it moves the queue on.
    assert_eq!(
        fx.host.drains(),
        vec![(session.id.clone(), Some(fx.branch.id.clone()), false)]
    );
    assert_eq!(
        fx.host.last_reconcile_event().unwrap().phase,
        ReconcilePhase::Idle
    );
}

#[test]
fn an_unreachable_workstation_leaves_the_row_and_backs_off() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_workspace(WorkspaceProbe::Unreachable {
        kind: UnreachableKind::Unreachable,
        message: "sq blox command timed out after 15s".to_string(),
    });

    assert!(fx.reconcile(&session));

    fx.assert_untouched(&session);
    assert!(fx.host.statuses().is_empty());
    assert!(fx.host.drains().is_empty(), "the branch stays blocked");
    let event = fx.host.last_reconcile_event().unwrap();
    assert_eq!(event.phase, ReconcilePhase::Waiting);
    assert_eq!(event.attempts, 1);
    assert_eq!(
        event.reason,
        Some(ReconcileReason::new(
            ReconcileReasonKind::Unreachable,
            "sq blox command timed out after 15s"
        ))
    );
    assert!(event.next_attempt_at.unwrap() > crate::store::now_timestamp());
    assert_eq!(event.branch_id.as_deref(), Some(fx.branch.id.as_str()));

    // The sweep respects the backoff; a "Check now" does not.
    assert!(!fx
        .reconciler
        .reconcile_blocking(&session.id, Trigger::Sweep));
    assert!(fx.reconcile(&session));
    assert_eq!(fx.host.last_reconcile_event().unwrap().attempts, 2);
    fx.assert_untouched(&session);
}

#[test]
fn a_signed_out_cli_is_reported_as_such() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_workspace(WorkspaceProbe::Unreachable {
        kind: UnreachableKind::NotAuthenticated,
        message: "Not authenticated with Blox. Run: sq login".to_string(),
    });

    fx.reconcile(&session);

    fx.assert_untouched(&session);
    let reason = fx.host.last_reconcile_event().unwrap().reason.unwrap();
    assert_eq!(reason.kind, ReconcileReasonKind::NotAuthenticated);
}

/// While suspended nothing can be read; once it runs again, every process
/// that was on it reads `FAILED` ("bloxlet restarted"), even one whose turn
/// had finished — so the recording, not the process status, decides.
#[test]
fn a_suspended_workstation_waits_then_settles_from_the_recording_once_resumed() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_workspace(WorkspaceProbe::Suspended);

    fx.reconcile(&session);

    fx.assert_untouched(&session);
    let reason = fx.host.last_reconcile_event().unwrap().reason.unwrap();
    assert_eq!(reason.kind, ReconcileReasonKind::Suspended);
    assert!(!fx.blox.called("process"));

    fx.blox.set_workspace(WorkspaceProbe::Running);
    fx.blox.set_process(
        RemoteProcessStatus::Gone {
            message: Some("bloxlet restarted while command was running".to_string()),
        },
        Some("cmd-old"),
    );
    fx.blox.set_recorded(RecordedTurn::Ended {
        result: Some(TurnResult::Stopped(StopReason::EndTurn)),
    });
    fx.blox.on_attach(|target, store, _| {
        // The replacement process replays the session, final message and all.
        store
            .add_session_message(&target.session_id, MessageRole::Assistant, "All done.")
            .unwrap();
        Ok(process_gone(Some(
            "bloxlet restarted while command was running".to_string(),
        )))
    });

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Completed,
        None,
        CompletionReason::TurnComplete,
    );
    // The gone process's recording is read before the attach starts a
    // replacement, and both reach the pinned process.
    let calls = fx.blox.calls();
    let recorded = calls.iter().position(|c| c == "recorded:cmd-old").unwrap();
    let attach = calls
        .iter()
        .position(|c| c == "attach:Some(\"cmd-old\")")
        .unwrap();
    assert!(recorded < attach, "{calls:?}");
    assert_eq!(
        fx.row(&session.id).blox_command_id.as_deref(),
        Some("cmd-old")
    );
    let messages = fx.store.get_session_messages(&session.id).unwrap();
    assert_eq!(messages.last().unwrap().content, "All done.");
}

#[test]
fn a_workstation_that_is_not_running_waits() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_workspace(WorkspaceProbe::NotRunning("starting".to_string()));

    fx.reconcile(&session);

    fx.assert_untouched(&session);
    let reason = fx.host.last_reconcile_event().unwrap().reason.unwrap();
    assert_eq!(reason.kind, ReconcileReasonKind::WorkstationNotRunning);
    assert_eq!(reason.message, "The workstation is starting");
}

/// A turn still running is followed live: the row is this process's
/// `running` row for as long as the attach follows it.
#[test]
fn a_running_turn_is_reattached_and_followed_to_its_end() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Running, Some("cmd-live"));
    let seen = Arc::new(Mutex::new(None));
    let registry = Arc::clone(&fx.registry);
    let seen_in_attach = Arc::clone(&seen);
    fx.blox.on_attach(move |target, store, _| {
        let row = store.get_session(&target.session_id).unwrap().unwrap();
        *seen_in_attach.lock().unwrap() = Some((
            row.status,
            row.owner_pid,
            registry.is_live(&target.session_id),
        ));
        Ok(ended_turn())
    });

    fx.reconcile(&session);

    assert_eq!(
        seen.lock().unwrap().clone(),
        Some((SessionStatus::Running, Some(std::process::id()), true))
    );
    fx.assert_settled(
        &session,
        SessionStatus::Completed,
        None,
        CompletionReason::TurnComplete,
    );
    // Nothing is read before the attach, but the recording is once the turn
    // is over, for work it may have left running.
    let calls = fx.blox.calls();
    let attach = calls.iter().position(|c| c.starts_with("attach")).unwrap();
    let recorded: Vec<usize> = (0..calls.len())
        .filter(|&i| calls[i].starts_with("recorded"))
        .collect();
    assert_eq!(recorded.len(), 1, "{calls:?}");
    assert!(attach < recorded[0], "{calls:?}");
    let statuses = fx.host.statuses.lock().unwrap().clone();
    assert_eq!(statuses[0].status, "running");
    assert_eq!(statuses[0].session_type.as_deref(), Some("commit"));
    assert_eq!(
        statuses[0].branch_id.as_deref(),
        Some(fx.branch.id.as_str())
    );
    assert_eq!(statuses[1].status, "completed");
    // The terminal emit carries it too: the frontend dropped its record of
    // the session when it went `disconnected`.
    assert_eq!(statuses[1].session_type.as_deref(), Some("commit"));
}

/// The offline turn committed and wrote its final message while nothing
/// was attached: the catch-up backfills the message, and the commit is found
/// against the turn's persisted base.
#[test]
fn an_idle_turn_is_backfilled_and_its_commit_linked_before_completing() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.repo.write_file("README.md", "changed offline\n");
    let offline_commit = fx.repo.commit("offline change");
    fx.blox
        .set_process(RemoteProcessStatus::Idle, Some("cmd-1"));
    fx.blox.on_attach(|target, store, _| {
        store
            .add_session_message(
                &target.session_id,
                MessageRole::Assistant,
                "Committed the change.",
            )
            .unwrap();
        Ok(ended_turn())
    });

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Completed,
        None,
        CompletionReason::TurnComplete,
    );
    let commit = fx
        .store
        .get_commit_by_session(&session.id)
        .unwrap()
        .unwrap();
    assert_eq!(commit.sha.as_deref(), Some(offline_commit.as_str()));
    let messages = fx.store.get_session_messages(&session.id).unwrap();
    assert_eq!(messages.last().unwrap().content, "Committed the change.");
    assert_eq!(
        fx.row(&session.id).blox_command_id.as_deref(),
        Some("cmd-1")
    );
    // Completion effects, then the queued follow-up and the branch.
    assert_eq!(
        fx.host.side_effects.lock().unwrap().clone(),
        vec![(session.id.clone(), "completed".to_string())]
    );
    assert_eq!(
        fx.host.drains(),
        vec![(session.id.clone(), Some(fx.branch.id.clone()), true)]
    );
}

/// Note extraction reads the backfilled final assistant message of the
/// offline turn.
#[test]
fn a_note_session_extracts_its_note_from_the_backfilled_message() {
    let fx = Fixture::new();
    let mut session =
        Session::new_running("write the note", fx.repo.path()).with_branch(&fx.branch.id);
    session.agent_id = Some("acp-1".to_string());
    fx.store.create_session(&session).unwrap();
    let note = store::Note::new(&fx.branch.id, "Draft", "").with_session(&session.id);
    fx.store.create_note(&note).unwrap();
    fx.store
        .add_session_message(&session.id, MessageRole::User, "write the note")
        .unwrap();
    fx.store
        .transition_from_running(&session.id, SessionStatus::Disconnected, None, None)
        .unwrap();
    fx.blox.on_attach(|target, store, _| {
        store
            .add_session_message(
                &target.session_id,
                MessageRole::Assistant,
                "Here it is.\n\n---\n\n# Offline findings\n\nWritten while Staged was away.",
            )
            .unwrap();
        Ok(ended_turn())
    });

    fx.reconcile(&fx.row(&session.id));

    assert_eq!(fx.row(&session.id).status, SessionStatus::Completed);
    let note = fx.store.get_note(&note.id).unwrap().unwrap();
    assert!(
        note.content.contains("Written while Staged was away."),
        "{}",
        note.content
    );
}

#[test]
fn a_failed_process_is_backfilled_then_ends_with_blox_s_message() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    let failure = "agent exited with status 137";
    fx.blox.set_process(
        RemoteProcessStatus::Gone {
            message: Some(failure.to_string()),
        },
        Some("cmd-1"),
    );
    fx.blox.set_recorded(RecordedTurn::InFlight);
    fx.blox.on_attach(|_, _, _| Ok(process_gone(None)));

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Error,
        Some(failure),
        CompletionReason::Crashed,
    );
    assert!(
        fx.blox.called("attach"),
        "the transcript is caught up first"
    );
    assert_eq!(
        fx.host.drains(),
        vec![(session.id.clone(), Some(fx.branch.id.clone()), false)]
    );
}

/// No turn state in the recording and no failure recorded: don't guess.
#[test]
fn a_turn_nothing_describes_settles_resumable_rather_than_completed() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Gone { message: None }, Some("cmd-1"));
    fx.blox.set_recorded(RecordedTurn::Unknown);
    fx.blox.on_attach(|_, _, _| Ok(process_gone(None)));

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        Some(TURN_UNDETERMINED),
        CompletionReason::Interrupted,
    );
    assert!(CompletionReason::Interrupted.is_resumable());
}

#[test]
fn a_recorded_cancelled_turn_settles_cancelled() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Gone { message: None }, Some("cmd-1"));
    fx.blox.set_recorded(RecordedTurn::Ended {
        result: Some(TurnResult::Stopped(StopReason::Cancelled)),
    });
    fx.blox.on_attach(|_, _, _| Ok(process_gone(None)));

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        None,
        CompletionReason::Interrupted,
    );
}

/// Staged recorded a resumed turn's prompt, then lost the connection before
/// the agent got it: the turn the attach saw end is the one before. The row
/// ends resumable, saying why, with no hooks run for a turn that never ran.
#[test]
fn a_prompt_the_agent_never_received_settles_resumable_not_completed() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.repo
        .write_file("README.md", "an earlier turn's change\n");
    fx.repo.commit("earlier change");
    fx.blox.on_attach(|_, _, _| {
        Ok(AttachOutcome::Ended {
            result: Some(TurnResult::Stopped(StopReason::EndTurn)),
            latest_prompt: LatestPrompt::Missing,
            background_work_live: false,
        })
    });

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        Some(PROMPT_NOT_RECEIVED),
        CompletionReason::Interrupted,
    );
    let commit = fx
        .store
        .get_commit_by_session(&session.id)
        .unwrap()
        .unwrap();
    assert_eq!(commit.sha, None, "no commit hook ran");
    assert_eq!(
        fx.host.drains(),
        vec![(session.id.clone(), Some(fx.branch.id.clone()), false)]
    );
}

/// The same for a gone process whose recording ends idle: the idle is the
/// previous turn's.
#[test]
fn a_gone_process_that_never_received_the_prompt_settles_resumable() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Gone { message: None }, Some("cmd-1"));
    fx.blox.set_recorded(RecordedTurn::Ended {
        result: Some(TurnResult::Stopped(StopReason::EndTurn)),
    });
    fx.blox.on_attach(|_, _, _| {
        Ok(AttachOutcome::ProcessGone {
            message: None,
            latest_prompt: LatestPrompt::Missing,
        })
    });

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        Some(PROMPT_NOT_RECEIVED),
        CompletionReason::Interrupted,
    );
}

/// An error result fails the turn, as the live path fails it when the
/// bridge answers the prompt with that error.
#[test]
fn an_errored_turn_settles_failed() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.on_attach(|_, _, _| {
        Ok(AttachOutcome::Ended {
            result: Some(TurnResult::Failed {
                message: Some("API Error: 500".to_string()),
            }),
            latest_prompt: LatestPrompt::Replayed,
            background_work_live: false,
        })
    });

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Error,
        Some("API Error: 500"),
        CompletionReason::Crashed,
    );
    assert_eq!(fx.host.statuses(), vec!["running", "error"]);
}

/// A turn that ended before the attach was made: its `result` went to the
/// lost connection and the replay holds none, so the recording decides.
#[test]
fn an_idle_turn_that_errored_settles_failed_from_its_recording() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_recorded(RecordedTurn::Ended {
        result: Some(TurnResult::Failed {
            message: Some("API Error: 529 overloaded".to_string()),
        }),
    });
    fx.blox.on_attach(|_, _, _| {
        Ok(AttachOutcome::Ended {
            result: None,
            latest_prompt: LatestPrompt::Replayed,
            background_work_live: false,
        })
    });

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Error,
        Some("API Error: 529 overloaded"),
        CompletionReason::Crashed,
    );
    assert!(fx.blox.called("recorded:cmd-1"));
}

#[test]
fn an_idle_turn_that_was_aborted_settles_cancelled_from_its_recording() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_recorded(RecordedTurn::Ended {
        result: Some(TurnResult::Stopped(StopReason::Cancelled)),
    });
    fx.blox.on_attach(|_, _, _| {
        Ok(AttachOutcome::Ended {
            result: None,
            latest_prompt: LatestPrompt::Replayed,
            background_work_live: false,
        })
    });

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        None,
        CompletionReason::Interrupted,
    );
}

/// With nothing in the recording either, an ended turn reads as completed,
/// as before; and a result the attach saw beats the recording's.
#[test]
fn an_idle_turn_with_no_result_anywhere_settles_completed() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_recorded(RecordedTurn::Unknown);
    fx.blox.on_attach(|_, _, _| {
        Ok(AttachOutcome::Ended {
            result: None,
            latest_prompt: LatestPrompt::Replayed,
            background_work_live: false,
        })
    });

    fx.reconcile(&session);

    assert_eq!(fx.row(&session.id).status, SessionStatus::Completed);
    assert!(fx.blox.called("recorded:cmd-1"));

    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_recorded(RecordedTurn::Ended {
        result: Some(TurnResult::Failed { message: None }),
    });
    fx.reconcile(&session);
    assert_eq!(fx.row(&session.id).status, SessionStatus::Completed);
}

#[test]
fn a_recorded_errored_turn_settles_failed() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Gone { message: None }, Some("cmd-1"));
    fx.blox.set_recorded(RecordedTurn::Ended {
        result: Some(TurnResult::Failed { message: None }),
    });
    fx.blox.on_attach(|_, _, _| Ok(process_gone(None)));

    fx.reconcile(&session);

    let row = fx.row(&session.id);
    assert_eq!(row.status, SessionStatus::Error);
    assert_eq!(row.completion_reason, Some(CompletionReason::Crashed));
    assert!(row.error_message.is_some());
}

// =============================================================================
// Work that outlives the turn
// =============================================================================

/// The reason a row held for its session's work reports.
fn assert_held(fx: &Fixture, session: &Session) {
    let row = fx.row(&session.id);
    assert_eq!(row.status, SessionStatus::Disconnected);
    assert_eq!(row.owner_pid, None);
    assert!(!fx.registry.is_live(&session.id));
    assert!(fx.host.drains().is_empty(), "the branch stays blocked");
    let event = fx.host.last_reconcile_event().unwrap();
    assert_eq!(event.phase, ReconcilePhase::Waiting);
    assert_eq!(
        event.reason,
        Some(ReconcileReason::new(
            ReconcileReasonKind::BackgroundWork,
            BACKGROUND_WORK_LIVE
        ))
    );
    assert_eq!(event.attempts, 0, "held work is not a failure");
    let commit = fx
        .store
        .get_commit_by_session(&session.id)
        .unwrap()
        .unwrap();
    assert_eq!(commit.sha, None, "no completion hook ran");
}

/// The process reads `IDLE` — every prompt answered — but the recording
/// shows a background task the turn started still live: the row is held,
/// without being claimed (so it doesn't flip `running` and back) or
/// attached to, and checked again after a short wait rather than a backoff.
#[test]
fn an_idle_session_still_working_after_its_turn_is_held_without_claiming_it() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_recorded(RecordedTurn::Holding {
        result: Some(TurnResult::Stopped(StopReason::EndTurn)),
    });

    assert!(fx.reconcile(&session));

    fx.assert_untouched(&session);
    assert_held(&fx, &session);
    assert!(!fx.blox.called("attach"));
    assert!(fx.host.statuses().is_empty());
    let next = fx.host.last_reconcile_event().unwrap().next_attempt_at;
    let wait = next.unwrap() - now_timestamp();
    assert!(
        (50_000..=HELD_WORK_RECHECK.as_millis() as i64).contains(&wait),
        "{wait}"
    );
    assert!(
        !fx.reconciler
            .reconcile_blocking(&session.id, Trigger::Sweep),
        "the sweep waits for the recheck"
    );
}

/// Held while the recording shows work going — tasks live, then a cycle a
/// settled task woke, with no prompt in flight — and settled from the
/// turn's result once it shows the work over.
#[test]
fn a_held_session_is_checked_until_its_work_ends_then_settles() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_recorded(RecordedTurn::Holding {
        result: Some(TurnResult::Stopped(StopReason::EndTurn)),
    });
    fx.reconcile(&session);
    assert_held(&fx, &session);
    let first = fx.host.last_reconcile_event().unwrap().next_attempt_at;

    fx.blox.set_recorded(RecordedTurn::InFlight);
    fx.reconcile(&session);
    assert_held(&fx, &session);
    let second = fx.host.last_reconcile_event().unwrap().next_attempt_at;
    assert!(
        second.unwrap() - first.unwrap() >= HELD_WORK_RECHECK.as_millis() as i64 - 10_000,
        "the recheck spaces out"
    );
    assert!(!fx.blox.called("attach"));

    fx.blox.set_recorded(RecordedTurn::Ended {
        result: Some(TurnResult::Stopped(StopReason::EndTurn)),
    });
    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Completed,
        None,
        CompletionReason::TurnComplete,
    );
    assert_eq!(fx.host.statuses(), vec!["running", "completed"]);
    assert_eq!(
        fx.host.drains(),
        vec![(session.id.clone(), Some(fx.branch.id.clone()), true)]
    );
}

/// A followed turn that ends with background work live on the stream goes
/// back to `disconnected`, held, rather than settling.
#[test]
fn a_followed_turn_that_leaves_work_live_goes_back_to_disconnected() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Running, Some("cmd-live"));
    fx.blox.on_attach(|_, _, _| {
        Ok(AttachOutcome::Ended {
            result: Some(TurnResult::Stopped(StopReason::EndTurn)),
            latest_prompt: LatestPrompt::Replayed,
            background_work_live: true,
        })
    });

    assert!(fx.reconcile(&session));

    assert_held(&fx, &session);
    assert_eq!(fx.host.statuses(), vec!["running", "disconnected"]);
    assert!(fx.host.side_effects.lock().unwrap().is_empty());
}

/// The same when only the recording knows: tasks started before the attach
/// are never re-sent to it.
#[test]
fn a_followed_turn_whose_recording_shows_work_live_is_held() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Running, Some("cmd-live"));
    fx.blox.set_recorded(RecordedTurn::Holding {
        result: Some(TurnResult::Stopped(StopReason::EndTurn)),
    });

    fx.reconcile(&session);

    assert_held(&fx, &session);
    assert_eq!(fx.host.statuses(), vec!["running", "disconnected"]);
}

/// Work still going at the live hold's ceiling settles the turn anyway: a
/// hung background shell doesn't hold the branch forever.
#[test]
fn held_work_past_the_hold_ceiling_settles_the_turn() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_recorded(RecordedTurn::Holding {
        result: Some(TurnResult::Stopped(StopReason::EndTurn)),
    });
    let reconciler = Reconciler::new(
        Arc::clone(&fx.store),
        Arc::clone(&fx.registry),
        fx.blox.clone(),
        fx.host.clone(),
    )
    .with_held_work_ceiling(Duration::ZERO);

    reconciler.reconcile_blocking(&session.id, Trigger::CheckNow);
    assert_held(&fx, &session);

    reconciler.reconcile_blocking(&session.id, Trigger::CheckNow);

    fx.assert_settled(
        &session,
        SessionStatus::Completed,
        None,
        CompletionReason::TurnComplete,
    );
    assert!(fx.blox.called("attach"));
}

/// A Stop that reaches a followed turn as it ends with work live does what
/// a Stop during the live hold does: the work is cancelled, and the turn,
/// already over, stands.
#[test]
fn a_stop_as_a_followed_turn_leaves_work_live_cancels_the_work() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Running, Some("cmd-live"));
    let registry = Arc::clone(&fx.registry);
    fx.blox.on_attach(move |target, _, _| {
        assert!(registry.cancel(&target.session_id));
        Ok(AttachOutcome::Ended {
            result: Some(TurnResult::Stopped(StopReason::EndTurn)),
            latest_prompt: LatestPrompt::Replayed,
            background_work_live: true,
        })
    });

    fx.reconcile(&session);

    assert!(fx.blox.called("cancel+backfill:acp-1"));
    fx.assert_settled(
        &session,
        SessionStatus::Completed,
        None,
        CompletionReason::TurnComplete,
    );
}

/// A gone process took the work the turn left running with it: the turn
/// stands, from its result.
#[test]
fn a_gone_process_s_held_turn_settles_from_its_result() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Gone { message: None }, Some("cmd-1"));
    fx.blox.set_recorded(RecordedTurn::Holding {
        result: Some(TurnResult::Failed {
            message: Some("API Error: 500".to_string()),
        }),
    });
    fx.blox.on_attach(|_, _, _| Ok(process_gone(None)));

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Error,
        Some("API Error: 500"),
        CompletionReason::Crashed,
    );
}

// =============================================================================
// Agents without turn state
// =============================================================================

/// An agent without the Claude bridge's turn state (Codex, Goose, Pi, Amp)
/// on a process of its own: the recording's answer to the prompt says how
/// the turn ended, an error one failing it as the live path does.
#[test]
fn an_agent_without_turn_state_settles_from_its_prompt_answer() {
    let failure = "Prompt failed: Error { code: -32603: Internal error, message: \"Internal \
                   error\", data: None }";
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.store.set_session_provider(&session.id, "codex").unwrap();
    fx.blox.set_recorded(RecordedTurn::Answered {
        result: TurnResult::Failed {
            message: Some(failure.to_string()),
        },
    });
    fx.blox.on_attach(|_, _, _| {
        Ok(AttachOutcome::Ended {
            result: None,
            latest_prompt: LatestPrompt::Replayed,
            background_work_live: false,
        })
    });

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Error,
        Some(failure),
        CompletionReason::Crashed,
    );
    assert_eq!(
        fx.host.drains(),
        vec![(session.id.clone(), Some(fx.branch.id.clone()), false)]
    );

    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_recorded(RecordedTurn::Answered {
        result: TurnResult::Stopped(StopReason::Cancelled),
    });
    fx.blox.on_attach(|_, _, _| {
        Ok(AttachOutcome::Ended {
            result: None,
            latest_prompt: LatestPrompt::Replayed,
            background_work_live: false,
        })
    });

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        None,
        CompletionReason::Interrupted,
    );
}

/// A gone process's last prompt answer doesn't say a newer prompt wasn't in
/// flight when it went: nothing describes the turn.
#[test]
fn a_gone_process_s_prompt_answer_does_not_describe_its_turn() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Gone { message: None }, Some("cmd-1"));
    fx.blox.set_recorded(RecordedTurn::Answered {
        result: TurnResult::Stopped(StopReason::EndTurn),
    });
    fx.blox.on_attach(|_, _, _| Ok(process_gone(None)));

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        Some(TURN_UNDETERMINED),
        CompletionReason::Interrupted,
    );
}

/// A failed attach hands the row back as it found it — `disconnected`, owned
/// by nobody — and backs off.
#[test]
fn a_failed_attach_leaves_the_row_disconnected_and_backs_off() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.never_connects();
    fx.blox
        .on_attach(|_, _, _| Err("ACP protocol failed: connection reset".to_string()));

    fx.reconcile(&session);

    let row = fx.row(&session.id);
    assert_eq!(row.status, SessionStatus::Disconnected);
    assert_eq!(row.owner_pid, None);
    assert!(!fx.registry.is_live(&session.id));
    // Never connected, so never announced running: the frontend only hears
    // the row is still disconnected.
    assert_eq!(fx.host.statuses(), vec!["disconnected"]);
    assert!(fx.host.drains().is_empty());
    let event = fx.host.last_reconcile_event().unwrap();
    assert_eq!(event.phase, ReconcilePhase::Waiting);
    assert_eq!(
        event.reason.unwrap().kind,
        ReconcileReasonKind::AttachFailed
    );
}

// =============================================================================
// Rows ended or deleted outside reconcile
// =============================================================================

/// A `disconnected` row a teardown or delete is about to take: its remote
/// turn is cancelled first, on a thread of its own, and the row is left to
/// the caller.
#[test]
fn a_disconnected_row_about_to_go_has_its_turn_cancelled() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    let blox: Arc<dyn ReconcileBlox> = fx.blox.clone();

    let cancel = super::stop::cancel_abandoned_turn_with(&fx.store, blox, &session)
        .expect("a cancel for a disconnected row");
    cancel.join().unwrap();

    assert_eq!(fx.blox.calls(), vec!["cancel:acp-1"]);
    fx.assert_untouched(&session);
    assert!(fx.host.statuses().is_empty());
}

/// An unconfirmed cancel is only logged: there is no row left to say so on.
#[test]
fn an_abandoned_turn_whose_cancel_fails_is_only_logged() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_cancel(Err("unreachable".to_string()));
    let blox: Arc<dyn ReconcileBlox> = fx.blox.clone();

    super::stop::cancel_abandoned_turn_with(&fx.store, blox, &session)
        .unwrap()
        .join()
        .unwrap();

    fx.assert_untouched(&session);
}

#[test]
fn only_a_disconnected_row_with_a_turn_is_cancelled_before_it_goes() {
    let fx = Fixture::new();
    let blox: Arc<dyn ReconcileBlox> = fx.blox.clone();

    // Settled: nothing runs.
    let settled = fx.disconnected_commit_session();
    fx.reconcile(&settled);
    let settled = fx.row(&settled.id);
    assert_eq!(settled.status, SessionStatus::Completed);
    let calls = fx.blox.calls().len();
    assert!(
        super::stop::cancel_abandoned_turn_with(&fx.store, Arc::clone(&blox), &settled).is_none()
    );

    // No agent session: no turn to cancel.
    let mut never_prompted = fx.disconnected_commit_session();
    never_prompted.agent_id = None;
    assert!(super::stop::cancel_abandoned_turn_with(&fx.store, blox, &never_prompted).is_none());
    assert_eq!(fx.blox.calls().len(), calls);
}

// =============================================================================
// Idempotence and races
// =============================================================================

#[test]
fn reconciling_a_settled_row_again_does_nothing() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.reconcile(&session);
    assert_eq!(fx.row(&session.id).status, SessionStatus::Completed);
    let settled = fx.row(&session.id);
    let calls = fx.blox.calls().len();
    let statuses = fx.host.statuses().len();

    fx.reconcile(&session);

    assert_eq!(
        fx.blox.calls().len(),
        calls,
        "no Blox call for a settled row"
    );
    assert_eq!(fx.host.statuses().len(), statuses);
    assert_eq!(fx.row(&session.id).updated_at, settled.updated_at);
}

/// An attempt in flight is the only one: a second trigger for the same row
/// starts nothing until it finishes.
#[test]
fn only_one_attempt_runs_per_row() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    let reconciler = Arc::clone(&fx.reconciler);
    let session_id = session.id.clone();
    let second = Arc::new(Mutex::new(None));
    let second_in_hook = Arc::clone(&second);
    fx.blox.during_workspace(move || {
        let mut second = second_in_hook.lock().unwrap();
        if second.is_none() {
            *second = Some(reconciler.reconcile_blocking(&session_id, Trigger::CheckNow));
        }
    });

    assert!(fx.reconcile(&session));

    assert_eq!(*second.lock().unwrap(), Some(false));
    assert_eq!(fx.row(&session.id).status, SessionStatus::Completed);
}

/// A Stop's fallback write lands while the attempt is asking Blox: the
/// attempt's claim fails, and the Stop's write stands.
#[test]
fn a_stop_that_lands_first_wins_the_row() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    let store = Arc::clone(&fx.store);
    let session_id = session.id.clone();
    fx.blox.during_workspace(move || {
        std::thread::sleep(Duration::from_millis(2));
        store
            .update_session_status(
                &session_id,
                SessionStatus::Cancelled,
                None,
                Some(&CompletionReason::Interrupted),
            )
            .unwrap();
    });

    fx.reconcile(&session);

    let row = fx.row(&session.id);
    assert_eq!(row.status, SessionStatus::Cancelled);
    assert_eq!(row.completion_reason, Some(CompletionReason::Interrupted));
    assert!(!fx.blox.called("attach"));
    assert!(fx.host.statuses().is_empty());
    assert!(!fx.registry.is_live(&session.id));
}

/// Another Staged instance's reconcile claims the row first: this one backs
/// away without touching it.
#[test]
fn another_reconcile_that_claims_first_wins_the_row() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    let store = Arc::clone(&fx.store);
    let observed = session.updated_at;
    let session_id = session.id.clone();
    fx.blox.during_workspace(move || {
        std::thread::sleep(Duration::from_millis(2));
        assert!(store
            .claim_disconnected_session(&session_id, observed)
            .unwrap());
    });

    fx.reconcile(&session);

    assert_eq!(fx.row(&session.id).status, SessionStatus::Running);
    assert!(!fx.blox.called("attach"));
    assert!(fx.host.statuses().is_empty());
    assert!(!fx.registry.is_live(&session.id));
}

/// A rewrite of the row while the workstation is being asked — the other
/// instance's failed attempt, which writes it `disconnected` again — makes the
/// snapshot stale, so even the no-claim settle (a deleted workstation) leaves
/// it alone.
#[test]
fn a_stale_snapshot_never_settles_the_row() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_workspace(WorkspaceProbe::Deleted);
    let store = Arc::clone(&fx.store);
    let session_id = session.id.clone();
    let observed = session.updated_at;
    fx.blox.during_workspace(move || {
        std::thread::sleep(Duration::from_millis(2));
        assert!(store
            .claim_disconnected_session(&session_id, observed)
            .unwrap());
        store
            .transition_from_running(&session_id, SessionStatus::Disconnected, None, None)
            .unwrap();
    });

    fx.reconcile(&session);

    let row = fx.row(&session.id);
    assert_eq!(row.status, SessionStatus::Disconnected);
    assert_ne!(row.updated_at, session.updated_at);
    assert!(fx.host.statuses().is_empty());
    // The rewritten row is picked up by the next attempt.
    fx.blox.during_workspace(|| {});
    fx.reconcile(&row);
    assert_eq!(fx.row(&session.id).status, SessionStatus::Error);
}

/// While the row is `disconnected` its branch queue stays parked behind it;
/// settling it `completed` kicks the branch, and the queued work starts.
#[tokio::test]
async fn the_queue_waits_while_disconnected_and_moves_once_completed() {
    let store = Arc::new(Store::in_memory().unwrap());
    let drain_store = Arc::clone(&store);
    let kicker = Arc::new(BranchQueueKicker::new(Arc::new(
        move |branch_id: String| {
            let store = Arc::clone(&drain_store);
            Box::pin(async move {
                crate::session_commands::claim_startable_queued_sessions(&store, &branch_id)
                    .map(|claimed| !claimed.is_empty())
            })
        },
    )));
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("failed to build mock app");
    app.manage(Arc::clone(&kicker));
    let fx = Fixture::with(
        store,
        RecordingHost {
            app: Some(app.handle().clone()),
            ..RecordingHost::default()
        },
    );
    let session = fx.disconnected_commit_session();
    let queued = Session::new_queued("note").with_branch(&fx.branch.id);
    fx.store.create_session(&queued).unwrap();
    fx.store
        .create_note(&store::Note::new(&fx.branch.id, "note", "").with_session(&queued.id))
        .unwrap();

    // Unreachable: the row stays, and so does the note behind it, kick or no.
    fx.blox.set_workspace(WorkspaceProbe::Unreachable {
        kind: UnreachableKind::Unreachable,
        message: "timed out".to_string(),
    });
    fx.reconcile(&session);
    kicker.kick(&fx.branch.id, KickSource::Startup);
    kicker.wait_until_idle().await;
    assert_eq!(fx.row(&queued.id).status, SessionStatus::Queued);
    assert_eq!(
        kicker.kicked(),
        vec![(fx.branch.id.clone(), KickSource::Startup)]
    );

    fx.blox.set_workspace(WorkspaceProbe::Running);
    fx.reconcile(&session);
    kicker.wait_until_idle().await;
    assert_eq!(fx.row(&session.id).status, SessionStatus::Completed);
    assert_eq!(
        kicker.kicked().last(),
        Some(&(fx.branch.id.clone(), KickSource::Reconciled))
    );
    assert_eq!(fx.row(&queued.id).status, SessionStatus::Running);
}

// =============================================================================
// Stop
// =============================================================================

/// The Stop's cancel backfills the transcript before the row ends: the
/// catch-up writes while the row is still `disconnected`, so the tool call
/// the turn was running ends as the agent ended it, not "Running" for good.
#[test]
fn a_stop_backfills_the_transcript_before_the_row_ends() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_in_backfill = Arc::clone(&seen);
    fx.blox.on_backfill(move |target, store| {
        let row = store.get_session(&target.session_id).unwrap().unwrap();
        seen_in_backfill.lock().unwrap().push(row.status);
    });

    fx.reconciler
        .stop_blocking(&session.id, CompletionReason::Interrupted);

    assert_eq!(*seen.lock().unwrap(), vec![SessionStatus::Disconnected]);
    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        None,
        CompletionReason::Interrupted,
    );
}

#[test]
fn a_stop_on_a_disconnected_row_cancels_the_remote_turn_first() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();

    fx.reconciler
        .stop_blocking(&session.id, CompletionReason::Interrupted);

    assert_eq!(
        fx.blox.calls(),
        vec!["workspace:ws-1", "cancel+backfill:acp-1"]
    );
    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        None,
        CompletionReason::Interrupted,
    );
    assert_eq!(fx.host.statuses(), vec!["cancelled"]);
    assert_eq!(
        fx.host.skipped_dependents.lock().unwrap().clone(),
        vec![session.id.clone()]
    );
    assert_eq!(
        fx.host.drains(),
        vec![(session.id.clone(), Some(fx.branch.id.clone()), false)]
    );
}

#[test]
fn a_stop_the_workstation_never_confirms_says_the_turn_may_still_run() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_cancel(Ok(CancelOutcome::Unconfirmed));

    fx.reconciler
        .stop_blocking(&session.id, CompletionReason::Interrupted);

    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        Some(MAY_STILL_RUN),
        CompletionReason::Interrupted,
    );
}

/// The workstation can't be reached, so the cancel can't be sent: the row
/// is ended anyway — the user asked — and says why it may not be over.
#[test]
fn a_stop_with_the_workstation_unreachable_ends_the_row_locally() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_workspace(WorkspaceProbe::Unreachable {
        kind: UnreachableKind::Unreachable,
        message: "timed out".to_string(),
    });

    fx.reconciler
        .stop_blocking(&session.id, CompletionReason::Interrupted);

    assert!(!fx.blox.called("cancel"));
    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        Some(MAY_STILL_RUN),
        CompletionReason::Interrupted,
    );
}

/// A Stop that arrives while an attempt is still asking Blox is taken over
/// by that attempt, which cancels instead of attaching.
#[test]
fn a_stop_during_an_attempt_turns_it_into_the_stop() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    let reconciler = Arc::clone(&fx.reconciler);
    let session_id = session.id.clone();
    let once = AtomicBool::new(false);
    fx.blox.during_workspace(move || {
        if !once.swap(true, Ordering::SeqCst) {
            reconciler.stop_blocking(&session_id, CompletionReason::Interrupted);
        }
    });

    fx.reconcile(&session);

    assert!(fx.blox.called("cancel"));
    assert!(!fx.blox.called("attach"));
    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        None,
        CompletionReason::Interrupted,
    );
}

/// A Stop that lands after the attempt's last look before claiming the row —
/// here, as it checks for a quit — is not lost: the attempt takes it over
/// once registered, instead of attaching.
#[test]
fn a_stop_racing_the_claim_is_taken_over_before_attaching() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    let reconciler = Arc::clone(&fx.reconciler);
    let session_id = session.id.clone();
    // The first check is the one starting the attempt.
    *fx.host.during_quit_check.lock().unwrap() = Some(Box::new(move |checks| {
        if checks == 1 {
            reconciler.stop_blocking(&session_id, CompletionReason::Interrupted);
        }
    }));

    fx.reconcile(&session);

    assert!(!fx.blox.called("attach"), "{:?}", fx.blox.calls());
    assert!(fx.blox.called("cancel"));
    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        None,
        CompletionReason::Interrupted,
    );
}

/// A Stop that lands while an attempt is finding the workstation
/// unreachable is answered as the attempt ends, not after its backoff.
#[test]
fn a_stop_during_an_attempt_that_backs_off_is_answered_at_once() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_workspace(WorkspaceProbe::Unreachable {
        kind: UnreachableKind::Unreachable,
        message: "timed out".to_string(),
    });
    let reconciler = Arc::clone(&fx.reconciler);
    let session_id = session.id.clone();
    let once = AtomicBool::new(false);
    fx.blox.during_workspace(move || {
        if !once.swap(true, Ordering::SeqCst) {
            reconciler.stop_blocking(&session_id, CompletionReason::Interrupted);
        }
    });

    fx.reconcile(&session);

    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        Some(MAY_STILL_RUN),
        CompletionReason::Interrupted,
    );
    assert!(fx.reconciler.states().is_empty());
    assert!(fx
        .host
        .reconcile_events
        .lock()
        .unwrap()
        .iter()
        .all(|event| event.phase != ReconcilePhase::Waiting));
}

/// A parent project session's Stop keeps its own completion reason, as it
/// does on a running session.
#[test]
fn a_project_session_stop_ends_the_row_with_its_reason() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();

    fx.reconciler
        .stop_blocking(&session.id, CompletionReason::ProjectSessionInterrupted);

    assert!(fx.blox.called("cancel"));
    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        None,
        CompletionReason::ProjectSessionInterrupted,
    );
}

/// Once the row is claimed `running`, a Stop goes through the registry like
/// any running session's: the attach lets go, and the turn is cancelled over
/// a fresh connection.
#[test]
fn a_stop_while_following_a_turn_cancels_it() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Running, Some("cmd-live"));
    let registry = Arc::clone(&fx.registry);
    fx.blox.on_attach(move |target, _, control| {
        assert!(
            registry.cancel(&target.session_id),
            "the attempt is registered"
        );
        assert!(control.token.is_cancelled());
        Ok(AttachOutcome::Live)
    });

    fx.reconcile(&session);

    assert!(fx.blox.called("cancel"));
    fx.assert_settled(
        &session,
        SessionStatus::Cancelled,
        None,
        CompletionReason::Interrupted,
    );
    assert_eq!(
        fx.host.drains(),
        vec![(session.id.clone(), Some(fx.branch.id.clone()), false)]
    );
}

/// A quit that lands while a turn is followed leaves it running on the
/// workstation, and the row `disconnected` again, with nothing to back off
/// from.
#[test]
fn a_quit_while_following_a_turn_leaves_it_disconnected() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox
        .set_process(RemoteProcessStatus::Running, Some("cmd-live"));
    let registry = Arc::clone(&fx.registry);
    fx.blox.on_attach(move |target, _, _| {
        registry.cancel_with_completion_reason(&target.session_id, CompletionReason::AppQuit);
        Ok(AttachOutcome::Live)
    });

    fx.reconcile(&session);

    let row = fx.row(&session.id);
    assert_eq!(row.status, SessionStatus::Disconnected);
    assert_eq!(row.owner_pid, None);
    assert!(!fx.blox.called("cancel"));
    assert!(fx.host.drains().is_empty());
    let event = fx.host.last_reconcile_event().unwrap();
    assert_eq!(event.attempts, 0);
    assert_eq!(event.phase, ReconcilePhase::Idle);
}

#[test]
fn a_quitting_app_starts_no_attempt() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.host.quitting.store(true, Ordering::SeqCst);

    assert!(!fx.reconcile(&session));

    assert!(fx.blox.calls().is_empty());
    fx.assert_untouched(&session);
}

#[test]
fn an_unacknowledged_remote_stop_is_retried_over_a_fresh_connection() {
    let fx = Fixture::new();
    let mut session = Session::new_running("work", fx.repo.path()).with_branch(&fx.branch.id);
    session.agent_id = Some("acp-1".to_string());
    fx.store.create_session(&session).unwrap();

    assert_eq!(
        stop_unacknowledged_turn(&fx.store, fx.blox.as_ref(), &session.id),
        None
    );
    assert_eq!(fx.blox.calls(), vec!["cancel+backfill:acp-1"]);

    fx.blox.set_cancel(Err("connection refused".to_string()));
    assert_eq!(
        stop_unacknowledged_turn(&fx.store, fx.blox.as_ref(), &session.id).as_deref(),
        Some(MAY_STILL_RUN)
    );
}

// =============================================================================
// Triggers and helpers
// =============================================================================

#[test]
fn a_reachable_workstation_retries_only_its_failed_rows() {
    let fx = Fixture::new();
    let failed = fx.disconnected_commit_session();
    let untried = fx.disconnected_commit_session();
    fx.blox.set_workspace(WorkspaceProbe::Unreachable {
        kind: UnreachableKind::Unreachable,
        message: "timed out".to_string(),
    });
    fx.reconcile(&failed);
    let calls = fx.blox.calls().len();

    // Too soon after the failed attempt.
    assert!(!fx
        .reconciler
        .reconcile_blocking(&failed.id, Trigger::WorkspaceReachable));
    // Never failed: left to the sweep.
    assert!(!fx
        .reconciler
        .reconcile_blocking(&untried.id, Trigger::WorkspaceReachable));
    assert_eq!(fx.blox.calls().len(), calls);

    // Past the spacing, the failed row goes again.
    fx.reconciler
        .rows
        .lock()
        .unwrap()
        .get_mut(&failed.id)
        .unwrap()
        .last_attempt = Some(std::time::Instant::now() - NUDGE_MIN_SPACING);
    assert!(fx
        .reconciler
        .reconcile_blocking(&failed.id, Trigger::WorkspaceReachable));
}

#[test]
fn states_report_rows_being_reconciled() {
    let fx = Fixture::new();
    let session = fx.disconnected_commit_session();
    fx.blox.set_workspace(WorkspaceProbe::Suspended);
    fx.reconcile(&session);

    let states = fx.reconciler.states();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].session_id, session.id);
    assert_eq!(states[0].phase, ReconcilePhase::Waiting);

    fx.blox.set_workspace(WorkspaceProbe::Running);
    fx.reconcile(&session);
    assert!(fx.reconciler.states().is_empty());
}

#[test]
fn the_sweep_forgets_rows_that_moved_on_without_it() {
    let fx = Fixture::new();
    let moved_on = fx.disconnected_commit_session();
    let waiting = fx.disconnected_commit_session();
    fx.blox.set_workspace(WorkspaceProbe::Unreachable {
        kind: UnreachableKind::Unreachable,
        message: "timed out".to_string(),
    });
    fx.reconcile(&moved_on);
    fx.reconcile(&waiting);
    assert_eq!(fx.reconciler.states().len(), 2);

    // Settled by something other than this reconciler (a resume, another
    // instance, recovery), while its last attempt still reads `Waiting`.
    let row = fx.row(&moved_on.id);
    assert!(fx
        .store
        .transition_from_disconnected_observed_at(
            &moved_on.id,
            row.updated_at,
            SessionStatus::Cancelled,
            None,
            Some(&CompletionReason::Interrupted),
        )
        .unwrap());

    // Both rows are inside their backoff: the sweep starts no attempt.
    let calls = fx.blox.calls().len();
    assert_eq!(fx.reconciler.reconcile_due(Trigger::Sweep), 0);
    assert_eq!(fx.blox.calls().len(), calls);

    let states = fx.reconciler.states();
    assert_eq!(states.len(), 1, "only the still-disconnected row is kept");
    assert_eq!(states[0].session_id, waiting.id);
    assert_eq!(states[0].phase, ReconcilePhase::Waiting);

    let event = fx.host.last_reconcile_event().unwrap();
    assert_eq!(event.session_id, moved_on.id);
    assert_eq!(event.phase, ReconcilePhase::Idle);
    assert_eq!(event.reason, None);
    assert_eq!(event.attempts, 0);
    assert_eq!(event.next_attempt_at, None);
    assert_eq!(event.branch_id.as_deref(), Some(fx.branch.id.as_str()));
}

#[test]
fn backoff_doubles_from_a_minute_up_to_half_an_hour() {
    let secs: Vec<u64> = (1..=7).map(|n| backoff(n).as_secs()).collect();
    assert_eq!(secs, vec![60, 120, 240, 480, 960, 1800, 1800]);
    assert_eq!(backoff(u32::MAX), BACKOFF_CAP);
}

#[test]
fn held_work_is_rechecked_from_a_minute_up_to_five() {
    let secs: Vec<u64> = (1..=5).map(|n| held_work_recheck(n).as_secs()).collect();
    assert_eq!(secs, vec![60, 120, 240, 300, 300]);
    assert_eq!(held_work_recheck(u32::MAX), HELD_WORK_RECHECK_CAP);
}

#[test]
fn ws_info_answers_map_to_probes() {
    use crate::blox::{BloxError, WorkspaceInfo};
    let info = |status: Option<&str>| {
        Ok(WorkspaceInfo {
            name: "ws-1".to_string(),
            workstation_id: None,
            status: status.map(str::to_string),
            extra: serde_json::Value::Null,
        })
    };
    assert_eq!(
        probe_from_ws_info(info(Some("running"))),
        WorkspaceProbe::Running
    );
    assert_eq!(
        probe_from_ws_info(info(Some("suspended"))),
        WorkspaceProbe::Suspended
    );
    assert_eq!(
        probe_from_ws_info(info(Some("deleted"))),
        WorkspaceProbe::Deleted
    );
    assert_eq!(
        probe_from_ws_info(info(Some("stopped"))),
        WorkspaceProbe::NotRunning("stopped".to_string())
    );
    assert_eq!(
        probe_from_ws_info(Err(BloxError::CommandFailed(
            "workspace ws-1 Not Found".to_string()
        ))),
        WorkspaceProbe::Deleted
    );
    assert!(matches!(
        probe_from_ws_info(Err(BloxError::NotAuthenticated)),
        WorkspaceProbe::Unreachable {
            kind: UnreachableKind::NotAuthenticated,
            ..
        }
    ));
    assert!(matches!(
        probe_from_ws_info(Err(BloxError::Timeout(15))),
        WorkspaceProbe::Unreachable {
            kind: UnreachableKind::Unreachable,
            ..
        }
    ));
}

#[test]
fn the_reconcile_event_is_camel_case_with_snake_case_values() {
    let event = SessionReconcileEvent {
        session_id: "s".to_string(),
        branch_id: Some("b".to_string()),
        project_id: None,
        phase: ReconcilePhase::Waiting,
        reason: Some(ReconcileReason::new(
            ReconcileReasonKind::NotAuthenticated,
            "sq login",
        )),
        attempts: 2,
        next_attempt_at: Some(5),
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        serde_json::json!({
            "sessionId": "s",
            "branchId": "b",
            "projectId": null,
            "phase": "waiting",
            "reason": { "kind": "not_authenticated", "message": "sq login" },
            "attempts": 2,
            "nextAttemptAt": 5,
        })
    );
    assert_eq!(
        serde_json::to_value(ReconcileReasonKind::BackgroundWork).unwrap(),
        "background_work"
    );
}
