//! Session orchestration — lifecycle, cancellation, status events.
//!
//! This module owns the *lifecycle* of a session but delegates the actual
//! agent protocol work to an [`AgentDriver`](crate::agent::AgentDriver).
//! It:
//!
//! 1. Persists the user message
//! 2. Registers the session for cancellation
//! 3. Spawns a background thread that runs the driver
//! 4. On completion, atomically transitions the DB status
//! 5. Runs completion side effects for pr/push sessions (persisting the
//!    parsed outcome and emitting `pr-created` / `push-completed`, see
//!    [`crate::session_completion`])
//! 6. Emits a single `session-status-changed` event
//!
//! The frontend never sees streaming events — it polls the DB.
//!
//! ## Cancellation & deletion
//!
//! [`SessionRegistry`] tracks every running session via a
//! [`CancellationToken`]. Cancelling a session signals the token, which
//! causes the driver to exit early. The driver is responsible for killing
//! its child process (e.g. via `kill_on_drop`).
//!
//! **Race on delete:** When a session is cancelled and then immediately
//! deleted, the background thread may still attempt a handful of DB writes
//! (status update, trailing message flushes). These are safe because:
//!
//! - `session_messages.session_id` has `REFERENCES sessions(id) ON DELETE
//!   CASCADE` with `PRAGMA foreign_keys=ON`, so INSERTs referencing a
//!   deleted session fail with an FK error rather than creating orphan rows.
//! - UPDATEs to already-cascade-deleted message rows are no-ops (0 rows
//!   affected).
//! - `UPDATE sessions SET status = … WHERE id = ?` on a deleted session is
//!   also a no-op.
//! - All write paths in the background thread use `let _ =` or
//!   `log::error!`, so FK failures are swallowed gracefully.

pub(crate) mod attachments;
mod disconnect;
pub(crate) mod reconcile;

pub(crate) use disconnect::may_still_run_on_workstation;

use std::cell::Cell;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use acp_client::{AgentRunOutcome, McpServer, McpServerHttp};

use crate::actions::{ActionExecutor, ActionRegistry};
use crate::agent::{AcpDriver, MessageWriter};
use crate::commit_reassociation::Reassociation;
use crate::git::Span;
use crate::shell_env::ShellEnvCache;
use crate::store::{
    AcpConfigSelection, Comment, CommentAuthor, CommentType, CompletionReason, FailureStrategy,
    MessageRole, PipelineExecution, PipelineKind, PipelineStep, SessionMessage, SessionStatus,
    StepStatus, StepType, Store,
};

const PIPELINE_STEP_PROMPT_OUTPUT_MAX_CHARS: usize = 30_000;
const PIKCHR_VALIDATION_MAX_REPAIR_ATTEMPTS: usize = 2;

/// [`git_identity_env_from_global_config`] on the blocking pool, for async
/// callers: it runs `git config --global` twice.
pub async fn git_identity_env_off_runtime() -> Vec<(String, String)> {
    tauri::async_runtime::spawn_blocking(git_identity_env_from_global_config)
        .await
        .unwrap_or_default()
}

pub fn git_identity_env_from_global_config() -> Vec<(String, String)> {
    let Some(name) = global_git_config_value("user.name") else {
        return vec![];
    };
    let Some(email) = global_git_config_value("user.email") else {
        return vec![];
    };

    vec![
        ("GIT_AUTHOR_NAME".to_string(), name.clone()),
        ("GIT_AUTHOR_EMAIL".to_string(), email.clone()),
        ("GIT_COMMITTER_NAME".to_string(), name),
        ("GIT_COMMITTER_EMAIL".to_string(), email),
    ]
}

fn global_git_config_value(key: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--global", key])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

// =============================================================================
// Event types
// =============================================================================

/// Emitted when an individual pipeline step changes status.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStepEvent {
    pub session_id: String,
    pub step_index: usize,
    pub label: String,
    pub step_type: StepType,
    pub status: StepStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

/// Emitted when a session's status changes. The only event the frontend needs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusEvent {
    pub session_id: String,
    pub status: String,
    pub error_message: Option<String>,
    pub completion_reason: Option<String>,
    pub rate_limit_resets_at: Option<String>,
    pub rate_limit_kind: Option<String>,
    /// The branch the session runs on. Invariant the frontend load-bears on:
    /// *every* repo-session event carries this — queue-drain, immediate launch,
    /// resume, and terminal emits alike — and a project-session event never
    /// does, because project sessions don't run on a branch.
    /// `activeSessionIds.ts` keys the child-badge query set on its presence, so
    /// a repo-session emit site that omits it silently regresses to a
    /// per-child, always-empty `listChildSessions` round-trip. Also what lets
    /// the frontend register the session and refresh the branch timeline.
    pub branch_id: Option<String>,
    pub project_id: Option<String>,
    pub session_type: Option<String>,
    /// Set only on the `cancelled` a queued session gets when the session it
    /// was gated on did not succeed (see `pipeline_dependency`): it never
    /// ran, so this client never registered it, and its completion is a
    /// "skipped" notice rather than a session end. Omitted when false.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dependency_skipped: bool,
}

/// Emitted while a *running* session is held open past turn end for background
/// work (see [`emit_background_hold`]).
///
/// A presentational sub-state of `running`, not a status of its own: the
/// frontend swaps its running indicator for "waiting on background task (N)"
/// plus a stop affordance until `holding` goes false.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBackgroundHoldEvent {
    pub session_id: String,
    /// Whether the session is holding right now. False withdraws the wait —
    /// a new turn took over, or the session is tearing down.
    pub holding: bool,
    /// Live background tasks the agent is reporting; 0 when not holding.
    pub live_tasks: usize,
    /// The live tasks by name, when the agent names them (typed asyncTasks
    /// announce each spawn's metadata). Raw-mode connections only know the
    /// count, so this stays empty and the frontend keeps the bare-count row.
    pub tasks: Vec<SessionBackgroundHoldTask>,
    pub branch_id: Option<String>,
    pub project_id: Option<String>,
}

/// One named background task in a [`SessionBackgroundHoldEvent`]. The id keys
/// the per-task stop (`stop_session_async_task`); the rest is presentation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBackgroundHoldTask {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub output_file_path: Option<String>,
}

/// The hold a session is reporting right now, as `get_session_background_hold`
/// answers it.
///
/// [`SessionBackgroundHoldEvent`] is emitted on *change*, so a pane that mounts
/// mid-hold — a reloaded window, a newly opened peer window, a pane switched
/// back to — has already missed every report and would render a plain
/// "Thinking…" until the task set next changes (with one long task, that is the
/// whole wait). This is the same payload minus the routing context, so a pane
/// renders the snapshot exactly as it would have rendered the event it missed.
///
/// The cleared default (`holding: false`) is the answer for a session that
/// isn't running, isn't holding, or has never connected.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBackgroundHoldSnapshot {
    pub holding: bool,
    pub live_tasks: usize,
    pub tasks: Vec<SessionBackgroundHoldTask>,
}

impl From<acp_client::BackgroundHoldStatus> for SessionBackgroundHoldSnapshot {
    fn from(status: acp_client::BackgroundHoldStatus) -> Self {
        Self {
            holding: status.holding,
            live_tasks: status.live_tasks,
            tasks: status
                .tasks
                .into_iter()
                .map(|task| SessionBackgroundHoldTask {
                    id: task.id,
                    name: task.name,
                    description: task.description,
                    output_file_path: task.output_file_path,
                })
                .collect(),
        }
    }
}

/// The reset time and limit kind a paused session recovers from, as classified
/// out of the provider's error payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RateLimitRecovery {
    resets_at: Option<String>,
    kind: Option<String>,
}

// =============================================================================
// Session registry — tracks running sessions for cancellation
// =============================================================================

/// Tracks all running sessions so they can be cancelled from the outside.
///
/// Managed as Tauri state. The background thread for each session removes
/// itself from the registry when it exits (regardless of outcome).
pub struct SessionRegistry {
    inner: std::sync::Mutex<RegistryInner>,
}

#[derive(Default)]
struct RegistryInner {
    /// Sessions that have registered a cancellation token and are running.
    running: HashMap<String, Arc<RunningSession>>,
    /// Cancellations requested for sessions that haven't registered yet.
    ///
    /// A queued session flips to `running` in the DB a moment before
    /// `start_session` calls `register`, so a project-session cancel arriving in
    /// that startup window finds nothing in `running`. Recording the intent here
    /// lets `register` apply it the instant the session appears, closing the
    /// race that a fixed-delay retry could miss for slow (e.g. remote) startups.
    pending_cancellations: HashMap<String, CompletionReason>,
    /// Liveness holds (see [`SessionRegistry::hold`]), counted per session id
    /// so overlapping holds on one row release independently.
    held: HashMap<String, usize>,
    /// The epoch the next `register` stamps on its entry.
    next_epoch: u64,
}

/// Which registration an entry came from, so a `deregister` removes only the
/// entry its caller made.
///
/// Entries are keyed by session id and a later registration *replaces* an
/// earlier one — the pipeline handoff relies on that swap. Since a session
/// thread now deregisters after its terminal write, a Resume that wins the
/// row between that write and the deregister registers a successor under the
/// same id; without this the finishing thread would remove the successor's
/// entry, and the stranded-session sweep would end a live turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RegistrationEpoch(u64);

impl RegistrationEpoch {
    /// The bare counter, for records that name a registration (a control
    /// grant's actor) without holding the registry's type.
    pub(crate) fn value(self) -> u64 {
        self.0
    }
}

struct RunningSession {
    epoch: RegistrationEpoch,
    token: CancellationToken,
    cancellation_completion_reason: std::sync::Mutex<Option<CompletionReason>>,
    /// Set by [`SessionRegistry::mark_finished`] just before the owner's
    /// terminal write. The entry stays in the map until after that write (the
    /// stranded-session sweep needs it there), so for a moment a *finished*
    /// turn's entry is what a Resume's `register` finds under the same id.
    /// This is how `register` tells that from the pipeline handoff, the one
    /// predecessor whose accepted cancellation it must carry.
    finished: std::sync::atomic::AtomicBool,
    /// Per-task stop handle for the session's current ACP connection, set once
    /// the connection is up (and replaced when a Pikchr correction
    /// re-connects). `None` until then, or for sessions that never connect
    /// (e.g. externally registered ones).
    async_task_stop: std::sync::Mutex<Option<acp_client::AsyncTaskStopHandle>>,
    /// Turn-control handle for the session's current ACP connection — how a
    /// message reaches the agent *now* (steered into the turn in flight, or
    /// sent in place of it) rather than after the turn. Set and replaced
    /// alongside `async_task_stop`.
    turn_control: std::sync::Mutex<Option<acp_client::TurnControlHandle>>,
    /// Detach handle for the session's current ACP connection, for a remote
    /// session only — how a quit leaves the remote turn running (see
    /// [`SessionRegistry::detach`]). Set and replaced alongside
    /// `async_task_stop`.
    detach: std::sync::Mutex<Option<acp_client::DetachHandle>>,
    /// The completion reason a [`SessionRegistry::detach`] recorded, for the
    /// run to use if the detach lands before the prompt is sent and the run
    /// ends `cancelled` rather than `disconnected`.
    detach_reason: std::sync::Mutex<Option<CompletionReason>>,
    /// Set for a run reattaching to its remote turn (see
    /// [`SessionRegistry::reattaching`]); fired by a detach that finds no
    /// live connection to kill — between the reattach's attempts, or before
    /// its first is up — so the run stops reattaching and leaves the turn
    /// running, as a detach does.
    reattach_detach: std::sync::Mutex<Option<CancellationToken>>,
    /// The last background hold the session's connection reported, so a client
    /// that mounts mid-hold can ask for the wait it missed. The hold is
    /// reported on change only; the driver reports the cleared default on the
    /// way out of every hold, so this returns to "not holding" without needing
    /// its own reset.
    background_hold: std::sync::Mutex<acp_client::BackgroundHoldStatus>,
}

impl RunningSession {
    /// Record the completion reason to persist and signal cancellation. A
    /// `ProjectSessionInterrupted` reason overrides a previously stored one so
    /// an explicit project cancel wins over an in-flight interrupt.
    ///
    /// Always called with the registry lock held — by
    /// [`RegistryInner::cancel_registered`] for an entry already in the map,
    /// and by [`SessionRegistry::register`] for one about to be inserted. That
    /// is what makes [`accepted_cancellation`](Self::accepted_cancellation) a
    /// complete answer: no cancel can be part-way through applying to an entry
    /// a lock-holder is reading, replacing or removing.
    ///
    /// Safe to hold the lock across, because nothing here reaches back into the
    /// registry: the reason mutex is only ever taken *under* the registry lock
    /// (never the reverse), and `CancellationToken::cancel` notifies its
    /// waiters, which schedules the waiting tasks rather than running them
    /// inline.
    fn apply_cancellation(&self, completion_reason: CompletionReason) {
        let mut stored_reason = self.cancellation_completion_reason.lock().unwrap();
        if stored_reason.is_none()
            || matches!(
                completion_reason,
                CompletionReason::ProjectSessionInterrupted
            )
        {
            *stored_reason = Some(completion_reason);
        }
        self.token.cancel();
    }

    /// The cancellation this entry has already accepted, if any.
    ///
    /// `Some` is a commitment, not a status read: the registry answered `true`
    /// to the `cancel` that recorded it, so
    /// [`cancel_session_impl`](crate::session_commands::cancel_session_impl)
    /// took the fast path and wrote no status, leaving this entry's observer
    /// to record the terminal state. Any path that drops the entry without an
    /// observer has to hand that commitment on — see
    /// [`SessionRegistry::register`] and
    /// [`SessionRegistry::register_for_startup`].
    ///
    /// `None` is the matching commitment in the other direction, and only
    /// because cancels apply under the registry lock: read by a lock-holder
    /// that is about to replace or remove this entry, it means no `cancel` has
    /// answered `true` for it, so every later one will find the entry gone,
    /// answer `false`, and write its own status.
    fn accepted_cancellation(&self) -> Option<CompletionReason> {
        self.cancellation_completion_reason.lock().unwrap().clone()
    }

    /// Whether the owner has reached its terminal write (see `finished`).
    fn is_finished(&self) -> bool {
        self.finished.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl RegistryInner {
    /// Apply `completion_reason` to `session_id`'s entry, reporting whether
    /// there was one.
    ///
    /// Taking `&RegistryInner` rather than `&SessionRegistry` is the point: the
    /// only way to call it is through the lock guard, so the lookup and the
    /// [`apply_cancellation`](RunningSession::apply_cancellation) cannot be
    /// split by a `register` or a `deregister` on another thread. Held apart
    /// — the entry cloned out under the lock and cancelled after releasing it
    /// — a cancel could answer `true` and then land on an `Arc` already out of
    /// the map, which is a lost cancellation rather than a late one: the `true`
    /// tells `cancel_session_impl` to write no status, and the entry's
    /// successor (a handoff's replacement) or its remover
    /// ([`SessionRegistry::deregister_reporting_cancellation`]) read `None` and
    /// take over nothing.
    ///
    /// The split was never as wide as it looked, which is why it is worth
    /// naming what now holds it shut. `apply_cancellation` keeps the reason
    /// mutex across `token.cancel()`, and both takeover readers go through
    /// [`accepted_cancellation`](RunningSession::accepted_cancellation), which
    /// needs that same mutex — so a cancel already inside `apply_cancellation`
    /// was serialized against them anyway, leaving only the instant between the
    /// map lookup and that mutex. That is an accident of two unrelated lock
    /// scopes, invisible at both sites and undone by any tidying that releases
    /// the reason guard before firing the token. This lock is the one that is
    /// about the invariant.
    fn cancel_registered(&self, session_id: &str, completion_reason: CompletionReason) -> bool {
        match self.running.get(session_id) {
            Some(running_session) => {
                running_session.apply_cancellation(completion_reason);
                true
            }
            None => false,
        }
    }
}

/// A session startup that failed. Its terminal state — the cancellation (if
/// any) the registry accepted for it while it was registered, else this
/// error — has already been recorded by the time the caller sees this; see
/// [`SessionRegistry::register_for_startup`].
#[derive(Debug)]
struct StartupFailure {
    error: String,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(RegistryInner::default()),
        }
    }

    /// Register a new session and return a `CancellationToken` for it.
    ///
    /// A cancellation already recorded for this session id carries onto the new
    /// entry, from either of the two places one can be waiting:
    ///
    /// - `pending_cancellations`, where [`cancel_or_defer`](Self::cancel_or_defer)
    ///   parks an intent for a session that hasn't registered yet (DB already
    ///   `running`, token not yet registered).
    /// - the entry this call *replaces*. `start_pipeline_session` deliberately
    ///   skips `deregister` on an AI handoff so this insert swaps the token with
    ///   no gap (see [`PipelineOutcome::HandedOffToAi`]), and a cancel landing in
    ///   that swap window fires the pipeline's token — which the pipeline, past
    ///   its own cancellation checkpoints, will never observe — and answers
    ///   `true`, so no status is written. Dropping it here would leave the id in
    ///   shutdown's cancel snapshot with nothing that can honour the cancel:
    ///   `wait_for_sessions` would block on it for the whole `SHUTDOWN_BUDGET`
    ///   and `app.exit(0)` would orphan the agent child.
    ///
    /// Only one can be set at a time — `cancel_or_defer` parks an intent only
    /// when nothing is registered — so the pending intent is simply preferred.
    ///
    /// Both reads are complete because this whole body runs under the one lock
    /// that cancels are applied under (see
    /// [`RegistryInner::cancel_registered`]). A cancel racing the swap is
    /// therefore on one side of it or the other: applied before, and carried;
    /// or after, when it finds the replacement and fires *its* token. Neither
    /// is the lost cancellation a split lookup-then-apply would allow, where
    /// the cancel answers `true` to a caller that writes no status and then
    /// lands on the predecessor this call has already dropped.
    ///
    /// Finding a *live* entry to replace almost always means the handoff: a
    /// session thread deregisters only after its terminal DB write, so while
    /// its entry is present the row is normally still `running` and
    /// `transition_to_running` refuses to start another turn on it. The
    /// exception is a Resume that wins the row in the instant between that
    /// write and the deregister. The returned [`RegistrationEpoch`] is what
    /// keeps the finishing thread's deregister off the successor's entry, and
    /// the predecessor's `finished` mark (set before its terminal write, see
    /// [`mark_finished`](Self::mark_finished)) is what keeps its accepted
    /// cancellation off the successor's token: a Stop recorded on a turn that
    /// has ended was answered, not lost, and starting the resumed turn
    /// pre-cancelled would kill the turn the user just asked for. Only an
    /// unfinished predecessor — the handoff — has a cancellation nobody will
    /// otherwise honour.
    fn register(&self, session_id: &str) -> (CancellationToken, RegistrationEpoch) {
        let token = CancellationToken::new();
        let mut inner = self.inner.lock().unwrap();
        let epoch = RegistrationEpoch(inner.next_epoch);
        inner.next_epoch += 1;
        let running_session = Arc::new(RunningSession {
            epoch,
            token: token.clone(),
            cancellation_completion_reason: std::sync::Mutex::new(None),
            finished: std::sync::atomic::AtomicBool::new(false),
            async_task_stop: std::sync::Mutex::new(None),
            turn_control: std::sync::Mutex::new(None),
            detach: std::sync::Mutex::new(None),
            detach_reason: std::sync::Mutex::new(None),
            reattach_detach: std::sync::Mutex::new(None),
            background_hold: std::sync::Mutex::new(acp_client::BackgroundHoldStatus::default()),
        });
        let pending = inner.pending_cancellations.remove(session_id);
        let carried = pending.or_else(|| {
            inner
                .running
                .get(session_id)
                .filter(|previous| !previous.is_finished())
                .and_then(|previous| previous.accepted_cancellation())
        });
        if let Some(completion_reason) = carried {
            running_session.apply_cancellation(completion_reason);
        }
        inner
            .running
            .insert(session_id.to_string(), running_session);
        (token, epoch)
    }

    /// Remove the entry `epoch` registered, if it is still the one in the map.
    ///
    /// Called by the owning thread or task once it has written the session's
    /// terminal state — never before. A `running` row this process owns with
    /// no entry is, by definition, a row nothing will finish, and that is the
    /// whole of what `session_queue::sweep` acts on; deregistering ahead of
    /// the write would hand it a live session. An entry under the same id with
    /// a different epoch belongs to a successor (see [`RegistrationEpoch`])
    /// and is left alone.
    fn deregister(&self, session_id: &str, epoch: RegistrationEpoch) {
        let mut inner = self.inner.lock().unwrap();
        if inner
            .running
            .get(session_id)
            .is_some_and(|running| running.epoch == epoch)
        {
            inner.running.remove(session_id);
        }
    }

    /// [`deregister`](Self::deregister), reporting the cancellation the entry
    /// had accepted. The read and the removal happen under one lock, so a
    /// Stop either reached the entry first and is reported here, or finds no
    /// entry and takes `cancel_session_impl`'s own path for the row.
    ///
    /// For an owner whose final write leaves the row non-terminal: a Stop
    /// trusts the entry's owner to record it, and a `disconnected` write
    /// records none, so the owner has to hand it on.
    fn deregister_reporting_cancellation_of(
        &self,
        session_id: &str,
        epoch: RegistrationEpoch,
    ) -> Option<CompletionReason> {
        let mut inner = self.inner.lock().unwrap();
        if !inner
            .running
            .get(session_id)
            .is_some_and(|running| running.epoch == epoch)
        {
            return None;
        }
        inner
            .running
            .remove(session_id)
            .and_then(|running| running.accepted_cancellation())
    }

    /// Mark the entry `epoch` registered as having decided its terminal
    /// state, just before the write that records it.
    ///
    /// The entry stays (the sweep needs it until the row has left `running`),
    /// still answers `cancel` with `true` (the owner reads its token once
    /// more after the write, to gate the queued follow-up), and still reports
    /// its accepted cancellation to the takeover readers. What changes is
    /// what a [`register`](Self::register) under the same id inherits from
    /// it: nothing. Only a *live* predecessor — the pipeline handoff — hands
    /// its cancellation on. Epoch-guarded like `deregister`, so a finishing
    /// thread cannot mark a successor's entry.
    fn mark_finished(&self, session_id: &str, epoch: RegistrationEpoch) {
        let inner = self.inner.lock().unwrap();
        if let Some(running) = inner
            .running
            .get(session_id)
            .filter(|running| running.epoch == epoch)
        {
            running
                .finished
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Remove whatever entry `session_id` has and report the cancellation it
    /// had accepted, for a caller that is taking over the entry's job of
    /// recording the terminal state.
    ///
    /// Not what an ordinary [`deregister`](Self::deregister) wants: the session
    /// thread deregisters *after* writing the terminal state itself, and only
    /// its own entry. This takes any entry, because the caller is ending a
    /// session it did not register — the shutdown refusal taking over the
    /// pipeline handoff's entry. A failed startup registered its own entry
    /// and so deregisters by epoch, like the thread would have.
    fn deregister_reporting_cancellation(&self, session_id: &str) -> Option<CompletionReason> {
        let mut inner = self.inner.lock().unwrap();
        let removed = inner.running.remove(session_id);
        removed.and_then(|running| running.accepted_cancellation())
    }

    /// Mark `session_id` as owned by a task that will write its terminal
    /// state, without making it cancellable.
    ///
    /// The lighter of the registry's two entry kinds. A full `register` says
    /// "a thread owns this session and observes its token": `cancel` answers
    /// `true` and writes nothing, trusting the owner to record the cancel.
    /// A hold says only "someone will finish this row": `cancel` does not see
    /// it, so a Stop takes `cancel_session_impl`'s fallback and writes
    /// `cancelled` straight to the row, which is what the immediate pull and
    /// the start paths that hold one already rely on; and shutdown's
    /// `cancel_owned_sessions` does not wait on it. What a hold does is make
    /// [`is_live`](Self::is_live) true, which keeps the stranded-session sweep
    /// off the row.
    ///
    /// Take it *before* the `running` write it covers and drop it after the
    /// terminal write — or after the runner's own `register` has taken over,
    /// for a path that hands the row to `start_session` /
    /// `start_pipeline_session`. Those register at entry, so a hold spanning
    /// the call overlaps the thread's entry and the row is live throughout.
    pub(crate) fn hold(self: &Arc<Self>, session_id: &str) -> LivenessHold {
        *self
            .inner
            .lock()
            .unwrap()
            .held
            .entry(session_id.to_string())
            .or_insert(0) += 1;
        LivenessHold {
            registry: Arc::clone(self),
            session_id: session_id.to_string(),
        }
    }

    fn release_hold(&self, session_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(count) = inner.held.get_mut(session_id) {
            *count -= 1;
            if *count == 0 {
                inner.held.remove(session_id);
            }
        }
    }

    /// Whether something in this process owns `session_id`'s `running` row:
    /// a registered token or a [`hold`](Self::hold). The stranded-session
    /// sweep's one question.
    pub(crate) fn is_live(&self, session_id: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.running.contains_key(session_id) || inner.held.contains_key(session_id)
    }

    /// Register `session_id` around its fallible startup work: the token
    /// exists before `startup` runs, and a failed startup deregisters on the
    /// way out.
    ///
    /// Registering *before* the slow half of session startup — driver
    /// construction resolves the agent binary through login-shell probes, and
    /// probes every known agent when no provider is pinned, so it's seconds of
    /// wall clock, not statements — is what makes a session that is still
    /// starting up visible to cancellation. The shutdown path cancels a
    /// snapshot of registered ids and holds the exit open until they
    /// deregister, and a user cancel only reaches a token the registry can
    /// find. Unregistered, both land nowhere while the startup goes on to
    /// spawn an agent child; registered, they fire this token, which the
    /// session thread takes a last look at before it connects — so a cancel
    /// that lands during the startup spawns no agent child at all, and one
    /// that lands after that look meets the post-spawn check and
    /// `graceful_stop`.
    ///
    /// What registration does *not* buy is a stop bounded by
    /// `SHUTDOWN_BUDGET`. The startup it now covers is blocking and not
    /// token-aware — `doctor` gives each login-shell probe a 10s timeout, and
    /// the unpinned branch pays that per provider — so a cancel is only
    /// *observed* once that work ends, routinely past the 2s budget. Shutdown
    /// then times out on this session, warns, sweeps its row to
    /// cancelled/`app_quit`, and exits, killing the still-probing thread with
    /// the process. That is a clean end rather than a leak precisely because
    /// of the gate before `connect`: the thread has spawned no agent child,
    /// and now never will.
    ///
    /// Deregistering on failure is the other half of the contract: the
    /// session thread that normally deregisters never spawns on that path,
    /// and a stale entry would hold shutdown's `wait_for_sessions` open for
    /// its full budget and misreport `is_running`.
    ///
    /// That deregister is also where a cancellation would go missing, so the
    /// failure reports it instead of dropping it: the cancel that fired this
    /// token was answered `true` and wrote no status, and the thread that would
    /// have recorded the terminal state never spawns. The caller records it —
    /// see [`finish_cancelled_before_run`].
    ///
    /// It records it *inside* `record_failure`, which runs with the entry
    /// still registered, because the row is `running` under our pid until
    /// that write and a `running` row with no entry is what the
    /// stranded-session sweep ends — as `Error` / "Staged lost track of this
    /// session", in place of the real cause. The closure gets the startup
    /// error and the cancellation the entry had accepted when it ran, and
    /// writes whichever terminal state fits; the entry leaves only after it
    /// returns. The cancellation it is handed is a peek, like
    /// [`refused_start_completion_reason`]'s: a Stop landing between the peek
    /// and the removal is answered `true`, writes nothing, and is recorded on
    /// the row as the startup error rather than as `cancelled`. The row is
    /// terminal either way, which is what the Stop asked for; a second peek
    /// ahead of the removal still reports it, so the mismatch is at least
    /// logged.
    ///
    /// The report misses nothing, because the removal and every cancel take the
    /// same lock (see [`RegistryInner::cancel_registered`]): a cancel is either
    /// applied before the entry leaves the map and reported here, or it arrives
    /// after, finds nothing, answers `false`, and is written straight to the
    /// store by `cancel_session_impl`'s fallback.
    fn register_for_startup<T>(
        &self,
        session_id: &str,
        startup: impl FnOnce() -> Result<T, String>,
        record_failure: impl FnOnce(&str, Option<CompletionReason>),
    ) -> Result<(CancellationToken, RegistrationEpoch, T), StartupFailure> {
        let (token, epoch) = self.register(session_id);
        match startup() {
            Ok(value) => Ok((token, epoch, value)),
            Err(error) => {
                // Finished before the write and removed by epoch after it,
                // like the session thread's own exit: a Resume that wins the
                // row between the two registers a successor under this id,
                // which must neither inherit the Stop this entry accepted nor
                // be taken out of the map with it (see `mark_finished`,
                // `deregister`).
                self.mark_finished(session_id, epoch);
                let accepted = self.cancellation_completion_reason(session_id);
                record_failure(&error, accepted.clone());
                // Scoped to this epoch for the same reason the removal is: by
                // now the row is terminal and a Resume may have registered a
                // successor under this id, whose Stop is its own to report.
                let at_removal = self.cancellation_completion_reason_of(session_id, epoch);
                self.deregister(session_id, epoch);
                if accepted.is_none() && at_removal.is_some() {
                    log::info!(
                        "Session {session_id} was cancelled while its failed startup was being \
                         recorded; the row carries the startup error"
                    );
                }
                Err(StartupFailure { error })
            }
        }
    }

    /// Cancel a running session. Returns true if the session was found and
    /// signalled, false if it wasn't running (already finished or unknown).
    pub fn cancel(&self, session_id: &str) -> bool {
        self.cancel_with_completion_reason(session_id, CompletionReason::Interrupted)
    }

    /// Cancel a running session and remember the completion reason it should persist.
    ///
    /// The `true` is a commitment `cancel_session_impl` relies on — it writes
    /// no status of its own — and [`RegistryInner::cancel_registered`] is what
    /// lets the registry keep it: by the time this returns, the cancellation is
    /// recorded on an entry that was still in the map when it was applied, so
    /// whoever takes that entry out sees it and takes over.
    pub fn cancel_with_completion_reason(
        &self,
        session_id: &str,
        completion_reason: CompletionReason,
    ) -> bool {
        self.inner
            .lock()
            .unwrap()
            .cancel_registered(session_id, completion_reason)
    }

    /// Cancel `session_id` if it's running, otherwise record the cancellation so
    /// [`register`](Self::register) applies it the moment the session's token is
    /// registered.
    ///
    /// This closes the startup race where a queued session is already `running`
    /// in the DB but `start_session` hasn't registered its cancellation token
    /// yet: a plain [`cancel_with_completion_reason`](Self::cancel_with_completion_reason)
    /// would find nothing and silently drop the cancellation. Deferring the
    /// intent guarantees it lands however long startup takes (e.g. a remote
    /// review awaiting a network-bound `git rev-parse`), which a single
    /// fixed-delay retry could outlast. The check-or-record *and the cancel it
    /// may choose* happen under one lock, so neither can interleave with a
    /// concurrent `register`: the intent is parked before a registration can
    /// claim to have found none, and a direct cancel is applied before a
    /// replacement can read past it.
    pub fn cancel_or_defer(&self, session_id: &str, completion_reason: CompletionReason) {
        let mut inner = self.inner.lock().unwrap();
        // Registered already (possibly between an earlier cancel attempt and
        // this call) — cancel it directly. Otherwise record the intent for
        // `register` to apply.
        if !inner.cancel_registered(session_id, completion_reason.clone()) {
            inner
                .pending_cancellations
                .insert(session_id.to_string(), completion_reason);
        }
    }

    pub fn cancellation_completion_reason(&self, session_id: &str) -> Option<CompletionReason> {
        self.inner
            .lock()
            .unwrap()
            .running
            .get(session_id)
            .and_then(|running| {
                running
                    .cancellation_completion_reason
                    .lock()
                    .unwrap()
                    .clone()
            })
    }

    /// [`cancellation_completion_reason`](Self::cancellation_completion_reason)
    /// for the entry `epoch` registered only; `None` if a successor has taken
    /// the id over (see [`RegistrationEpoch`]).
    fn cancellation_completion_reason_of(
        &self,
        session_id: &str,
        epoch: RegistrationEpoch,
    ) -> Option<CompletionReason> {
        self.inner
            .lock()
            .unwrap()
            .running
            .get(session_id)
            .filter(|running| running.epoch == epoch)
            .and_then(|running| {
                running
                    .cancellation_completion_reason
                    .lock()
                    .unwrap()
                    .clone()
            })
    }

    /// Returns true if the given session is currently tracked as running.
    pub fn is_running(&self, session_id: &str) -> bool {
        self.inner.lock().unwrap().running.contains_key(session_id)
    }

    /// Store the per-task stop handle for `session_id`'s current ACP
    /// connection, replacing the previous connection's handle (a Pikchr
    /// correction re-connects on the same session).
    fn set_async_task_stop_handle(
        &self,
        session_id: &str,
        handle: acp_client::AsyncTaskStopHandle,
    ) {
        if let Some(running) = self.inner.lock().unwrap().running.get(session_id) {
            *running.async_task_stop.lock().unwrap() = Some(handle);
        }
    }

    /// The per-task stop handle for `session_id`'s current ACP connection —
    /// `None` when the session isn't running or hasn't connected yet.
    pub fn async_task_stop_handle(
        &self,
        session_id: &str,
    ) -> Option<acp_client::AsyncTaskStopHandle> {
        self.inner
            .lock()
            .unwrap()
            .running
            .get(session_id)
            .and_then(|running| running.async_task_stop.lock().unwrap().clone())
    }

    /// Store the turn-control handle for `session_id`'s current ACP
    /// connection, replacing the previous connection's handle (a Pikchr
    /// correction re-connects on the same session).
    pub(crate) fn set_turn_control_handle(
        &self,
        session_id: &str,
        handle: acp_client::TurnControlHandle,
    ) {
        if let Some(running) = self.inner.lock().unwrap().running.get(session_id) {
            *running.turn_control.lock().unwrap() = Some(handle);
        }
    }

    /// The turn-control handle for `session_id`'s current ACP connection —
    /// `None` when the session isn't running or hasn't connected yet (the
    /// startup window between `register` and `connect`).
    pub fn turn_control_handle(&self, session_id: &str) -> Option<acp_client::TurnControlHandle> {
        self.inner
            .lock()
            .unwrap()
            .running
            .get(session_id)
            .and_then(|running| running.turn_control.lock().unwrap().clone())
    }

    /// Store the detach handle for `session_id`'s current remote ACP
    /// connection, replacing the previous connection's handle.
    fn set_detach_handle(&self, session_id: &str, handle: acp_client::DetachHandle) {
        if let Some(running) = self.inner.lock().unwrap().running.get(session_id) {
            *running.detach.lock().unwrap() = Some(handle);
        }
    }

    /// Let go of a remote session without stopping it: the local proxy is
    /// killed outright — no `session/cancel` and no SIGINT, either of which
    /// would stop the turn on the workstation — and the run then writes the
    /// row `disconnected`, or `cancelled` with `reason` if no prompt had been
    /// sent yet.
    ///
    /// The token is deliberately not cancelled: the run must not read this as
    /// a Stop. A run reattaching to its remote turn ([`Self::reattaching`]) is
    /// told to stop reattaching as well, whether or not a connection was up to
    /// kill — a detached attempt ends with the very error the reattach tries
    /// again after — and leaves the row `disconnected` just the same. Returns
    /// `false` when there is nothing to detach — a local session, one not yet
    /// connected, or one already tearing down — in which case the caller
    /// should cancel instead.
    pub(crate) fn detach(&self, session_id: &str, reason: CompletionReason) -> bool {
        let Some(running) = self.inner.lock().unwrap().running.get(session_id).cloned() else {
            return false;
        };
        // Recorded before the kill, so the run can never settle without it.
        *running.detach_reason.lock().unwrap() = Some(reason);
        // The connection first: detached, it leaves the turn running before
        // the reattach lets go of it.
        let handle = running.detach.lock().unwrap().clone();
        let detached = handle.is_some_and(|handle| handle.detach());
        let reattach = running.reattach_detach.lock().unwrap().clone();
        if let Some(reattach) = &reattach {
            reattach.cancel();
        }
        if detached || reattach.is_some() {
            return true;
        }
        *running.detach_reason.lock().unwrap() = None;
        false
    }

    /// Whether [`Self::detach`] would let `session_id` go right now, rather
    /// than leave its caller to cancel it. It checks the same two things,
    /// a live remote connection or a reattach, and changes nothing. The quit
    /// prompt uses it so it counts only the sessions a quit would stop. Only
    /// a snapshot: a Blox session that is still connecting reads `false`
    /// until its connection is up.
    pub(crate) fn would_detach(&self, session_id: &str) -> bool {
        let Some(running) = self.inner.lock().unwrap().running.get(session_id).cloned() else {
            return false;
        };
        let connected = running
            .detach
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(acp_client::DetachHandle::is_armed);
        connected || running.reattach_detach.lock().unwrap().is_some()
    }

    /// Mark `session_id`'s run as reattaching to its remote turn, returning
    /// the token every [`Self::detach`] of the entry fires from then on,
    /// whether or not it also killed a live connection: the reattach stops
    /// (between attempts, while one connects, or once a detached attempt
    /// returns) and tries nothing again, and the run leaves the turn running
    /// on the workstation and the row `disconnected`. `detach`'s only caller
    /// is a quit, so nothing else ends a reattach this way. A token that
    /// never fires for a session with no entry.
    fn reattaching(&self, session_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        if let Some(running) = self.inner.lock().unwrap().running.get(session_id) {
            *running.reattach_detach.lock().unwrap() = Some(token.clone());
        }
        token
    }

    /// [`Self::reattaching`], for tests outside this module that need a
    /// session the quit would detach.
    #[cfg(test)]
    pub(crate) fn reattaching_for_test(&self, session_id: &str) -> CancellationToken {
        self.reattaching(session_id)
    }

    /// The reason [`detach`](Self::detach) recorded on the entry `epoch`
    /// registered, if it detached that entry.
    fn detach_reason_of(
        &self,
        session_id: &str,
        epoch: RegistrationEpoch,
    ) -> Option<CompletionReason> {
        self.inner
            .lock()
            .unwrap()
            .running
            .get(session_id)
            .filter(|running| running.epoch == epoch)
            .and_then(|running| running.detach_reason.lock().unwrap().clone())
    }

    /// Retain the hold `session_id`'s connection just reported, so a client
    /// that mounts after it was emitted can still learn about the wait.
    /// Recorded before the event is emitted, so a client reacting to the event
    /// by querying can never read a staler answer than the one it reacted to.
    fn record_background_hold(&self, session_id: &str, status: acp_client::BackgroundHoldStatus) {
        if let Some(running) = self.inner.lock().unwrap().running.get(session_id) {
            *running.background_hold.lock().unwrap() = status;
        }
    }

    /// The hold `session_id` is reporting right now, as its last report left
    /// it. The cleared default for a session that isn't running (deregistered
    /// on exit), hasn't connected, or was never holding.
    pub fn background_hold(&self, session_id: &str) -> acp_client::BackgroundHoldStatus {
        self.inner
            .lock()
            .unwrap()
            .running
            .get(session_id)
            .map(|running| running.background_hold.lock().unwrap().clone())
            .unwrap_or_default()
    }

    /// Ids of every session this process is currently running.
    ///
    /// The shutdown path uses this to cancel them all: the registry, not the DB,
    /// is what says which running rows belong to *this* process's threads.
    pub fn running_session_ids(&self) -> Vec<String> {
        self.inner.lock().unwrap().running.keys().cloned().collect()
    }

    /// Wait until none of `session_ids` are registered as running, or until
    /// `timeout` elapses. Returns `true` if they all deregistered in time.
    ///
    /// Modelled on `ActionExecutor::wait_for_executions`: session threads
    /// deregister themselves as they exit, so polling the registry is how the
    /// shutdown path learns a cancelled session's agent is actually gone rather
    /// than exiting out from under it.
    pub fn wait_for_sessions(&self, session_ids: &[String], timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;

        loop {
            let all_stopped = {
                let inner = self.inner.lock().unwrap();
                session_ids
                    .iter()
                    .all(|session_id| !inner.running.contains_key(session_id))
            };

            if all_stopped {
                return true;
            }

            if std::time::Instant::now() >= deadline {
                return false;
            }

            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Wait until none of `session_ids` is [live](Self::is_live) — no
    /// registered token and no hold — or until `timeout` elapses. Returns
    /// `true` if they were all released in time.
    ///
    /// [`wait_for_sessions`](Self::wait_for_sessions) is the shutdown's wait
    /// and deliberately ignores holds; a branch teardown has to wait them out
    /// too, since a held row is one whose owner is about to write it.
    pub(crate) fn wait_until_released(&self, session_ids: &[String], timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if session_ids.iter().all(|id| !self.is_live(id)) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Register a session whose work is driven outside `start_session` (e.g. a
    /// pikchr diagram child session run by a `generate_pikchr` worker thread),
    /// so a user cancel reaches the actual work instead of taking
    /// `cancel_session`'s store-write fallback, which the worker never
    /// observes. Returns a guard exposing the session's cancellation token;
    /// dropping the guard deregisters the session.
    ///
    /// The entry carries the same shutdown contract as a `start_session` one,
    /// because `cancel_owned_sessions` and `wait_for_sessions` walk the whole
    /// registry rather than the sessions the runner started. Two obligations
    /// follow, and a caller that spawns an agent process owes both:
    ///
    /// - Hold the guard on whatever owns that process, for as long as it lives.
    ///   Released early — by, say, a request future that merely *awaits* the
    ///   work — the exit is free to proceed over a child still being stopped.
    /// - Claim the slot *before* consulting `app_lifecycle::is_quitting`, never
    ///   after, so the claim is either in the shutdown's snapshot or made
    ///   against a flag it has already published. See
    ///   `pikchr_mcp::reserve_child_session`.
    pub fn register_external(self: &Arc<Self>, session_id: &str) -> ExternalSessionRegistration {
        let (token, epoch) = self.register(session_id);
        ExternalSessionRegistration {
            token,
            epoch,
            registry: Arc::clone(self),
            session_id: session_id.to_string(),
        }
    }
}

/// A liveness hold from [`SessionRegistry::hold`]; dropping it releases the
/// hold.
#[must_use = "a hold keeps the sweep off the row only while it is alive"]
pub(crate) struct LivenessHold {
    registry: Arc<SessionRegistry>,
    session_id: String,
}

impl Drop for LivenessHold {
    fn drop(&mut self) {
        self.registry.release_hold(&self.session_id);
    }
}

/// Registry entry for an externally driven session, from
/// [`SessionRegistry::register_external`]. Holds the session in the registry —
/// where [`SessionRegistry::cancel`] can reach its token — until dropped.
pub struct ExternalSessionRegistration {
    registry: Arc<SessionRegistry>,
    session_id: String,
    token: CancellationToken,
    epoch: RegistrationEpoch,
}

impl ExternalSessionRegistration {
    /// The token [`SessionRegistry::cancel`] fires for this session.
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }
}

impl Drop for ExternalSessionRegistration {
    fn drop(&mut self) {
        self.registry.deregister(&self.session_id, self.epoch);
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Configuration for a session run.
pub struct SessionConfig {
    pub session_id: String,
    pub prompt: String,
    pub working_dir: PathBuf,
    pub agent_session_id: Option<String>,
    /// HEAD SHA before the session starts. When set, post-completion hooks
    /// will check if a new commit was created and update the linked commit
    /// record in the DB.
    pub pre_head_sha: Option<String>,
    /// ACP provider ID (e.g. "goose", "claude"). When `None`, the first
    /// available provider is used.
    pub provider: Option<String>,
    /// When set, the session runs via `blox acp <workspace_name>` instead
    /// of a local agent binary. Commit detection is skipped (no local git).
    pub workspace_name: Option<String>,
    /// Extra environment variables to pass to the agent process.
    pub extra_env: Vec<(String, String)>,
    /// Project ID for MCP tool server (project sessions only).
    /// When set, an MCP server is started and the agent is given access to
    /// `start_repo_session` and `add_project_repo` tools. The MCP server URL
    /// is injected into the ACP session via `NewSessionRequest`.
    pub mcp_project_id: Option<String>,
    /// Action executor for running setup actions in the MCP add_project_repo tool.
    /// Required when `mcp_project_id` is set so the MCP server can run prerun actions.
    pub action_executor: Option<Arc<ActionExecutor>>,
    /// Action registry for tracking running actions in the MCP add_project_repo tool.
    /// Required when `mcp_project_id` is set.
    pub action_registry: Option<Arc<ActionRegistry>>,
    /// Working directory on the remote workspace. When set, this path is sent
    /// to the remote agent in the `NewSessionRequest` instead of `"."`, so
    /// the agent operates in the correct repo directory (e.g.
    /// `/home/bloxer/cash-server` instead of the workspace default).
    pub remote_working_dir: Option<PathBuf>,
    /// Image IDs to include in the prompt. The runner reads the image files,
    /// base64-encodes them, and passes them as content blocks to the driver.
    pub image_ids: Vec<String>,
    /// Queued follow-up row that produced this run, if any.
    pub queued_message_id: Option<String>,
    /// Selected ACP config values to apply after session setup and before the
    /// prompt. Command handlers also store successful selections on the session
    /// row so queued and resumed sessions use their own selection.
    pub acp_config_selection: Option<AcpConfigSelection>,
    /// Branch that owns this session (branch-level sessions only).
    /// Threaded through so terminal events carry the same context as start events.
    pub branch_id: Option<String>,
    /// Project that owns this session. Set for both project-note sessions
    /// (directly) and branch-level sessions (via the branch's project).
    pub project_id: Option<String>,
    /// Expose the `generate_pikchr` MCP tool to this session. Set for local,
    /// note-writing sessions (project notes and local branch notes) so the
    /// agent can generate validated Pikchr diagrams (rendered and layout-checked
    /// for it) before finalizing a note. The server is attached as a *required*
    /// MCP server, so a provider that doesn't support HTTP MCP fails the session
    /// rather than silently dropping the tool. Only honored for local sessions
    /// (`workspace_name.is_none()`); remote sessions can't reach localhost.
    pub expose_pikchr_tools: bool,
    /// Parent project note id for project sessions. Passed to the MCP server so
    /// repo sessions started with `expected_outcome="child_note"` attach their
    /// note to this parent. `None` for non-project sessions.
    pub parent_project_note_id: Option<String>,
    /// Post-turn background hold (session-lifetime feature gate). When set,
    /// the agent connection is held open after each prompt resolves until its
    /// background tasks drain (or the configured hard cap expires), and
    /// post-completion hooks fire on that settled event instead of on the
    /// prompt resolving. `None` preserves the legacy teardown-immediately
    /// behavior; see [`crate::session_commands::default_background_hold`],
    /// which every construction site derives this from and which is now `Some`
    /// unless overridden.
    pub background_hold: Option<acp_client::BackgroundHoldConfig>,
}

/// What a start request did with the session it was handed.
///
/// A start is not always a start: [`start_session`] and
/// [`start_pipeline_session`] refuse one outright when a shutdown has already
/// been claimed. That refusal is not an error — see
/// [`refuse_start_during_shutdown`] — so it needs a way to say "nothing is
/// running" that no caller can turn into a failed session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStartOutcome {
    /// A runner thread has the session.
    Started,
    /// A shutdown was already under way, so nothing was started and the row was
    /// recorded `cancelled`/`app_quit`.
    RefusedShuttingDown,
}

impl SessionStartOutcome {
    /// Whether a runner thread actually took the session. What the queue drain
    /// reports as "started" — a refusal leaves the branch exactly as idle as a
    /// claim that lost its race.
    pub fn started(self) -> bool {
        matches!(self, Self::Started)
    }
}

/// Refuse to start `session_id` because a shutdown has been claimed, leaving
/// the row in the state the quit sweep would have given it.
///
/// The gate this serves is the coverage boundary the drain's own gates leave
/// open. `is_quitting` used to be read only on the queue-drain path, so every
/// other way into a session — the `start_session` and `resume_session`
/// commands, `start_branch_session`, `start_project_session` (all four also
/// dispatchable in web mode), and the four `start_pipeline_session` sites in
/// `prs` — could still enter inside `SHUTDOWN_BUDGET`, *after*
/// `cancel_owned_sessions` took its registry snapshot. Such a session
/// registers, so `sweep_active_sessions` puts its row right, but it is neither
/// cancelled nor waited on: `app.exit(0)` orphans its agent child, and a child
/// in its own process group is the one thing the sweep cannot fix. Every agent
/// start funnels through [`start_session`] and every pipeline through
/// [`start_pipeline_session`], so those two gates cover all of them — including
/// the project-MCP `start_repo_session`, which only ever enqueues a row and
/// drains.
///
/// Writing the row here rather than leaving it to the sweep is what makes the
/// refusal self-contained. The sweep is `shutdown_cleanup`'s *last* step, so a
/// refusal arriving after it — the window between the sweep and `app.exit(0)`
/// — would strand a `running` row under our pid for the next launch to report
/// as an errored session, which is the artifact this branch exists to prevent.
///
/// Dropping the registry entry matters on one path: the pipeline handoff skips
/// `deregister` so [`start_session`]'s `register` can swap the token with no
/// gap (see [`PipelineOutcome::HandedOffToAi`]). A refusal never reaches that
/// `register`, so without this the pipeline's entry would outlive the thread
/// that owned it — `is_running` true forever, and shutdown's
/// `wait_for_sessions` burning its whole budget on a session nothing can stop.
/// Taking the entry out also means taking over its job, so a cancellation it
/// had already accepted becomes the reason we persist, in preference to
/// `AppQuit`: the `cancel` that recorded it was answered `true` and wrote no
/// status. The entry goes *after* the write, like every other deregister: the
/// row is `running` until then, and a `running` row with no entry is what the
/// stranded-session sweep ends.
fn refuse_start_during_shutdown<R: tauri::Runtime>(
    session_id: &str,
    branch_id: Option<String>,
    project_id: Option<String>,
    store: &Store,
    app_handle: &AppHandle<R>,
    registry: &SessionRegistry,
) -> SessionStartOutcome {
    log::info!("Refusing to start session {session_id}: a shutdown is under way");
    // No drain follows this terminal state, unlike every other one: the drain
    // is `is_quitting`-gated and a refusal only happens while that is true, so
    // it would answer `Ok(false)` without touching a row. The branch's queue
    // stays queued and unclaimed, which is what a quit wants — those rows are
    // still available to another instance, and to the next launch.
    let _ = finish_cancelled_before_run(
        session_id,
        branch_id,
        project_id,
        store,
        app_handle,
        refused_start_completion_reason(registry, session_id),
    );
    // The handoff's entry, if this was one; a no-op for every other caller.
    // Unconditional removal is safe here, unlike `register_for_startup`'s
    // failure arm: `start_session` is `is_quitting`-gated, so no successor
    // can register under this id behind the refusal.
    let _ = registry.deregister_reporting_cancellation(session_id);
    SessionStartOutcome::RefusedShuttingDown
}

/// The reason a refused start's terminal write should carry.
///
/// A cancellation the session's entry has accepted wins over `AppQuit` — see
/// [`RunningSession::accepted_cancellation`] for why an entry leaving the
/// registry without an observer has to hand its cancellation on. Only the
/// pipeline handoff has an entry to find here; every other caller reaches
/// [`start_session`] with nothing registered, and gets `AppQuit`. The entry
/// itself is dropped by the caller once the write is done.
fn refused_start_completion_reason(
    registry: &SessionRegistry,
    session_id: &str,
) -> CompletionReason {
    registry
        .cancellation_completion_reason(session_id)
        .unwrap_or(CompletionReason::AppQuit)
}

/// Record the terminal state for a session that never reached its run: one
/// cancelled while its startup was still going and whose startup then failed,
/// or one [`refuse_start_during_shutdown`] turned away.
///
/// Nothing else can. `cancel_session_impl` took the registry's fast path — the
/// token was registered, so `cancel` answered `true` — and wrote no status,
/// leaving the session to record its own terminal state; on a failed startup
/// the thread that would have done that never spawns. Left unrecorded, the
/// Stop produces nothing the user can see: on the pipeline-handoff path
/// `finish_failed_pipeline_handoff_start` wins `transition_from_running` and
/// the row reads `error`/`Crashed`, and on the queued-branch path the `Err`
/// reaches nothing but a log line in the queue kicker's drain loop, leaving
/// the row `running` under our pid for the next launch to report as an
/// errored session.
///
/// Writing `Cancelled` first is also what restores the ordering the
/// pre-registration code had by accident: the `!was_running` fallback wrote it
/// at cancel time, so the startup failure's own `error` transition lost. The
/// error is still returned to the caller and still emitted by it — only the
/// persisted status differs.
///
/// The event is emitted whether or not the transition won, matching the
/// session thread's terminal emit: a row already moved on (deleted, say) still
/// needs the client's `running` state cleaned up.
///
/// Returns whether the transition won, which is what says this path owns the
/// terminal state — and so owes the branch queue the drain that state unblocks.
#[must_use]
fn finish_cancelled_before_run<R: tauri::Runtime>(
    session_id: &str,
    branch_id: Option<String>,
    project_id: Option<String>,
    store: &Store,
    app_handle: &AppHandle<R>,
    completion_reason: CompletionReason,
) -> bool {
    let transitioned = store
        .transition_from_running(
            session_id,
            SessionStatus::Cancelled,
            None,
            Some(&completion_reason),
        )
        .unwrap_or(false);
    log::info!(
        "Session {session_id} was cancelled before its run started (transition won: \
         {transitioned})"
    );
    emit_status(
        app_handle,
        session_id,
        SessionStatus::Cancelled.as_str(),
        None,
        Some(&completion_reason),
        None,
        branch_id,
        project_id,
    );
    transitioned
}

/// Record and announce the terminal state of a branch session whose start
/// failed after its row was already `running` and its caller had already
/// answered.
///
/// The immediate branch start inserts its row and responds before the prompt
/// context is built and before [`start_session`] runs (see
/// `session_commands::complete_running_branch_session_start`), so a failure in
/// either has no caller left to return to: the row has to carry it, the way
/// the session thread's own `Err` arm records a crashed run.
///
/// Gated on winning `transition_from_running` like
/// [`finish_failed_pipeline_handoff_start`]. A Stop that landed first has
/// already written `cancelled` — through `cancel_session_impl`'s fallback
/// while nothing was registered, or through [`finish_cancelled_before_run`] on
/// the way out of `start_session` — and emitted it, and an `error` behind that
/// would talk over the user's Stop. A row that is *gone* still gets the emit,
/// which is all that clears the client's `running` state.
///
/// Returns whether the transition won, which is what says this path owns the
/// terminal state — and so owes the branch queue the drain that state
/// unblocks. The caller drains; it holds the concrete handle the drain needs.
#[must_use]
pub(crate) fn finish_failed_before_run<R: tauri::Runtime>(
    session_id: &str,
    branch_id: Option<String>,
    project_id: Option<String>,
    store: &Store,
    app_handle: &AppHandle<R>,
    error: &str,
) -> bool {
    let transitioned = match store.transition_from_running(
        session_id,
        SessionStatus::Error,
        Some(error),
        Some(&CompletionReason::Crashed),
    ) {
        Ok(true) => {
            log::error!("Session {session_id} failed before its run started: {error}");
            true
        }
        Ok(false) => {
            // Routine, not a fault: a Stop that landed first already wrote
            // `cancelled` (or the row is gone), and that terminal state stands.
            log::debug!(
                "Session {session_id} failed before its run started, but its row was \
                 already finalized: {error}"
            );
            false
        }
        Err(store_error) => {
            // The row may still read `running`; nothing finalizes it until the
            // dead-owner sweep, so this is a fault worth surfacing.
            log::error!(
                "Session {session_id} failed before its run started ({error}), and \
                 recording the failure also failed: {store_error}"
            );
            false
        }
    };
    if transitioned || matches!(store.get_session(session_id), Ok(None)) {
        emit_status(
            app_handle,
            session_id,
            SessionStatus::Error.as_str(),
            Some(error.to_string()),
            Some(&CompletionReason::Crashed),
            None,
            branch_id,
            project_id,
        );
    }
    transitioned
}

/// Read the attachments for a prompt's content blocks: images as they are,
/// PDFs rendered page by page (see `attachments`). An id that cannot be read
/// is logged and skipped, so a bad attachment never blocks the message.
pub(crate) fn load_prompt_attachments(
    store: &Store,
    image_ids: &[String],
) -> attachments::PromptAttachments {
    let mut attachments = attachments::PromptAttachments::default();
    for image_id in image_ids {
        match store.get_image(image_id) {
            Ok(Some(image)) => {
                match crate::store::images::image_file_path(
                    &image.project_id,
                    &image.id,
                    &image.filename,
                ) {
                    Ok(path) => {
                        if let Ok(bytes) = std::fs::read(&path) {
                            attachments.add(&image, &bytes);
                        } else {
                            log::warn!(
                                "Failed to read image file for image {image_id}: {}",
                                path.display()
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to resolve file path for image {image_id}: {e}");
                    }
                }
            }
            Ok(None) => {
                log::warn!("Image {image_id} not found in store, skipping");
            }
            Err(e) => {
                log::warn!("Failed to fetch image {image_id} from store: {e}");
            }
        }
    }
    attachments
}

/// Start a session: persist the user message, spawn the agent, stream to DB.
///
/// Returns immediately — the actual agent work happens on a background task.
/// Emits `session-status-changed` when the session reaches a terminal state.
///
/// If `agent_session_id` is provided, the driver uses it to restore the
/// agent's conversation history before sending the new prompt. This is
/// used by `resume_session` for follow-up turns.
pub fn start_session(
    config: SessionConfig,
    store: Arc<Store>,
    app_handle: AppHandle,
    registry: Arc<SessionRegistry>,
) -> Result<SessionStartOutcome, String> {
    // The gate every agent start passes, whatever raised it — see
    // `refuse_start_during_shutdown`. It sits ahead of the registration
    // deliberately: a session registered here would have to be deregistered
    // again on the way out, and this is the one point where a start can still
    // be declined without anything having been spawned.
    if crate::app_lifecycle::is_quitting(&app_handle) {
        return Ok(refuse_start_during_shutdown(
            &config.session_id,
            config.branch_id.clone(),
            config.project_id.clone(),
            &store,
            &app_handle,
            &registry,
        ));
    }

    // Registered before the driver is constructed, not after — see
    // `register_for_startup` for why the slow construction must run with the
    // token already in the registry (a shutdown or user cancel landing during
    // it would otherwise miss a session about to spawn an agent child).
    // `start_pipeline_session` registers at entry for the same reason, and the
    // pipeline handoff's token-replacement contract (see
    // `PipelineOutcome::HandedOffToAi`) is preserved: the replacement just
    // happens before the slow work instead of after it.
    let transitioned_on_failure = Cell::new(false);
    let started = registry.register_for_startup(
        &config.session_id,
        || {
            // Create the driver eagerly so we fail fast if the agent isn't found.
            // Local sessions without an explicit provider resolve the first available
            // provider and persist it on the session. Review-producing callers resolve
            // a concrete provider before creating their session/review rows.
            // Also track the provider id the driver actually resolved to. The pikchr
            // sub-session (`generate_pikchr`) reuses it so its sub-agent matches the
            // agent the user chose, without re-running the (login-shell) discovery.
            let (driver, resolved_provider_id): (AcpDriver, Option<String>) =
                if let Some(ref ws_name) = config.workspace_name {
                    let mut d = AcpDriver::for_workspace(
                        ws_name,
                        config.provider.as_deref(),
                        &config.session_id,
                    )?;
                    if let Some(ref remote_dir) = config.remote_working_dir {
                        d = d.with_remote_working_dir(remote_dir.clone());
                    }
                    (d, config.provider.clone())
                } else {
                    match &config.provider {
                        Some(id) => (AcpDriver::new(id)?, Some(id.clone())),
                        None => {
                            // Resolve the first available provider and backfill it on
                            // the local session record so consumers see the provider
                            // that actually ran the agent.
                            let providers = crate::agent::discover_providers();
                            let first = providers.first().ok_or_else(|| {
                                "No ACP agent found. Install Goose, Claude Code, Codex, Pi, or Amp and ensure it's on your PATH.".to_string()
                            })?;
                            if let Err(e) =
                                store.set_session_provider(&config.session_id, &first.id)
                            {
                                log::warn!(
                                    "Failed to backfill provider on session {}: {e}",
                                    config.session_id
                                );
                            }
                            (AcpDriver::new(&first.id)?, Some(first.id.clone()))
                        }
                    }
                };

            // Persist the user message right away so it's visible immediately.
            // Include image IDs so the frontend can display them alongside the text.
            // We also mark attached images as session-scoped immediately after so they
            // don't appear in the branch timeline. Both operations are kept together;
            // if set_images_session_id fails we log a warning rather than aborting the
            // session, since the message was already persisted.
            //
            // The prompt's row id marks where this turn starts, so note
            // extraction only considers what the agent wrote in this turn.
            let turn_start_message_id = if let Some(ref queued_message_id) =
                config.queued_message_id
            {
                store
                    .add_session_message_with_images_from_queue(
                        &config.session_id,
                        MessageRole::User,
                        &config.prompt,
                        &config.image_ids,
                        queued_message_id,
                    )
                    .map_err(|e| format!("Failed to persist queued user message: {e}"))?
            } else {
                store
                    .add_session_message_with_images(
                        &config.session_id,
                        MessageRole::User,
                        &config.prompt,
                        &config.image_ids,
                    )
                    .map_err(|e| format!("Failed to persist user message: {e}"))?
            };

            if !config.image_ids.is_empty() {
                if let Err(e) = store.set_images_session_id(&config.image_ids, &config.session_id)
                {
                    log::warn!(
                        "Failed to associate images {:?} with session {}: {e}. \
                         Images may appear orphaned in the branch timeline.",
                        config.image_ids,
                        config.session_id
                    );
                }
            }

            Ok((driver, resolved_provider_id, turn_start_message_id))
        },
        // The failed start's terminal state, written while the entry is still
        // registered (see `register_for_startup`). A Stop the entry accepted
        // during the startup is the state the user asked for; otherwise the
        // row carries the real cause — "No ACP agent found", a missing
        // worktree — rather than being left `running` for the sweep to end
        // as "Staged lost track of this session". This closure finalizes
        // runner-originated failures — driver creation, provider resolution,
        // persisting the user message — for every caller that put the row
        // into `running`: the immediate and queued branch starts (whose own
        // `finish_failed_before_run` then loses the CAS and writes nothing),
        // the pipeline handoff (whose `finish_failed_pipeline_handoff_start`
        // stamps the step off the row this writes), and the plain
        // `start_session` / `resume_session` / `start_project_session`
        // commands. Those commands wrap the runner call in
        // `fail_session_start_on_error` (session_commands.rs), which covers
        // failures in the steps *before* the runner is entered — the project
        // note stub, the persisted ACP config selection — and, for a runner
        // failure, loses the `transition_from_running` CAS to this closure
        // and takes its `Ok(false)` "already finalized" debug arm.
        |error, accepted_cancellation| {
            let transitioned = match accepted_cancellation {
                Some(completion_reason) => finish_cancelled_before_run(
                    &config.session_id,
                    config.branch_id.clone(),
                    config.project_id.clone(),
                    &store,
                    &app_handle,
                    completion_reason,
                ),
                None => finish_failed_before_run(
                    &config.session_id,
                    config.branch_id.clone(),
                    config.project_id.clone(),
                    &store,
                    &app_handle,
                    error,
                ),
            };
            transitioned_on_failure.set(transitioned);
        },
    );
    let (cancel_token, registration, (driver, resolved_provider_id, turn_start_message_id)) =
        match started {
            Ok(started) => started,
            Err(failure) => {
                // A terminal state like any other, so it owes the branch its
                // drain: the session thread's terminal path kicks one for
                // every branch session it ends, and nothing else will do it
                // for a session whose thread never spawned — the `Err` below
                // reaches only a log line in the queue kicker's drain loop.
                // Without this the branch's remaining queued rows sit parked
                // until some unrelated session happens to finish.
                if transitioned_on_failure.get() {
                    drain_queued_after_terminal_state(
                        Arc::clone(&store),
                        Arc::clone(&registry),
                        app_handle.clone(),
                        config.session_id.clone(),
                        config.branch_id.clone(),
                        false,
                        crate::session_queue::KickSource::TerminalState,
                    );
                }
                return Err(failure.error);
            }
        };

    let selected_acp_config_options =
        crate::acp_config::selected_acp_config_options(config.acp_config_selection.as_ref());

    // The commit base lives on the row as well as in `config`, so a session
    // that outlives this runner (`disconnected`) can still find the commits
    // its turn made.
    if let Err(e) =
        store.set_session_pre_head_sha(&config.session_id, config.pre_head_sha.as_deref())
    {
        log::warn!(
            "Failed to record pre-run HEAD on session {}: {e}",
            config.session_id
        );
    }

    // The agent protocol may use !Send futures, so we spin up a dedicated
    // thread with its own single-threaded Tokio runtime + LocalSet.
    let session_id_for_status = config.session_id.clone();
    let store_for_status = Arc::clone(&store);

    std::thread::spawn(move || {
        // The session's stagedctl credential (control_server's class A
        // grant): read-write within this session's own project, delivered
        // through an owner-only per-session file whose path rides in
        // STAGEDCTL_LOCK. Held by this thread for the run's life so its Drop
        // — the revoke, and the file's removal — runs after the terminal
        // write below, and on a panic too. The revoke is by token, never by
        // session id: a resume or queued follow-up can register a successor
        // under this id before this thread exits, and its grant must stay.
        let control_lease = control_grant_request(&config, registration)
            .and_then(|request| crate::control_server::mint_session_lease(&app_handle, request));

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime for session");

        // How the last connection settled, recorded by the session loop below
        // so the terminal completion reason can distinguish a clean turn from a
        // hold the cap truncated. `Cell` because the async block only ever
        // borrows it immutably.
        let settle_reason: Cell<Option<acp_client::SessionSettleReason>> = Cell::new(None);
        // Whether, and in which phase, the last connection lost its transport
        // (see `disconnect::run_leaves_session_disconnected`). Reset with
        // `settle_reason`, for the same reason.
        let transport_loss: Cell<Option<acp_client::TransportLoss>> = Cell::new(None);
        // Whether a Stop's `session/cancel` went unanswered on the last
        // connection, so a remote turn may still be running behind it. Reset
        // with `settle_reason`.
        let cancel_unacknowledged = Cell::new(false);
        // Whether the last connection settled out of its background hold with
        // the work still going (see `SessionSettled::background_work_left`).
        // Reset with `settle_reason`.
        let background_work_left = Cell::new(false);

        let local = tokio::task::LocalSet::new();
        let result = local.block_on(&rt, async {
            // The user's per-agent variables (Doctor settings → Environment
            // variables) go under the session's own: both override the
            // inherited environment, and a session's requirements (a commit
            // session's git dates) stay in force. Local sessions only — a
            // remote session's `extra_env` reaches the `sq blox acp` proxy,
            // not the agent on the workstation.
            let agent_env = match (&config.workspace_name, &resolved_provider_id) {
                (None, Some(provider_id)) => crate::agent::env_vars::read_agent_env(provider_id),
                _ => Vec::new(),
            };
            // A session holding a control credential also gets STAGEDCTL_LOCK
            // pointing at its per-session credential file plus STAGEDCTL_BIN
            // pointing at the bundled sidecar, so the agent can run stagedctl
            // with zero configuration. Sessions with no lease — remote
            // (Blox) ones, which can't reach the socket or the file, and the
            // others `control_grant_request` excludes — get neither.
            let mut session_env = config.extra_env.clone();
            session_env.extend(crate::control_server::stagedctl_env(
                control_lease.as_ref(),
            ));
            let driver = driver
                .with_extra_env(crate::agent::env_vars::merge_agent_env(
                    agent_env,
                    session_env,
                ))
                // Each agent process the driver spawns is registered as this
                // session's until it exits (or the lease ends), so the
                // control server can tell a `stagedctl` run from inside this
                // session's process tree apart from the user's own, whatever
                // credential it presents (see `control_server::ancestry`).
                .with_agent_process_observer(
                    control_lease
                        .as_ref()
                        .map(|lease| lease.agent_process_observer()),
                )
                .with_background_hold(config.background_hold.clone())
                // Publish the hold so the UI can show "waiting on background
                // task (N)" — the session stays `running` throughout, so this
                // is a sub-state of running, not a terminal status.
                .with_background_hold_observer(config.background_hold.as_ref().map(|_| {
                    let app_handle = app_handle.clone();
                    let session_id = config.session_id.clone();
                    let branch_id = config.branch_id.clone();
                    let project_id = config.project_id.clone();
                    let registry = Arc::clone(&registry);
                    Arc::new(move |status: acp_client::BackgroundHoldStatus| {
                        // Retain before emitting: the report is a change
                        // delta, so a pane that mounts mid-hold (or reloads
                        // under one) has to ask for the wait it missed.
                        registry.record_background_hold(&session_id, status.clone());
                        emit_background_hold(
                            &app_handle,
                            &session_id,
                            status,
                            branch_id.clone(),
                            project_id.clone(),
                        );
                    }) as acp_client::BackgroundHoldObserver
                }));

            // For local sessions, hand the driver two env snapshots:
            // - the home/global snapshot, sanitized and PATH-extended, used
            //   only to resolve env-shebang interpreters for bridge startup;
            // - the working-directory snapshot (Hermit-activated), used as the
            //   actual agent process environment so the agent and every
            //   per-command shell it spawns see the same toolchain as pipeline
            //   steps and git ops.
            //
            // This lets npm ACP bridges start with the user's global Node even
            // when a repo-local Hermit Node shadows `node` in the execution
            // environment. Remote/workspace sessions proxy through `sq blox
            // acp` and don't execute locally, so they keep the fallback. A
            // cwd capture failure is non-fatal: log it and let the driver fall
            // back to spawning `$SHELL -ils` and exec'ing the agent.
            //
            // Both snapshots are shaped through `apply_managed_tools_env`
            // after capture, so the managed ACP bridges win over
            // user-installed copies (and over `GOOSE_SEARCH_PATHS` values
            // from the user's shell) for the agent and everything it spawns.
            let driver = if config.workspace_name.is_none() {
                let cache = shell_env_cache();
                let mut home_snapshot =
                    crate::shell_env::home_env_vars_with_extended_path(cache.as_ref()).await;
                crate::acp_tools::apply_managed_tools_env(&mut home_snapshot);
                let driver = driver.with_interpreter_env_snapshot(home_snapshot);
                match cache.get(&config.working_dir).await {
                    Ok(snapshot) => {
                        let mut vars = snapshot.vars().into_owned();
                        crate::acp_tools::apply_managed_tools_env(&mut vars);
                        // Put this instance's stagedctl shim dir on the
                        // agent's PATH so the bare `stagedctl` name resolves
                        // (STAGEDCTL_BIN above is the exact-path fallback,
                        // e.g. when this snapshot capture fails). Only for a
                        // session that holds a credential to use it with.
                        if control_lease.is_some() {
                            crate::control_server::apply_stagedctl_path(&mut vars);
                        }
                        driver.with_env_snapshot(vars)
                    }
                    Err(e) => {
                        log::warn!(
                            "Session {}: failed to capture shell env snapshot for {} \
                             (falling back to interactive-login-shell spawn): {e}",
                            config.session_id,
                            config.working_dir.display()
                        );
                        driver
                    }
                }
            } else {
                driver
            };

            // Accumulate every *required* MCP server into a single vec and
            // attach them in one with_mcp_servers call below. with_mcp_servers
            // replaces the driver's server list, so calling it per-block would
            // clobber earlier additions — and a project note sets both
            // mcp_project_id and expose_pikchr_tools, so that collision is
            // real.
            let mut required_mcp_servers: Vec<McpServer> = Vec::new();

            // Start MCP server for project sessions, injecting it via the ACP
            // NewSessionRequest. This is a *required* server: project sessions
            // are restricted to MCP-capable providers, so a missing transport
            // is a hard error.
            let _mcp_handle = if let Some(ref proj_id) = config.mcp_project_id {
                match crate::project_mcp::start_project_mcp_server(
                    proj_id.clone(),
                    Arc::clone(&store),
                    Arc::clone(&registry),
                    app_handle.clone(),
                    config.action_executor.clone(),
                    config.action_registry.clone(),
                    config.provider.clone(),
                    config.acp_config_selection.clone(),
                    config.parent_project_note_id.clone(),
                    config.session_id.clone(),
                    cancel_token.clone(),
                )
                .await
                {
                    Ok((port, handle)) => {
                        log::info!(
                            "Session {}: MCP server started on port {port}",
                            config.session_id
                        );
                        required_mcp_servers.push(McpServer::Http(McpServerHttp::new(
                            "builderbot",
                            format!("http://127.0.0.1:{port}/mcp"),
                        )));
                        Some(handle)
                    }
                    Err(e) => {
                        log::error!("Failed to start MCP server: {e}");
                        return Err(format!("Failed to start MCP server: {e}"));
                    }
                }
            } else {
                None
            };

            // For local note-writing sessions, additionally attach the pikchr
            // MCP server so the agent can generate validated diagrams (rendered
            // and layout-checked for it). This is a *required* server too: a
            // local note session assumes its provider speaks HTTP MCP, so an
            // unsupported transport fails the session (via the required-transport
            // check) rather than silently dropping the tool. Skipped for remote
            // sessions, which can't reach a localhost server. A *startup* failure
            // (port bind) is a local infra hiccup orthogonal to provider
            // capability, so it stays non-fatal: log and continue without the aid.
            let _pikchr_handle = if config.expose_pikchr_tools && config.workspace_name.is_none()
            {
                // `generate_pikchr` runs a sub-agent under the same provider the
                // session resolved. An empty id is tolerated: the server still
                // starts and `generate_pikchr` errors per-call rather than
                // failing session startup.
                let pikchr_provider = resolved_provider_id.clone().unwrap_or_default();
                match crate::pikchr_mcp::start_pikchr_mcp_server(
                    pikchr_provider,
                    config.session_id.clone(),
                    app_handle.clone(),
                    Arc::clone(&store),
                    Arc::clone(&registry),
                )
                .await
                {
                    Ok((port, handle)) => {
                        log::info!(
                            "Session {}: pikchr MCP server started on port {port}",
                            config.session_id
                        );
                        required_mcp_servers.push(McpServer::Http(McpServerHttp::new(
                            "pikchr",
                            format!("http://127.0.0.1:{port}/mcp"),
                        )));
                        Some(handle)
                    }
                    Err(e) => {
                        log::warn!(
                            "Session {}: failed to start pikchr MCP server \
                                 (continuing without it): {e}",
                            config.session_id
                        );
                        None
                    }
                }
            } else {
                None
            };

            let driver = if required_mcp_servers.is_empty() {
                driver
            } else {
                driver.with_mcp_servers(required_mcp_servers)
            };

            let attachments = load_prompt_attachments(&store, &config.image_ids);

            let mut prompt = attachments.prompt(&config.prompt);
            let mut agent_session_id = config.agent_session_id.clone();
            let mut repair_attempts = 0;
            let mut include_images = true;

            loop {
                let writer = Arc::new(MessageWriter::new(
                    config.session_id.clone(),
                    Arc::clone(&store),
                ));
                let store_trait: Arc<dyn acp_client::Store> = store.clone();
                let writer_trait: Arc<dyn acp_client::MessageWriter> = writer;
                let images = if include_images {
                    attachments.images.as_slice()
                } else {
                    &[]
                };
                include_images = false;

                // Last look before an agent process exists. Nothing between the
                // token's registration and here observes it: first the blocking,
                // non-token-aware half of startup (driver construction's
                // login-shell probes, then the env snapshot capture), then a run
                // of awaits — the project MCP server, the pikchr MCP server, and
                // reading plus base64-encoding the attached images. So a cancel
                // that landed anywhere in there — a Stop, or a quit whose
                // `SHUTDOWN_BUDGET` has since run out — is first observable at
                // this point. A session cancelled as early as its registration
                // therefore still stands both localhost MCP servers up on its way
                // here; they are tasks on this thread's runtime, so they go down
                // with it once the terminal handling below finishes. `connect`
                // spawns the child unconditionally: its own check sits *after*
                // the spawn, ahead of `initialize`, and leaves `graceful_stop`
                // to take the child back down, which only beats a quit's
                // `app.exit(0)` if the startup fit in the budget too. Bailing
                // here leaves nothing to take down. The `generate_pikchr` worker
                // gates its `driver.run` the same way.
                //
                // Past this check the path is synchronous into `cmd.spawn()`,
                // so what remains is a cancel landing inside those statements
                // — and that one the post-spawn check still answers.
                if cancel_token.is_cancelled() {
                    return Ok(AgentRunOutcome::Cancelled);
                }

                // Open a session-scoped connection, then send this turn's
                // prompt over it. Without a background hold the connection
                // still tears the bridge process down as soon as the prompt
                // resolves (so a connection serves exactly one turn); with
                // one, the prompt resolves at hold entry while the bridge
                // keeps serving background work, so wait for the settled
                // event before doing anything downstream — Pikchr
                // validation, post-completion hooks, and the terminal status
                // must all fire on actual teardown, not on the prompt
                // resolving.
                //
                // A Pikchr correction re-connects, so only the last
                // connection's settle reason describes how the session ended.
                settle_reason.set(None);
                transport_loss.set(None);
                cancel_unacknowledged.set(false);
                background_work_left.set(false);
                let turn_result = match driver
                    .connect(
                        &config.session_id,
                        &config.working_dir,
                        &store_trait,
                        &writer_trait,
                        &cancel_token,
                        agent_session_id.as_deref(),
                        &selected_acp_config_options,
                        None,
                    )
                    .await
                {
                    Ok(mut connection) => {
                        // Expose the connection's per-task stop where the
                        // `stop_session_async_task` command (another thread)
                        // can reach it; requests are only served while the
                        // connection holds for background work. The
                        // turn-control handle goes up next to it so a message
                        // for this session can be steered into (or sent in
                        // place of) the turn in flight from another thread.
                        registry.set_async_task_stop_handle(
                            &config.session_id,
                            connection.async_task_stop_handle(),
                        );
                        registry.set_turn_control_handle(
                            &config.session_id,
                            connection.turn_control_handle(),
                        );
                        if config.workspace_name.is_some() {
                            registry.set_detach_handle(
                                &config.session_id,
                                connection.detach_handle(),
                            );
                        }
                        let settled_rx = connection.take_settled_receiver();
                        let turn_result = connection.prompt(&prompt, images).await;
                        match settled_rx {
                            Some(rx) => match rx.await {
                                Ok(settled) => {
                                    settle_reason.set(Some(settled.reason));
                                    transport_loss.set(settled.transport_loss);
                                    cancel_unacknowledged.set(settled.cancel_unacknowledged);
                                    background_work_left.set(settled.background_work_left);
                                    match settled.reason {
                                        acp_client::SessionSettleReason::HeldUntilCap => {
                                            log::warn!(
                                                "Session {}: background hold cap expired before \
                                                 quiescence; post-completion hooks run \
                                                 best-effort",
                                                config.session_id
                                            );
                                        }
                                        acp_client::SessionSettleReason::HoldStopped => {
                                            log::info!(
                                                "Session {}: background hold stopped before \
                                                 quiescence; the completed turn stands and \
                                                 post-completion hooks run best-effort",
                                                config.session_id
                                            );
                                        }
                                        _ => {}
                                    }
                                    settled.fold_turn_result(turn_result)
                                }
                                // The connection task died without settling;
                                // the turn result is the best information
                                // available.
                                Err(_) => turn_result,
                            },
                            None => turn_result,
                        }
                    }
                    Err(e) => Err(e),
                };

                if !should_validate_pikchr_after_turn(&turn_result, cancel_token.is_cancelled()) {
                    return turn_result;
                }

                match validate_latest_session_pikchr(&store, &config.session_id)? {
                    LatestAssistantPikchrValidation::SkippedFinalMessage
                    | LatestAssistantPikchrValidation::NoPikchr
                    | LatestAssistantPikchrValidation::Valid => {
                        return Ok(AgentRunOutcome::Completed);
                    }
                    LatestAssistantPikchrValidation::Invalid(error) => {
                        if repair_attempts >= PIKCHR_VALIDATION_MAX_REPAIR_ATTEMPTS {
                            return Err(format!(
                                "Pikchr validation failed after {} repair attempts: block starting at line {}: {}",
                                PIKCHR_VALIDATION_MAX_REPAIR_ATTEMPTS,
                                error.line_number,
                                error.message
                            ));
                        }

                        repair_attempts += 1;
                        let prompt_is_for_note =
                            session_has_note_artifact(&store, &config.session_id);
                        let corrective_prompt =
                            build_pikchr_correction_prompt(&error, prompt_is_for_note);
                        store
                            .add_session_message(
                                &config.session_id,
                                MessageRole::User,
                                &corrective_prompt,
                            )
                            .map_err(|e| {
                                format!("Failed to persist Pikchr correction prompt: {e}")
                            })?;

                        if let Some(stored_id) =
                            stored_agent_session_id(&store, &config.session_id)?
                        {
                            agent_session_id = Some(stored_id);
                        }
                        if agent_session_id.is_none() {
                            return Err(
                                "Pikchr validation failed but no agent session id is available for correction"
                                    .to_string(),
                            );
                        }
                        prompt = corrective_prompt;
                    }
                }
            }
        });

        if let Err(ref e) = result {
            if config.acp_config_selection.is_some()
                && acp_client::is_config_selection_unavailable_error(e)
            {
                if let Err(clear_err) =
                    store_for_status.set_session_acp_config_selection(&session_id_for_status, None)
                {
                    log::warn!(
                        "Failed to clear stale ACP config selection for session {session_id_for_status}: {clear_err}"
                    );
                }
            }
        }

        // The `sq` proxy went while bloxlet still runs the agent, which may
        // still be finishing the turn: pick it back up there — unless a quit
        // detached it, which means to leave the turn running. The reattached
        // turn ends like a dead owner's does (see `ReattachEnd::settlement`);
        // it was in flight on the agent, so losing it again leaves it there.
        let last_settlement = RunSettlement {
            settle_reason: settle_reason.get(),
            transport_loss: transport_loss.get(),
            cancel_unacknowledged: cancel_unacknowledged.get(),
            background_work_left: background_work_left.get(),
        };
        let (result, settlement, reattached) = match result {
            Err(error)
                if disconnect::run_picks_up_detached_turn(
                    &error,
                    cancel_token.is_cancelled(),
                    last_settlement.settle_reason,
                ) =>
            {
                // Marked before the quit check: a quit after it detaches
                // the reattach, and one before it is seen here.
                let detached = registry.reattaching(&session_id_for_status);
                if crate::app_lifecycle::is_quitting(&app_handle) {
                    (Err(error), last_settlement, false)
                } else {
                    let on_detach_handle = |handle: acp_client::DetachHandle| {
                        registry.set_detach_handle(&session_id_for_status, handle)
                    };
                    let reattached = local.block_on(
                        &rt,
                        reattach_detached_turn(
                            &store_for_status,
                            &config,
                            turn_start_message_id,
                            &cancel_token,
                            &detached,
                            &error,
                            &on_detach_handle,
                        ),
                    );
                    match reattached {
                        Some(end) => {
                            let settlement = end.settlement();
                            (end.result, settlement, true)
                        }
                        None => (Err(error), last_settlement, false),
                    }
                }
            }
            result => (result, last_settlement, false),
        };

        // A remote turn the connection lost — dropped transport, or a quit's
        // detach — may still be running on the workstation, as may background
        // work a hold lost: record the row `disconnected`, with no hooks,
        // completion effects or queue drain.
        if disconnect::run_leaves_session_disconnected(
            config.workspace_name.is_some(),
            cancel_token.is_cancelled(),
            registry.cancellation_completion_reason(&session_id_for_status)
                == Some(CompletionReason::AppQuit),
            result.as_ref().err().map(String::as_str),
            settlement.settle_reason,
            if reattached {
                Some(REATTACHED_TURN_IN_FLIGHT)
            } else {
                settlement.transport_loss
            },
            settlement.background_work_left,
        ) {
            finish_disconnected_run(
                &config,
                &result,
                registration,
                &store_for_status,
                &registry,
                &app_handle,
            );
            return;
        }

        finish_session_run(
            &config,
            result,
            settlement,
            turn_start_message_id,
            &cancel_token,
            registration,
            store_for_status,
            registry,
            app_handle,
        );
    });

    Ok(SessionStartOutcome::Started)
}

/// The control credential a session start asks for, or `None` when it gets
/// none: remote sessions (no path to the socket), sessions with no project
/// (no scope to confine a grant to) and project (MCP) sessions, whose MCP
/// tools stay their only control surface. The gate itself lives with the
/// grants (`control_server::session_grant_applies`); this just reads the
/// config.
fn control_grant_request(
    config: &SessionConfig,
    registration: RegistrationEpoch,
) -> Option<crate::control_server::SessionGrantRequest> {
    if !crate::control_server::session_grant_applies(
        config.workspace_name.as_deref(),
        config.project_id.as_deref(),
        config.mcp_project_id.as_deref(),
    ) {
        return None;
    }
    Some(crate::control_server::SessionGrantRequest {
        session_id: config.session_id.clone(),
        epoch: registration.value(),
        project_id: config.project_id.clone()?,
        branch_id: config.branch_id.clone(),
    })
}

/// How a run's last connection settled, beyond its result: what the
/// session runner reads off [`acp_client::SessionSettled`] to decide how the
/// run ends. A run that never settled (it failed first, or was reattached to
/// after its connection went) has the default: no reason, no loss.
#[derive(Clone, Copy, Debug, Default)]
struct RunSettlement {
    settle_reason: Option<acp_client::SessionSettleReason>,
    transport_loss: Option<acp_client::TransportLoss>,
    /// The Stop's `session/cancel` went unanswered before the connection
    /// settled, so a remote turn may still be running behind it.
    cancel_unacknowledged: bool,
    /// The connection settled out of its background hold with the work still
    /// going (see `SessionSettled::background_work_left`).
    background_work_left: bool,
}

/// The transport verdict of a turn reattached to after its connection went:
/// it was in flight on the agent, so losing it again (or detaching) leaves it
/// there, unless the agent itself was lost.
const REATTACHED_TURN_IN_FLIGHT: acp_client::TransportLoss = acp_client::TransportLoss {
    phase: acp_client::TransportLossPhase::Turn,
};

/// How a reattach to a turn in flight on the agent ended (see
/// [`reattach_agent`]).
#[derive(Debug)]
struct ReattachEnd {
    result: Result<AgentRunOutcome, String>,
    /// A Stop ended the reattach without the agent surely having its
    /// `session/cancel`: no connection was up to send it — the Stop came
    /// during the wait between attempts, or before one connected — or the
    /// connection's went out on a transport already going (see
    /// `acp_client::SessionSettled::cancel_unacknowledged`).
    cancel_unacknowledged: bool,
}

impl ReattachEnd {
    /// An end with no Stop the agent may have missed.
    fn of(result: Result<AgentRunOutcome, String>) -> Self {
        Self {
            result,
            cancel_unacknowledged: false,
        }
    }

    /// How one attempt's connection settled; `Err` when none came up, which
    /// left a Stop meanwhile unsent.
    ///
    /// A Stop on a connection that saw its transport go is unacknowledged
    /// too, whatever the connection made of it: a reattached turn is always
    /// in flight on the agent, and a Stop that lands while a dead proxy is
    /// still being torn down (bloxlet asked what became of the agent) settles
    /// the attempt cancelled without its `session/cancel` ever reaching the
    /// workstation. This is the reattach's form of the first connection's
    /// Stop-over-Turn-loss rule in
    /// [`disconnect::stopped_turn_needs_fresh_cancel`].
    fn settled(
        settled: Result<acp_client::SessionSettled, String>,
        cancel_token: &CancellationToken,
    ) -> Self {
        match settled {
            Ok(settled) => Self {
                result: settled.outcome,
                cancel_unacknowledged: settled.cancel_unacknowledged
                    || (cancel_token.is_cancelled() && settled.transport_loss.is_some()),
            },
            Err(error) => Self {
                result: Err(error),
                cancel_unacknowledged: cancel_token.is_cancelled(),
            },
        }
    }

    /// The run's settlement. The turn was in flight on the agent, so a Stop
    /// it may not have had is cancelled again over a fresh connection (see
    /// [`disconnect::stopped_turn_needs_fresh_cancel`]); one the connection
    /// sent while connected stands.
    fn settlement(&self) -> RunSettlement {
        RunSettlement {
            cancel_unacknowledged: self.cancel_unacknowledged,
            ..RunSettlement::default()
        }
    }
}

/// End a run that [`disconnect::run_leaves_session_disconnected`] says may
/// still be running on its workstation: the row goes `disconnected`, owned by
/// no process and still busy to its branch, with no hooks, completion side
/// effects or queue drain. `registration` is the run's registry entry, which
/// this ends.
fn finish_disconnected_run(
    config: &SessionConfig,
    result: &Result<AgentRunOutcome, String>,
    registration: RegistrationEpoch,
    store: &Store,
    registry: &SessionRegistry,
    app_handle: &AppHandle,
) {
    let session_id = &config.session_id;
    match result {
        Err(e) => log::warn!(
            "Session {session_id} lost its connection with the turn in flight; it may still be \
             running on its workstation ({e})"
        ),
        Ok(_) => log::warn!(
            "Session {session_id} lost its connection holding for background work still \
             running on its workstation; leaving it for reconcile"
        ),
    }
    registry.mark_finished(session_id, registration);
    let transitioned = store
        .transition_from_running(session_id, SessionStatus::Disconnected, None, None)
        .unwrap_or(false);
    let late_stop = disconnect::deregister_disconnected_run(registry, session_id, registration);
    if transitioned {
        emit_status(
            app_handle,
            session_id,
            SessionStatus::Disconnected.as_str(),
            None,
            None,
            None,
            config.branch_id.clone(),
            config.project_id.clone(),
        );
        // A Stop that reached the entry after the run read its token trusted
        // this thread to record it; the `disconnected` row doesn't, so
        // reconcile ends it as for any Stop on one.
        if let Some(reason) = late_stop {
            log::info!(
                "Session {session_id} was stopped as it went disconnected; handing the Stop to \
                 reconcile"
            );
            if let Some(reconciler) = reconcile::active() {
                reconciler.request_stop(session_id, reason);
            }
        }
    }
}

/// The normal completion path every agent run ends through: post-completion
/// hooks for a turn that completed, the terminal state, the completion side
/// effects, the terminal status event, and the queued follow-up and branch
/// drains. `registration` is the run's registry entry, which this ends.
#[allow(clippy::too_many_arguments)]
fn finish_session_run(
    config: &SessionConfig,
    result: Result<AgentRunOutcome, String>,
    settlement: RunSettlement,
    turn_start_message_id: i64,
    cancel_token: &CancellationToken,
    registration: RegistrationEpoch,
    store_for_status: Arc<Store>,
    registry: Arc<SessionRegistry>,
    app_handle: AppHandle,
) {
    let session_id_for_status = config.session_id.clone();
    let cancellation_completion_reason = registry
        .cancellation_completion_reason(&session_id_for_status)
        .unwrap_or(CompletionReason::Interrupted);

    // Transition the session to its terminal state, but only if it is
    // still "running". This prevents a late-arriving "completed" from
    // overwriting a "cancelled" that was set by a concurrent cancel
    // request. If the transition returns false the cancel_session
    // command (or delete) already moved the status — we skip the event
    // to avoid a duplicate.
    //
    // If the session was cancelled and then deleted, these DB writes
    // are harmless no-ops — see module-level docs on the race.
    //
    // Read once: the arms below and the late-Stop check after the hooks
    // need to agree on what "cancelled before the run settled" means.
    let cancel_requested = cancel_token.is_cancelled();
    let cancelled_for_quit = cancellation_completion_reason == CompletionReason::AppQuit;
    let (new_status, error_msg, completion_reason, rate_limit_recovery) = match result {
        Ok(AgentRunOutcome::Cancelled) if cancel_requested => (
            "cancelled",
            None,
            cancellation_completion_reason.clone(),
            None,
        ),
        // Ended without a Stop: the agent cancelled the turn itself, or a
        // detach landed before the prompt was sent and recorded why.
        Ok(AgentRunOutcome::Cancelled) => (
            "cancelled",
            None,
            registry
                .detach_reason_of(&session_id_for_status, registration)
                .unwrap_or(CompletionReason::Interrupted),
            None,
        ),
        // A cancel that landed on a turn which had already completed and
        // held for its background work stopped only the *wait*. Fall
        // through to the completed arm so the stop can't erase the turn's
        // artifacts (commit detection, note generation, pr/push outcomes).
        Ok(AgentRunOutcome::Completed)
            if cancel_requested
                && !completed_turn_survives_late_cancel(settlement.settle_reason) =>
        {
            (
                "cancelled",
                None,
                cancellation_completion_reason.clone(),
                None,
            )
        }
        Ok(AgentRunOutcome::Completed) => (
            "completed",
            None,
            completed_turn_completion_reason(settlement.settle_reason),
            None,
        ),
        Err(ref e) if cancel_requested => {
            log::info!("Session {session_id_for_status} cancelled (error during teardown: {e})");
            ("cancelled", None, cancellation_completion_reason, None)
        }
        Err(ref e) => {
            log::error!("Session {session_id_for_status} failed: {e}");
            let session_snapshot = store_for_status
                .get_session(&session_id_for_status)
                .ok()
                .flatten();
            let provider_metadata = session_snapshot
                .as_ref()
                .and_then(|session| session.provider_metadata.as_ref());
            let provider = session_snapshot
                .as_ref()
                .and_then(|session| session.provider.as_deref())
                .or(config.provider.as_deref());
            // A rate limit pauses the session; anything else fails it, as a
            // lost remote agent when that is what the error says.
            let completion_reason = match completion_reason_for_driver_error(e, provider_metadata) {
                CompletionReason::Crashed => failed_run_completion_reason(e),
                reason => reason,
            };
            let rate_limit_recovery = (completion_reason == CompletionReason::RateLimited)
                .then(|| rate_limit_recovery_details(e, provider_metadata));
            if let Some(recovery) = rate_limit_recovery.as_ref() {
                crate::rate_limit_metrics::record_rate_limit_hit(
                    &session_id_for_status,
                    provider,
                    recovery.kind.as_deref(),
                    recovery.resets_at.is_some(),
                );
            }
            (
                if completion_reason == CompletionReason::RateLimited {
                    "paused"
                } else {
                    "error"
                },
                Some(e.clone()),
                completion_reason,
                rate_limit_recovery,
            )
        }
    };

    // A remote Stop the agent never acknowledged — or one that won over a
    // transport lost mid-turn — may have left the turn running on the
    // workstation: cancel it again over a fresh connection, and say so on the
    // row if that can't be confirmed either.
    let stopped_turn_needs_fresh_cancel = disconnect::stopped_turn_needs_fresh_cancel(
        config.workspace_name.is_some(),
        new_status == "cancelled",
        cancelled_for_quit,
        settlement.cancel_unacknowledged,
        settlement.transport_loss,
    );
    // A Stop during the hold whose out-of-turn cancel nothing showed landing:
    // the turn stands completed, but its background work may run on,
    // unrecorded, so cancel it again the same way. Only logged — the row is
    // not a cancelled one to carry the message.
    if !stopped_turn_needs_fresh_cancel
        && disconnect::held_work_needs_fresh_cancel(
            config.workspace_name.is_some(),
            cancel_requested && !cancelled_for_quit,
            settlement.background_work_left,
        )
    {
        log::warn!(
            "Session {session_id_for_status}: background work outlived the hold's Stop; \
             cancelling over a fresh connection"
        );
        if let Some(message) = reconcile::stop_outliving_work(
            &store_for_status,
            &reconcile::LiveBlox,
            &session_id_for_status,
        ) {
            log::warn!("Session {session_id_for_status}: {message}");
        }
    }
    let error_msg = if stopped_turn_needs_fresh_cancel {
        log::warn!(
            "Session {session_id_for_status}: Stop went unacknowledged ({:?}); cancelling over \
             a fresh connection",
            settlement.transport_loss
        );
        // Returns once the cancel is known, the backfill still going: the row
        // ends now, and the transcript catches up after it (see
        // `reconcile::stop`).
        let announce_app = app_handle.clone();
        reconcile::stop_unacknowledged_turn(
            &store_for_status,
            Arc::new(reconcile::LiveBlox),
            &session_id_for_status,
            move |event| {
                crate::web_server::emit_to_all(&announce_app, "session-reconcile-changed", event)
            },
        )
        .or(error_msg)
    } else {
        error_msg
    };

    let completed_successfully =
        terminal_state_completed_successfully(new_status, &completion_reason);

    // Run post-completion hooks before transitioning status.
    // These detect artifacts produced by the session (commits, notes).
    //
    // `result` only materializes once the connection *settles* (the agent
    // is torn down) — under a background hold that is quiescence or the
    // hold cap, not the prompt resolving — so a commit made by an
    // out-of-turn background continuation is visible to these hooks.
    detect_turn_artifacts(
        config,
        completed_successfully,
        &completion_reason,
        &store_for_status,
        turn_start_message_id,
    );

    // The registry entry is still in place during the hooks, so a Stop
    // that lands while they run fires this token: `cancel` answers `true`
    // and `cancel_session_impl` writes nothing, leaving this thread to
    // record it. (It used to find the entry already gone and write
    // `cancelled` straight to the row; same outcome, different writer.)
    // Only a Stop *newer* than the match above counts — one it already
    // saw may have been folded into `completed` deliberately, see
    // `completed_turn_survives_late_cancel`.
    let (new_status, error_msg, completion_reason) = if !cancel_requested
        && cancel_token.is_cancelled()
    {
        log::info!("Session {session_id_for_status} cancelled during its post-completion hooks");
        let reason = registry
            .cancellation_completion_reason(&session_id_for_status)
            .unwrap_or(CompletionReason::Interrupted);
        ("cancelled", None, reason)
    } else {
        (new_status, error_msg, completion_reason)
    };

    // Decided: from here the entry is a finished turn's, and a Resume that
    // wins the row between the write below and the deregister must not
    // inherit a Stop that was aimed at this turn (see `mark_finished`).
    registry.mark_finished(&session_id_for_status, registration);
    let status_enum = SessionStatus::parse(new_status).unwrap();
    let transitioned = if let Some(recovery) = rate_limit_recovery.as_ref() {
        store_for_status
            .transition_running_to_rate_limited(
                &session_id_for_status,
                error_msg.as_deref(),
                recovery.resets_at.as_deref(),
                recovery.kind.as_deref(),
            )
            .unwrap_or(false)
    } else {
        store_for_status
            .transition_from_running(
                &session_id_for_status,
                status_enum.clone(),
                error_msg.as_deref(),
                Some(&completion_reason),
            )
            .unwrap_or(false)
    };

    // Deregister only now that the terminal state is written: a `running`
    // row this process owns with no registry entry is what the
    // stranded-session sweep ends, so the entry has to outlive the row's
    // time in `running`. Epoch-guarded, because a Resume that wins the row
    // between the write above and this line has registered a successor
    // under the same id (see `RegistrationEpoch`).
    registry.deregister(&session_id_for_status, registration);

    // Parse and persist pr/push outcomes and emit the pr-created /
    // push-completed domain events. Gated on winning the transition so
    // exactly one writer runs them, and emitted before the terminal
    // status event so clients see the outcome first.
    if transitioned {
        crate::session_completion::run_completion_side_effects(
            &store_for_status,
            &app_handle,
            &session_id_for_status,
            config.branch_id.as_deref(),
            new_status,
        );
    }

    // Always emit the terminal status event, even if the DB row was already
    // deleted (e.g. user deleted the pending commit). This lets the frontend
    // clean up sidebar "running" state as a safety net.
    emit_status(
        &app_handle,
        &session_id_for_status,
        new_status,
        error_msg,
        Some(&completion_reason),
        rate_limit_recovery.as_ref(),
        config.branch_id.clone(),
        config.project_id.clone(),
    );

    if transitioned {
        if status_enum == SessionStatus::Paused {
            if let Err(e) = crate::rate_limit_scheduler::record_rate_limit_pause(
                &store_for_status,
                &session_id_for_status,
                crate::store::now_timestamp(),
            ) {
                log::warn!(
                    "Failed to schedule rate-limit recovery for session {session_id_for_status}: {e}"
                );
            }
            // Fall through to the branch queue drain below. A paused
            // session is not running, so work the user queued behind it has
            // to be released or the branch deadlocks: the queue waits for a
            // terminal hook that never fires, while auto-resume waits for
            // the queue to empty. Only the branch queue drains —
            // `completed_successfully` is false for a pause, so this
            // session's own queued follow-up messages keep waiting for the
            // resume that can actually deliver them.
        }

        let branch_id = config.branch_id.clone();
        // Read the token again rather than reusing what the terminal-state
        // match saw: the entry stayed registered through the hooks and up
        // to the `deregister` above, so a Stop landing anywhere in that
        // stretch reached this token. One that landed before the terminal
        // write was recorded by the late-Stop check and is already
        // `cancelled` in the row; one that landed between the write and
        // the deregister changed no row — the turn was over — but still
        // says the user does not want the queued follow-up to run.
        //
        // Naming the shared body rather than a command is the point: the
        // Tauri command and the web dispatcher's `cancel_session` arm both
        // route through `session_commands::cancel_session_impl`, so this
        // holds whichever transport the Stop arrived on. A transport that
        // cancelled through the registry alone would leave the row
        // `running`, let the transition win, and drain the follow-up
        // anyway.
        let should_drain_queued_message =
            queued_follow_up_should_start(completed_successfully, cancel_token.is_cancelled());
        let session_id_for_follow_up = session_id_for_status.clone();
        let action_executor_for_follow_up = config
            .action_executor
            .clone()
            .unwrap_or_else(|| Arc::new(ActionExecutor::new()));
        let action_registry_for_follow_up = config
            .action_registry
            .clone()
            .unwrap_or_else(|| Arc::new(ActionRegistry::new()));

        if should_drain_queued_message || branch_id.is_some() {
            let store_for_follow_up = Arc::clone(&store_for_status);
            let registry_for_follow_up = Arc::clone(&registry);
            let app_handle_for_follow_up = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                if should_drain_queued_message {
                    match crate::session_commands::drain_queued_message_for_session(
                        Arc::clone(&store_for_follow_up),
                        Arc::clone(&registry_for_follow_up),
                        Arc::clone(&action_executor_for_follow_up),
                        Arc::clone(&action_registry_for_follow_up),
                        app_handle_for_follow_up.clone(),
                        session_id_for_follow_up.clone(),
                    )
                    .await
                    {
                        Ok(true) => {
                            log::info!(
                                "Drained queued follow-up message for session {session_id_for_follow_up}"
                            );
                        }
                        Ok(false) => {}
                        Err(e) => {
                            log::error!(
                                "Failed to drain queued follow-up message for session {session_id_for_follow_up}: {e}"
                            );
                        }
                    }
                }

                if let Some(branch_id) = branch_id {
                    crate::session_queue::kick(
                        &app_handle_for_follow_up,
                        &branch_id,
                        crate::session_queue::KickSource::TerminalState,
                    );
                }
            });
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LatestAssistantPikchrValidation {
    SkippedFinalMessage,
    NoPikchr,
    Valid,
    Invalid(crate::pikchr_validation::PikchrValidationError),
}

fn should_validate_pikchr_after_turn(
    result: &Result<AgentRunOutcome, String>,
    is_cancelled: bool,
) -> bool {
    matches!(result, Ok(AgentRunOutcome::Completed)) && !is_cancelled
}

fn validate_latest_session_pikchr(
    store: &Store,
    session_id: &str,
) -> Result<LatestAssistantPikchrValidation, String> {
    let messages = store
        .get_session_messages(session_id)
        .map_err(|e| format!("Failed to load session messages for Pikchr validation: {e}"))?;
    Ok(validate_latest_message_pikchr(&messages))
}

fn validate_latest_message_pikchr(messages: &[SessionMessage]) -> LatestAssistantPikchrValidation {
    let Some(message) = messages.last() else {
        return LatestAssistantPikchrValidation::SkippedFinalMessage;
    };
    if message.role != MessageRole::Assistant {
        return LatestAssistantPikchrValidation::SkippedFinalMessage;
    }

    let blocks = crate::pikchr_validation::extract_pikchr_blocks(&message.content);
    if blocks.is_empty() {
        return LatestAssistantPikchrValidation::NoPikchr;
    }

    match crate::pikchr_validation::validate_pikchr_blocks(&message.content)
        .into_iter()
        .next()
    {
        Some(error) => LatestAssistantPikchrValidation::Invalid(error),
        None => LatestAssistantPikchrValidation::Valid,
    }
}

fn stored_agent_session_id(store: &Store, session_id: &str) -> Result<Option<String>, String> {
    store
        .get_session(session_id)
        .map_err(|e| format!("Failed to load session agent id for Pikchr correction: {e}"))
        .map(|session| {
            session.and_then(|s| {
                crate::session_commands::resumable_agent_session_id(s.agent_id.as_deref())
            })
        })
}

pub(crate) fn session_has_note_artifact(store: &Store, session_id: &str) -> bool {
    store
        .get_note_by_session(session_id)
        .ok()
        .flatten()
        .is_some()
        || store
            .get_project_note_by_session(session_id)
            .ok()
            .flatten()
            .is_some()
}

fn build_pikchr_correction_prompt(
    error: &crate::pikchr_validation::PikchrValidationError,
    note_format_required: bool,
) -> String {
    let note_format = if note_format_required {
        "\n\nThis session is producing a Staged note. Preserve the required final response format:\n\
- Start with a fenced block whose opening line is exactly: ```suggested-next-steps\n\
- Put only a JSON object in that block, with nullable string fields `suggestedNextCommitStep` and `suggestedNextNoteStep`.\n\
- After that block, include a `---` separator on its own line.\n\
- Start the note content immediately after `---` with a markdown H1 (`# <Title>`).\n\
- Do not wrap the note content in code fences."
    } else {
        ""
    };

    format!(
        "<action>\n\
The final assistant response contains an invalid fenced `pikchr` code block.\n\
\n\
The Pikchr block starting at line {} failed to parse:\n\
{}\n\
\n\
Return a complete corrected final response. Fix the Pikchr code so every fenced `pikchr` block parses successfully.\n\
\n\
Do not edit files, run git operations, create commits, or change repository state.\
{note_format}\n\
</action>",
        error.line_number, error.message
    )
}

// =============================================================================
// Pipeline execution
// =============================================================================

/// Configuration for a pipeline-driven session.
pub struct PipelineConfig {
    pub session_id: String,
    /// Original user-facing session prompt. Used when a command failure falls
    /// through to a generic AI handoff without a step-specific prompt.
    pub prompt: String,
    pub steps: Vec<PipelineStep>,
    /// The pipeline execution state persisted with the session. Passed in so
    /// `run_pipeline` doesn't reconstruct it from scratch (the caller already
    /// built and persisted one via `PipelineExecution::from_steps`).
    pub pipeline: PipelineExecution,
    pub working_dir: PathBuf,
    /// HEAD before the deterministic pipeline began. Used by rebase pipelines
    /// when they hand off to AI after conflicts.
    pub pre_head_sha: Option<String>,
    /// ACP provider ID (e.g. "goose", "claude").
    pub provider: Option<String>,
    /// Workspace name for remote branches.
    pub workspace_name: Option<String>,
    /// Remote working directory for remote branches.
    pub remote_working_dir: Option<PathBuf>,
    /// Branch that owns this pipeline session. Copied to `SessionConfig` on
    /// AI handoff and used by `emit_status` for terminal events.
    pub branch_id: Option<String>,
    /// Project that owns this pipeline session.
    pub project_id: Option<String>,
}

/// Result of running a pipeline — tells the caller what happened.
pub enum PipelineOutcome {
    /// All steps succeeded without needing AI.
    CompletedWithoutAi,
    /// An AI handoff occurred; a normal AI session was started with this prompt.
    /// `ai_step_index` is `Some(idx)` when the handoff originated from an
    /// explicit `AiHandoff` step (so we can mark it failed if `start_session`
    /// errors). It is `None` when the handoff came from a failed Command step's
    /// `HandoffToAi` failure strategy — in that case remaining steps are already
    /// marked as skipped and no further pipeline updates are needed.
    HandedOffToAi {
        prompt: String,
        ai_step_index: Option<usize>,
    },
    /// The pipeline was aborted because a step failed AND the configured abort
    /// marker was found in the output. This signals a known, expected failure
    /// (e.g. non-fast-forward rejection) that the frontend handles specially.
    ///
    /// Note: if a step uses `Abort { marker: Some(m) }` but the marker is NOT
    /// found in the output, the pipeline falls through to `HandedOffToAi`
    /// instead — only marker-matched failures produce this variant.
    Aborted { step_index: usize },
    /// The pipeline was cancelled externally.
    Cancelled,
}

enum PipelineCommandResult {
    Completed(Output),
    Cancelled { stdout: Vec<u8>, stderr: Vec<u8> },
}

/// Start a pipeline-driven session. Runs deterministic command steps first,
/// then hands off to AI if needed.
///
/// Like `start_session`, this returns immediately and runs in the background.
pub fn start_pipeline_session(
    config: PipelineConfig,
    store: Arc<Store>,
    app_handle: AppHandle,
    registry: Arc<SessionRegistry>,
) -> Result<SessionStartOutcome, String> {
    // The pipeline half of the shutdown gate — see
    // `refuse_start_during_shutdown`. A pipeline spawns no agent child of its
    // own, but it runs command steps in their own process groups and can hand
    // off to an AI session, so an exit through the middle of one leaves the
    // same mess by a longer route. The artifact resolution matches the
    // `PipelineOutcome::Cancelled` arm below, because that is what this is: a
    // pipeline cancelled before it ran a step.
    if crate::app_lifecycle::is_quitting(&app_handle) {
        resolve_pipeline_artifacts_without_ai(&config, &store, false);
        return Ok(refuse_start_during_shutdown(
            &config.session_id,
            config.branch_id.clone(),
            config.project_id.clone(),
            &store,
            &app_handle,
            &registry,
        ));
    }

    let (cancel_token, registration) = registry.register(&config.session_id);
    let session_id = config.session_id.clone();
    let store_for_status = Arc::clone(&store);

    // We use a dedicated OS thread + single-threaded runtime (matching start_session)
    // because the pipeline may hand off to an AI session that uses !Send futures
    // (via the agent protocol's LocalSet requirement). Keeping the same threading
    // model avoids mixing runtime contexts.
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime for pipeline session");

        let mut config = config;
        let local = tokio::task::LocalSet::new();
        let outcome = local.block_on(&rt, async {
            run_pipeline(&mut config, &store, &app_handle, &cancel_token).await
        });

        match outcome {
            PipelineOutcome::CompletedWithoutAi => {
                // Pipeline completed successfully — transition session to completed.
                resolve_pipeline_artifacts_without_ai(&config, &store_for_status, true);
                let status_enum = SessionStatus::Completed;
                let status_str = status_enum.as_str();
                let reason = CompletionReason::TurnComplete;
                // Marked, written, deregistered — the order `start_session`'s
                // thread keeps: the entry is what keeps the stranded-session
                // sweep off a `running` row, and the mark is what keeps a
                // Resume's successor from inheriting this turn's Stop.
                registry.mark_finished(&session_id, registration);
                let transitioned = store_for_status
                    .transition_from_running(&session_id, status_enum, None, Some(&reason))
                    .unwrap_or(false);
                registry.deregister(&session_id, registration);
                if transitioned {
                    crate::session_completion::run_completion_side_effects(
                        &store_for_status,
                        &app_handle,
                        &session_id,
                        config.branch_id.as_deref(),
                        status_str,
                    );
                }
                emit_status(
                    &app_handle,
                    &session_id,
                    status_str,
                    None,
                    Some(&reason),
                    None,
                    config.branch_id.clone(),
                    config.project_id.clone(),
                );
                if transitioned {
                    drain_queued_after_terminal_state(
                        Arc::clone(&store_for_status),
                        Arc::clone(&registry),
                        app_handle.clone(),
                        session_id.clone(),
                        config.branch_id.clone(),
                        true,
                        crate::session_queue::KickSource::TerminalState,
                    );
                }
            }
            PipelineOutcome::HandedOffToAi {
                prompt,
                ai_step_index,
            } => {
                // The pipeline wants to start an AI session with the built prompt.
                // We reuse the same session_id. The session is still "running".
                //
                // We intentionally skip deregister here: start_session's register()
                // call will atomically replace the old cancel token. This avoids a
                // window where the session has no token registered (during which a
                // cancel request would be silently lost). A cancel that lands on
                // the pipeline's entry *before* that replacement isn't lost either
                // — `register` carries a cancelled predecessor's reason onto the
                // new entry — which matters because this thread is already past
                // every point that would have observed the pipeline's token.
                let pre_head_sha = pre_head_for_pipeline_handoff(&config);
                let extra_env = if store_for_status
                    .get_commit_by_session(&session_id)
                    .ok()
                    .flatten()
                    .is_some()
                {
                    git_identity_env_from_global_config()
                } else {
                    vec![]
                };

                // Try to start the AI session now.
                let ai_config = SessionConfig {
                    session_id: session_id.clone(),
                    prompt,
                    working_dir: config.working_dir.clone(),
                    agent_session_id: None,
                    pre_head_sha,
                    provider: config.provider.clone(),
                    workspace_name: config.workspace_name.clone(),
                    extra_env,
                    mcp_project_id: None,
                    action_executor: None,
                    action_registry: None,
                    remote_working_dir: config.remote_working_dir.clone(),
                    image_ids: vec![],
                    queued_message_id: None,
                    acp_config_selection: None,
                    branch_id: config.branch_id.clone(),
                    project_id: config.project_id.clone(),
                    // Deterministic pipelines hand off to a code-focused AI step,
                    // not a note-writing session.
                    expose_pikchr_tools: false,
                    // Pipelines are branch-scoped and never set up the project MCP
                    // server (mcp_project_id is None above), so there is no parent
                    // project note for a `child_note` outcome to attach to.
                    parent_project_note_id: None,
                    background_hold: crate::session_commands::default_background_hold(),
                };
                let handoff = start_session(
                    ai_config,
                    store_for_status.clone(),
                    app_handle.clone(),
                    Arc::clone(&registry),
                );
                // A refusal has already taken over this session's registry
                // entry — the one this handoff deliberately left in place for
                // `register` to swap — and written the row
                // cancelled/`app_quit`. What it can't know about is the
                // pipeline's own artifact, which still needs the resolution a
                // cancelled pipeline gives it.
                if matches!(handoff, Ok(SessionStartOutcome::RefusedShuttingDown)) {
                    resolve_pipeline_artifacts_without_ai(&config, &store_for_status, false);
                }
                if let Err(e) = handoff {
                    log::error!("Failed to start AI session after pipeline handoff: {e}");
                    resolve_pipeline_artifacts_without_ai(&config, &store_for_status, false);
                    if finish_failed_pipeline_handoff_start(
                        &config,
                        &store_for_status,
                        &registry,
                        registration,
                        &app_handle,
                        &e,
                        ai_step_index,
                    ) {
                        drain_queued_after_terminal_state(
                            Arc::clone(&store_for_status),
                            Arc::clone(&registry),
                            app_handle.clone(),
                            session_id.clone(),
                            config.branch_id.clone(),
                            false,
                            crate::session_queue::KickSource::TerminalState,
                        );
                    }
                }
            }
            PipelineOutcome::Aborted { step_index } => {
                // Pipeline aborted (e.g. non-fast-forward). Mark as completed so
                // the frontend can inspect the pipeline steps for the failure —
                // except for a pull, which has no such affordance and, when it
                // was drained off the branch queue, no one watching it. See
                // `aborted_pipeline_error`.
                resolve_pipeline_artifacts_without_ai(&config, &store_for_status, false);
                let error = aborted_pipeline_error(&config, &store_for_status, step_index);
                // The pipeline ran to its conclusion either way; only the
                // outcome differs, so the completion reason stays the same.
                let reason = CompletionReason::TurnComplete;
                let status_enum = if error.is_some() {
                    SessionStatus::Error
                } else {
                    SessionStatus::Completed
                };
                let status_str = status_enum.as_str();
                registry.mark_finished(&session_id, registration);
                let transitioned = store_for_status
                    .transition_from_running(
                        &session_id,
                        status_enum,
                        error.as_deref(),
                        Some(&reason),
                    )
                    .unwrap_or(false);
                registry.deregister(&session_id, registration);
                if transitioned {
                    // Classifies the abort (e.g. push rejected non-fast-forward)
                    // and emits the push-completed domain event.
                    crate::session_completion::run_completion_side_effects(
                        &store_for_status,
                        &app_handle,
                        &session_id,
                        config.branch_id.as_deref(),
                        status_str,
                    );
                }
                emit_status(
                    &app_handle,
                    &session_id,
                    status_str,
                    error,
                    Some(&reason),
                    None,
                    config.branch_id.clone(),
                    config.project_id.clone(),
                );
                if transitioned {
                    drain_queued_after_terminal_state(
                        Arc::clone(&store_for_status),
                        Arc::clone(&registry),
                        app_handle.clone(),
                        session_id.clone(),
                        config.branch_id.clone(),
                        false,
                        crate::session_queue::KickSource::TerminalState,
                    );
                }
            }
            PipelineOutcome::Cancelled => {
                resolve_pipeline_artifacts_without_ai(&config, &store_for_status, false);
                let reason = registry
                    .cancellation_completion_reason(&session_id)
                    .unwrap_or(CompletionReason::Interrupted);
                registry.mark_finished(&session_id, registration);
                let transitioned = store_for_status
                    .transition_from_running(
                        &session_id,
                        SessionStatus::Cancelled,
                        None,
                        Some(&reason),
                    )
                    .unwrap_or(false);
                registry.deregister(&session_id, registration);
                emit_status(
                    &app_handle,
                    &session_id,
                    "cancelled",
                    None,
                    Some(&reason),
                    None,
                    config.branch_id.clone(),
                    config.project_id.clone(),
                );
                if transitioned {
                    drain_queued_after_terminal_state(
                        Arc::clone(&store_for_status),
                        Arc::clone(&registry),
                        app_handle.clone(),
                        session_id.clone(),
                        config.branch_id.clone(),
                        false,
                        crate::session_queue::KickSource::TerminalState,
                    );
                }
            }
        }
    });

    Ok(SessionStartOutcome::Started)
}

/// Finish a pipeline whose AI handoff couldn't be started: stamp the AiHandoff
/// step, and record and announce the terminal state if nobody has yet. Reports
/// whether this path is the one that wrote it.
///
/// Normally nobody is this path. [`start_session`] records a failed start's
/// terminal state itself, while the entry it took over from this pipeline is
/// still registered (see `register_for_startup`): `error`/`Crashed` with the
/// startup's message, or `cancelled` for a Stop that landed while the driver
/// resolved — and it emits for the write it made, and for a row already gone.
/// The CAS here is the safety net under that, and every emit hangs off
/// winning it, because losing it means another writer has already recorded a
/// terminal state this one must not talk over: an ungated `error` behind a
/// Stop's `cancelled` leaves the row saying one thing and the client's last
/// event another.
///
/// The step is a different matter. Only this path knows which step the
/// handoff came from, so it is stamped "Failed to start AI session: …"
/// whenever the row reads `error` — by this write or by `start_session`'s —
/// and left alone when a Stop's `cancelled` took the row, since a Stop is not
/// the handoff step failing.
fn finish_failed_pipeline_handoff_start<R: tauri::Runtime>(
    config: &PipelineConfig,
    store: &Store,
    registry: &SessionRegistry,
    registration: RegistrationEpoch,
    app_handle: &AppHandle<R>,
    error: &str,
    ai_step_index: Option<usize>,
) -> bool {
    let session_id = &config.session_id;
    // Usually no-ops: `start_session` replaced the pipeline's entry on the
    // way in and its failed startup already removed the replacement. Kept
    // for the shape — marked, written, and the entry, whichever it is, goes
    // after the write.
    registry.mark_finished(session_id, registration);
    let transitioned = store
        .transition_from_running(
            session_id,
            SessionStatus::Error,
            Some(error),
            Some(&CompletionReason::Crashed),
        )
        .unwrap_or(false);
    registry.deregister(session_id, registration);

    let errored = transitioned
        || store
            .get_session(session_id)
            .ok()
            .flatten()
            .is_some_and(|row| row.status == SessionStatus::Error);
    if errored {
        if let Some(step_index) = ai_step_index {
            mark_ai_handoff_step_failed(store, app_handle, session_id, step_index, error);
        }
    }

    if transitioned {
        emit_status(
            app_handle,
            session_id,
            SessionStatus::Error.as_str(),
            Some(error.to_string()),
            Some(&CompletionReason::Crashed),
            None,
            config.branch_id.clone(),
            config.project_id.clone(),
        );
    }

    transitioned
}

/// Stamp the AiHandoff step that couldn't start as failed, and publish it.
fn mark_ai_handoff_step_failed<R: tauri::Runtime>(
    store: &Store,
    app_handle: &AppHandle<R>,
    session_id: &str,
    step_index: usize,
    error: &str,
) {
    let Ok(Some(session)) = store.get_session(session_id) else {
        return;
    };
    let Some(mut pipeline) = session.pipeline else {
        return;
    };
    if step_index >= pipeline.steps.len() {
        return;
    }
    pipeline.steps[step_index].status = StepStatus::Failed;
    pipeline.steps[step_index].error = Some(format!("Failed to start AI session: {error}"));
    pipeline.steps[step_index].completed_at = Some(crate::store::now_timestamp());
    let _ = store.update_session_pipeline(session_id, &pipeline);
    emit_pipeline_step(
        app_handle,
        session_id,
        step_index,
        &pipeline.steps[step_index],
    );
}

/// Error message for an aborted pipeline, or `None` when the abort is an expected
/// outcome the session should still complete with.
///
/// Only a pull produces an error. A push rejected as non-fast-forward completes so
/// the frontend can read the steps and offer a force push, but a pull that can't
/// fast-forward is a dead end whose fix is a user decision (rebase onto origin, or
/// reset to origin) — and a pull drained off the branch queue runs headless, so
/// the error status is the only way the user hears about it.
///
/// The failing step's output is read back from the persisted pipeline, which
/// `run_pipeline` writes before it returns `Aborted`.
fn aborted_pipeline_error(
    config: &PipelineConfig,
    store: &Store,
    step_index: usize,
) -> Option<String> {
    if config.pipeline.kind.as_ref() != Some(&PipelineKind::Pull) {
        return None;
    }

    let step = store
        .get_session(&config.session_id)
        .ok()
        .flatten()
        .and_then(|session| session.pipeline)
        .and_then(|pipeline| pipeline.steps.into_iter().nth(step_index));
    let Some(step) = step else {
        return Some("Pull failed".to_string());
    };

    let detail = step
        .output
        .as_deref()
        .map(str::trim)
        .filter(|output| !output.is_empty());
    Some(match detail {
        Some(detail) => format!("{} failed:\n\n{detail}", step.label),
        None => format!("{} failed", step.label),
    })
}

fn pre_head_for_pipeline_handoff(config: &PipelineConfig) -> Option<String> {
    match config.pipeline.kind.as_ref() {
        Some(PipelineKind::Rebase) => config.pre_head_sha.clone(),
        Some(PipelineKind::Squash) => match current_pipeline_head(config) {
            Ok(head) => Some(head),
            Err(e) => {
                log::warn!("Failed to capture squash handoff HEAD: {e}");
                None
            }
        },
        // A push never rewrites local history and a pull never hands off, so
        // neither has a pre-pipeline HEAD to compare against.
        Some(PipelineKind::Push | PipelineKind::Pull) | None => None,
    }
}

/// A git runner for the checkout this pipeline runs in. `remote_working_dir`
/// is the literal directory `run_remote_pipeline_command` `cd`s into, so every
/// read through this runner is about the same repo the pipeline's own commands
/// were — not whatever sibling clone a bare `ws exec` happens to land in.
fn pipeline_git_runner(config: &PipelineConfig) -> crate::branches::GitRunner<'_> {
    crate::branches::git_runner(
        &config.working_dir,
        config.workspace_name.as_deref(),
        config
            .remote_working_dir
            .as_deref()
            .and_then(|dir| dir.to_str())
            .map(str::to_string),
    )
}

fn current_pipeline_head(config: &PipelineConfig) -> Result<String, String> {
    crate::branches::head_sha(&pipeline_git_runner(config))
}

/// Whether a session's persisted pipeline is a rebase. Read from the session
/// row rather than an in-memory config so it still answers correctly for an AI
/// handoff that outlived the pipeline that started it.
fn session_is_rebase_pipeline(store: &Store, session_id: &str) -> bool {
    store
        .get_session(session_id)
        .ok()
        .flatten()
        .and_then(|session| session.pipeline)
        .and_then(|pipeline| pipeline.kind)
        .is_some_and(|kind| kind == PipelineKind::Rebase)
}

/// Reattach a branch's commit metadata (and reviews) to the SHAs a rebase
/// rewrote them into, and report whether the rebase had finished — callers that
/// must not touch a row mid-rebase gate on the returned
/// [`Reassociation::MidRebase`].
///
/// `git` is the caller's own runner, so the reassociation reads the checkout
/// the caller just read HEAD from rather than resolving a second one.
///
/// Best-effort: a failure here only leaves the rows orphaned, which is what
/// happened before reassociation existed, so an `Err` folds into `Done`. It can
/// only be raised once HEAD was seen attached, so the caller's detection should
/// carry on regardless — `MidRebase` is the only answer that means "defer".
/// A branch row that won't load is the exception: nothing was checked, so it
/// reads as `MidRebase`, the same "don't touch anything" direction.
fn reassociate_rebased_commits(
    store: &Store,
    branch_id: &str,
    git: &crate::branches::GitRunner<'_>,
) -> Reassociation {
    let branch = match store.get_branch(branch_id) {
        Ok(Some(branch)) => branch,
        Ok(None) => {
            log::warn!("Branch {branch_id} not found for commit reassociation");
            return Reassociation::MidRebase;
        }
        Err(e) => {
            log::warn!("Failed to load branch {branch_id} for commit reassociation: {e}");
            return Reassociation::MidRebase;
        }
    };

    match crate::commit_reassociation::reassociate_after_rebase(store, &branch, git) {
        Ok(Reassociation::Done { remapped: 0 }) => Reassociation::Done { remapped: 0 },
        Ok(Reassociation::Done { remapped }) => {
            log::info!("Reassociated {remapped} rebased commit(s) on branch {branch_id}");
            Reassociation::Done { remapped }
        }
        // Already logged inside the module, with the branch name it wanted.
        Ok(Reassociation::MidRebase) => Reassociation::MidRebase,
        Err(e) => {
            log::warn!("Failed to reassociate rebased commits on branch {branch_id}: {e}");
            Reassociation::Done { remapped: 0 }
        }
    }
}

fn resolve_pipeline_artifacts_without_ai(config: &PipelineConfig, store: &Store, completed: bool) {
    match config.pipeline.kind.as_ref() {
        Some(PipelineKind::Rebase) if completed => {
            finalize_rebase_pipeline_without_ai(config, store);
        }
        Some(PipelineKind::Rebase | PipelineKind::Squash) => {
            if let Err(e) = store.delete_pending_commit_for_session(&config.session_id) {
                log::warn!(
                    "Failed to resolve pending commit for terminal pipeline session {}: {e}",
                    config.session_id
                );
            }
        }
        // Push and pull pipelines create no artifact, so there is nothing to
        // resolve.
        Some(PipelineKind::Push | PipelineKind::Pull) | None => {}
    }
}

fn finalize_rebase_pipeline_without_ai(config: &PipelineConfig, store: &Store) {
    let Some(pre_head_sha) = config.pre_head_sha.as_deref() else {
        let _ = store.delete_pending_commit_for_session(&config.session_id);
        return;
    };

    let commit = match store.get_commit_by_session(&config.session_id) {
        Ok(Some(commit)) => commit,
        Ok(None) => return,
        Err(e) => {
            log::warn!(
                "Failed to load pending commit for rebase pipeline session {}: {e}",
                config.session_id
            );
            return;
        }
    };

    // One runner for both reads below, so the HEAD this claims and the commits
    // reassociation lists can't come from different checkouts.
    let git = pipeline_git_runner(config);
    let current_head = match crate::branches::head_sha(&git) {
        Ok(head) => head,
        Err(e) => {
            log::error!(
                "Failed to get HEAD after rebase pipeline session {}: {e}",
                config.session_id
            );
            return;
        }
    };

    if current_head == pre_head_sha {
        if let Err(e) = store.delete_pending_commit_for_session(&config.session_id) {
            log::warn!(
                "Failed to remove no-op rebase pending commit for session {}: {e}",
                config.session_id
            );
        }
        return;
    }

    // The rebase rewrote every SHA on the branch, orphaning the metadata rows
    // that pointed at the old ones. Reattach them *before* claiming the new
    // HEAD: once the pre-existing head row owns it, `complete_pending_commit_sha`
    // takes its "target SHA already owned" branch and drops this pipeline's
    // pending row — so the top commit keeps its authoring session instead of
    // the mechanical "Rebase branch" one. A head commit authored outside
    // Staged has no prior row, so the rebase session keeps it, as before.
    //
    // This path runs when the pipeline drove the rebase to completion itself,
    // and its own HEAD-moved check above already established there's something
    // to claim, so the returned outcome isn't a gate here: a `MidRebase` answer
    // (an unattached HEAD, or a branch that wouldn't resolve) just means the
    // rows stay orphaned while the claim below proceeds as it always has.
    let _ = reassociate_rebased_commits(store, &commit.branch_id, &git);

    match store.complete_pending_commit_sha(&commit.id, &commit.branch_id, &current_head) {
        Ok(true) => log::info!(
            "Rebase pipeline session {} updated pending commit to {}",
            config.session_id,
            &current_head[..7.min(current_head.len())]
        ),
        Ok(false) => log::info!(
            "Rebase pipeline session {} resolved duplicate commit SHA {}",
            config.session_id,
            &current_head[..7.min(current_head.len())]
        ),
        Err(e) => log::error!(
            "Failed to complete rebase pending commit for session {}: {e}",
            config.session_id
        ),
    }
}

/// Kick the queue progression a session's terminal state unblocks: its own
/// queued follow-up message when the turn earned one, and the next queued
/// session on its branch — or, for a project session, the branches of its
/// project with a deferred push armed, which wait for the project to go idle.
///
/// Every caller gates this on winning `transition_from_running` — losing means
/// another writer owns the terminal state, and the drain with it. Shared by the
/// pipeline's four terminal arms and by the startup-failure path, whose row is
/// just as terminal (`cancelled`, written by [`finish_cancelled_before_run`])
/// and whose branch queue would otherwise sit parked until some unrelated
/// session happened to finish.
pub(crate) fn drain_queued_after_terminal_state(
    store: Arc<Store>,
    registry: Arc<SessionRegistry>,
    app_handle: AppHandle,
    session_id: String,
    branch_id: Option<String>,
    drain_message: bool,
    kick_source: crate::session_queue::KickSource,
) {
    tauri::async_runtime::spawn(async move {
        if drain_message {
            match crate::session_commands::drain_queued_message_for_session(
                Arc::clone(&store),
                Arc::clone(&registry),
                Arc::new(ActionExecutor::new()),
                Arc::new(ActionRegistry::new()),
                app_handle.clone(),
                session_id.clone(),
            )
            .await
            {
                Ok(true) => log::info!("Drained queued follow-up message for session {session_id}"),
                Ok(false) => {}
                Err(e) => log::error!(
                    "Failed to drain queued follow-up message after a terminal state for session {session_id}: {e}"
                ),
            }
        }

        match branch_id {
            Some(branch_id) => crate::session_queue::kick(&app_handle, &branch_id, kick_source),
            // A project-note session (any other branchless session resolves to
            // no project and kicks nothing). Its end is invisible to the
            // branch queues, so the deferred pushes waiting on it are kicked
            // from here; the drain re-checks that the project is idle.
            None => match store.get_project_id_for_session(&session_id) {
                Ok(Some(project_id)) => crate::session_queue::kick_branches_with_deferred_push(
                    &app_handle,
                    &store,
                    Some(&project_id),
                    crate::session_queue::KickSource::ProjectSessionEnded,
                ),
                Ok(None) => {}
                Err(e) => log::warn!(
                    "Could not resolve the project of ended session {session_id} for its deferred pushes: {e}"
                ),
            },
        }
    });
}

/// Execute pipeline steps sequentially, emitting events as each step progresses.
async fn run_pipeline(
    config: &mut PipelineConfig,
    store: &Arc<Store>,
    app_handle: &AppHandle,
    cancel_token: &CancellationToken,
) -> PipelineOutcome {
    // Capture HEAD before the first step for rebase pipelines. This is deferred
    // from session creation so the (potentially slow) remote HEAD lookup doesn't
    // block session visibility in the UI.
    if config.pre_head_sha.is_none() && config.pipeline.kind.as_ref() == Some(&PipelineKind::Rebase)
    {
        match current_pipeline_head(config) {
            Ok(head) => config.pre_head_sha = Some(head),
            Err(e) => log::warn!("Failed to capture pre-rebase HEAD: {e}"),
        }
    }

    // Use the pipeline execution state that was already persisted with the
    // session, rather than reconstructing from step definitions. This keeps
    // the data flow clear: callers build + persist the pipeline, and we
    // mutate that same instance as steps progress.
    let mut execution = config.pipeline.clone();

    // Collect outputs from prior steps for template substitution.
    let mut step_outputs: Vec<String> = Vec::new();

    for (idx, step) in config.steps.iter().enumerate() {
        if cancel_token.is_cancelled() {
            // Mark remaining steps as skipped.
            for remaining in execution.steps[idx..].iter_mut() {
                remaining.status = StepStatus::Skipped;
            }
            let _ = store.update_session_pipeline(&config.session_id, &execution);
            return PipelineOutcome::Cancelled;
        }

        execution.current_step = idx;

        match step {
            PipelineStep::Command {
                label,
                command,
                on_failure,
            } => {
                // Mark step as running.
                execution.steps[idx].status = StepStatus::Running;
                execution.steps[idx].started_at = Some(crate::store::now_timestamp());
                let _ = store.update_session_pipeline(&config.session_id, &execution);
                emit_pipeline_step(app_handle, &config.session_id, idx, &execution.steps[idx]);

                // Execute the shell command — locally or on a remote workspace.
                let result = if let Some(ref ws_name) = config.workspace_name {
                    run_remote_pipeline_command(
                        command,
                        ws_name,
                        config
                            .remote_working_dir
                            .as_deref()
                            .and_then(|p| p.to_str()),
                        cancel_token,
                    )
                    .await
                } else {
                    run_pipeline_command(command, &config.working_dir, cancel_token).await
                };

                match result {
                    Ok(PipelineCommandResult::Completed(output)) => {
                        let combined =
                            combine_normalized_command_output(&output.stdout, &output.stderr);

                        if output.status.success() {
                            execution.steps[idx].status = StepStatus::Succeeded;
                            execution.steps[idx].output = Some(combined.clone());
                            execution.steps[idx].completed_at = Some(crate::store::now_timestamp());
                            let _ = store.update_session_pipeline(&config.session_id, &execution);
                            emit_pipeline_step(
                                app_handle,
                                &config.session_id,
                                idx,
                                &execution.steps[idx],
                            );
                            step_outputs.push(format_step_output_for_prompt(
                                label, command, &combined, false,
                            ));
                        } else {
                            // Command failed — apply failure strategy.
                            execution.steps[idx].status = StepStatus::Failed;
                            execution.steps[idx].output = Some(combined.clone());
                            execution.steps[idx].error =
                                Some(format!("Exit code: {}", output.status));
                            execution.steps[idx].completed_at = Some(crate::store::now_timestamp());

                            step_outputs.push(format_step_output_for_prompt(
                                label, command, &combined, true,
                            ));

                            match on_failure {
                                FailureStrategy::Abort { marker } => {
                                    if let Some(m) = marker {
                                        // combined is already display-normalized, so only
                                        // strip hostile chars for marker matching.
                                        let marker_output =
                                            crate::terminal_output::strip_prompt_hostile_chars(
                                                &combined,
                                            );
                                        if marker_output.contains(m.as_str()) {
                                            // Mark remaining steps as skipped.
                                            for remaining in execution.steps[idx + 1..].iter_mut() {
                                                remaining.status = StepStatus::Skipped;
                                            }
                                            let _ = store.update_session_pipeline(
                                                &config.session_id,
                                                &execution,
                                            );
                                            emit_pipeline_step(
                                                app_handle,
                                                &config.session_id,
                                                idx,
                                                &execution.steps[idx],
                                            );
                                            return PipelineOutcome::Aborted { step_index: idx };
                                        }
                                        // Marker not found — fall through to AI handoff.
                                        let prompt = format!(
                                            "{}\n\nStep '{}' failed. Diagnose and fix using the output below:\n\n{}",
                                            config.prompt,
                                            label,
                                            step_outputs.join("\n\n")
                                        );
                                        for remaining in execution.steps[idx + 1..].iter_mut() {
                                            remaining.status = StepStatus::Skipped;
                                        }
                                        let _ = store.update_session_pipeline(
                                            &config.session_id,
                                            &execution,
                                        );
                                        emit_pipeline_step(
                                            app_handle,
                                            &config.session_id,
                                            idx,
                                            &execution.steps[idx],
                                        );
                                        return PipelineOutcome::HandedOffToAi {
                                            prompt,
                                            ai_step_index: None,
                                        };
                                    }
                                    // No marker — always abort.
                                    for remaining in execution.steps[idx + 1..].iter_mut() {
                                        remaining.status = StepStatus::Skipped;
                                    }
                                    let _ = store
                                        .update_session_pipeline(&config.session_id, &execution);
                                    emit_pipeline_step(
                                        app_handle,
                                        &config.session_id,
                                        idx,
                                        &execution.steps[idx],
                                    );
                                    return PipelineOutcome::Aborted { step_index: idx };
                                }
                                FailureStrategy::HandoffToAi { prompt_template } => {
                                    let prompt = prompt_template
                                        .replace("{step_outputs}", &step_outputs.join("\n\n"));
                                    for remaining in execution.steps[idx + 1..].iter_mut() {
                                        remaining.status = StepStatus::Skipped;
                                    }
                                    let _ = store
                                        .update_session_pipeline(&config.session_id, &execution);
                                    emit_pipeline_step(
                                        app_handle,
                                        &config.session_id,
                                        idx,
                                        &execution.steps[idx],
                                    );
                                    return PipelineOutcome::HandedOffToAi {
                                        prompt,
                                        ai_step_index: None,
                                    };
                                }
                                FailureStrategy::Continue => {
                                    let _ = store
                                        .update_session_pipeline(&config.session_id, &execution);
                                    emit_pipeline_step(
                                        app_handle,
                                        &config.session_id,
                                        idx,
                                        &execution.steps[idx],
                                    );
                                    // Continue to next step.
                                }
                            }
                        }
                    }
                    Ok(PipelineCommandResult::Cancelled { stdout, stderr }) => {
                        let combined = combine_normalized_command_output(&stdout, &stderr);
                        execution.steps[idx].status = StepStatus::Skipped;
                        execution.steps[idx].output = if combined.is_empty() {
                            None
                        } else {
                            Some(combined)
                        };
                        execution.steps[idx].error = Some("Cancelled by user".to_string());
                        execution.steps[idx].completed_at = Some(crate::store::now_timestamp());
                        for remaining in execution.steps[idx + 1..].iter_mut() {
                            remaining.status = StepStatus::Skipped;
                        }
                        let _ = store.update_session_pipeline(&config.session_id, &execution);
                        emit_pipeline_step(
                            app_handle,
                            &config.session_id,
                            idx,
                            &execution.steps[idx],
                        );
                        return PipelineOutcome::Cancelled;
                    }
                    Err(e) => {
                        // Failed to even spawn the command. Always hand off to AI
                        // regardless of the step's configured `on_failure` strategy
                        // — spawn failures (e.g. missing shell, permission denied)
                        // are environmental issues that need AI/human diagnosis,
                        // and the step's failure strategy is designed for command
                        // *output* classification, not spawn-level errors.
                        execution.steps[idx].status = StepStatus::Failed;
                        execution.steps[idx].error = Some(format!("Failed to execute: {e}"));
                        execution.steps[idx].completed_at = Some(crate::store::now_timestamp());
                        let prompt = format!(
                            "{}\n\nStep '{}' failed to execute: {e}\n\n{}",
                            config.prompt,
                            label,
                            step_outputs.join("\n\n")
                        );
                        for remaining in execution.steps[idx + 1..].iter_mut() {
                            remaining.status = StepStatus::Skipped;
                        }
                        let _ = store.update_session_pipeline(&config.session_id, &execution);
                        emit_pipeline_step(
                            app_handle,
                            &config.session_id,
                            idx,
                            &execution.steps[idx],
                        );
                        return PipelineOutcome::HandedOffToAi {
                            prompt,
                            ai_step_index: None,
                        };
                    }
                }
            }
            PipelineStep::AiHandoff {
                prompt_template, ..
            } => {
                // Build the prompt and hand off to AI.
                execution.steps[idx].status = StepStatus::Running;
                execution.steps[idx].started_at = Some(crate::store::now_timestamp());
                let _ = store.update_session_pipeline(&config.session_id, &execution);
                emit_pipeline_step(app_handle, &config.session_id, idx, &execution.steps[idx]);

                let prompt = prompt_template.replace("{step_outputs}", &step_outputs.join("\n\n"));

                // Mark the step as succeeded — the handoff itself completed.
                // The AI session's success/failure is tracked by the session
                // status, not this pipeline step. If start_session later fails,
                // start_pipeline_session will mark this step as failed.
                execution.steps[idx].status = StepStatus::Succeeded;
                execution.steps[idx].completed_at = Some(crate::store::now_timestamp());
                let _ = store.update_session_pipeline(&config.session_id, &execution);
                emit_pipeline_step(app_handle, &config.session_id, idx, &execution.steps[idx]);

                return PipelineOutcome::HandedOffToAi {
                    prompt,
                    ai_step_index: Some(idx),
                };
            }
        }
    }

    // All steps completed without AI handoff.
    execution.completed_without_ai = true;
    let _ = store.update_session_pipeline(&config.session_id, &execution);
    PipelineOutcome::CompletedWithoutAi
}

/// Shared cache of interactive-login-shell env snapshots, keyed by
/// environment root (a worktree's subdirectories share its snapshot; see
/// [`ShellEnvCache::env_key`]). Spawning `$SHELL -ils` to capture
/// `.zshrc`-driven PATH (e.g. Hermit) costs seconds on a dev machine with nvm
/// and Hermit hooks (about 9 s measured, more with several at once); this
/// amortises it to once per worktree per TTL window. Being the only cache,
/// its capture limit is the process-wide one: at most four shells at once,
/// with `$HOME` let through ahead of the queue.
pub fn shell_env_cache() -> &'static Arc<ShellEnvCache> {
    static CACHE: OnceLock<Arc<ShellEnvCache>> = OnceLock::new();
    CACHE.get_or_init(|| Arc::new(ShellEnvCache::new()))
}

async fn run_pipeline_command(
    command: &str,
    working_dir: &PathBuf,
    cancel_token: &CancellationToken,
) -> io::Result<PipelineCommandResult> {
    run_pipeline_command_with_cache(shell_env_cache(), command, working_dir, cancel_token).await
}

/// Same as [`run_pipeline_command`] but lets the caller pass an explicit cache.
/// Used by tests to pre-seed snapshots or point at a hermetic fake `$SHELL`.
async fn run_pipeline_command_with_cache(
    cache: &ShellEnvCache,
    command: &str,
    working_dir: &PathBuf,
    cancel_token: &CancellationToken,
) -> io::Result<PipelineCommandResult> {
    // Apply the cached interactive-login-shell env so Hermit-managed
    // binaries are on PATH (matters for git hooks invoked by pipeline
    // steps). On capture failure fall back to `sh -lc`, which at least
    // sources `/etc/profile`/`~/.profile`.
    let snapshot = match cache.get(working_dir).await {
        Ok(env) => Some(env),
        Err(e) => {
            log::warn!(
                "Failed to capture shell env for {}: {e}; falling back to sh -lc",
                working_dir.display()
            );
            None
        }
    };

    let mut cmd = tokio::process::Command::new("sh");
    let sh_args: &[&str] = if snapshot.is_some() {
        &["-c", command]
    } else {
        &["-lc", command]
    };
    cmd.args(sh_args)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(snapshot) = &snapshot {
        snapshot.apply_to(&mut cmd);
    }

    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd.spawn()?;
    let stdout_task = child.stdout.take().map(spawn_pipe_reader);
    let stderr_task = child.stderr.take().map(spawn_pipe_reader);

    // Wait for either the child to exit or cancellation. Using `child.wait()`
    // directly (instead of a polling loop with `try_wait()`) avoids both the
    // 50ms poll latency and a potential pipe deadlock: the pipe readers run as
    // spawned tasks on the same single-threaded runtime, so they only make
    // progress when the main task yields. `child.wait()` yields properly,
    // allowing the pipe readers to drain output concurrently.
    tokio::select! {
        result = child.wait() => {
            let status = result?;
            let stdout = collect_pipe_output(stdout_task, "stdout").await;
            let stderr = collect_pipe_output(stderr_task, "stderr").await;
            Ok(PipelineCommandResult::Completed(Output {
                status,
                stdout,
                stderr,
            }))
        }
        _ = cancel_token.cancelled() => {
            terminate_pipeline_child(&mut child);
            match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    log::warn!("Failed to wait for cancelled pipeline command: {e}");
                }
                Err(_) => {
                    force_kill_pipeline_child(&mut child);
                    if let Err(e) = child.wait().await {
                        log::warn!("Failed to wait for killed pipeline command: {e}");
                    }
                }
            }

            let stdout = collect_pipe_output(stdout_task, "stdout").await;
            let stderr = collect_pipe_output(stderr_task, "stderr").await;
            Ok(PipelineCommandResult::Cancelled { stdout, stderr })
        }
    }
}

/// Execute a pipeline command on a remote Blox workspace via `ws_exec`.
///
/// Wraps the command in `cd <remote_dir> && sh -lc '<command>'` so it runs
/// in the correct working directory on the remote, mirroring how the local
/// path runs `sh -lc` with `current_dir`.
async fn run_remote_pipeline_command(
    command: &str,
    workspace_name: &str,
    remote_working_dir: Option<&str>,
    cancel_token: &CancellationToken,
) -> io::Result<PipelineCommandResult> {
    // Build the remote shell command.
    let shell_command = if let Some(dir) = remote_working_dir {
        // Escape single quotes in the directory and command for the outer sh -c.
        let escaped_dir = dir.replace('\'', "'\\''");
        let escaped_cmd = command.replace('\'', "'\\''");
        format!("cd '{escaped_dir}' && sh -lc '{escaped_cmd}'")
    } else {
        let escaped_cmd = command.replace('\'', "'\\''");
        format!("sh -lc '{escaped_cmd}'")
    };

    let ws = workspace_name.to_string();
    // Tracked: push runs pre-push hooks and a rebase can take minutes, well
    // past the transient path's 60 s limit.
    let handle = tokio::task::spawn_blocking(move || {
        crate::blox::ws_exec_output(
            &ws,
            crate::blox::ExecMode::Tracked,
            &["sh", "-c", &shell_command],
        )
    });

    // Check for pre-existing cancellation before waiting.
    if cancel_token.is_cancelled() {
        handle.abort();
        return Ok(PipelineCommandResult::Cancelled {
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
    }

    tokio::select! {
        result = handle => {
            match result {
                Ok(Ok(output)) => {
                    use std::os::unix::process::ExitStatusExt;
                    let status = if output.success {
                        std::process::ExitStatus::from_raw(0)
                    } else {
                        // Encode exit code 1 in wait-status format (exit code << 8).
                        std::process::ExitStatus::from_raw(1 << 8)
                    };
                    Ok(PipelineCommandResult::Completed(Output {
                        status,
                        stdout: output.stdout,
                        stderr: output.stderr,
                    }))
                }
                Ok(Err(e)) => {
                    // Infrastructure error (CLI not found, timeout, auth failure).
                    Err(io::Error::other(e.to_string()))
                }
                Err(e) => {
                    // spawn_blocking panicked or was cancelled.
                    Err(io::Error::other(e.to_string()))
                }
            }
        }
        _ = cancel_token.cancelled() => {
            // ws_exec is blocking and not cancellable — the background thread
            // will finish naturally, but we return immediately.
            Ok(PipelineCommandResult::Cancelled {
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }
}

fn combine_normalized_command_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = crate::terminal_output::normalize_display_bytes(stdout);
    let stderr = crate::terminal_output::normalize_display_bytes(stderr);
    if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        stderr
    } else {
        format!("{stdout}\n{stderr}")
    }
}

fn format_step_output_for_prompt(label: &str, command: &str, output: &str, failed: bool) -> String {
    // Input is already display-normalized (from combine_normalized_command_output),
    // so only strip hostile control chars without re-running CR/ANSI processing.
    let output = crate::terminal_output::strip_prompt_hostile_chars(output);
    let output =
        crate::terminal_output::truncate_for_prompt(&output, PIPELINE_STEP_PROMPT_OUTPUT_MAX_CHARS);
    let status = if failed { " (FAILED)" } else { "" };
    format!("### {label}{status}\nCommand: {command}\n```\n{output}\n```")
}

fn completion_reason_for_driver_error(
    error: &str,
    provider_metadata: Option<&serde_json::Value>,
) -> CompletionReason {
    if is_rate_limit_error(error, provider_metadata) {
        CompletionReason::RateLimited
    } else {
        CompletionReason::Crashed
    }
}

fn rate_limit_recovery_details(
    error: &str,
    provider_metadata: Option<&serde_json::Value>,
) -> RateLimitRecovery {
    let mut details = RateLimitRecovery::default();

    if let Some(metadata) = provider_metadata {
        if let Some(rate_limit) = metadata
            .pointer("/acp/usageUpdate/_meta")
            .and_then(|meta| meta.get("_claude/rateLimit"))
        {
            details.resets_at = timestamp_field(rate_limit, &["resetsAt", "resetAt"]);
            details.kind = string_field(rate_limit, &["rateLimitType", "type", "status"]);
        }

        if details.resets_at.is_none() {
            details.resets_at = metadata
                .pointer("/acp/promptError/data")
                .and_then(|data| timestamp_field(data, &["resetsAt", "resetAt", "retryAt"]));
        }

        if details.kind.is_none() {
            details.kind = metadata
                .pointer("/acp/promptError/data")
                .and_then(|data| string_field(data, &["errorKind", "kind", "type"]));
        }
    }

    if details.kind.is_none() {
        let normalized = error.to_ascii_lowercase();
        details.kind = if normalized.contains("overloaded") {
            Some("overloaded".to_string())
        } else if normalized.contains("usage limit") || normalized.contains("quota") {
            Some("usage_limit".to_string())
        } else {
            Some("rate_limit".to_string())
        };
    }

    details
}

fn string_field(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(|field| field.as_str()))
        .map(str::to_string)
}

/// Extract a reset timestamp, normalized to an RFC3339 UTC string.
///
/// claude-agent-acp forwards the SDK's `rate_limit_info` verbatim, and the
/// SDK's `resetsAt` is a numeric epoch timestamp (`SDKRateLimitInfo` declares
/// `resetsAt?: number`), so numeric fields — and strings that are bare
/// integers — are converted here to the RFC3339 form the stored column, the
/// scheduler's parser, and the frontend's `new Date(...)` copy all expect.
fn timestamp_field(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        let field = value.get(*name)?;
        if let Some(text) = field.as_str() {
            let text = text.trim();
            if let Ok(epoch) = text.parse::<i64>() {
                return epoch_to_rfc3339_utc(epoch);
            }
            return (!text.is_empty()).then(|| text.to_string());
        }
        let epoch = field
            .as_i64()
            .or_else(|| field.as_f64().map(|epoch| epoch.trunc() as i64))?;
        epoch_to_rfc3339_utc(epoch)
    })
}

/// Format a positive epoch timestamp as RFC3339 UTC with millisecond
/// precision. Epoch seconds and millis are split by magnitude: seconds don't
/// pass 10^11 until year 5138, while millis crossed it in 1973.
fn epoch_to_rfc3339_utc(epoch: i64) -> Option<String> {
    if epoch <= 0 {
        return None;
    }
    let millis = if epoch >= 100_000_000_000 {
        epoch
    } else {
        epoch.checked_mul(1_000)?
    };
    let datetime =
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000).ok()?;
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        datetime.year(),
        u8::from(datetime.month()),
        datetime.day(),
        datetime.hour(),
        datetime.minute(),
        datetime.second(),
        datetime.millisecond()
    ))
}

fn is_rate_limit_error(error: &str, provider_metadata: Option<&serde_json::Value>) -> bool {
    let normalized = error.to_ascii_lowercase();

    if provider_metadata
        .map(metadata_has_non_retryable_limit)
        .unwrap_or(false)
        || has_non_retryable_limit_marker(&normalized)
    {
        return false;
    }

    if provider_metadata
        .map(metadata_has_rate_limit)
        .unwrap_or(false)
    {
        return true;
    }

    has_rate_limit_marker(&normalized)
}

fn metadata_has_non_retryable_limit(metadata: &serde_json::Value) -> bool {
    metadata
        .pointer("/acp/promptError/data/errorKind")
        .and_then(|kind| kind.as_str())
        .map(|kind| {
            let kind = kind.to_ascii_lowercase();
            kind == "billing_error" || kind == "credits_exhausted"
        })
        .unwrap_or(false)
}

fn metadata_has_rate_limit(metadata: &serde_json::Value) -> bool {
    if metadata
        .pointer("/acp/promptError/data/errorKind")
        .and_then(|kind| kind.as_str())
        .map(|kind| {
            let kind = kind.to_ascii_lowercase();
            kind == "rate_limit" || kind == "overloaded"
        })
        .unwrap_or(false)
    {
        return true;
    }

    let Some(rate_limit) = metadata
        .pointer("/acp/usageUpdate/_meta")
        .and_then(|meta| meta.get("_claude/rateLimit"))
    else {
        return false;
    };

    rate_limit
        .get("status")
        .and_then(|status| status.as_str())
        .map(|status| {
            let status = status.to_ascii_lowercase();
            status == "rejected" || status == "limited" || status == "rate_limited"
        })
        .unwrap_or(false)
}

fn has_non_retryable_limit_marker(normalized: &str) -> bool {
    // Auth and hard billing/credit failures need user action, not retry/resume.
    let non_retryable_markers = [
        "please run /login",
        "run /login",
        "not authenticated",
        "auth login",
        "billing_error",
        "billing error",
        "credits exhausted",
        "credit exhausted",
        "insufficient_quota",
        "insufficient quota",
    ];
    non_retryable_markers
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn has_rate_limit_marker(normalized: &str) -> bool {
    // Phrase markers are specific enough to substring-match, which keeps
    // suffix forms matching ("rate limited", "rate limits", "rateLimitType").
    let substring_markers = [
        "rate limit",
        "ratelimit",
        "rate_limit",
        "too many requests",
        "resource_exhausted",
        "resource exhausted",
        "usage limit",
        "usage_limit",
        "usagelimit",
        "overloaded",
        "resetsat",
        "resets at",
    ];
    // Driver error text can include arbitrary process stderr, where "429"
    // appears inside longer numbers (exit codes, byte counts) and "quota"
    // inside unrelated words ("quotation"); these only count as whole tokens.
    let token_markers = ["429", "quota"];
    substring_markers
        .iter()
        .any(|marker| normalized.contains(marker))
        || token_markers
            .iter()
            .any(|marker| contains_token(normalized, marker))
}

/// Substring match that only counts occurrences not embedded in a longer
/// alphanumeric run: "http 429" and "error(429)" match "429", "14290" and
/// "4429" do not. Non-ASCII bytes count as boundaries, which is safe because
/// a multi-byte character can never continue an ASCII token.
fn contains_token(text: &str, token: &str) -> bool {
    let bytes = text.as_bytes();
    let mut search_from = 0;
    while let Some(offset) = text[search_from..].find(token) {
        let start = search_from + offset;
        let end = start + token.len();
        let boundary_before = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let boundary_after = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if boundary_before && boundary_after {
            return true;
        }
        search_from = start + 1;
    }
    false
}

fn spawn_pipe_reader<R>(mut pipe: R) -> tokio::task::JoinHandle<io::Result<Vec<u8>>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut output = Vec::new();
        pipe.read_to_end(&mut output).await?;
        Ok(output)
    })
}

async fn collect_pipe_output(
    handle: Option<tokio::task::JoinHandle<io::Result<Vec<u8>>>>,
    stream_name: &str,
) -> Vec<u8> {
    let Some(mut handle) = handle else {
        return Vec::new();
    };

    match tokio::time::timeout(Duration::from_secs(2), &mut handle).await {
        Ok(Ok(Ok(output))) => output,
        Ok(Ok(Err(e))) => {
            log::warn!("Failed to read pipeline command {stream_name}: {e}");
            Vec::new()
        }
        Ok(Err(e)) => {
            log::warn!("Pipeline command {stream_name} reader task failed: {e}");
            Vec::new()
        }
        Err(_) => {
            handle.abort();
            log::warn!("Timed out reading pipeline command {stream_name}");
            Vec::new()
        }
    }
}

fn terminate_pipeline_child(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        if send_signal_to_pipeline_process_group(pid, libc::SIGTERM).is_ok() {
            return;
        }
    }

    if let Err(e) = child.start_kill() {
        log::debug!("Failed to terminate pipeline command process: {e}");
    }
}

fn force_kill_pipeline_child(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        if send_signal_to_pipeline_process_group(pid, libc::SIGKILL).is_ok() {
            return;
        }
    }

    if let Err(e) = child.start_kill() {
        log::debug!("Failed to kill pipeline command process: {e}");
    }
}

#[cfg(unix)]
fn send_signal_to_pipeline_process_group(pid: u32, signal: libc::c_int) -> io::Result<()> {
    let pgid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "process id exceeds i32"))?;

    // SAFETY: kill(2) does not dereference pointers. A negative pid targets the
    // process group created for the pipeline command.
    let result = unsafe { libc::kill(-pgid, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn emit_pipeline_step<R: tauri::Runtime>(
    app_handle: &AppHandle<R>,
    session_id: &str,
    step_index: usize,
    step: &crate::store::PipelineStepStatus,
) {
    let event = PipelineStepEvent {
        session_id: session_id.to_string(),
        step_index,
        label: step.label.clone(),
        step_type: step.step_type.clone(),
        status: step.status.clone(),
        output: step.output.clone(),
        error: step.error.clone(),
        started_at: step.started_at,
        completed_at: step.completed_at,
    };
    crate::web_server::emit_to_all(app_handle, "pipeline-step-changed", &event);
}

// =============================================================================
// Orphaned session recovery
// =============================================================================

/// Recover any sessions whose owner process is no longer alive.
///
/// Each session records the PID of the Staged process that started it
/// (`owner_pid`). Run once at startup and then on a timer by
/// [`spawn_dead_session_sweep`]; for each running session:
/// - `owner_pid` is our own PID → ours; its session thread owns the row, skip.
/// - `owner_pid` belongs to a live process → another Staged instance owns
///   it; leave it alone.
/// - `owner_pid` is dead (or NULL for pre-migration rows) → a remote session
///   whose agent has answered its turn is taken over and reattached to, since
///   its agent runs on the workspace and may outlive the owner (see
///   [`reattach_dead_owner_session`]). Any other Blox session whose turn may
///   still be running on its workstation is left `disconnected` (see
///   [`disconnect::may_still_run_on_workstation`]); its branch stays busy, so
///   there is nothing to kick. Anything else ended with its owner, so
///   transition it to error with `AppQuit` reason and emit
///   `session-status-changed` so the frontend learns the outcome.
///
/// Only `running` rows are swept. A `queued` row carries no owner — it waits
/// for a branch drain, and whether one is still coming depends on the
/// branch's other sessions, dependency gating and in-flight worktree
/// operations — so there is no cheap test here for a queued row that nothing
/// will ever drain.
pub fn recover_dead_sessions(store: Arc<Store>, app_handle: AppHandle) {
    let reattach_handle = app_handle.clone();
    recover_dead_sessions_with(store, app_handle, |store, session, running| {
        if dead_owner_recovery(store, session) != DeadOwnerRecovery::Reattach {
            return false;
        }
        let sole_turn_of_owner = is_owners_sole_turn(store, session, running);
        reattach_dead_owner_session(
            Arc::clone(store),
            &reattach_handle,
            session.clone(),
            sole_turn_of_owner,
        );
        true
    });
}

/// [`recover_dead_sessions`], with the reattach step as a parameter: `reattach`
/// takes over a dead owner's row (returning `true`) when its agent can be
/// reattached to. Generic over the runtime so the rest is testable.
pub(crate) fn recover_dead_sessions_with<R: tauri::Runtime>(
    store: Arc<Store>,
    app_handle: AppHandle<R>,
    reattach: impl Fn(&Arc<Store>, &crate::store::Session, &[crate::store::Session]) -> bool,
) {
    let sessions = match store.get_running_sessions() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[session_runner] Failed to query running sessions: {e}");
            return;
        }
    };

    for session in &sessions {
        let should_cancel = match session.owner_pid {
            None => true,
            Some(pid) if pid == std::process::id() => false,
            Some(pid) if !is_process_alive(pid) => true,
            Some(_pid) => false,
        };
        if should_cancel {
            if reattach(&store, session, &sessions) {
                continue;
            }
            recover_dead_session(&store, &app_handle, session);
        }
    }
}

/// End `session` — read `running`, with its owner process gone — or leave it
/// `disconnected`, and announce the status it now has.
///
/// `session` is a snapshot, so the row may have moved on since it was read. A
/// write that lost to the same recovery by another instance's sweep is still
/// announced: that sweep's event reached only its own windows, and this
/// instance's would otherwise keep the row `running`. A row anything else
/// moved on — resumed, settled by reconcile, deleted — is announced by
/// whatever moved it, and repeating this sweep's status would tell the
/// frontend it went back (`disconnected` after `completed`, say).
fn recover_dead_session<R: tauri::Runtime>(
    store: &Store,
    app_handle: &AppHandle<R>,
    session: &crate::store::Session,
) {
    let disconnected = disconnect::may_still_run_on_workstation(store, session);
    let (status, reason) = if disconnected {
        (SessionStatus::Disconnected, None)
    } else {
        (SessionStatus::Error, Some(CompletionReason::AppQuit))
    };
    let transitioned = store
        .transition_from_running(&session.id, status.clone(), None, reason.as_ref())
        .unwrap_or(false);
    if transitioned {
        log::info!(
            "Recovered session {} whose owner process {:?} is gone",
            session.id,
            session.owner_pid
        );
    }
    let holds_status = transitioned
        || store
            .get_session(&session.id)
            .ok()
            .flatten()
            .is_some_and(|row| row.status == status);
    if !holds_status {
        log::debug!(
            "Session {} moved on before its dead-owner recovery; not announcing {}",
            session.id,
            status.as_str()
        );
        return;
    }
    let recovered_branch_id = store.get_branch_id_for_session(&session.id).ok().flatten();
    let recovered_project_id = store.get_project_id_for_session(&session.id).ok().flatten();
    emit_status(
        app_handle,
        &session.id,
        status.as_str(),
        None,
        reason.as_ref(),
        None,
        recovered_branch_id.clone(),
        recovered_project_id,
    );
    if transitioned && !disconnected {
        if let Some(branch_id) = recovered_branch_id {
            crate::session_queue::kick(
                app_handle,
                &branch_id,
                crate::session_queue::KickSource::DeadOwnerRecovered,
            );
        }
    }
}

/// How [`recover_dead_sessions`] ends a `running` row whose owner is gone.
#[derive(Debug, PartialEq, Eq)]
enum DeadOwnerRecovery {
    /// Its agent went with the owner: record [`CompletionReason::AppQuit`].
    AppQuit,
    /// A remote session whose agent runs on its workspace, where it may still
    /// be serving the turn: reattach to it.
    Reattach,
}

/// Whether [`recover_dead_sessions`] would reattach to `session` in place.
#[cfg(test)]
pub(crate) fn dead_owner_reattaches(store: &Store, session: &crate::store::Session) -> bool {
    dead_owner_recovery(store, session) == DeadOwnerRecovery::Reattach
}

fn dead_owner_recovery(store: &Store, session: &crate::store::Session) -> DeadOwnerRecovery {
    // Without an agent session there is nothing to reattach to: the owner
    // went before the agent had one.
    let has_agent_session =
        crate::session_commands::resumable_agent_session_id(session.agent_id.as_deref()).is_some();
    // A follow-up turn has one from an earlier turn, though, so the agent
    // must also have answered this turn's prompt: an owner that went while
    // still connecting never sent it, and the idle agent would read as the
    // turn having completed.
    let turn_reached_agent = || {
        store
            .get_turn_prompt_message(&session.id)
            .ok()
            .flatten()
            .is_some_and(|prompt| {
                store
                    .agent_answered_message(&session.id, prompt.id)
                    .unwrap_or(false)
            })
    };
    if has_agent_session && session_workspace_name(store, session).is_some() && turn_reached_agent()
    {
        DeadOwnerRecovery::Reattach
    } else {
        DeadOwnerRecovery::AppQuit
    }
}

/// Whether `session` was its owner's only running turn on the agent process
/// it runs on (same workspace, same provider), among the `running` sessions.
///
/// The agent answers a turn's `session/prompt` on every connection to that
/// process, and its request id names the instance that sent it but not the
/// session, so only then does that answer tell a reattached turn it ended.
fn is_owners_sole_turn(
    store: &Store,
    session: &crate::store::Session,
    running: &[crate::store::Session],
) -> bool {
    let Some(owner_pid) = session.owner_pid else {
        return false;
    };
    let workspace = session_workspace_name(store, session);
    let shares_its_process = |other: &crate::store::Session| {
        other.id != session.id
            && other.owner_pid == Some(owner_pid)
            && other.provider == session.provider
            && session_workspace_name(store, other) == workspace
    };
    !running.iter().any(shares_its_process)
}

/// The workspace a session's branch lives on, when it is a remote branch —
/// found through the same links a resume follows, from the store alone.
fn session_workspace_name(store: &Store, session: &crate::store::Session) -> Option<String> {
    let branch_id = session
        .branch_id
        .clone()
        .or_else(|| {
            let commit = store.get_commit_by_session(&session.id).ok().flatten();
            commit.map(|commit| commit.branch_id)
        })
        .or_else(|| {
            let note = store.get_note_by_session(&session.id).ok().flatten();
            note.map(|note| note.branch_id)
        })
        .or_else(|| {
            let review = store.get_review_by_session(&session.id).ok().flatten();
            review.map(|review| review.branch_id)
        })?;
    store.get_branch(&branch_id).ok().flatten()?.workspace_name
}

/// Take over remote session `session`, whose owner process is gone, and
/// finish it the way its owner would have: reattach to its agent on the
/// workspace, wait for the turn the owner prompted to end, and run the
/// normal completion path ([`finish_session_run`]) — or fail it as
/// [`CompletionReason::RemoteAgentLost`] when that agent is gone too.
/// `sole_turn_of_owner` says whether the agent's answer to the dead owner's
/// prompts identifies the turn (see [`is_owners_sole_turn`]).
fn reattach_dead_owner_session(
    store: Arc<Store>,
    app_handle: &AppHandle,
    session: crate::store::Session,
    sole_turn_of_owner: bool,
) {
    use tauri::Manager;
    let Some(registry) = app_handle
        .try_state::<Arc<SessionRegistry>>()
        .map(|registry| Arc::clone(&registry))
    else {
        return;
    };
    // Registered before the row becomes ours: a `running` row this process
    // owns with no entry is what the stranded-session sweep ends. The quit
    // flag is read after, so a shutdown either sees the entry or has already
    // published the flag; a row left here is the next launch's to recover.
    let (cancel_token, registration) = registry.register(&session.id);
    // Marked before the quit check too: until its first connection is up,
    // a quit stops the reattach instead of cancelling the turn.
    let detached = registry.reattaching(&session.id);
    let adopted = !crate::app_lifecycle::is_quitting(app_handle)
        && store
            .adopt_running_session(&session.id, session.owner_pid)
            .unwrap_or(false);
    if !adopted {
        registry.deregister(&session.id, registration);
        return;
    }
    log::info!(
        "Reattaching to remote session {} whose owner process {:?} is gone",
        session.id,
        session.owner_pid
    );
    let app_handle = app_handle.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime for session");
        let local = tokio::task::LocalSet::new();
        let on_detach_handle =
            |handle: acp_client::DetachHandle| registry.set_detach_handle(&session.id, handle);
        let reattached = local.block_on(&rt, async {
            let config = crate::session_commands::reattach_session_config(&store, &session).await?;
            let owner = TurnOwner {
                pid: session.owner_pid,
                sole_turn: sole_turn_of_owner,
            };
            Ok::<_, String>(
                reattach_turn(
                    &store,
                    config,
                    owner,
                    &cancel_token,
                    &detached,
                    &on_detach_handle,
                )
                .await,
            )
        });
        match reattached {
            // The turn was in flight on the agent: a quit's detach, or its
            // `sq` proxy going for good, leaves it there.
            Ok((config, _, end))
                if disconnect::run_leaves_session_disconnected(
                    config.workspace_name.is_some(),
                    cancel_token.is_cancelled(),
                    registry.cancellation_completion_reason(&session.id)
                        == Some(CompletionReason::AppQuit),
                    end.result.as_ref().err().map(String::as_str),
                    None,
                    Some(REATTACHED_TURN_IN_FLIGHT),
                    false,
                ) =>
            {
                finish_disconnected_run(
                    &config,
                    &end.result,
                    registration,
                    &store,
                    &registry,
                    &app_handle,
                )
            }
            Ok((config, turn_start_message_id, end)) => {
                let settlement = end.settlement();
                finish_session_run(
                    &config,
                    end.result,
                    settlement,
                    turn_start_message_id,
                    &cancel_token,
                    registration,
                    store,
                    registry,
                    app_handle,
                )
            }
            Err(error) => {
                let project_id = store.get_project_id_for_session(&session.id).ok().flatten();
                let transitioned = finish_failed_before_run(
                    &session.id,
                    session.branch_id.clone(),
                    project_id,
                    &store,
                    &app_handle,
                    &error,
                );
                registry.deregister(&session.id, registration);
                if transitioned {
                    drain_queued_after_terminal_state(
                        store,
                        registry,
                        app_handle,
                        session.id,
                        session.branch_id,
                        false,
                        crate::session_queue::KickSource::TerminalState,
                    );
                }
            }
        }
    });
}

/// Reattach to the remote session `config` describes and wait out the turn
/// its previous owner prompted, returning the config (with the HEAD the turn
/// started from, for commit detection), where the turn's messages start, and
/// how the turn ended.
async fn reattach_turn(
    store: &Arc<Store>,
    mut config: SessionConfig,
    owner: TurnOwner,
    cancel_token: &CancellationToken,
    detached: &CancellationToken,
    on_detach_handle: &dyn Fn(acp_client::DetachHandle),
) -> (SessionConfig, i64, ReattachEnd) {
    // The turn starts at its prompt — not at a steer delivered during it,
    // which is a later user message.
    let turn_start = store
        .get_turn_prompt_message(&config.session_id)
        .ok()
        .flatten();
    let turn_start_message_id = turn_start.as_ref().map_or(0, |message| message.id);
    let turn_started_at = turn_start.map(|message| message.created_at);
    if let Some(turn_started_at) = turn_started_at {
        if store
            .get_commit_by_session(&config.session_id)
            .ok()
            .flatten()
            .is_some()
        {
            let working_dir = config.working_dir.clone();
            let workspace_name = config.workspace_name.clone();
            let remote_dir = config
                .remote_working_dir
                .as_deref()
                .and_then(|dir| dir.to_str())
                .map(str::to_string);
            config.pre_head_sha = tauri::async_runtime::spawn_blocking(move || {
                let git = crate::branches::git_runner(
                    &working_dir,
                    workspace_name.as_deref(),
                    remote_dir,
                );
                head_at(&git, turn_started_at)
            })
            .await
            .unwrap_or_else(|e| Err(format!("HEAD lookup task failed: {e}")))
            .inspect_err(|e| {
                log::warn!(
                    "Session {}: could not find the HEAD its turn started from, so its \
                     commit goes undetected: {e}",
                    config.session_id
                )
            })
            .ok()
            .flatten();
        }
    }

    let prompted_at = turn_started_at.unwrap_or(0);
    let connected_at = turn_connected_at(store, &config.session_id, turn_start_message_id);
    let result = reattach_agent(
        store,
        &config,
        cancel_token,
        detached,
        prompted_at,
        connected_at,
        owner,
        on_detach_handle,
    )
    .await;
    (config, turn_start_message_id, result)
}

/// When the connection serving the run that message `prompt_id` opened had
/// initialized with the agent (see [`acp_client::ReattachTurn::connected_at`]).
fn turn_connected_at(store: &Store, session_id: &str, prompt_id: i64) -> Option<i64> {
    store
        .get_turn_initialize_message(session_id, prompt_id)
        .ok()
        .flatten()
        .map(|message| message.created_at)
}

/// The app instance that prompted a turn being reattached to.
#[derive(Clone, Copy, Debug)]
struct TurnOwner {
    /// Its PID, which the request ids of its prompts carry.
    pid: Option<u32>,
    /// Whether the turn was its only one on the agent process (see
    /// [`acp_client::ReattachTurn::sole_turn_of_owner`]).
    sole_turn: bool,
}

/// How long [`reattach_agent`] waits before each further attempt at a turn
/// whose `sq` proxy went again while bloxlet still runs its agent; after the
/// last, it fails with that. `sq` going is usually transient — an expired
/// login, a network blip, bloxlet briefly unreachable — so attempts made
/// back to back would tend to fail the same way.
#[cfg(not(test))]
const DETACHED_TURN_REATTACH_DELAYS: [std::time::Duration; 2] = [
    std::time::Duration::from_secs(10),
    std::time::Duration::from_secs(30),
];
#[cfg(test)]
const DETACHED_TURN_REATTACH_DELAYS: [std::time::Duration; 2] = [
    std::time::Duration::from_millis(10),
    std::time::Duration::from_millis(30),
];

/// Reattach to the turn of remote session `config` that `owner` prompted at
/// `prompted_at`, through a connection that had initialized by
/// `connected_at` (both ms since the epoch), and wait for it to end (see
/// [`AcpDriver::reattach`]), reconnecting after a delay when its `sq` proxy
/// goes again while bloxlet still runs the agent.
///
/// Each connection's detach handle goes to `on_detach_handle`, so a quit can
/// let go of the turn without cancelling it; `detached` fires for a quit
/// that finds no connection up (see [`retry_detached_reattach`]).
///
/// Each connection's `session/load` replays the session, and the turn went
/// on with no connection attached: what the replay holds that the transcript
/// lacks is recorded, and a stored tool call it shows finished is settled in
/// place, as reconcile's catch-up does.
#[allow(clippy::too_many_arguments)]
async fn reattach_agent(
    store: &Arc<Store>,
    config: &SessionConfig,
    cancel_token: &CancellationToken,
    detached: &CancellationToken,
    prompted_at: i64,
    connected_at: Option<i64>,
    owner: TurnOwner,
    on_detach_handle: &dyn Fn(acp_client::DetachHandle),
) -> ReattachEnd {
    let reattachable = (|| {
        let agent_session_id = config
            .agent_session_id
            .clone()
            .or_else(|| {
                stored_agent_session_id(store, &config.session_id)
                    .ok()
                    .flatten()
            })
            .ok_or("The session has no agent session to reattach to")?;
        let workspace_name = config
            .workspace_name
            .as_deref()
            .ok_or("The session is not on a remote workspace")?;
        // The same argv as the session's own runs, so `sq` attaches to the
        // workstation process that ran the turn.
        let mut driver = AcpDriver::for_workspace(
            workspace_name,
            config.provider.as_deref(),
            &config.session_id,
        )?;
        if let Some(ref remote_dir) = config.remote_working_dir {
            driver = driver.with_remote_working_dir(remote_dir.clone());
        }
        Ok::<_, String>((driver, agent_session_id))
    })();
    let (driver, agent_session_id) = match reattachable {
        Ok(reattachable) => reattachable,
        Err(error) => return ReattachEnd::of(Err(error)),
    };
    let store_trait: Arc<dyn acp_client::Store> = store.clone();
    let turn = acp_client::ReattachTurn {
        agent_session_id,
        prompted_at,
        connected_at,
        owner_pid: owner.pid,
        sole_turn_of_owner: owner.sole_turn,
    };
    let (driver, store_trait, turn) = (&driver, &store_trait, &turn);
    retry_detached_reattach(
        &config.session_id,
        cancel_token,
        detached,
        move || async move {
            let writer: Arc<dyn acp_client::MessageWriter> = Arc::new(MessageWriter::catching_up(
                config.session_id.clone(),
                Arc::clone(store),
            ));
            let settled = driver
                .reattach_detachable(
                    &config.session_id,
                    &config.working_dir,
                    store_trait,
                    &writer,
                    cancel_token,
                    turn,
                    on_detach_handle,
                )
                .await;
            ReattachEnd::settled(settled, cancel_token)
        },
    )
    .await
}

/// Run `reattach` until it ends other than detached (see
/// [`acp_client::is_remote_agent_detached_error`]), waiting out each of
/// [`DETACHED_TURN_REATTACH_DELAYS`] before trying again; after the last it
/// fails with the detached error.
///
/// `detached` is a quit (see [`SessionRegistry::reattaching`]): between
/// attempts, or while one connects or runs. It ends the reattach at once
/// with the turn still in flight on the agent — an error the run leaves
/// `disconnected`, as a quit's detach of a live connection does — and
/// nothing is tried again after it.
async fn retry_detached_reattach<F, Fut>(
    session_id: &str,
    cancel_token: &CancellationToken,
    detached: &CancellationToken,
    mut reattach: F,
) -> ReattachEnd
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ReattachEnd>,
{
    let mut delays = DETACHED_TURN_REATTACH_DELAYS.iter();
    loop {
        // An attempt still connecting goes with its proxy, killed on drop:
        // as a detach does, without a `session/cancel` or a SIGINT.
        let end = tokio::select! {
            biased;
            () = detached.cancelled() => {
                return ReattachEnd::of(Err(QUIT_WHILE_REATTACHING.to_string()));
            }
            end = reattach() => end,
        };
        let error = match &end.result {
            Err(error)
                if acp_client::is_remote_agent_detached_error(error)
                    && !cancel_token.is_cancelled()
                    && !detached.is_cancelled() =>
            {
                error.clone()
            }
            _ => return end,
        };
        let Some(delay) = delays.next() else {
            return end;
        };
        log::warn!(
            "Session {session_id}: {error}; reattaching to its turn again in {}s",
            delay.as_secs_f32()
        );
        // A stop while waiting ends the session at once. No connection is
        // watching the turn then, so nothing cancels it on the agent: the run
        // cancels it over a fresh one. A quit leaves the turn as it is.
        tokio::select! {
            _ = tokio::time::sleep(*delay) => {}
            _ = cancel_token.cancelled() => {
                return ReattachEnd {
                    result: Ok(AgentRunOutcome::Cancelled),
                    cancel_unacknowledged: true,
                };
            }
            _ = detached.cancelled() => return end,
        }
    }
}

/// How a reattach a quit cut short ends: the turn in flight on the agent,
/// left there.
const QUIT_WHILE_REATTACHING: &str =
    "Detached with the turn still in flight (Staged quit while reattaching to it)";

/// Pick up the turn `config` prompted (its prompt is row
/// `turn_start_message_id`) after its `sq` proxy went with `error` while
/// bloxlet still ran the agent, rather than failing a turn the agent may
/// still be finishing. `None` when the turn never reached the agent, so there
/// is nothing to pick up and `error` stands.
async fn reattach_detached_turn(
    store: &Arc<Store>,
    config: &SessionConfig,
    turn_start_message_id: i64,
    cancel_token: &CancellationToken,
    detached: &CancellationToken,
    error: &str,
    on_detach_handle: &dyn Fn(acp_client::DetachHandle),
) -> Option<ReattachEnd> {
    let prompted_at = store
        .get_session_messages_since(&config.session_id, turn_start_message_id, &[])
        .ok()
        .and_then(|messages| {
            messages
                .into_iter()
                .find(|message| message.id == turn_start_message_id)
        })
        .map(|message| message.created_at)?;
    let connected_at = turn_connected_at(store, &config.session_id, turn_start_message_id);
    // A proxy that went while the session was still setting up may never
    // have sent the prompt, and the idle agent would then read as the turn
    // having completed: only a turn the agent has answered is picked up.
    let turn_reached_agent = store
        .agent_answered_message(&config.session_id, turn_start_message_id)
        .unwrap_or(false);
    if !turn_reached_agent {
        return None;
    }
    log::warn!(
        "Session {}: {error}; reattaching to its turn",
        config.session_id
    );
    // This process sent the prompt, through a proxy that has gone. It may
    // have had other turns on it, so its answer doesn't mark this turn's end.
    let owner = TurnOwner {
        pid: Some(std::process::id()),
        sole_turn: false,
    };
    Some(
        reattach_agent(
            store,
            config,
            cancel_token,
            detached,
            prompted_at,
            connected_at,
            owner,
            on_detach_handle,
        )
        .await,
    )
}

/// The HEAD a turn prompted at `at` (ms since the epoch) started from: what
/// HEAD's reflog says it was then, or — when the reflog can't be read or
/// doesn't reach back that far — [`head_before`]. The reflog survives
/// rebases and merges that leave committer dates out of order.
///
/// Git stamps reflog entries in whole seconds, so one made in the prompt's
/// own second can't be ordered against it. Such an entry is taken as the
/// turn's (see [`prompt_second`]).
fn head_at(git: &crate::branches::GitRunner<'_>, at: i64) -> Result<Option<String>, String> {
    let prompt_second = prompt_second(at);
    // No spaces in the format: a remote runner's arguments pass through
    // `ws exec`'s command line.
    let reflog = git(&[
        "reflog",
        "show",
        "--format=%H:%gd",
        "--date=unix",
        "HEAD",
        "--",
    ]);
    let entries = match reflog {
        Ok(reflog) => parse_head_reflog(&reflog),
        Err(e) => {
            log::debug!("HEAD reflog unreadable, falling back to commit dates: {e}");
            Vec::new()
        }
    };
    // Newest first: the first entry certainly made before the prompt is
    // HEAD as it started.
    let reaches_back = entries
        .last()
        .is_some_and(|(_, time)| *time < prompt_second);
    if reaches_back {
        let sha = entries.into_iter().find(|(_, time)| *time < prompt_second);
        return Ok(sha.map(|(sha, _)| sha));
    }
    head_before(git, at)
}

/// The whole second (since the epoch) holding `at` (ms since the epoch).
///
/// Git dates commits and reflog entries in whole seconds, so one in this
/// second may have come before or after `at`. [`head_at`] and
/// [`head_before`] take it as having come after, as part of the turn
/// prompted at `at`. Taking it as before would treat a commit the turn made
/// within a second of its prompt as the HEAD it started from, and leave the
/// turn's commit unlinked. Taking it as after errs only when HEAD moved in
/// that second before the prompt and the turn then made no commit: the
/// turn is credited with the commit HEAD started on. After a follow-up
/// that is the session's own earlier commit, which changes nothing, and a
/// commit another row already holds is not claimed twice.
///
/// The stamps come from the workstation's clock and `at` from the app's, so
/// a workstation running behind by less than the time from prompt to a
/// first commit is tolerated too. On a remote turn that gap is at least the
/// seconds `sq` takes to connect.
fn prompt_second(at: i64) -> i64 {
    at.div_euclid(1000)
}

/// `(sha, unix time)` of each entry of `git reflog show --format=%H:%gd
/// --date=unix`, newest first (`<sha>:HEAD@{<time>}` per line).
fn parse_head_reflog(reflog: &str) -> Vec<(String, i64)> {
    reflog
        .lines()
        .filter_map(|line| {
            let (sha, selector) = line.trim().split_once(':')?;
            let time = selector.strip_prefix("HEAD@{")?.strip_suffix('}')?;
            Some((sha.to_string(), time.parse().ok()?))
        })
        .collect()
}

/// The branch's HEAD as of `at` (ms since the epoch): its newest commit dated
/// before `at`'s second, or `None` when it has none that old. A turn's
/// commits come after its prompt, so this approximates the HEAD a turn
/// prompted at `at` started from; one dated in the prompt's own second is
/// taken as the turn's (see [`prompt_second`]).
fn head_before(git: &crate::branches::GitRunner<'_>, at: i64) -> Result<Option<String>, String> {
    // `--before` keeps commits dated at its bound, so bound it a second
    // earlier.
    let before = format!("--before=@{}", prompt_second(at) - 1);
    let sha = git(&["rev-list", "-1", &before, "HEAD"])?;
    let sha = sha.trim();
    Ok((!sha.is_empty()).then(|| sha.to_string()))
}

/// How often [`spawn_dead_session_sweep`] re-runs [`recover_dead_sessions`].
const DEAD_SESSION_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Re-run [`recover_dead_sessions`] periodically for the life of the process.
///
/// The startup sweep only helps the instance that restarts. Every build
/// flavour shares one database, status events are delivered in-process, and a
/// desktop window hydrates its busy state once at load — so a session owned
/// by an instance that dies later stays `running` in every *other* window's
/// sidebar until one of them happens to restart. Sweeping on a timer finalizes
/// the row and emits its terminal event from whichever instance is still up.
pub fn spawn_dead_session_sweep(store: Arc<Store>, app_handle: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(DEAD_SESSION_SWEEP_INTERVAL);
        // A sweep that overruns the interval (several instances, each with
        // foreign running rows to probe) should be followed by one tick a full
        // interval later, not by a burst of catch-up ticks.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick completes immediately; startup already ran that sweep.
        interval.tick().await;
        loop {
            interval.tick().await;
            // The sweep is synchronous (store mutex, a `kill -0` subprocess per
            // foreign-pid row), so run it off the async workers.
            let store = Arc::clone(&store);
            let app_handle = app_handle.clone();
            if let Err(e) = tauri::async_runtime::spawn_blocking(move || {
                recover_dead_sessions(store, app_handle);
            })
            .await
            {
                log::error!("[session_runner] Dead-session sweep panicked: {e}");
            }
        }
    });
}

/// On startup, return unsent queued follow-up messages claimed by dead owners
/// to the visible queue so they can be retried manually or by a later drain.
pub fn recover_stale_queued_session_messages(store: &Store) {
    let messages = match store.list_sending_queued_session_messages() {
        Ok(messages) => messages,
        Err(e) => {
            log::warn!("[session_runner] Failed to query sending queued messages: {e}");
            return;
        }
    };

    for message in messages {
        let should_release = match message.owner_pid {
            None => true,
            Some(pid) if pid == std::process::id() => true,
            Some(pid) if !is_process_alive(pid) => true,
            Some(_) => false,
        };

        if should_release {
            match store.release_queued_session_message(&message.id, None) {
                Ok(true) => log::info!(
                    "Recovered stale queued follow-up message {} for session {}",
                    message.id,
                    message.session_id
                ),
                Ok(false) => {}
                Err(e) => log::warn!(
                    "Failed to recover stale queued follow-up message {}: {e}",
                    message.id
                ),
            }
        }
    }
}

/// Check whether a process is alive by sending signal 0 via the `kill` command.
///
/// `kill -0 pid` succeeds (exit 0) if the process exists and we have permission
/// to signal it. It also exits 0 on some systems when the process exists but we
/// lack permission (EPERM). A non-zero exit means the process is gone (ESRCH).
pub(crate) fn is_process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// =============================================================================
// Post-completion hooks
// =============================================================================

/// Detect artifacts produced by a session and update the DB accordingly.
///
/// - **Commits**: If a pending commit record is linked to this session and
///   HEAD has moved since the session started, record the new SHA.
///   For remote workspaces, HEAD is checked via `blox ws_exec`.
/// - **Notes**: If a note is linked to this session, parse the last assistant
///   message of the completed turn (messages from `turn_start_message_id`
///   onwards) for content after the first `---`. Earlier turns are never
///   re-extracted: their media sources may be gone, and re-ingesting them
///   would replace saved attachments with unavailable placeholders.
fn run_post_completion_hooks(
    session_id: &str,
    working_dir: &std::path::Path,
    pre_head_sha: Option<&str>,
    workspace_name: Option<&str>,
    remote_working_dir: Option<&str>,
    store: &Arc<Store>,
    turn_start_message_id: i64,
) {
    detect_session_commit(
        session_id,
        working_dir,
        pre_head_sha,
        workspace_name,
        remote_working_dir,
        store,
    );

    // --- Note extraction (repo notes and project notes) ---
    //
    // Both paths share the same message-scanning and title-resolution logic
    // via `resolve_note_title_and_body`, differing only in which DB record
    // they read/write.
    struct NoteTarget {
        id: String,
        is_amendment: bool,
        kind: NoteKind,
    }
    enum NoteKind {
        Repo,
        Project,
    }

    let note_targets: Vec<NoteTarget> = [
        store
            .get_note_by_session(session_id)
            .ok()
            .flatten()
            .map(|n| NoteTarget {
                id: n.id,
                is_amendment: !n.content.is_empty(),
                kind: NoteKind::Repo,
            }),
        store
            .get_project_note_by_session(session_id)
            .ok()
            .flatten()
            .map(|n| NoteTarget {
                id: n.id,
                is_amendment: !n.content.is_empty(),
                kind: NoteKind::Project,
            }),
    ]
    .into_iter()
    .flatten()
    .collect();

    if !note_targets.is_empty() {
        // Scan this turn's assistant messages once for all note targets.
        if let Ok(messages) =
            store.get_session_messages_since(session_id, turn_start_message_id, &[])
        {
            let note_content = messages
                .iter()
                .rev()
                .filter(|m| m.role == MessageRole::Assistant)
                .find_map(|m| extract_note_content(&m.content));

            // Search assistant messages in reverse so the *last* message
            // containing the fenced block wins (mirrors extract_note_content).
            let suggested_next_steps = messages
                .iter()
                .rev()
                .filter(|m| m.role == MessageRole::Assistant)
                .find_map(|m| extract_suggested_next_steps(&m.content));
            if let Some(ref steps) = suggested_next_steps {
                log::info!(
                    "Session {session_id}: extracted suggested next steps — commit: {:?}, note: {:?}",
                    steps.suggested_next_commit_step,
                    steps.suggested_next_note_step,
                );
            }

            for target in &note_targets {
                let label = match target.kind {
                    NoteKind::Repo => "note",
                    NoteKind::Project => "project note",
                };
                if let Some(ref note_content) = note_content {
                    let (final_title, body) =
                        resolve_note_title_and_body(note_content, store, session_id);
                    log::info!(
                        "Session {session_id}: {} {label} \"{final_title}\"",
                        if target.is_amendment {
                            "amended"
                        } else {
                            "extracted"
                        }
                    );
                    // Also carry the stored next steps: a rewriting turn that
                    // omits the block keeps them rather than nulling them.
                    let scope =
                        match target.kind {
                            NoteKind::Repo => {
                                store.get_note(&target.id).ok().flatten().and_then(|note| {
                                    store
                                        .get_branch(&note.branch_id)
                                        .ok()
                                        .flatten()
                                        .map(|branch| {
                                            (
                                                Some(note.branch_id),
                                                branch.project_id,
                                                note.content,
                                                (
                                                    note.suggested_next_commit_step,
                                                    note.suggested_next_note_step,
                                                ),
                                            )
                                        })
                                })
                            }
                            NoteKind::Project => store
                                .get_project_note(&target.id)
                                .ok()
                                .flatten()
                                .map(|note| {
                                    (
                                        None,
                                        note.project_id,
                                        note.content,
                                        (
                                            note.suggested_next_commit_step,
                                            note.suggested_next_note_step,
                                        ),
                                    )
                                }),
                        };
                    let Some((branch_id, project_id, previous_content, stored_steps)) = scope
                    else {
                        log::warn!("Cannot resolve media scope for {label} {}", target.id);
                        continue;
                    };
                    let media = crate::note_media::ingest_note_media(
                        store,
                        crate::note_media::NoteScope {
                            branch_id: branch_id.as_deref(),
                            project_id: &project_id,
                            previous_content: &previous_content,
                        },
                        session_id,
                        workspace_name,
                        remote_working_dir
                            .filter(|_| workspace_name.is_some())
                            .map(std::path::Path::new)
                            .unwrap_or(working_dir),
                        &body,
                    );
                    // The turn's block, when present, replaces both steps; a
                    // turn without one leaves the stored pair alone, as the
                    // whole-transcript scan did before extraction was scoped
                    // to the turn. A first turn stores None either way.
                    let (sncs, snns) = match &suggested_next_steps {
                        Some(steps) => (
                            steps.suggested_next_commit_step.as_deref(),
                            steps.suggested_next_note_step.as_deref(),
                        ),
                        None => (stored_steps.0.as_deref(), stored_steps.1.as_deref()),
                    };
                    let result = match target.kind {
                        NoteKind::Repo => store.update_note_title_and_content(
                            &target.id,
                            &final_title,
                            &media.markdown,
                            sncs,
                            snns,
                        ),
                        NoteKind::Project => store.update_project_note_title_and_content(
                            &target.id,
                            &final_title,
                            &media.markdown,
                            sncs,
                            snns,
                        ),
                    };
                    media.finish(store, &previous_content, result.is_ok());
                    if let Err(e) = result {
                        log::error!("Failed to update {label} content: {e}");
                    }
                } else {
                    if target.is_amendment {
                        log::info!("Session {session_id}: turn did not rewrite the {label}; keeping existing content and attachments");
                    } else {
                        log::warn!("Session {session_id}: {label} session completed but no --- found in assistant output");
                    }
                    let result = match target.kind {
                        NoteKind::Repo => store.mark_note_completed(&target.id),
                        NoteKind::Project => store.mark_project_note_completed(&target.id),
                    };
                    if let Err(e) = result {
                        log::error!("Failed to mark {label} completed: {e}");
                    }
                }
            }
        }
    }

    // --- Review comment and title extraction ---
    if let Ok(Some(review)) = store.get_review_by_session(session_id) {
        if let Ok(messages) = store.get_session_messages(session_id) {
            let full_text: String = messages
                .iter()
                .filter(|m| m.role == MessageRole::Assistant)
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            // Extract and save review title (always attempt, even if comments exist)
            if review.title.is_none() {
                // Fall back to the ACP-provided session title when no
                // review-title block is found.
                let title = match extract_review_title(&full_text) {
                    Some(title) => {
                        log::info!("Session {session_id}: extracted review title: {title}");
                        Some(title)
                    }
                    None => {
                        let acp_title = store
                            .get_session(session_id)
                            .ok()
                            .flatten()
                            .as_ref()
                            .and_then(non_empty_acp_title);
                        if let Some(title) = &acp_title {
                            log::info!("Session {session_id}: no review-title block found; using ACP session title as review title: {title}");
                        }
                        acp_title
                    }
                };
                if let Some(title) = title {
                    if let Err(e) = store.update_review_title(&review.id, &title) {
                        log::error!("Failed to update review title: {e}");
                    }
                } else {
                    log::warn!("Session {session_id}: review session completed but no review-title block found");
                    // Still mark the review as completed so completed_at is set
                    // for timeline sorting, even without a title.
                    if let Err(e) = store.mark_review_completed(&review.id) {
                        log::error!("Failed to mark review completed: {e}");
                    }
                }
            }

            // Extract and save review comments
            if review.comments.is_empty() {
                let comments = extract_review_comments(&full_text);
                if comments.is_empty() {
                    log::warn!("Session {session_id}: review session completed but no review-comments block found");
                } else {
                    log::info!(
                        "Session {session_id}: extracted {} review comments",
                        comments.len()
                    );
                    for comment in comments {
                        if let Err(e) = store.add_comment(&review.id, &comment) {
                            log::error!("Failed to add review comment: {e}");
                        }
                    }
                }
            }
        }
    }
}

/// Which post-completion hooks a turn's terminal state calls for.
#[derive(Debug, PartialEq, Eq)]
enum TurnArtifactHooks {
    /// The turn completed: detect every artifact it produced.
    All,
    /// The turn's remote agent was lost: link the commit it may have made
    /// before it went. The commit survives on the workspace, and a resume
    /// re-captures `pre_head_sha`, so no later turn would link it. Its notes
    /// and pr/push output are a cut-off turn's, so they are not extracted.
    CommitOnly,
    /// Nothing the turn left is trusted as its result.
    Skip,
}

fn turn_artifact_hooks(
    completed_successfully: bool,
    completion_reason: &CompletionReason,
) -> TurnArtifactHooks {
    if completed_successfully {
        TurnArtifactHooks::All
    } else if *completion_reason == CompletionReason::RemoteAgentLost {
        TurnArtifactHooks::CommitOnly
    } else {
        TurnArtifactHooks::Skip
    }
}

/// Run the post-completion hooks a turn that has reached its terminal state
/// calls for (see [`turn_artifact_hooks`]).
fn detect_turn_artifacts(
    config: &SessionConfig,
    completed_successfully: bool,
    completion_reason: &CompletionReason,
    store: &Arc<Store>,
    turn_start_message_id: i64,
) {
    let remote_working_dir = config
        .remote_working_dir
        .as_deref()
        .and_then(|dir| dir.to_str());
    match turn_artifact_hooks(completed_successfully, completion_reason) {
        TurnArtifactHooks::All => run_post_completion_hooks(
            &config.session_id,
            &config.working_dir,
            config.pre_head_sha.as_deref(),
            config.workspace_name.as_deref(),
            remote_working_dir,
            store,
            turn_start_message_id,
        ),
        TurnArtifactHooks::CommitOnly => detect_session_commit(
            &config.session_id,
            &config.working_dir,
            config.pre_head_sha.as_deref(),
            config.workspace_name.as_deref(),
            remote_working_dir,
            store,
        ),
        TurnArtifactHooks::Skip => {}
    }
}

/// Link the commit session `session_id` made: when HEAD of the checkout it
/// ran in has moved past `pre_head_sha` (HEAD when its turn started), record
/// the new HEAD on the session's commit row. See
/// [`run_post_completion_hooks`].
fn detect_session_commit(
    session_id: &str,
    working_dir: &std::path::Path,
    pre_head_sha: Option<&str>,
    workspace_name: Option<&str>,
    remote_working_dir: Option<&str>,
    store: &Arc<Store>,
) {
    if let Some(pre_sha) = pre_head_sha {
        // Look for any commit linked to this session — not just pending (sha IS NULL)
        // ones — so we also detect amended commits on resumed sessions.
        if let Ok(Some(commit)) = store.get_commit_by_session(session_id) {
            // Get current HEAD from the checkout the session ran in — the
            // branch's clone directory on a remote workspace, which is not
            // where a bare `ws exec` lands when the workspace holds more than
            // one repo. The same runner is handed to reassociation below, so
            // the two can't end up describing different repos.
            let git = crate::branches::git_runner(
                working_dir,
                workspace_name,
                remote_working_dir.map(str::to_string),
            );

            match crate::branches::head_sha(&git) {
                Ok(current_head) if current_head != pre_sha => {
                    // A rebase pipeline that handed off to AI (conflicts, a
                    // failed fetch) lands here instead of
                    // `finalize_rebase_pipeline_without_ai`. If its turn ended
                    // with the rebase still stopped on a conflict, HEAD is
                    // detached on a partially applied commit — a SHA `git
                    // rebase --abort` erases — so nothing may claim it: not
                    // the pending row, not an amend, and no diff cache keyed
                    // on it. Skip the whole arm; the rows self-resolve on a
                    // later turn, because resumed sessions re-capture
                    // `pre_head_sha` and land back here once HEAD is attached
                    // again (after `--continue` finishes or `--abort`
                    // restores, reassociation plus the duplicate-SHA branch of
                    // `complete_pending_commit_sha` settle every row), while a
                    // turn that never comes leaves the pending row
                    // `sha IS NULL` — an ordinary failed commit attempt.
                    //
                    // Asking is the same call that does the work: the rebase
                    // rewrote SHAs just the same as the no-AI path, and
                    // reassociation has to check the attached HEAD anyway
                    // before it repoints anything, so it reports which way it
                    // went. That also keeps the load-bearing ordering — reattach
                    // the orphaned rows before the pending row below claims the
                    // new HEAD, see `finalize_rebase_pipeline_without_ai` for
                    // why — true by construction.
                    let rebase_pipeline = session_is_rebase_pipeline(store, session_id);
                    let mid_rebase = rebase_pipeline
                        && reassociate_rebased_commits(store, &commit.branch_id, &git)
                            == Reassociation::MidRebase;
                    if mid_rebase {
                        log::info!(
                            "Session {session_id}: rebase still in flight (HEAD detached), \
                             leaving commit detection for a later turn"
                        );
                    } else {
                        log::info!(
                            "Session {session_id}: new commit detected ({} → {})",
                            &pre_sha[..7.min(pre_sha.len())],
                            &current_head[..7.min(current_head.len())]
                        );
                        let recorded = if commit.sha.is_none() {
                            match store.complete_pending_commit_sha(
                                &commit.id,
                                &commit.branch_id,
                                &current_head,
                            ) {
                                Ok(recorded) => recorded,
                                Err(e) => {
                                    log::error!("Failed to update pending commit SHA: {e}");
                                    false
                                }
                            }
                        } else {
                            match store.get_commit_by_sha(&commit.branch_id, &current_head) {
                                Ok(Some(existing)) if existing.id != commit.id => {
                                    log::warn!(
                                        "Session {session_id}: target commit SHA already has metadata row {}, skipping update",
                                        existing.id
                                    );
                                    false
                                }
                                Ok(_) => {
                                    if let Err(e) =
                                        store.update_commit_sha(&commit.id, &current_head)
                                    {
                                        log::error!("Failed to update commit SHA: {e}");
                                        false
                                    } else {
                                        true
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to check existing commit SHA: {e}");
                                    false
                                }
                            }
                        };

                        if recorded {
                            // Spawn background diff caching for remote branches.
                            if let Some(ws_name) = workspace_name {
                                let commit_shas: Vec<String> = store
                                    .list_commits_for_branch(&commit.branch_id)
                                    .unwrap_or_default()
                                    .into_iter()
                                    .filter_map(|c| c.sha)
                                    .collect();
                                crate::diff_cache::spawn_cache_branch_diff(
                                    Arc::clone(store),
                                    commit.branch_id.clone(),
                                    ws_name.to_string(),
                                    current_head.clone(),
                                    commit_shas,
                                );
                            }
                        }
                    }
                }
                Ok(_) => {
                    if commit.sha.is_none() {
                        log::info!("Session {session_id}: no new commit (HEAD unchanged), leaving pending commit as failed");
                    } else {
                        log::info!("Session {session_id}: no commit change (HEAD unchanged)");
                    }
                }
                Err(e) => {
                    log::error!("Failed to get HEAD SHA after session: {e}");
                }
            }
        }
    }
}

/// Completion reason for a session whose last turn completed, given how its
/// agent connection settled.
///
/// A truncated wait is the one case a completed turn is *not* `TurnComplete`:
/// background work was still unconfirmed when the wait was cut off — by the
/// hard cap (`HeldUntilCap`), or by a Stop / agent exit during the hold
/// (`HoldStopped`) — so the session records the truncation, resumable so the
/// user can nudge it, while post-completion hooks still run best-effort.
/// Every other way of settling (no hold at all, or a confirmed-quiescent one)
/// is a clean turn.
fn completed_turn_completion_reason(
    settle_reason: Option<acp_client::SessionSettleReason>,
) -> CompletionReason {
    match settle_reason {
        Some(acp_client::SessionSettleReason::HeldUntilCap) => CompletionReason::HeldUntilCap,
        Some(acp_client::SessionSettleReason::HoldStopped) => CompletionReason::HoldStopped,
        _ => CompletionReason::TurnComplete,
    }
}

/// Whether a completed turn stays completed when a cancel is observed at
/// teardown, rather than being recorded `cancelled`.
///
/// Settling is not instantaneous: after the connection reports its outcome the
/// task still finalizes the writer and gracefully stops the child (seconds, for
/// a remote session). A Stop pressed in that window cancels the token but has
/// nothing left to stop — the turn finished and the hold already reached its
/// own outcome. Recording `cancelled` there would erase a fully-drained turn's
/// artifacts and skip its post-completion hooks: exactly what the background
/// hold exists to prevent.
///
/// So every reason reached *through* a hold survives a late cancel:
/// - `HoldStopped` — the connection settled knowing about the cancel; it
///   applied to the wait, not the turn (a cancel during the hold can only
///   settle this way, never `Quiescent`).
/// - `ResponseComplete` — the caller accepted a finished response.
/// - `Quiescent` — the background set drained and the agent confirmed idle.
/// - `HeldUntilCap` — the cap truncated the wait before the cancel arrived.
///
/// `Immediate` (the connection tore down as the prompt resolved, so there was
/// no hold) and a connection that never reported a settle reason keep the
/// legacy semantics: they have no post-turn window of their own, and a cancel
/// racing their teardown has always recorded `cancelled`.
fn completed_turn_survives_late_cancel(
    settle_reason: Option<acp_client::SessionSettleReason>,
) -> bool {
    matches!(
        settle_reason,
        Some(
            acp_client::SessionSettleReason::Quiescent
                | acp_client::SessionSettleReason::ResponseComplete
                | acp_client::SessionSettleReason::HeldUntilCap
                | acp_client::SessionSettleReason::HoldStopped
        )
    )
}

/// Whether a finished turn should immediately start the next queued message.
///
/// Recording finished work and *starting new work* are separate decisions, so
/// this is deliberately stricter than
/// [`terminal_state_completed_successfully`] alone. Since a turn that held for
/// background work survives a late cancel (see
/// [`completed_turn_survives_late_cancel`]), a Stop pressed in the teardown
/// window records `completed` — and a session with a queued follow-up would
/// then launch it. From the user's side they pressed Stop on a session the UI
/// still showed as running and got a new turn instead of a stop.
///
/// So the completed turn keeps everything it earned — its status, its hooks,
/// its commit detection, its pr/push outcomes — and the Stop keeps its one
/// meaning: no further work starts.
fn queued_follow_up_should_start(completed_successfully: bool, cancel_requested: bool) -> bool {
    completed_successfully && !cancel_requested
}

/// Why a session whose run failed ended: its remote agent was lost under it,
/// or the agent failed it.
fn failed_run_completion_reason(error: &str) -> CompletionReason {
    // A turn still detached once reattaching gave up lost its agent as far
    // as the session can tell.
    if acp_client::is_remote_agent_lost_error(error)
        || acp_client::is_remote_agent_detached_error(error)
    {
        CompletionReason::RemoteAgentLost
    } else {
        CompletionReason::Crashed
    }
}

/// Whether a terminal state is "the agent did the work" — the gate for
/// post-completion hooks, auto-review, and (with
/// [`queued_follow_up_should_start`] on top) draining a queued follow-up.
///
/// `HeldUntilCap` and `HoldStopped` count: the turn itself completed and only
/// the *wait* for its background work was truncated, so artifacts it produced
/// are still worth detecting (best-effort, per the background-hold plan).
fn terminal_state_completed_successfully(
    status: &str,
    completion_reason: &CompletionReason,
) -> bool {
    status == SessionStatus::Completed.as_str()
        && matches!(
            completion_reason,
            CompletionReason::TurnComplete
                | CompletionReason::HeldUntilCap
                | CompletionReason::HoldStopped
        )
}

/// Extract note content from a single assistant message.
///
/// Callers are responsible for choosing which message to pass — typically the
/// **last** assistant message that contains a `---`. Within that message we
/// use the **first** HR (outside code fences) as the note separator, because
/// the agent places the `---` at the boundary between preamble and note
/// content.
///
/// Primary path: find the first markdown horizontal rule (`---`, `***`, `___`)
/// on its own line (outside code fences) and return everything after it.
///
/// Fallback path: handle model outputs where the rule is accidentally attached
/// to prior text (for example, `Preamble.---\n# Title`) by accepting inline
/// rules only when the remaining content starts with an H1.
pub(crate) fn extract_note_content(text: &str) -> Option<String> {
    let sanitized = strip_suggested_next_steps_blocks(text);
    let text = sanitized.as_deref().unwrap_or(text);
    extract_note_after_standalone_hr(text).or_else(|| extract_note_after_inline_hr(text))
}

/// Remove assistant response metadata before scanning for the note separator.
/// The note body remains normal markdown and can still contain fenced code.
fn strip_suggested_next_steps_blocks(text: &str) -> Option<String> {
    let marker = "```suggested-next-steps";
    let mut output = String::new();
    let mut last_copied = 0;
    let mut search_from = 0;
    let mut removed_any = false;

    while search_from < text.len() {
        let Some(rel_pos) = find_suggested_next_steps_opening_fence(&text[search_from..], marker)
        else {
            break;
        };
        let start_pos = search_from + rel_pos;
        let block_start = start_pos + marker.len();
        let Some(newline_pos) = text[block_start..].find('\n') else {
            break;
        };
        let content_start = block_start + newline_pos + 1;
        let Some(end_pos) = find_closing_fence(&text[content_start..]) else {
            break;
        };

        let closing_start = content_start + end_pos;
        let after_closing = text[closing_start..]
            .find('\n')
            .map(|newline| closing_start + newline + 1)
            .unwrap_or(text.len());

        output.push_str(&text[last_copied..start_pos]);
        last_copied = after_closing;
        search_from = after_closing;
        removed_any = true;
    }

    if removed_any {
        output.push_str(&text[last_copied..]);
        Some(output)
    } else {
        None
    }
}

fn extract_note_after_standalone_hr(text: &str) -> Option<String> {
    // Look for --- on its own line (possibly with surrounding whitespace).
    // We match the same patterns markdown parsers treat as thematic breaks:
    // a line containing only ---, ***, or ___ (with optional spaces).
    //
    // We scan from the **top** so the *first* HR wins. The caller already
    // selects the last assistant message that contains a separator, so
    // within that message the first `---` marks the start of the note.
    //
    // We track whether we are inside a fenced code block (``` or ~~~) and
    // skip any HR that appears inside one.
    let lines: Vec<&str> = text.lines().collect();

    // Pre-compute which lines are inside a code fence.
    let mut in_fence = vec![false; lines.len()];
    let mut inside = false;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            inside = !inside;
        }
        in_fence[i] = inside;
    }

    for i in 0..lines.len() {
        if in_fence[i] {
            continue;
        }
        let trimmed = lines[i].trim();
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            let remaining: String = lines[i + 1..].join("\n");
            let trimmed_remaining = remaining.trim().to_string();
            if !trimmed_remaining.is_empty() {
                return Some(trimmed_remaining);
            }
        }
    }
    None
}

fn extract_note_after_inline_hr(text: &str) -> Option<String> {
    // Pre-compute a set of byte offsets that fall inside fenced code blocks
    // so we can skip inline HRs that appear in code examples.
    let fence_ranges = compute_fence_ranges(text);

    let mut best: Option<(usize, String)> = None;

    for marker in ["---", "***", "___"] {
        let marker_char = marker.chars().next().unwrap();
        for (idx, _) in text.match_indices(marker) {
            // Skip markers inside fenced code blocks.
            if fence_ranges
                .iter()
                .any(|(start, end)| idx >= *start && idx < *end)
            {
                continue;
            }

            let marker_end = idx + marker.len();

            // Ignore markers that are part of longer runs like ----.
            if text[..idx].ends_with(marker_char) || text[marker_end..].starts_with(marker_char) {
                continue;
            }

            let remaining = text[marker_end..].trim_start();
            if !remaining.starts_with("# ") {
                continue;
            }

            // Keep the *first* (earliest) match — the note starts at the
            // first separator in the message.
            match best {
                Some((best_idx, _)) if idx >= best_idx => {}
                _ => best = Some((idx, remaining.to_string())),
            }
        }
    }

    best.map(|(_, content)| content)
}

/// Return byte-offset ranges `(start, end)` for content inside fenced code
/// blocks (``` or ~~~). The ranges cover from the character after the opening
/// fence line's newline to the start of the closing fence line.
fn compute_fence_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut fence_start: Option<usize> = None;

    for (line_start, line) in line_byte_offsets(text) {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if fence_start.is_some() {
                // Closing fence — record the range.
                ranges.push((fence_start.unwrap(), line_start));
                fence_start = None;
            } else {
                // Opening fence — content starts after this line.
                let after_line = line_start + line.len() + 1; // +1 for newline
                fence_start = Some(after_line.min(text.len()));
            }
        }
    }
    ranges
}

/// Iterate over `(byte_offset, line_str)` pairs for each line in `text`.
fn line_byte_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.lines().map(move |line| {
        let start = offset;
        // +1 accounts for the newline character (or end of string for last line)
        offset += line.len() + 1;
        (start, line)
    })
}

/// Strip leading `<action>...</action>` blocks from a prompt so that fallback
/// title generation uses the user's actual text rather than injected XML.
fn strip_action_wrapper(prompt: &str) -> &str {
    let trimmed = prompt.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<action>") {
        if let Some(end) = rest.find("</action>") {
            return rest[end + "</action>".len()..].trim_start();
        }
    }
    prompt
}

/// Extract a title (leading `# H1`) from note content.
///
/// Returns `(title, body_without_title)`. If no H1 is found, title is empty
/// and body is the full content.
fn extract_note_title(content: &str) -> (String, String) {
    let trimmed = content.trim_start();
    if let Some(rest) = trimmed.strip_prefix("# ") {
        // Find the end of the first line (the title)
        if let Some(newline_pos) = rest.find('\n') {
            let title = rest[..newline_pos].trim().to_string();
            let body = rest[newline_pos + 1..].trim_start().to_string();
            (title, body)
        } else {
            // The entire content is just the title
            (rest.trim().to_string(), String::new())
        }
    } else {
        (String::new(), content.to_string())
    }
}

/// The session's ACP-provided title, trimmed, when set and non-empty.
///
/// Used as an end-of-session fallback name when the session produced no
/// better title (note without an H1, review without a review-title block).
fn non_empty_acp_title(session: &crate::store::Session) -> Option<String> {
    session
        .acp_title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// Parse note content into a final `(title, body)` pair. When the note has
/// no H1 heading, fall back to the ACP-provided session title, then the
/// session prompt.
///
/// Shared by both repo-note and project-note extraction paths.
fn resolve_note_title_and_body(
    note_content: &str,
    store: &Store,
    session_id: &str,
) -> (String, String) {
    let (title, body) = extract_note_title(note_content);
    if !title.is_empty() {
        return (title, body);
    }
    let session = store.get_session(session_id).ok().flatten();
    let final_title = session
        .as_ref()
        .and_then(non_empty_acp_title)
        .or_else(|| {
            session.as_ref().map(|s| {
                let prompt = strip_action_wrapper(&s.prompt);
                let t: String = prompt.chars().take(80).collect();
                if prompt.len() > 80 {
                    format!("{t}…")
                } else {
                    t
                }
            })
        })
        .unwrap_or_else(|| "Untitled Note".to_string());
    (final_title, body)
}

/// Extract review comments from assistant output.
///
/// Looks for ```review-comments fenced blocks and parses the JSON array inside.
/// Each object should have `path`, `span` (with `start` and `end`), and `content`.
fn extract_review_comments(text: &str) -> Vec<Comment> {
    let mut comments = Vec::new();

    // Find all ```review-comments blocks
    let marker_start = "```review-comments";

    let mut search_from = 0;
    while let Some(rel_pos) = find_opening_fence(&text[search_from..], marker_start) {
        let start_pos = search_from + rel_pos;
        let block_start = start_pos + marker_start.len();
        // Skip to the next line after the opening marker
        let json_start = match text[block_start..].find('\n') {
            Some(pos) => block_start + pos + 1,
            None => break,
        };

        // Find the closing ``` — must be on its own line (start of line),
        // not an embedded code fence inside JSON string values like ```rust.
        // We look for "\n```" where the ``` is followed by EOF, newline, or
        // only whitespace (not an info-string like "rust" or "sql").
        if let Some(end_pos) = find_closing_fence(&text[json_start..]) {
            let json_str = &text[json_start..json_start + end_pos];

            // Parse the JSON array
            if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(json_str.trim()) {
                for item in parsed {
                    let path = item.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let content = item.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let comment_type_str = item.get("type").and_then(|v| v.as_str());
                    let span_obj = item.get("span");
                    let start = span_obj
                        .and_then(|s| s.get("start"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    let end = span_obj
                        .and_then(|s| s.get("end"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;

                    if !path.is_empty() && !content.is_empty() {
                        let mut comment = Comment::new(path, Span::new(start, end), content)
                            .with_author(CommentAuthor::Agent);
                        if let Some(ct) = comment_type_str.and_then(CommentType::parse) {
                            comment = comment.with_comment_type(ct);
                        }
                        comments.push(comment);
                    }
                }
            } else {
                log::warn!("Failed to parse review-comments JSON block");
            }

            search_from = json_start + end_pos + 3; // skip past the closing ```
        } else {
            break;
        }
    }

    comments
}

/// Extract the review title from assistant output.
///
/// Looks for a ```review-title fenced block and returns the trimmed text inside.
/// The opening fence must appear at the start of a line to avoid matching the
/// marker when it appears inside regular prose (e.g. the LLM discussing the
/// extraction logic itself).
fn extract_review_title(text: &str) -> Option<String> {
    let marker = "```review-title";
    let start_pos = find_opening_fence(text, marker)?;
    let block_start = start_pos + marker.len();
    let content_start = block_start + text[block_start..].find('\n')? + 1;
    let end_pos = find_closing_fence(&text[content_start..])?;
    let title = text[content_start..content_start + end_pos].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

/// Structured representation of the suggested next steps extracted from a
/// `suggested-next-steps` fenced block.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuggestedNextSteps {
    suggested_next_commit_step: Option<String>,
    suggested_next_note_step: Option<String>,
}

/// Extract suggested next steps from assistant output.
///
/// Looks for a ```suggested-next-steps fenced block and parses the JSON object
/// inside.  Returns `None` if the block is missing or cannot be parsed.
fn extract_suggested_next_steps(text: &str) -> Option<SuggestedNextSteps> {
    let marker = "```suggested-next-steps";
    let start_pos = find_suggested_next_steps_opening_fence(text, marker)?;
    let block_start = start_pos + marker.len();
    let content_start = block_start + text[block_start..].find('\n')? + 1;
    let end_pos = find_closing_fence(&text[content_start..])?;
    let json_str = text[content_start..content_start + end_pos].trim();
    match serde_json::from_str::<SuggestedNextSteps>(json_str) {
        Ok(steps) => Some(steps),
        Err(e) => {
            log::warn!("Failed to parse suggested-next-steps JSON: {e}");
            None
        }
    }
}

/// Find a `suggested-next-steps` opening fence.
///
/// The normal fence finder requires markers to appear at the start of a line
/// because review extraction should not match markers mentioned in prose. Notes
/// can arrive with this fence attached to the final sentence, so this finder
/// accepts inline markers while still requiring the rest of the marker line to
/// contain only optional whitespace before the newline.
fn find_suggested_next_steps_opening_fence(text: &str, marker: &str) -> Option<usize> {
    let mut pos = 0;
    while pos < text.len() {
        let candidate = text[pos..].find(marker)?;
        let abs = pos + candidate;
        let after_marker = &text[abs + marker.len()..];
        if after_marker
            .find('\n')
            .is_some_and(|newline| after_marker[..newline].trim().is_empty())
        {
            return Some(abs);
        }
        pos = abs + marker.len();
    }
    None
}

/// Find an opening fence marker (e.g. ` ```review-title `) that appears at the
/// start of a line (position 0 or immediately after `\n`).  Returns the byte
/// offset of the marker within `text`, or `None` if no line-start match exists.
fn find_opening_fence(text: &str, marker: &str) -> Option<usize> {
    let mut pos = 0;
    while pos < text.len() {
        let candidate = text[pos..].find(marker)?;
        let abs = pos + candidate;
        if abs == 0 || text.as_bytes()[abs - 1] == b'\n' {
            return Some(abs);
        }
        pos = abs + marker.len();
    }
    None
}

/// Find the closing ``` fence for a code block.
///
/// A closing fence must appear at the start of a line (after a newline) and
/// must consist of exactly ``` followed by EOF, a newline, or only whitespace.
/// This distinguishes it from opening fences like ```rust or ``` embedded
/// within JSON string values (where the ``` appears mid-line after `\n` escape
/// sequences in the JSON, not actual newlines in the text).
fn find_closing_fence(text: &str) -> Option<usize> {
    let fence = "```";
    let mut pos = 0;
    while pos < text.len() {
        let remaining = &text[pos..];
        let candidate = remaining.find(fence)?;
        let abs = pos + candidate;

        // Must be at column 0: either start of text or preceded by '\n'
        let at_line_start = abs == 0 || text.as_bytes()[abs - 1] == b'\n';
        if at_line_start {
            // Check what follows the ```: must be EOF, newline, or whitespace-only to EOL
            let after = &text[abs + fence.len()..];
            let is_closing = if after.is_empty() {
                true
            } else {
                match after.find('\n') {
                    Some(nl) => after[..nl].trim().is_empty(),
                    None => after.trim().is_empty(),
                }
            };
            if is_closing {
                return Some(abs);
            }
        }

        pos = abs + fence.len();
    }
    None
}

/// Generic over the runtime purely so the paths that end a session *before* it
/// runs (see [`finish_cancelled_before_run`]) and the stranded-session sweep
/// can be driven by a mock app in tests. Every production caller passes the
/// concrete handle.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_status<R: tauri::Runtime>(
    app_handle: &AppHandle<R>,
    session_id: &str,
    status: &str,
    error: Option<String>,
    completion_reason: Option<&CompletionReason>,
    rate_limit_recovery: Option<&RateLimitRecovery>,
    branch_id: Option<String>,
    project_id: Option<String>,
) {
    let event = SessionStatusEvent {
        session_id: session_id.to_string(),
        status: status.to_string(),
        error_message: error,
        completion_reason: completion_reason.map(|r| r.as_str().to_string()),
        rate_limit_resets_at: rate_limit_recovery.and_then(|details| details.resets_at.clone()),
        rate_limit_kind: rate_limit_recovery.and_then(|details| details.kind.clone()),
        branch_id,
        project_id,
        session_type: None,
        dependency_skipped: false,
    };
    crate::web_server::emit_to_all(app_handle, "session-status-changed", &event);
}

/// Emit a `session-background-hold` event: the session is (or is no longer)
/// held open past turn end for background work.
///
/// Deliberately *not* a status change — the session stays `running` while it
/// holds, so this only carries the presentational sub-state the frontend
/// renders in place of its running indicator ("waiting on background task
/// (N)", with a stop affordance). Keeping it off `SessionStatus` avoids a
/// schema migration for a state that is never terminal.
///
/// Emitted on *change*, so a client that wasn't listening yet asks for the
/// current hold instead — see [`SessionBackgroundHoldSnapshot`].
fn emit_background_hold(
    app_handle: &AppHandle,
    session_id: &str,
    status: acp_client::BackgroundHoldStatus,
    branch_id: Option<String>,
    project_id: Option<String>,
) {
    let SessionBackgroundHoldSnapshot {
        holding,
        live_tasks,
        tasks,
    } = status.into();
    let event = SessionBackgroundHoldEvent {
        session_id: session_id.to_string(),
        holding,
        live_tasks,
        tasks,
        branch_id,
        project_id,
    };
    crate::web_server::emit_to_all(app_handle, "session-background-hold", &event);
}

/// Emit a `session-status-changed` event with `"running"` status and branch/project
/// context for a repo session that is starting *immediately*, outside the queue
/// drain. The callers are the immediate branch start in
/// `start_or_queue_branch_session_for_store` and the `prs.rs` pipelines; the
/// event goes out before the caller answers the client so the frontend registers
/// the session and refreshes the branch timeline before it can complete.
///
/// Not the MCP path: `start_repo_session` writes a `queued` stub and kicks the
/// drain, and `start_queued_session_for_branch` announces `running` itself when
/// it claims the row. `branch_id` is a non-optional `&str` here because the
/// emitted event must carry it — see the invariant documented on
/// [`SessionStatusEvent::branch_id`].
pub fn emit_session_running(
    app_handle: &AppHandle,
    session_id: &str,
    branch_id: &str,
    project_id: &str,
    session_type: &str,
) {
    let event = SessionStatusEvent {
        session_id: session_id.to_string(),
        status: "running".to_string(),
        error_message: None,
        completion_reason: None,
        rate_limit_resets_at: None,
        rate_limit_kind: None,
        branch_id: Some(branch_id.to_string()),
        project_id: Some(project_id.to_string()),
        session_type: Some(session_type.to_string()),
        dependency_skipped: false,
    };
    crate::web_server::emit_to_all(app_handle, "session-status-changed", &event);
}

#[cfg(test)]
mod tests {
    mod note_completion;
    use super::*;
    use crate::git::strip_git_env;
    use std::cell::RefCell;

    fn session_message(role: MessageRole, content: &str) -> SessionMessage {
        SessionMessage {
            id: 0,
            session_id: "session-1".to_string(),
            role,
            content: content.to_string(),
            created_at: 0,
            image_ids: vec![],
            acp: Default::default(),
        }
    }

    #[test]
    fn a_cap_truncated_hold_completes_with_its_own_reason() {
        use acp_client::SessionSettleReason;

        assert_eq!(
            completed_turn_completion_reason(Some(SessionSettleReason::HeldUntilCap)),
            CompletionReason::HeldUntilCap
        );
        // A stop (or agent exit) during the hold truncates the wait the same
        // way — the turn completed, only its background work went unconfirmed.
        assert_eq!(
            completed_turn_completion_reason(Some(SessionSettleReason::HoldStopped)),
            CompletionReason::HoldStopped
        );
        // Teardown right after the prompt (no hold), a confirmed-quiescent
        // hold, and a connection that never reported a settle reason are all
        // clean turns.
        for settle in [
            None,
            Some(SessionSettleReason::Immediate),
            Some(SessionSettleReason::Quiescent),
        ] {
            assert_eq!(
                completed_turn_completion_reason(settle),
                CompletionReason::TurnComplete,
                "{settle:?} should be a clean turn"
            );
        }
    }

    #[test]
    fn a_cap_truncated_hold_still_runs_post_completion_hooks() {
        // The turn completed; only the wait for its background work was cut
        // short, so artifacts it produced are still worth detecting.
        assert!(terminal_state_completed_successfully(
            "completed",
            &CompletionReason::HeldUntilCap
        ));
        assert!(terminal_state_completed_successfully(
            "completed",
            &CompletionReason::HoldStopped
        ));
        assert!(terminal_state_completed_successfully(
            "completed",
            &CompletionReason::TurnComplete
        ));
        for reason in [
            CompletionReason::Interrupted,
            CompletionReason::ProjectSessionInterrupted,
            CompletionReason::Crashed,
            CompletionReason::RemoteAgentLost,
            CompletionReason::AppQuit,
            CompletionReason::Unknown,
        ] {
            assert!(
                !terminal_state_completed_successfully("completed", &reason),
                "{reason:?} is not a successful turn"
            );
        }
        assert!(!terminal_state_completed_successfully(
            "cancelled",
            &CompletionReason::HeldUntilCap
        ));
    }

    #[test]
    fn a_run_that_lost_its_remote_agent_fails_as_remote_agent_lost() {
        assert_eq!(
            failed_run_completion_reason(
                "Remote agent lost: bloxlet reports the agent process (command 27101663) \
                 as failed: bloxlet restarted while command was running"
            ),
            CompletionReason::RemoteAgentLost
        );
        assert_eq!(
            failed_run_completion_reason(
                "Remote agent detached: the `sq blox acp` proxy exited, and whether the agent \
                 process (command 27101663) still runs is unknown: `sq blox ws commands` timed out"
            ),
            CompletionReason::RemoteAgentLost,
            "a turn reattaching could not pick back up is lost as far as the session can tell"
        );
        assert_eq!(
            failed_run_completion_reason("ACP protocol failed: server shut down unexpectedly"),
            CompletionReason::Crashed
        );
    }

    #[test]
    fn a_dead_owners_remote_session_is_reattached_and_a_local_one_quit() {
        let store = Store::in_memory().unwrap();
        let project = crate::store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let local = crate::store::Branch::new(&project.id, "local", "main");
        store.create_branch(&local).unwrap();
        let remote = crate::store::Branch::new_remote(&project.id, "remote", "main", "ws-1");
        store.create_branch(&remote).unwrap();
        let dir = std::env::temp_dir();
        // The rows a run writes once the agent has started answering it.
        let answered_run = |session: &crate::store::Session| {
            store
                .add_session_message(&session.id, MessageRole::User, "go")
                .unwrap();
            store
                .add_acp_metadata_message(
                    &session.id,
                    &crate::store::AcpMessageMetadata {
                        acp_event_kind: Some("initialize".to_string()),
                        ..Default::default()
                    },
                )
                .unwrap();
            store
                .add_session_message(&session.id, MessageRole::Assistant, "On it")
                .unwrap();
        };

        // A local agent ends with the app that ran it.
        let session = crate::store::Session::new_running("go", &dir)
            .with_branch(&local.id)
            .with_agent("agent-local");
        store.create_session(&session).unwrap();
        answered_run(&session);
        assert_eq!(
            dead_owner_recovery(&store, &session),
            DeadOwnerRecovery::AppQuit
        );

        // A remote agent may still be serving the turn, whether the session
        // reaches its branch directly or through the commit it is making.
        let session = crate::store::Session::new_running("go", &dir)
            .with_branch(&remote.id)
            .with_agent("agent-remote");
        store.create_session(&session).unwrap();
        answered_run(&session);
        assert_eq!(
            dead_owner_recovery(&store, &session),
            DeadOwnerRecovery::Reattach
        );
        let session = crate::store::Session::new_running("go", &dir).with_agent("agent-commit");
        store.create_session(&session).unwrap();
        let commit = crate::store::Commit::new_pending(&remote.id).with_session(&session.id);
        store.create_commit(&commit).unwrap();
        answered_run(&session);
        assert_eq!(
            dead_owner_recovery(&store, &session),
            DeadOwnerRecovery::Reattach
        );

        // A follow-up run has the earlier run's agent session, but an owner
        // that went while it was still connecting never sent its prompt: the
        // idle agent must not read as that run having completed.
        let session = crate::store::Session::new_running("go", &dir)
            .with_branch(&remote.id)
            .with_agent("agent-follow-up");
        store.create_session(&session).unwrap();
        answered_run(&session);
        store
            .add_session_message(&session.id, MessageRole::User, "and push it")
            .unwrap();
        assert_eq!(
            dead_owner_recovery(&store, &session),
            DeadOwnerRecovery::AppQuit
        );
        store
            .add_acp_metadata_message(
                &session.id,
                &crate::store::AcpMessageMetadata {
                    acp_event_kind: Some("initialize".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            dead_owner_recovery(&store, &session),
            DeadOwnerRecovery::AppQuit,
            "a connection that set up but has no answer may not have sent the prompt"
        );
        store
            .add_session_message(&session.id, MessageRole::ToolCall, "git push")
            .unwrap();
        assert_eq!(
            dead_owner_recovery(&store, &session),
            DeadOwnerRecovery::Reattach
        );

        // A remote owner that died before the agent had a session leaves
        // nothing to reattach to.
        let session = crate::store::Session::new_running("go", &dir).with_branch(&remote.id);
        assert_eq!(
            dead_owner_recovery(&store, &session),
            DeadOwnerRecovery::AppQuit
        );
        let pending = format!(
            "{}start",
            crate::session_commands::PENDING_BRANCH_START_AGENT_ID_PREFIX
        );
        let session = crate::store::Session::new_running("go", &dir)
            .with_branch(&remote.id)
            .with_agent(&pending);
        assert_eq!(
            dead_owner_recovery(&store, &session),
            DeadOwnerRecovery::AppQuit
        );
    }

    #[test]
    fn head_before_finds_the_head_a_turn_started_from() {
        let dir = std::env::temp_dir().join(format!(
            "staged-head-before-{}",
            crate::store::now_timestamp()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init"]);
        run_git(&dir, &["config", "user.email", "test@example.com"]);
        run_git(&dir, &["config", "user.name", "Test User"]);
        let commit_at = |secs: i64| {
            std::fs::write(dir.join("file.txt"), secs.to_string()).unwrap();
            run_git(&dir, &["add", "file.txt"]);
            let mut command = std::process::Command::new("git");
            command.args(["commit", "-m", "change"]).current_dir(&dir);
            strip_git_env(&mut command);
            let date = format!("@{secs} +0000");
            command
                .env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date);
            assert!(command.output().unwrap().status.success());
            run_git(&dir, &["rev-parse", "HEAD"])
        };
        let first = commit_at(1_000_000_000);
        let second = commit_at(1_000_000_100);
        let third = commit_at(1_000_000_200);
        let git: crate::branches::GitRunner<'_> = Box::new(|args| Ok(run_git(&dir, args)));

        // `at` is in milliseconds, like the message timestamps it comes from.
        assert_eq!(
            head_before(&git, 1_000_000_150_000).unwrap(),
            Some(second.clone())
        );
        assert_eq!(
            head_before(&git, 1_000_000_101_000).unwrap(),
            Some(second.clone())
        );
        // A commit dated in the prompt's own second may be the turn's.
        assert_eq!(
            head_before(&git, 1_000_000_100_999).unwrap(),
            Some(first.clone())
        );
        assert_eq!(
            head_before(&git, 1_000_000_100_000).unwrap(),
            Some(first.clone())
        );
        assert_eq!(head_before(&git, 1_000_000_099_000).unwrap(), Some(first));
        assert_eq!(head_before(&git, 1_100_000_000_000).unwrap(), Some(third));
        assert_eq!(head_before(&git, 999_999_999_000).unwrap(), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A turn that brings in a commit dated before its prompt (a rebase onto
    /// or a reset to older upstream work) leaves commit dates out of order.
    /// HEAD's reflog still says where HEAD was when the turn started.
    #[test]
    fn head_at_reads_the_reflog_rather_than_commit_dates() {
        let dir = std::env::temp_dir().join(format!(
            "staged-head-at-{}-{}",
            std::process::id(),
            crate::store::now_timestamp()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init"]);
        run_git(&dir, &["config", "user.email", "test@example.com"]);
        run_git(&dir, &["config", "user.name", "Test User"]);
        // Commit and reflog times both follow the committer date.
        let git_at = |secs: i64, args: &[&str]| {
            let mut command = std::process::Command::new("git");
            command.args(args).current_dir(&dir);
            strip_git_env(&mut command);
            let date = format!("@{secs} +0000");
            command
                .env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date);
            let output = command.output().unwrap();
            assert!(output.status.success(), "git {args:?} failed");
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        std::fs::write(dir.join("file.txt"), "one\n").unwrap();
        run_git(&dir, &["add", "file.txt"]);
        git_at(1_000_000_000, &["commit", "-m", "start"]);
        let start = run_git(&dir, &["rev-parse", "HEAD"]);
        let tree = run_git(&dir, &["write-tree"]);
        let upstream = git_at(
            1_000_000_200,
            &["commit-tree", &tree, "-p", &start, "-m", "upstream"],
        );
        // The turn, prompted at 1_000_000_500, resets onto it at 600.
        git_at(1_000_000_600, &["reset", "--hard", &upstream]);
        let git: crate::branches::GitRunner<'_> = Box::new(|args| Ok(run_git(&dir, args)));

        assert_eq!(
            head_before(&git, 1_000_000_500_000).unwrap(),
            Some(upstream.clone()),
            "commit dates alone put the turn's own change before it"
        );
        assert_eq!(
            head_at(&git, 1_000_000_500_000).unwrap(),
            Some(start.clone())
        );
        assert_eq!(head_at(&git, 1_000_000_700_000).unwrap(), Some(upstream));
        // Before the reflog begins, commit dates are all there is.
        assert_eq!(head_at(&git, 999_999_000_000).unwrap(), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A repo whose HEAD starts at a commit dated `start` and whose turn,
    /// prompted at `prompted_at` (ms), commits at `turn_commit` (seconds),
    /// with a pending commit row for that turn's session. Returns the repo,
    /// the store, the commit row id, the session id and the two SHAs.
    fn turn_committed_at(
        label: &str,
        start: i64,
        turn_commit: i64,
    ) -> (PathBuf, Store, String, String, String, String) {
        let dir = std::env::temp_dir().join(format!(
            "staged-{label}-{}-{}",
            std::process::id(),
            crate::store::now_timestamp()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init"]);
        run_git(&dir, &["config", "user.email", "test@example.com"]);
        run_git(&dir, &["config", "user.name", "Test User"]);
        let commit_at = |secs: i64, message: &str| {
            std::fs::write(dir.join("file.txt"), message).unwrap();
            run_git(&dir, &["add", "file.txt"]);
            let mut command = std::process::Command::new("git");
            command.args(["commit", "-m", message]).current_dir(&dir);
            strip_git_env(&mut command);
            // Commit and reflog times both follow the committer date.
            let date = format!("@{secs} +0000");
            command
                .env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date);
            assert!(command.output().unwrap().status.success());
            run_git(&dir, &["rev-parse", "HEAD"])
        };
        let start_sha = commit_at(start, "start");
        let turn_sha = commit_at(turn_commit, "turn");

        let store = Store::in_memory().unwrap();
        let project = crate::store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = crate::store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        let session = crate::store::Session::new_running("Commit the fix", &dir);
        store.create_session(&session).unwrap();
        let commit = crate::store::Commit::new_pending(&branch.id).with_session(&session.id);
        store.create_commit(&commit).unwrap();
        (dir, store, commit.id, session.id, start_sha, turn_sha)
    }

    /// Git stamps reflog entries in whole seconds. One in the prompt's own
    /// second can't be ordered against it, and may be the turn's own commit,
    /// so it is not taken for the HEAD the turn started from.
    #[test]
    fn head_at_takes_a_reflog_entry_in_the_prompts_second_as_the_turns() {
        let (dir, _store, _commit, _session, start, turn) =
            turn_committed_at("head-at-same-second", 1_000_000_000, 1_000_000_500);
        let git: crate::branches::GitRunner<'_> = Box::new(|args| Ok(run_git(&dir, args)));

        for prompted_at in [1_000_000_500_000, 1_000_000_500_200, 1_000_000_500_999] {
            assert_eq!(
                head_at(&git, prompted_at).unwrap(),
                Some(start.clone()),
                "prompted at {prompted_at}"
            );
        }
        // A second later the commit certainly came first.
        assert_eq!(head_at(&git, 1_000_000_501_000).unwrap(), Some(turn));
        assert_eq!(head_at(&git, 1_000_000_499_999).unwrap(), Some(start));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The review's case end to end: a reattached turn whose agent committed
    /// in the same second as its prompt still links that commit.
    #[test]
    fn a_reattached_turn_links_a_commit_made_in_its_prompts_second() {
        let (dir, store, commit, session, start, turn) =
            turn_committed_at("same-second-commit", 1_000_000_000, 1_000_000_500);
        let store = Arc::new(store);
        let git: crate::branches::GitRunner<'_> = Box::new(|args| Ok(run_git(&dir, args)));
        let pre_head = head_at(&git, 1_000_000_500_200).unwrap();
        assert_eq!(pre_head.as_deref(), Some(start.as_str()));

        detect_session_commit(&session, &dir, pre_head.as_deref(), None, None, &store);

        let linked = store.get_commit(&commit).unwrap().unwrap();
        assert_eq!(linked.sha.as_deref(), Some(turn.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn head_reflog_lines_parse_and_noise_is_skipped() {
        assert_eq!(
            parse_head_reflog(
                "ede99057:HEAD@{1000000600}\n\
                 Last login: today\n\
                 f933ae9c:HEAD@{1000000000}\n\
                 abc:HEAD@{soon}\n"
            ),
            vec![
                ("ede99057".to_string(), 1_000_000_600),
                ("f933ae9c".to_string(), 1_000_000_000),
            ]
        );
    }

    #[test]
    fn only_a_remote_agent_lost_turn_links_its_commit_without_completing() {
        assert_eq!(
            turn_artifact_hooks(true, &CompletionReason::TurnComplete),
            TurnArtifactHooks::All
        );
        assert_eq!(
            turn_artifact_hooks(false, &CompletionReason::RemoteAgentLost),
            TurnArtifactHooks::CommitOnly
        );
        for reason in [
            CompletionReason::Crashed,
            CompletionReason::AppQuit,
            CompletionReason::Interrupted,
            CompletionReason::Unknown,
        ] {
            assert_eq!(
                turn_artifact_hooks(false, &reason),
                TurnArtifactHooks::Skip,
                "{reason:?}"
            );
        }
    }

    /// `sq` going is usually transient, so a turn it detached from is tried
    /// again only after a growing wait — and a stop during one ends it, with
    /// no connection up to have sent the agent its cancel.
    #[tokio::test(start_paused = true)]
    async fn a_detached_reattachment_is_retried_after_growing_delays() {
        let detached = || {
            ReattachEnd::of(Err(
                "Remote agent detached: the `sq blox acp` proxy exited".to_string()
            ))
        };
        let cancel = CancellationToken::new();
        let no_quit = CancellationToken::new();
        let started = tokio::time::Instant::now();

        let mut attempts = Vec::new();
        let end = retry_detached_reattach("s", &cancel, &no_quit, || {
            attempts.push(started.elapsed());
            std::future::ready(detached())
        })
        .await;
        let [first, second] = DETACHED_TURN_REATTACH_DELAYS;
        assert_eq!(
            attempts,
            [std::time::Duration::ZERO, first, first + second],
            "each attempt after the first waits longer"
        );
        assert!(first < second);
        assert!(acp_client::is_remote_agent_detached_error(
            &end.result.unwrap_err()
        ));

        // Anything else ends it at once.
        let mut attempts = 0;
        let end = retry_detached_reattach("s", &cancel, &no_quit, || {
            attempts += 1;
            std::future::ready(if attempts == 1 {
                detached()
            } else {
                ReattachEnd::of(Ok(AgentRunOutcome::Completed))
            })
        })
        .await;
        assert_eq!((attempts, end.result), (2, Ok(AgentRunOutcome::Completed)));

        // A stop during the wait.
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(first / 2).await;
            stop.cancel();
        });
        let mut attempts = 0;
        let end = retry_detached_reattach("s", &cancel, &no_quit, || {
            attempts += 1;
            std::future::ready(detached())
        })
        .await;
        assert!(end.cancel_unacknowledged, "no connection sent its cancel");
        assert_eq!((attempts, end.result), (1, Ok(AgentRunOutcome::Cancelled)));
    }

    /// A quit that finds no connection up to detach — during the wait
    /// between attempts, while one connects, or before the first — ends the
    /// reattach at once, without cancelling: the turn is left in flight on
    /// the agent, and the run leaves the row `disconnected`.
    #[tokio::test(start_paused = true)]
    async fn a_quit_while_reattaching_leaves_the_turn_in_flight() {
        let detached = || Err("Remote agent detached: the `sq blox acp` proxy exited".to_string());
        let [first, _] = DETACHED_TURN_REATTACH_DELAYS;
        let cancel = CancellationToken::new();
        let quit_in = |after: std::time::Duration| {
            let quit = CancellationToken::new();
            let quitting = quit.clone();
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                quitting.cancel();
            });
            quit
        };
        let leaves_disconnected = |end: &ReattachEnd| {
            !end.cancel_unacknowledged
                && disconnect::run_leaves_session_disconnected(
                    true,
                    false,
                    false,
                    end.result.as_ref().err().map(String::as_str),
                    None,
                    Some(REATTACHED_TURN_IN_FLIGHT),
                    false,
                )
        };

        // During the wait between attempts.
        let quit = quit_in(first / 2);
        let mut attempts = 0;
        let end = retry_detached_reattach("s", &cancel, &quit, || {
            attempts += 1;
            std::future::ready(ReattachEnd::of(detached()))
        })
        .await;
        assert_eq!(attempts, 1);
        assert!(acp_client::is_remote_agent_detached_error(
            end.result.as_ref().unwrap_err()
        ));
        assert!(leaves_disconnected(&end), "{end:?}");

        // While an attempt connects, or runs.
        let quit = quit_in(first / 2);
        let end = retry_detached_reattach("s", &cancel, &quit, std::future::pending).await;
        assert_eq!(end.result, Err(QUIT_WHILE_REATTACHING.to_string()));
        assert!(leaves_disconnected(&end));

        // Before the first: none is ever polled.
        let quit = CancellationToken::new();
        quit.cancel();
        let connected = std::cell::Cell::new(0);
        let end = retry_detached_reattach("s", &cancel, &quit, || async {
            connected.set(connected.get() + 1);
            ReattachEnd::of(Ok(AgentRunOutcome::Completed))
        })
        .await;
        assert_eq!(
            (connected.get(), end.result),
            (0, Err(QUIT_WHILE_REATTACHING.to_string()))
        );
        assert!(!cancel.is_cancelled());
    }

    #[test]
    fn a_dead_owners_answer_identifies_a_turn_only_when_it_was_its_sole_one() {
        let store = Store::in_memory().unwrap();
        let project = crate::store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let on_ws = |workspace: &str| {
            let branch =
                crate::store::Branch::new_remote(&project.id, workspace, "main", workspace);
            store.create_branch(&branch).unwrap();
            branch.id
        };
        let (ws_1, ws_2) = (on_ws("ws-1"), on_ws("ws-2"));
        let dir = std::env::temp_dir();
        let running = |branch: &str, owner_pid: u32, provider: &str| {
            let mut session = crate::store::Session::new_running("go", &dir).with_branch(branch);
            session.owner_pid = Some(owner_pid);
            session.provider = Some(provider.to_string());
            session
        };

        let turn = running(&ws_1, 4242, "claude");
        let elsewhere = [
            running(&ws_2, 4242, "claude"),
            running(&ws_1, 5151, "claude"),
            running(&ws_1, 4242, "goose"),
        ];
        let mut sessions = vec![turn.clone()];
        sessions.extend(elsewhere);
        assert!(is_owners_sole_turn(&store, &turn, &sessions));

        sessions.push(running(&ws_1, 4242, "claude"));
        assert!(!is_owners_sole_turn(&store, &turn, &sessions));
    }

    #[test]
    fn a_turn_that_settled_after_holding_survives_a_cancel_at_teardown() {
        use acp_client::SessionSettleReason;

        // Every reason reached through a hold: a Stop landing in the teardown
        // window can't turn a finished, drained turn into a cancellation.
        for settle in [
            SessionSettleReason::HoldStopped,
            SessionSettleReason::Quiescent,
            SessionSettleReason::HeldUntilCap,
        ] {
            assert!(
                completed_turn_survives_late_cancel(Some(settle)),
                "{settle:?} settled after a real hold and must survive a late cancel"
            );
        }
    }

    #[test]
    fn a_turn_that_never_held_keeps_the_legacy_cancel_semantics() {
        use acp_client::SessionSettleReason;

        // No hold means no post-turn window of its own, so a cancel racing
        // teardown still records `cancelled` — as it did before the hold.
        assert!(!completed_turn_survives_late_cancel(Some(
            SessionSettleReason::Immediate
        )));
        // A connection that died without settling reported nothing to trust.
        assert!(!completed_turn_survives_late_cancel(None));
    }

    #[test]
    fn a_stop_at_teardown_keeps_the_turn_but_starts_no_queued_follow_up() {
        use acp_client::SessionSettleReason;

        // A held turn that settled cleanly and then caught a Stop in the
        // teardown window: it still records as a completed, hook-running turn…
        let settle = Some(SessionSettleReason::Quiescent);
        assert!(completed_turn_survives_late_cancel(settle));
        let completed_successfully = terminal_state_completed_successfully(
            "completed",
            &completed_turn_completion_reason(settle),
        );
        assert!(completed_successfully);
        // …but the Stop still means "no more work": the queued message waits
        // for the user to send it rather than opening a turn they didn't ask
        // for on a session the UI was still showing as running.
        assert!(!queued_follow_up_should_start(completed_successfully, true));
    }

    #[test]
    fn an_uncancelled_completed_turn_still_drains_its_queued_follow_up() {
        // The ordinary path is untouched: every completion the hooks run for
        // keeps draining, including a turn whose background wait was truncated.
        for reason in [
            CompletionReason::TurnComplete,
            CompletionReason::HeldUntilCap,
            CompletionReason::HoldStopped,
        ] {
            let completed_successfully =
                terminal_state_completed_successfully("completed", &reason);
            assert!(
                queued_follow_up_should_start(completed_successfully, false),
                "{reason:?} completed the turn, so its queued follow-up should start"
            );
        }
        // And a turn that didn't complete never drained, cancel or no cancel.
        for cancel_requested in [false, true] {
            assert!(!queued_follow_up_should_start(false, cancel_requested));
        }
    }

    #[test]
    fn every_completed_settle_reason_agrees_on_the_reason_it_records() {
        use acp_client::SessionSettleReason;

        // The two helpers are read together in the terminal-state match, so a
        // reason exempted from the late cancel must also name a completed
        // turn — an exemption that recorded `interrupted` would be incoherent.
        let mut exempted = 0;
        for settle in [
            SessionSettleReason::Immediate,
            SessionSettleReason::Quiescent,
            SessionSettleReason::HeldUntilCap,
            SessionSettleReason::HoldStopped,
        ] {
            if completed_turn_survives_late_cancel(Some(settle)) {
                exempted += 1;
                assert!(
                    terminal_state_completed_successfully(
                        "completed",
                        &completed_turn_completion_reason(Some(settle))
                    ),
                    "{settle:?} survives a late cancel, so its reason must run the hooks"
                );
            }
        }
        // Everything above is conditional on the exemption, so an emptied
        // exemption set would satisfy the invariant vacuously. Count the
        // reasons that took the branch — the three reached through a hold —
        // so the test still fails if the exemption it describes goes away.
        assert!(
            exempted >= 3,
            "expected every hold-reached reason to be exempt from the late cancel, found {exempted}"
        );
    }

    #[test]
    fn a_cap_truncated_hold_is_resumable_but_a_clean_turn_is_not() {
        assert!(CompletionReason::HeldUntilCap.is_resumable());
        assert!(CompletionReason::HoldStopped.is_resumable());
        assert!(!CompletionReason::TurnComplete.is_resumable());
    }

    #[test]
    fn pikchr_validation_skips_when_latest_message_is_not_assistant() {
        let messages = vec![session_message(MessageRole::User, "Thanks")];

        assert_eq!(
            validate_latest_message_pikchr(&messages),
            LatestAssistantPikchrValidation::SkippedFinalMessage
        );
    }

    #[test]
    fn pikchr_validation_allows_assistant_message_without_pikchr() {
        let messages = vec![session_message(
            MessageRole::Assistant,
            "Done. No diagrams in this response.",
        )];

        assert_eq!(
            validate_latest_message_pikchr(&messages),
            LatestAssistantPikchrValidation::NoPikchr
        );
    }

    #[test]
    fn pikchr_validation_allows_valid_pikchr_in_latest_assistant_message() {
        let messages = vec![session_message(
            MessageRole::Assistant,
            "```pikchr\nbox \"Start\" fit\n```",
        )];

        assert_eq!(
            validate_latest_message_pikchr(&messages),
            LatestAssistantPikchrValidation::Valid
        );
    }

    #[test]
    fn pikchr_validation_reports_invalid_pikchr_for_corrective_prompt() {
        let messages = vec![session_message(
            MessageRole::Assistant,
            "Intro\n```pikchr\nbox \"unterminated\n```",
        )];

        let outcome = validate_latest_message_pikchr(&messages);
        match outcome {
            LatestAssistantPikchrValidation::Invalid(error) => {
                assert_eq!(error.line_number, 2);
                assert!(!error.message.is_empty());

                let prompt = build_pikchr_correction_prompt(&error, true);
                assert!(prompt.contains("line 2"));
                assert!(prompt.contains(&error.message));
                assert!(prompt.contains("complete corrected final response"));
                assert!(prompt.contains("Do not edit files"));
                assert!(prompt.contains("run git operations"));
                assert!(prompt.contains("create commits"));
                assert!(prompt.contains("```suggested-next-steps"));
                assert!(prompt.contains("`---` separator"));
                assert!(prompt.contains("# <Title>"));
            }
            other => panic!("expected invalid Pikchr, got {other:?}"),
        }
    }

    #[test]
    fn pikchr_validation_skips_failed_cancelled_and_interrupted_turns() {
        let completed = Ok(AgentRunOutcome::Completed);
        let cancelled = Ok(AgentRunOutcome::Cancelled);
        let failed = Err("agent crashed".to_string());

        assert!(should_validate_pikchr_after_turn(&completed, false));
        assert!(!should_validate_pikchr_after_turn(&cancelled, false));
        assert!(!should_validate_pikchr_after_turn(&failed, false));
        assert!(!should_validate_pikchr_after_turn(&completed, true));
        assert!(!should_validate_pikchr_after_turn(&cancelled, true));
        assert!(!should_validate_pikchr_after_turn(&failed, true));
    }

    #[test]
    fn pikchr_validation_ignores_older_assistant_when_final_message_is_non_assistant() {
        let messages = vec![
            session_message(MessageRole::Assistant, "```pikchr\nbox \"unterminated\n```"),
            session_message(MessageRole::User, "Please continue"),
        ];

        assert_eq!(
            validate_latest_message_pikchr(&messages),
            LatestAssistantPikchrValidation::SkippedFinalMessage
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pipeline_command_cancellation_stops_current_step() {
        // Pre-warm the global cache so the elapsed-time assertion measures pure
        // cancellation latency rather than first-time shell-env capture (which
        // can take seconds under parallel-test load).
        let _ = shell_env_cache()
            .get(&std::env::temp_dir())
            .await
            .expect("warm cache");

        let cancel_token = CancellationToken::new();
        let cancel_after_start = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_after_start.cancel();
        });

        let started = std::time::Instant::now();
        let result =
            run_pipeline_command("sleep 5 & wait", &std::env::temp_dir(), &cancel_token).await;

        assert!(matches!(
            result,
            Ok(PipelineCommandResult::Cancelled { .. })
        ));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    // ---------------------------------------------------------------------
    // Integration: `run_pipeline_command_with_cache` snapshot/fallback paths
    //
    // These tests inject a hermetic `ShellEnvCache` via the test seam so
    // pipeline behaviour can be exercised without depending on the
    // developer's `$SHELL` or `.zshrc`.
    // ---------------------------------------------------------------------

    /// Write `content` to a 0755 tempfile suitable for use as `$SHELL`.
    ///
    /// Returns a `TempPath` (not `NamedTempFile`) so the writable fd is closed
    /// before the script gets exec'd: Linux refuses to exec a file whose inode
    /// still has `i_writecount > 0` and returns ETXTBSY. macOS has no such
    /// check, which is why the prior `NamedTempFile` shape passed locally but
    /// failed on Linux CI.
    #[cfg(unix)]
    fn write_fake_shell(content: &str) -> tempfile::TempPath {
        use std::os::unix::fs::PermissionsExt;
        let temp_path = tempfile::Builder::new()
            .prefix("staged-fake-shell-")
            .suffix(".sh")
            .tempfile()
            .expect("create fake shell tempfile")
            .into_temp_path();
        std::fs::write(&temp_path, content).expect("write script");
        let mut perms = std::fs::metadata(&temp_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temp_path, perms).expect("chmod 755");
        temp_path
    }

    /// G20: When the cache produces a snapshot, its env vars reach the child
    /// process spawned for the pipeline step.
    #[cfg(unix)]
    #[tokio::test]
    async fn snapshot_path_cached_env_reaches_child() {
        let shell = write_fake_shell(
            "#!/bin/sh\nPATH=/usr/bin:/bin\nPIPELINE_TEST_TOKEN=snapshot-marker-abc\nexport PATH PIPELINE_TEST_TOKEN\nexec /bin/sh -s\n",
        );
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
        let dir = tempfile::tempdir().expect("tempdir");

        let cancel = CancellationToken::new();
        let result = run_pipeline_command_with_cache(
            &cache,
            "echo $PIPELINE_TEST_TOKEN",
            &dir.path().to_path_buf(),
            &cancel,
        )
        .await
        .expect("run_pipeline_command_with_cache should succeed");

        let output = match result {
            PipelineCommandResult::Completed(o) => o,
            PipelineCommandResult::Cancelled { .. } => panic!("unexpected cancellation"),
        };
        assert!(output.status.success(), "command should succeed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("snapshot-marker-abc"),
            "child must see PIPELINE_TEST_TOKEN from the snapshot; stdout={stdout:?}"
        );
    }

    /// G21: When the cache returns `Err`, `run_pipeline_command_with_cache`
    /// falls back to `sh -lc` and the command still runs.
    #[cfg(unix)]
    #[tokio::test]
    async fn fallback_path_when_cache_returns_err() {
        let shell = write_fake_shell("#!/bin/sh\nexit 1\n");
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
        let dir = std::env::temp_dir();

        let cancel = CancellationToken::new();
        let result = run_pipeline_command_with_cache(&cache, "echo fallback-ok", &dir, &cancel)
            .await
            .expect("fallback path should still spawn and run");

        let output = match result {
            PipelineCommandResult::Completed(o) => o,
            PipelineCommandResult::Cancelled { .. } => panic!("unexpected cancellation"),
        };
        assert!(output.status.success(), "fallback sh -lc should succeed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("fallback-ok"),
            "fallback should still produce the echo output; stdout={stdout:?}"
        );
    }

    /// G22: Cancellation still terminates the child even after a snapshot is
    /// applied — guards against a future refactor that loses `kill_on_drop`
    /// or the cancellation `select!` arm.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_under_snapshot_branch() {
        let shell =
            write_fake_shell("#!/bin/sh\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n");
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
        let dir = std::env::temp_dir();

        let cancel_token = CancellationToken::new();
        let cancel_after_start = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_after_start.cancel();
        });

        let started = std::time::Instant::now();
        let result =
            run_pipeline_command_with_cache(&cache, "sleep 5 & wait", &dir, &cancel_token).await;
        assert!(matches!(
            result,
            Ok(PipelineCommandResult::Cancelled { .. })
        ));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    /// G23: `current_dir` survives `apply_to` — `pwd` reports the directory
    /// passed to `run_pipeline_command_with_cache`, not the test's cwd.
    #[cfg(unix)]
    #[tokio::test]
    async fn current_dir_survives_apply_to() {
        let shell =
            write_fake_shell("#!/bin/sh\nPATH=/usr/bin:/bin\nexport PATH\nexec /bin/sh -s\n");
        let cache =
            ShellEnvCache::with_shell_and_ttl(shell.to_path_buf(), Duration::from_secs(3600));
        let dir = tempfile::tempdir().expect("tempdir");
        let resolved = std::fs::canonicalize(dir.path()).unwrap_or_else(|_| dir.path().to_owned());

        let cancel = CancellationToken::new();
        let result = run_pipeline_command_with_cache(&cache, "pwd", &resolved, &cancel)
            .await
            .expect("pwd should succeed");
        let output = match result {
            PipelineCommandResult::Completed(o) => o,
            PipelineCommandResult::Cancelled { .. } => panic!("unexpected cancellation"),
        };
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let reported = stdout.trim();
        let reported_path = std::fs::canonicalize(PathBuf::from(reported))
            .unwrap_or_else(|_| PathBuf::from(reported));
        assert_eq!(
            reported_path, resolved,
            "child should run in the requested working_dir; pwd reported {reported:?}"
        );
    }

    #[test]
    fn pipeline_command_output_collapses_progress_for_prompt() {
        let output = combine_normalized_command_output(b"10%\r20%\rdone\n", b"");
        assert_eq!(output, "done");

        let prompt_output =
            format_step_output_for_prompt("Build", "just build --verbose", &output, false);
        assert!(prompt_output.contains("### Build\nCommand: just build --verbose\n```\ndone\n```"));
        let command_position = prompt_output.find("Command: just build --verbose").unwrap();
        let output_position = prompt_output.find("done").unwrap();
        assert!(command_position < output_position);
        assert!(prompt_output.contains("```\ndone\n```"));
        assert!(!prompt_output.contains("10%"));
        assert!(!prompt_output.contains("20%"));
    }

    #[test]
    fn driver_error_completion_reason_detects_rate_limits() {
        assert_eq!(
            completion_reason_for_driver_error(
                r#"Prompt failed: Error { code: InternalError, message: "rate limit exceeded", data: Some(Object {"errorKind": String("rate_limit")}) }"#,
                None,
            ),
            CompletionReason::RateLimited
        );
        assert_eq!(
            completion_reason_for_driver_error(
                "Prompt failed: RequestError(429, \"Rate limit exceeded. Try again later.\")",
                None,
            ),
            CompletionReason::RateLimited
        );
        assert_eq!(
            completion_reason_for_driver_error(
                "Prompt failed: UsageLimitReached { resets_at: Some(2026-06-24T15:45:00Z) }",
                None,
            ),
            CompletionReason::RateLimited
        );
        assert_eq!(
            completion_reason_for_driver_error("RESOURCE_EXHAUSTED: quota exceeded", None),
            CompletionReason::RateLimited
        );
        assert_eq!(
            completion_reason_for_driver_error("upstream overloaded; retry later", None),
            CompletionReason::RateLimited
        );
        assert_eq!(
            completion_reason_for_driver_error(
                "Prompt failed",
                Some(&serde_json::json!({
                    "acp": {
                        "promptError": {
                            "code": -32603,
                            "message": "provider failed",
                            "data": {
                                "errorKind": "rate_limit"
                            }
                        }
                    }
                })),
            ),
            CompletionReason::RateLimited
        );
        assert_eq!(
            completion_reason_for_driver_error(
                "Prompt failed",
                Some(&serde_json::json!({
                    "acp": {
                        "usageUpdate": {
                            "used": 123,
                            "size": 200000,
                            "_meta": {
                                "_claude/rateLimit": {
                                    "status": "rejected",
                                    "rateLimitType": "subscription",
                                    "resetsAt": "2026-06-24T15:45:00Z"
                                }
                            }
                        }
                    }
                })),
            ),
            CompletionReason::RateLimited
        );
    }

    #[test]
    fn rate_limit_recovery_details_extracts_reset_metadata() {
        let details = rate_limit_recovery_details(
            "Prompt failed",
            Some(&serde_json::json!({
                "acp": {
                    "usageUpdate": {
                        "_meta": {
                            "_claude/rateLimit": {
                                "status": "rejected",
                                "rateLimitType": "subscription",
                                "resetsAt": "2026-06-24T15:45:00Z"
                            }
                        }
                    }
                }
            })),
        );

        assert_eq!(details.resets_at.as_deref(), Some("2026-06-24T15:45:00Z"));
        assert_eq!(details.kind.as_deref(), Some("subscription"));
    }

    #[test]
    fn rate_limit_recovery_details_normalizes_numeric_reset_timestamps() {
        // claude-agent-acp forwards the SDK's rate_limit_info verbatim, where
        // resetsAt is a numeric epoch, not an RFC3339 string.
        let details = rate_limit_recovery_details(
            "Prompt failed",
            Some(&serde_json::json!({
                "acp": {
                    "usageUpdate": {
                        "_meta": {
                            "_claude/rateLimit": {
                                "status": "rejected",
                                "rateLimitType": "five_hour",
                                "resetsAt": 1_782_315_900
                            }
                        }
                    }
                }
            })),
        );
        assert_eq!(
            details.resets_at.as_deref(),
            Some("2026-06-24T15:45:00.000Z")
        );
        assert_eq!(details.kind.as_deref(), Some("five_hour"));

        // Epoch millis and numeric strings normalize to the same instant.
        assert_eq!(
            timestamp_field(
                &serde_json::json!({ "resetsAt": 1_782_315_900_000_i64 }),
                &["resetsAt"],
            )
            .as_deref(),
            Some("2026-06-24T15:45:00.000Z")
        );
        assert_eq!(
            timestamp_field(
                &serde_json::json!({ "retryAt": "1782315900" }),
                &["retryAt"]
            )
            .as_deref(),
            Some("2026-06-24T15:45:00.000Z")
        );

        // RFC3339 strings pass through untouched; garbage stays absent.
        assert_eq!(
            timestamp_field(
                &serde_json::json!({ "resetsAt": "2026-06-24T15:45:00Z" }),
                &["resetsAt"],
            )
            .as_deref(),
            Some("2026-06-24T15:45:00Z")
        );
        assert_eq!(
            timestamp_field(&serde_json::json!({ "resetsAt": -5 }), &["resetsAt"]),
            None
        );
    }

    #[test]
    fn bare_number_and_quotation_markers_do_not_classify_as_rate_limits() {
        // "429" embedded in a longer number (exit codes, byte counts in
        // stderr) is not a rate-limit signal; a whole 429 token is.
        assert_eq!(
            completion_reason_for_driver_error("agent process exited with code 14290", None),
            CompletionReason::Crashed
        );
        assert_eq!(
            completion_reason_for_driver_error("wrote 4429 bytes to stderr", None),
            CompletionReason::Crashed
        );
        assert_eq!(
            completion_reason_for_driver_error("HTTP 429 returned by provider", None),
            CompletionReason::RateLimited
        );

        // "quota" as part of another word is not a quota error.
        assert_eq!(
            completion_reason_for_driver_error("unbalanced quotation mark in config", None),
            CompletionReason::Crashed
        );
        assert_eq!(
            completion_reason_for_driver_error("You exceeded your current quota", None),
            CompletionReason::RateLimited
        );
    }

    #[test]
    fn driver_error_completion_reason_leaves_actionable_failures_as_crashes() {
        assert_eq!(
            completion_reason_for_driver_error("Please run /login before continuing", None),
            CompletionReason::Crashed
        );
        assert_eq!(
            completion_reason_for_driver_error(
                r#"Prompt failed: Error { data: Some(Object {"errorKind": String("billing_error")}) }"#,
                None,
            ),
            CompletionReason::Crashed
        );
        assert_eq!(
            completion_reason_for_driver_error("Credits exhausted for this account", None),
            CompletionReason::Crashed
        );
        assert_eq!(
            completion_reason_for_driver_error("agent process exited unexpectedly", None),
            CompletionReason::Crashed
        );
        assert_eq!(
            completion_reason_for_driver_error(
                "Prompt failed: rate limit",
                Some(&serde_json::json!({
                    "acp": {
                        "promptError": {
                            "code": -32603,
                            "message": "billing failed",
                            "data": {
                                "errorKind": "billing_error"
                            }
                        }
                    }
                })),
            ),
            CompletionReason::Crashed
        );
    }

    /// A session at its AI handoff: the row is `running` and the handoff step
    /// is still pending, which is the state `start_session` is called in.
    fn store_with_pending_ai_handoff() -> (Store, PipelineConfig) {
        let store = Store::in_memory().unwrap();
        let steps = vec![PipelineStep::AiHandoff {
            label: "Write PR title and body".to_string(),
            prompt_template: "{step_outputs}".to_string(),
        }];
        let pipeline = PipelineExecution::from_steps(&steps);
        let mut session =
            crate::store::Session::new_running("handoff", std::path::Path::new("/tmp"));
        session.pipeline = Some(pipeline.clone());
        store.create_session(&session).unwrap();

        let config = PipelineConfig {
            session_id: session.id,
            prompt: "handoff".to_string(),
            steps,
            pipeline,
            working_dir: PathBuf::from("/tmp"),
            pre_head_sha: None,
            provider: None,
            workspace_name: None,
            remote_working_dir: None,
            branch_id: None,
            project_id: None,
        };
        (store, config)
    }

    #[test]
    fn failed_pipeline_handoff_start_cleans_running_state() {
        let (store, config) = store_with_pending_ai_handoff();
        let app = mock_app();

        let registry = SessionRegistry::new();
        let (_, registration) = registry.register(&config.session_id);
        assert!(registry.is_running(&config.session_id));

        assert!(finish_failed_pipeline_handoff_start(
            &config,
            &store,
            &registry,
            registration,
            app.handle(),
            "provider unavailable",
            Some(0),
        ));

        assert!(!registry.is_running(&config.session_id));
        let failed = store.get_session(&config.session_id).unwrap().unwrap();
        assert_eq!(failed.status, SessionStatus::Error);
        assert_eq!(
            failed.error_message.as_deref(),
            Some("provider unavailable")
        );
        assert_eq!(failed.completion_reason, Some(CompletionReason::Crashed));
        let step = &failed.pipeline.unwrap().steps[0];
        assert_eq!(step.status, StepStatus::Failed);
        assert_eq!(
            step.error.as_deref(),
            Some("Failed to start AI session: provider unavailable")
        );
    }

    /// The handoff failure that *is* a Stop: `start_session` returns the
    /// startup error only after `finish_cancelled_before_run` has written
    /// `cancelled` and emitted it, so this path loses the transition — and
    /// everything it would otherwise have said loses with it. Ungated, the row
    /// reads cancelled while the last event the client saw says errored, and
    /// the AiHandoff step is blamed for the user's Stop.
    #[test]
    fn a_failed_handoff_leaves_a_cancelled_row_and_its_step_alone() {
        let (store, config) = store_with_pending_ai_handoff();
        let app = mock_app();

        assert!(
            finish_cancelled_before_run(
                &config.session_id,
                None,
                None,
                &store,
                app.handle(),
                CompletionReason::Interrupted,
            ),
            "the Stop's write is the one that takes the row"
        );

        assert!(!finish_failed_pipeline_handoff_start(
            &config,
            &store,
            &SessionRegistry::new(),
            RegistrationEpoch(0),
            app.handle(),
            "No ACP agent found.",
            Some(0),
        ));

        let row = store.get_session(&config.session_id).unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Cancelled);
        assert_eq!(row.completion_reason, Some(CompletionReason::Interrupted));
        let step = &row.pipeline.unwrap().steps[0];
        assert_eq!(
            step.status,
            StepStatus::Pending,
            "a Stop is not the handoff step failing"
        );
        assert_eq!(step.error, None);
    }

    /// The shape a failed handoff now has: `start_session` records the failed
    /// start on the row itself, while the entry is still registered, so this
    /// path loses the CAS and emits nothing — but it is the only one that
    /// knows which step the handoff came from, so the step is still stamped
    /// off the row's `error`.
    #[test]
    fn a_failed_handoff_stamps_the_step_off_a_row_start_session_already_ended() {
        let (store, config) = store_with_pending_ai_handoff();
        let app = mock_app();

        assert!(finish_failed_before_run(
            &config.session_id,
            None,
            None,
            &store,
            app.handle(),
            "No ACP agent found.",
        ));

        assert!(
            !finish_failed_pipeline_handoff_start(
                &config,
                &store,
                &SessionRegistry::new(),
                RegistrationEpoch(0),
                app.handle(),
                "No ACP agent found.",
                Some(0),
            ),
            "the row was already ended, so this path owns no terminal state"
        );

        let row = store.get_session(&config.session_id).unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Error);
        assert_eq!(row.error_message.as_deref(), Some("No ACP agent found."));
        let step = &row.pipeline.unwrap().steps[0];
        assert_eq!(step.status, StepStatus::Failed);
        assert_eq!(
            step.error.as_deref(),
            Some("Failed to start AI session: No ACP agent found.")
        );
    }

    /// The immediate branch start has answered its client by the time the
    /// context build or `start_session` can fail, so the row is the only place
    /// the failure can go: it reads `error`/`Crashed` with the message, and
    /// the write reports that it owns the terminal state (and so the drain).
    #[test]
    fn a_failed_start_after_the_response_records_the_error_on_the_row() {
        let store = Store::in_memory().unwrap();
        let session = crate::store::Session::new_running("prompt", &PathBuf::from("/tmp"));
        store.create_session(&session).unwrap();
        let app = mock_app();

        assert!(finish_failed_before_run(
            &session.id,
            Some("branch-1".to_string()),
            None,
            &store,
            app.handle(),
            "No worktree for branch: branch-1",
        ));

        let row = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Error);
        assert_eq!(
            row.error_message.as_deref(),
            Some("No worktree for branch: branch-1")
        );
        assert_eq!(row.completion_reason, Some(CompletionReason::Crashed));
    }

    /// A Stop that landed first — through `cancel_session_impl`'s fallback
    /// while nothing was registered, or through `finish_cancelled_before_run`
    /// on the way out of `start_session` — already owns the row. The failure
    /// must not talk over it, and must report that it recorded nothing.
    #[test]
    fn a_failed_start_after_the_response_leaves_a_stopped_row_alone() {
        let store = Store::in_memory().unwrap();
        let session = crate::store::Session::new_running("prompt", &PathBuf::from("/tmp"));
        store.create_session(&session).unwrap();
        let app = mock_app();

        assert!(finish_cancelled_before_run(
            &session.id,
            None,
            None,
            &store,
            app.handle(),
            CompletionReason::Interrupted,
        ));
        assert!(!finish_failed_before_run(
            &session.id,
            None,
            None,
            &store,
            app.handle(),
            "No ACP agent found.",
        ));

        let row = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Cancelled);
        assert_eq!(row.completion_reason, Some(CompletionReason::Interrupted));
        assert_eq!(row.error_message, None);
    }

    /// The drain the startup-failure path kicks hangs off this answer: a
    /// terminal state is drained by whoever recorded it, and a write that lost
    /// the row recorded nothing to drain on.
    #[test]
    fn a_cancel_before_the_run_reports_whether_it_recorded_the_terminal_state() {
        let store = Store::in_memory().unwrap();
        let session = crate::store::Session::new_running("prompt", &PathBuf::from("/tmp"));
        store.create_session(&session).unwrap();
        let app = mock_app();

        assert!(finish_cancelled_before_run(
            &session.id,
            Some("branch-1".to_string()),
            None,
            &store,
            app.handle(),
            CompletionReason::Interrupted,
        ));
        assert!(
            !finish_cancelled_before_run(
                &session.id,
                Some("branch-1".to_string()),
                None,
                &store,
                app.handle(),
                CompletionReason::AppQuit,
            ),
            "a row that is no longer running was recorded by someone else"
        );

        let row = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(row.completion_reason, Some(CompletionReason::Interrupted));
    }

    #[test]
    fn wait_for_sessions_returns_once_every_session_deregisters() {
        let registry = Arc::new(SessionRegistry::new());
        let (_, first) = registry.register("session-1");
        let (_, second) = registry.register("session-2");
        let session_ids = registry.running_session_ids();
        assert_eq!(session_ids.len(), 2);

        let deregistering = Arc::clone(&registry);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            deregistering.deregister("session-1", first);
            deregistering.deregister("session-2", second);
        });

        assert!(registry.wait_for_sessions(&session_ids, Duration::from_secs(2)));
        assert!(registry.running_session_ids().is_empty());
    }

    #[test]
    fn wait_for_sessions_times_out_while_a_session_is_still_running() {
        let registry = SessionRegistry::new();
        registry.register("session-1");

        assert!(!registry.wait_for_sessions(&["session-1".to_string()], Duration::from_millis(50)));
        // Unknown ids count as stopped, so a stale snapshot can't block a quit.
        assert!(registry.wait_for_sessions(&["gone".to_string()], Duration::from_millis(50)));
    }

    #[test]
    fn running_project_session_cancellation_records_completion_reason_override() {
        let registry = SessionRegistry::new();
        registry.register("session-1");
        registry.register("session-2");
        registry.register("session-3");

        assert!(registry.cancel_with_completion_reason(
            "session-1",
            CompletionReason::ProjectSessionInterrupted
        ));
        assert!(registry.cancel("session-2"));
        assert!(registry.cancel("session-3"));
        assert!(registry.cancel("session-1"));
        assert!(registry.cancel_with_completion_reason(
            "session-2",
            CompletionReason::ProjectSessionInterrupted
        ));

        assert_eq!(
            registry.cancellation_completion_reason("session-1"),
            Some(CompletionReason::ProjectSessionInterrupted)
        );
        assert_eq!(
            registry.cancellation_completion_reason("session-2"),
            Some(CompletionReason::ProjectSessionInterrupted)
        );
        assert_eq!(
            registry.cancellation_completion_reason("session-3"),
            Some(CompletionReason::Interrupted)
        );
    }

    #[test]
    fn the_registry_answers_the_hold_a_pane_mounting_mid_hold_missed() {
        let registry = SessionRegistry::new();
        let (_, holding_registration) = registry.register("session-holding");

        // Nothing reported yet: not holding, so a pane mounting before the
        // hold begins renders its usual running indicator.
        assert!(!registry.background_hold("session-holding").holding);

        let holding = acp_client::BackgroundHoldStatus {
            holding: true,
            live_tasks: 1,
            tasks: vec![acp_client::BackgroundHoldTask {
                id: "task-1".to_string(),
                name: Some("Run the tests".to_string()),
                description: None,
                output_file_path: None,
            }],
        };
        registry.record_background_hold("session-holding", holding.clone());

        // The report is a change delta and the pane wasn't listening for it,
        // so the retained answer is the whole wait — count and named tasks.
        assert_eq!(registry.background_hold("session-holding"), holding);
        let snapshot: SessionBackgroundHoldSnapshot =
            registry.background_hold("session-holding").into();
        assert!(snapshot.holding);
        assert_eq!(snapshot.live_tasks, 1);
        assert_eq!(snapshot.tasks.len(), 1);
        assert_eq!(snapshot.tasks[0].id, "task-1");
        assert_eq!(snapshot.tasks[0].name.as_deref(), Some("Run the tests"));

        // The driver reports the cleared default on the way out of every hold,
        // so no separate reset is needed to stop advertising the wait.
        registry.record_background_hold(
            "session-holding",
            acp_client::BackgroundHoldStatus::default(),
        );
        assert!(!registry.background_hold("session-holding").holding);

        // A deregistered (finished) session, and one that never existed, both
        // answer "not holding" rather than a not-found the client must handle.
        registry.record_background_hold("session-holding", holding);
        registry.deregister("session-holding", holding_registration);
        assert!(!registry.background_hold("session-holding").holding);
        assert!(!registry.background_hold("session-never-seen").holding);
    }

    #[test]
    fn cancel_or_defer_before_register_applies_at_registration() {
        let registry = SessionRegistry::new();

        // Cancellation arrives during the startup window, before the session
        // registers its token: nothing is running yet, so the intent is deferred.
        registry.cancel_or_defer(
            "session-startup",
            CompletionReason::ProjectSessionInterrupted,
        );
        assert!(!registry.is_running("session-startup"));

        // When the session finally registers, the deferred cancellation is
        // applied immediately: the token is cancelled and the reason recorded.
        let (token, _) = registry.register("session-startup");
        assert!(token.is_cancelled());
        assert_eq!(
            registry.cancellation_completion_reason("session-startup"),
            Some(CompletionReason::ProjectSessionInterrupted)
        );
    }

    /// A session thread deregisters after its terminal write, so a Resume can
    /// win the row and register a successor under the same id before the
    /// finishing thread gets there. Its deregister must not take the
    /// successor's entry: the new turn would be `running` and unregistered —
    /// exactly what the stranded-session sweep ends.
    #[test]
    fn deregister_leaves_a_successors_entry_in_place() {
        let registry = SessionRegistry::new();

        let (_, finishing) = registry.register("session-resumed");
        let (successor_token, successor) = registry.register("session-resumed");

        registry.deregister("session-resumed", finishing);
        assert!(
            registry.is_running("session-resumed"),
            "the finishing thread's deregister must not remove the successor"
        );
        assert!(
            registry.cancel("session-resumed"),
            "the successor's token is the one a Stop now reaches"
        );
        assert!(successor_token.is_cancelled());

        registry.deregister("session-resumed", successor);
        assert!(!registry.is_running("session-resumed"));
    }

    /// A quit that finds a run reattaching to its remote turn stops the
    /// reattach instead of cancelling the run — after detaching the
    /// connection when one is up, and here with none to kill (between
    /// attempts, or before the first is up): the turn is left running, and
    /// the run, not stopped, leaves the row `disconnected`.
    #[test]
    fn a_quit_stops_a_reattach_with_no_connection_to_detach() {
        let registry = SessionRegistry::new();
        let (token, epoch) = registry.register("session-reattaching");
        let detached = registry.reattaching("session-reattaching");

        assert!(registry.would_detach("session-reattaching"));
        assert!(!detached.is_cancelled(), "asking detaches nothing");
        assert!(registry.detach("session-reattaching", CompletionReason::AppQuit));
        assert!(detached.is_cancelled());
        assert!(!token.is_cancelled(), "a detach is no Stop");
        assert_eq!(
            registry.detach_reason_of("session-reattaching", epoch),
            Some(CompletionReason::AppQuit)
        );
        assert!(!registry.reattaching("unknown").is_cancelled());
    }

    fn reattached_connection_settled(cancel_unacknowledged: bool) -> acp_client::SessionSettled {
        acp_client::SessionSettled {
            outcome: Ok(AgentRunOutcome::Cancelled),
            reason: acp_client::SessionSettleReason::Cancelled,
            transport_loss: None,
            cancel_unacknowledged,
            background_work_left: false,
        }
    }

    fn needs_fresh_cancel(end: &ReattachEnd, cancelled_for_quit: bool) -> bool {
        let settlement = end.settlement();
        disconnect::stopped_turn_needs_fresh_cancel(
            true,
            end.result == Ok(AgentRunOutcome::Cancelled),
            cancelled_for_quit,
            settlement.cancel_unacknowledged,
            settlement.transport_loss,
        )
    }

    /// A Stop the reattached connection sent and the agent answered ends the
    /// reattach in one attempt, and stands: nothing cancels the turn again.
    #[tokio::test(start_paused = true)]
    async fn an_acknowledged_stop_on_a_reattached_turn_is_not_cancelled_again() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut attempts = 0;
        let end = retry_detached_reattach("s", &cancel, &CancellationToken::new(), || {
            attempts += 1;
            std::future::ready(ReattachEnd::settled(
                Ok(reattached_connection_settled(false)),
                &cancel,
            ))
        })
        .await;
        assert_eq!(
            (attempts, &end.result),
            (1, &Ok(AgentRunOutcome::Cancelled))
        );
        assert!(!needs_fresh_cancel(&end, false));
    }

    /// A Stop the agent may not have had — the reattached connection's
    /// cancel went unanswered, or no connection was up to send it — has the
    /// turn cancelled again over a fresh connection.
    #[test]
    fn an_unacknowledged_stop_on_a_reattached_turn_is_cancelled_again() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let unanswered = ReattachEnd::settled(Ok(reattached_connection_settled(true)), &cancel);
        assert!(needs_fresh_cancel(&unanswered, false));

        // Stopped before the attempt's connection came up: its error is the
        // run's, and the turn is cancelled again just the same.
        let unconnected = ReattachEnd::settled(Err("connection refused".to_string()), &cancel);
        assert!(unconnected.cancel_unacknowledged);
        let not_stopped = ReattachEnd::settled(
            Err("connection refused".to_string()),
            &CancellationToken::new(),
        );
        assert!(!not_stopped.cancel_unacknowledged);

        // Stopped while the reattached proxy's teardown asked bloxlet what
        // became of the agent: the connection maps its lost turn to
        // `Cancelled` with the cancel never sent, and says nothing of it but
        // the loss. The turn is cancelled again all the same.
        for phase in [
            acp_client::TransportLossPhase::Turn,
            acp_client::TransportLossPhase::Setup,
            acp_client::TransportLossPhase::Hold,
        ] {
            let lost = acp_client::SessionSettled {
                transport_loss: Some(acp_client::TransportLoss { phase }),
                ..reattached_connection_settled(false)
            };
            let end = ReattachEnd::settled(Ok(lost.clone()), &cancel);
            assert!(end.cancel_unacknowledged, "{phase:?}");
            assert!(needs_fresh_cancel(&end, false), "{phase:?}");
            assert!(!needs_fresh_cancel(&end, true), "a quit's, {phase:?}");

            // The same loss with no Stop is no Stop to repeat.
            let unstopped = ReattachEnd::settled(Ok(lost), &CancellationToken::new());
            assert!(!unstopped.cancel_unacknowledged, "{phase:?}");
        }
    }

    /// A quit's cancel is no Stop: whatever a connection made of it, the
    /// turn is never cancelled again. (A run reattaching is marked so before
    /// it checks for a quit, so a quit detaches it rather than cancelling it
    /// — see `a_quit_stops_a_reattach_with_no_connection_to_detach`.)
    #[test]
    fn a_quit_s_cancel_never_has_a_reattached_turn_cancelled_again() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let end = ReattachEnd::settled(Ok(reattached_connection_settled(true)), &cancel);
        assert!(needs_fresh_cancel(&end, false), "a Stop's would be");
        assert!(!needs_fresh_cancel(&end, true));
    }

    /// Only a connected remote session has a detach handle. Anything else —
    /// unknown, or registered but not connected (a local session never is) —
    /// answers `false` so the quit cancels it instead, and the refusal leaves
    /// neither a Stop nor a detach reason behind.
    #[test]
    fn detach_without_a_remote_connection_falls_back_to_cancel() {
        let registry = SessionRegistry::new();
        assert!(!registry.would_detach("unknown"));
        assert!(!registry.detach("unknown", CompletionReason::AppQuit));

        let (token, epoch) = registry.register("session-local");
        assert!(!registry.would_detach("session-local"));
        assert!(!registry.detach("session-local", CompletionReason::AppQuit));
        assert!(!token.is_cancelled());
        assert_eq!(registry.detach_reason_of("session-local", epoch), None);
    }

    /// A hold is the registry's lighter entry: it makes the row live for the
    /// sweep without making it cancellable, so a Stop still takes
    /// `cancel_session_impl`'s store-write fallback and shutdown's cancel
    /// snapshot does not wait on it. Overlapping holds release independently,
    /// and a hold coexists with a full registration for the same id.
    #[test]
    fn a_hold_counts_as_live_but_is_not_cancellable() {
        let registry = Arc::new(SessionRegistry::new());

        let hold = registry.hold("session-held");
        assert!(registry.is_live("session-held"));
        assert!(!registry.is_running("session-held"));
        assert!(!registry.cancel("session-held"));
        assert!(registry.running_session_ids().is_empty());

        let second = registry.hold("session-held");
        drop(hold);
        assert!(
            registry.is_live("session-held"),
            "the second hold is still up"
        );

        let (_, registration) = registry.register("session-held");
        drop(second);
        assert!(
            registry.is_live("session-held"),
            "the runner's registration carries the row once the hold is gone"
        );
        registry.deregister("session-held", registration);
        assert!(!registry.is_live("session-held"));
    }

    #[test]
    fn register_external_exposes_token_to_registry_cancel_until_dropped() {
        let registry = Arc::new(SessionRegistry::new());

        let registration = registry.register_external("diagram-child");
        let token = registration.token().clone();
        assert!(registry.is_running("diagram-child"));
        assert!(!token.is_cancelled());

        assert!(registry.cancel("diagram-child"));
        assert!(token.is_cancelled());

        drop(registration);
        assert!(!registry.is_running("diagram-child"));
        assert!(
            !registry.cancel("diagram-child"),
            "a dropped registration must leave nothing behind for cancel to find"
        );
    }

    #[test]
    fn cancel_or_defer_cancels_already_registered_session() {
        let registry = SessionRegistry::new();
        let (token, _) = registry.register("session-running");

        registry.cancel_or_defer(
            "session-running",
            CompletionReason::ProjectSessionInterrupted,
        );

        assert!(token.is_cancelled());
        assert_eq!(
            registry.cancellation_completion_reason("session-running"),
            Some(CompletionReason::ProjectSessionInterrupted)
        );
    }

    /// The shutdown path cancelling its registry snapshot while a session's
    /// driver is still constructing: the session is registered for the whole
    /// startup, so the cancel fires the token startup hands to the session
    /// thread, and the reason survives for the terminal write.
    #[test]
    fn register_for_startup_makes_the_session_cancellable_during_startup() {
        let registry = SessionRegistry::new();

        let (token, _, ()) = registry
            .register_for_startup(
                "session-starting",
                || {
                    assert!(registry.is_running("session-starting"));
                    assert!(registry.cancel_with_completion_reason(
                        "session-starting",
                        CompletionReason::AppQuit
                    ));
                    Ok(())
                },
                |_, _| panic!("a successful startup records no failure"),
            )
            .unwrap();

        assert!(token.is_cancelled());
        assert!(
            registry.is_running("session-starting"),
            "a successful startup must stay registered for its session thread to deregister"
        );
        assert_eq!(
            registry.cancellation_completion_reason("session-starting"),
            Some(CompletionReason::AppQuit)
        );
    }

    /// A failed startup never spawns the session thread that normally
    /// deregisters, so the failure path has to deregister itself — a stale
    /// entry would hold shutdown's `wait_for_sessions` open for its full
    /// budget on a session that can never stop.
    #[test]
    fn register_for_startup_deregisters_when_startup_fails() {
        let registry = SessionRegistry::new();

        let recorded = Cell::new(false);
        let result: Result<(CancellationToken, RegistrationEpoch, ()), StartupFailure> = registry
            .register_for_startup(
                "session-failing",
                || Err("no agent found".to_string()),
                |error, accepted| {
                    // The terminal write happens here, and the row it ends is
                    // still covered: the entry leaves only after this returns.
                    assert_eq!(error, "no agent found");
                    assert_eq!(accepted, None);
                    assert!(
                        registry.is_live("session-failing"),
                        "the failure must be recorded while the entry is still registered"
                    );
                    recorded.set(true);
                },
            );

        assert!(recorded.get(), "a failed startup must be recorded");
        let failure = result.unwrap_err();
        assert_eq!(failure.error, "no agent found");
        assert!(!registry.is_running("session-failing"));
        assert!(
            registry.wait_for_sessions(&["session-failing".to_string()], Duration::ZERO),
            "shutdown must not wait on a session whose startup failed"
        );
    }

    /// A Stop landing while the driver resolves, on a resolve that then fails:
    /// `cancel` answered `true` and so wrote no status, and the session thread
    /// that would record the terminal state never spawns. The failure hands the
    /// cancellation back rather than dropping it, so the caller can write
    /// `cancelled` instead of letting the startup's own error be the only
    /// outcome the user's Stop produced.
    #[test]
    fn register_for_startup_reports_a_cancellation_that_landed_during_startup() {
        let registry = SessionRegistry::new();

        let result: Result<(CancellationToken, RegistrationEpoch, ()), StartupFailure> = registry
            .register_for_startup(
                "session-cancelled-mid-startup",
                || {
                    assert!(
                        registry.cancel_with_completion_reason(
                            "session-cancelled-mid-startup",
                            CompletionReason::AppQuit
                        ),
                        "the cancel must take the registry's fast path, which writes no status"
                    );
                    Err("No ACP agent found.".to_string())
                },
                |_, accepted| {
                    assert_eq!(
                        accepted,
                        Some(CompletionReason::AppQuit),
                        "the recorder is handed the Stop, so it writes `cancelled`, not the error"
                    );
                },
            );

        let failure = result.unwrap_err();
        assert_eq!(failure.error, "No ACP agent found.");
        assert!(!registry.is_running("session-cancelled-mid-startup"));
    }

    /// The failed-startup path writes its terminal state and then removes
    /// its entry, and a Resume can win the now-terminal row in between and
    /// register a successor under the same id. That successor must start
    /// clean — the Stop this entry accepted was answered by the row it
    /// ended — and must still be registered once the failure path has left,
    /// or the resumed turn is `running` and unregistered, which the sweep
    /// ends. The mirror of `register_does_not_inherit_a_finished_predecessors_cancellation`
    /// for `register_for_startup`.
    #[test]
    fn a_failed_startup_leaves_a_resumes_entry_clean_and_in_place() {
        let registry = SessionRegistry::new();
        let resumed = RefCell::new(None);

        let result: Result<(CancellationToken, RegistrationEpoch, ()), StartupFailure> = registry
            .register_for_startup(
                "session-resumed-over-failure",
                || {
                    assert!(registry.cancel_with_completion_reason(
                        "session-resumed-over-failure",
                        CompletionReason::Interrupted
                    ));
                    Err("No ACP agent found.".to_string())
                },
                |_, accepted| {
                    assert_eq!(accepted, Some(CompletionReason::Interrupted));
                    // The terminal write lands here; the Resume registers
                    // over the finished entry before the removal.
                    let (token, _) = registry.register("session-resumed-over-failure");
                    *resumed.borrow_mut() = Some(token);
                },
            );

        assert!(result.is_err());
        let token = resumed.borrow_mut().take().expect("the resume registered");
        assert!(
            !token.is_cancelled(),
            "a Stop on the failed start must not cancel the resumed turn"
        );
        assert!(
            registry.is_running("session-resumed-over-failure"),
            "the failure path must not remove the successor's entry"
        );
        assert_eq!(
            registry.cancellation_completion_reason("session-resumed-over-failure"),
            None
        );
    }

    /// The pipeline handoff replaces a live entry rather than deregistering it,
    /// so the replacement has to inherit a cancel the predecessor accepted: the
    /// pipeline thread is past every point that would observe its own token, and
    /// `cancel` already answered `true`, so nothing else will honour the Stop.
    #[test]
    fn register_carries_a_cancelled_predecessors_state_onto_its_replacement() {
        let registry = SessionRegistry::new();

        let (pipeline_token, _) = registry.register("session-handoff");
        assert!(
            registry.cancel_with_completion_reason("session-handoff", CompletionReason::AppQuit)
        );
        assert!(pipeline_token.is_cancelled());

        let (ai_token, _) = registry.register("session-handoff");

        assert!(
            ai_token.is_cancelled(),
            "the handed-off session must start already cancelled"
        );
        assert_eq!(
            registry.cancellation_completion_reason("session-handoff"),
            Some(CompletionReason::AppQuit),
            "and must keep the reason its terminal write has to persist"
        );
    }

    /// A waker that parks the thread waking it until it is released.
    ///
    /// `CancellationToken::cancel` notifies its waiters synchronously, so a
    /// waker enrolled on a session's token stops a cancelling thread *inside*
    /// [`RunningSession::apply_cancellation`], which is the only interposition
    /// point this race has. Nothing in production wakes like this.
    ///
    /// What the two tests below pin is therefore which lock keeps a cancel and
    /// a takeover apart, not that they are kept apart at all: they would also
    /// pass against a lookup-then-apply split, because `apply_cancellation`
    /// holds the reason mutex across `token.cancel()` and every takeover reader
    /// wants that mutex too. Release the reason guard before firing the token
    /// and the split loses both tests while the registry lock keeps them.
    struct ParkingWaker {
        entered: std::sync::Mutex<std::sync::mpsc::Sender<()>>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl std::task::Wake for ParkingWaker {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            let _ = self.entered.lock().unwrap().send(());
            let _ = self.release.lock().unwrap().recv();
        }
    }

    /// A cancel stopped part-way through applying itself, holding whatever the
    /// registry handed it.
    struct ParkedCancel {
        canceller: Option<std::thread::JoinHandle<bool>>,
        release: std::sync::mpsc::Sender<()>,
        /// Kept alive for the whole park: dropping the enrolment early would
        /// take the notify lock the parked thread is inside.
        _enrolled: std::pin::Pin<Box<tokio_util::sync::WaitForCancellationFutureOwned>>,
        _waker: std::task::Waker,
    }

    impl ParkedCancel {
        /// Let the cancel finish, and answer what the registry told it.
        fn finish(mut self) -> bool {
            self.release
                .send(())
                .expect("the parked cancel must still be waiting");
            self.canceller
                .take()
                .expect("a parked cancel is only finished once")
                .join()
                .expect("the cancelling thread must not panic")
        }
    }

    /// Cancel `session_id` on another thread and stop it mid-apply: the
    /// completion reason is recorded and the token fired, but the thread has
    /// not yet returned the `true` that tells `cancel_session_impl` to write no
    /// status of its own.
    fn park_a_cancel_mid_apply(
        registry: &Arc<SessionRegistry>,
        session_id: &str,
        token: &CancellationToken,
        completion_reason: CompletionReason,
    ) -> ParkedCancel {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let waker: std::task::Waker = Arc::new(ParkingWaker {
            entered: std::sync::Mutex::new(entered_tx),
            release: std::sync::Mutex::new(release_rx),
        })
        .into();
        let mut enrolled = Box::pin(token.clone().cancelled_owned());
        assert!(
            std::future::Future::poll(
                enrolled.as_mut(),
                &mut std::task::Context::from_waker(&waker),
            )
            .is_pending(),
            "the token must still be live for the waker to enrol on"
        );

        let canceller = {
            let registry = Arc::clone(registry);
            let session_id = session_id.to_string();
            std::thread::spawn(move || {
                registry.cancel_with_completion_reason(&session_id, completion_reason)
            })
        };
        entered_rx
            .recv()
            .expect("the cancel must reach the token's waiters");

        ParkedCancel {
            canceller: Some(canceller),
            release: release_tx,
            _enrolled: enrolled,
            _waker: waker,
        }
    }

    /// How long to leave a thread that must be blocked a chance to prove it
    /// isn't. Only ever reported as "still running", so a slow machine can make
    /// this test weaker, never wrong.
    const BLOCKED_ENOUGH: Duration = Duration::from_millis(50);

    /// The carry has to see a cancel that is still being applied, not just one
    /// that finished. Reading past it would give the AI session a clean token
    /// while `cancel` answered `true` — so `cancel_session_impl` writes no
    /// status, the id stays in shutdown's cancel snapshot with nothing that can
    /// honour it, `wait_for_sessions` burns the whole budget, and `app.exit(0)`
    /// orphans the agent child.
    #[test]
    fn register_carries_a_cancellation_that_is_still_being_applied() {
        let registry = Arc::new(SessionRegistry::new());
        let (pipeline_token, _) = registry.register("session-handoff");
        let parked = park_a_cancel_mid_apply(
            &registry,
            "session-handoff",
            &pipeline_token,
            CompletionReason::AppQuit,
        );

        let swapping = {
            let registry = Arc::clone(&registry);
            std::thread::spawn(move || registry.register("session-handoff"))
        };
        std::thread::sleep(BLOCKED_ENOUGH);
        assert!(
            !swapping.is_finished(),
            "the swap must wait on the in-flight cancel rather than read past it"
        );

        assert!(
            parked.finish(),
            "the cancel found an entry, so nothing else wrote a status for it"
        );
        let (ai_token, _) = swapping.join().expect("the swapping thread must not panic");
        assert!(
            ai_token.is_cancelled(),
            "the handed-off session must start already cancelled"
        );
        assert_eq!(
            registry.cancellation_completion_reason("session-handoff"),
            Some(CompletionReason::AppQuit),
            "and must keep the reason its terminal write has to persist"
        );
    }

    /// The same for the other way an entry leaves without an observer. A
    /// startup-failure removal that read past an in-flight cancel would report
    /// `None`, `start_session` would write no `cancelled` row, and the Stop
    /// would produce nothing but an errored session.
    #[test]
    fn a_removal_reports_a_cancellation_that_is_still_being_applied() {
        let registry = Arc::new(SessionRegistry::new());
        let (token, _) = registry.register("session-failing-startup");
        let parked = park_a_cancel_mid_apply(
            &registry,
            "session-failing-startup",
            &token,
            CompletionReason::Interrupted,
        );

        let removing = {
            let registry = Arc::clone(&registry);
            std::thread::spawn(move || {
                registry.deregister_reporting_cancellation("session-failing-startup")
            })
        };
        std::thread::sleep(BLOCKED_ENOUGH);
        assert!(
            !removing.is_finished(),
            "the removal must wait on the in-flight cancel rather than read past it"
        );

        assert!(parked.finish());
        assert_eq!(
            removing.join().expect("the removing thread must not panic"),
            Some(CompletionReason::Interrupted),
            "the entry left without an observer, so its cancellation has to come back"
        );
    }

    /// The carry-forward is scoped to a cancelled predecessor: an ordinary
    /// handoff hands over a live session, and starting it pre-cancelled would
    /// kill the AI turn the pipeline just asked for.
    #[test]
    fn register_starts_clean_when_the_entry_it_replaces_was_not_cancelled() {
        let registry = SessionRegistry::new();

        registry.register("session-handoff");
        let (ai_token, _) = registry.register("session-handoff");

        assert!(!ai_token.is_cancelled());
        assert_eq!(
            registry.cancellation_completion_reason("session-handoff"),
            None
        );
    }

    /// A finished turn's entry stays in the map until after its terminal
    /// write, so a Resume that wins the row in that instant registers over
    /// it. A Stop that entry accepted (the late-Stop path) was answered — the
    /// turn is over — and must not fire the successor's token the way a
    /// handoff predecessor's does. The mark is epoch-guarded, so a finishing
    /// thread cannot stamp a successor's entry.
    #[test]
    fn register_does_not_inherit_a_finished_predecessors_cancellation() {
        let registry = SessionRegistry::new();

        // Marked under the wrong epoch: still a live predecessor, so the
        // handoff carry applies.
        let (_, epoch) = registry.register("session-handoff");
        assert!(registry.cancel("session-handoff"));
        registry.mark_finished("session-handoff", RegistrationEpoch(epoch.0 + 1));
        let (carried, _) = registry.register("session-handoff");
        assert!(carried.is_cancelled());

        // Marked under its own epoch: a finished turn. The entry is still
        // there for the sweep and still answers a Stop...
        let (token, epoch) = registry.register("session-resumed");
        registry.mark_finished("session-resumed", epoch);
        assert!(registry.is_live("session-resumed"));
        assert!(registry.cancel("session-resumed"));
        assert!(token.is_cancelled());
        // ...but the Resume that registers over it starts clean.
        let (resumed, _) = registry.register("session-resumed");
        assert!(
            !resumed.is_cancelled(),
            "a Stop on a finished turn must not cancel the resumed one"
        );
        assert_eq!(
            registry.cancellation_completion_reason("session-resumed"),
            None
        );
    }

    /// The turn-control handle follows the connection, not the registration:
    /// absent in the startup window before `connect`, replaced when a Pikchr
    /// correction re-connects on the same session, and gone with the entry.
    /// Reads for an unknown session answer `None` rather than erroring, since
    /// the caller routes on that (a message for a not-yet-connected session
    /// waits, one for a session that is not running takes the resume path).
    #[test]
    fn turn_control_handle_follows_the_connection() {
        let registry = SessionRegistry::new();
        let (_, epoch) = registry.register("session-steered");

        // Registered but not connected yet.
        assert!(registry.turn_control_handle("session-steered").is_none());

        let first = acp_client::TurnControlHandle::detached();
        registry.set_turn_control_handle("session-steered", first.clone());
        let read = registry
            .turn_control_handle("session-steered")
            .expect("handle set once the connection is up");
        assert!(read.same_connection(&first));

        // A re-connect hands out the new connection's handle.
        let second = acp_client::TurnControlHandle::detached();
        registry.set_turn_control_handle("session-steered", second.clone());
        let read = registry
            .turn_control_handle("session-steered")
            .expect("handle replaced on re-connect");
        assert!(read.same_connection(&second));
        assert!(!read.same_connection(&first));

        // Setting for a session that isn't registered is a no-op...
        registry
            .set_turn_control_handle("session-unknown", acp_client::TurnControlHandle::detached());
        assert!(registry.turn_control_handle("session-unknown").is_none());

        // ...and the handle leaves with the entry.
        registry.deregister("session-steered", epoch);
        assert!(registry.turn_control_handle("session-steered").is_none());
    }

    /// Every way into a session but the pipeline handoff arrives at
    /// `start_session` with nothing registered, so an ordinary refusal has only
    /// the quit's own reason to record.
    #[test]
    fn a_refused_start_with_nothing_registered_records_the_quit() {
        assert_eq!(
            refused_start_completion_reason(&SessionRegistry::new(), "session-never-registered"),
            CompletionReason::AppQuit
        );
    }

    /// The handoff is the one path that arrives with a live entry: it skips
    /// `deregister` so `register` can swap the token with no gap. A refusal
    /// never reaches that `register`, so it has to take the entry out itself —
    /// left behind, it would hold shutdown's `wait_for_sessions` open for the
    /// whole budget on a session no thread is driving. It does so only after
    /// the row's terminal write, like every other deregister: a `running` row
    /// with no entry is what the stranded-session sweep ends.
    #[test]
    fn a_refused_start_frees_the_handoff_predecessors_entry() {
        let store = Store::in_memory().unwrap();
        let session = crate::store::Session::new_running("handoff", std::path::Path::new("/tmp"));
        store.create_session(&session).unwrap();
        let app = mock_app();
        let registry = SessionRegistry::new();
        registry.register(&session.id);

        let outcome =
            refuse_start_during_shutdown(&session.id, None, None, &store, app.handle(), &registry);

        assert!(matches!(outcome, SessionStartOutcome::RefusedShuttingDown));
        let row = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Cancelled);
        assert_eq!(row.completion_reason, Some(CompletionReason::AppQuit));
        assert!(!registry.is_running(&session.id));
        assert!(
            registry.wait_for_sessions(std::slice::from_ref(&session.id), Duration::ZERO),
            "shutdown must not wait on a session the refusal took over"
        );
    }

    /// The reason read alone leaves the entry in place: the refusal removes
    /// it itself once the row is written.
    #[test]
    fn reading_a_refused_starts_reason_does_not_take_the_entry() {
        let registry = SessionRegistry::new();
        registry.register("session-handoff");

        assert_eq!(
            refused_start_completion_reason(&registry, "session-handoff"),
            CompletionReason::AppQuit
        );
        assert!(registry.is_running("session-handoff"));
    }

    /// And when a Stop had already landed on that entry, the refusal is what
    /// takes over its job of recording the terminal state: `cancel` answered
    /// `true` and so wrote no status, which makes its reason the one the row
    /// has to carry rather than the quit's.
    #[test]
    fn a_refused_start_persists_a_cancellation_the_predecessor_accepted() {
        let registry = SessionRegistry::new();
        registry.register("session-handoff");
        assert!(registry.cancel("session-handoff"));

        assert_eq!(
            refused_start_completion_reason(&registry, "session-handoff"),
            CompletionReason::Interrupted
        );
    }

    /// A `running` Blox session with an agent session, as the dead-owner
    /// sweep reads it.
    fn running_blox_session(store: &Store) -> crate::store::Session {
        let project = crate::store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = crate::store::Branch::new_remote(&project.id, "remote", "main", "ws-1");
        store.create_branch(&branch).unwrap();
        let mut session =
            crate::store::Session::new_running("prompt", std::path::Path::new("/tmp"))
                .with_branch(&branch.id);
        session.agent_id = Some("acp-1".to_string());
        store.create_session(&session).unwrap();
        store.get_session(&session.id).unwrap().unwrap()
    }

    fn status_events(
        app: &tauri::App<tauri::test::MockRuntime>,
    ) -> Arc<std::sync::Mutex<Vec<String>>> {
        use tauri::Listener;
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        app.handle()
            .listen_any("session-status-changed", move |event| {
                seen.lock().unwrap().push(event.payload().to_string());
            });
        events
    }

    #[test]
    fn a_dead_owners_blox_session_is_left_disconnected_and_announced() {
        let store = Store::in_memory().unwrap();
        let session = running_blox_session(&store);
        let app = mock_app();
        let events = status_events(&app);

        recover_dead_session(&store, app.handle(), &session);

        let row = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Disconnected);
        let events = events.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(events[0].contains("\"disconnected\""), "{events:?}");
    }

    /// Another instance's sweep wrote the same status first: its event only
    /// reached its own windows, so this one still announces it.
    #[test]
    fn a_recovery_another_sweep_made_first_is_still_announced() {
        let store = Store::in_memory().unwrap();
        let session = running_blox_session(&store);
        assert!(store
            .transition_from_running(&session.id, SessionStatus::Disconnected, None, None)
            .unwrap());
        let app = mock_app();
        let events = status_events(&app);

        recover_dead_session(&store, app.handle(), &session);

        let events = events.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(events[0].contains("\"disconnected\""), "{events:?}");
    }

    /// The row went `disconnected` and was then settled (reconcile in
    /// another instance) between the sweep's read and its write: announcing
    /// `disconnected` would send the settled session back in the UI.
    #[test]
    fn a_row_that_moved_on_since_the_sweep_read_it_is_not_announced() {
        let store = Store::in_memory().unwrap();
        let session = running_blox_session(&store);
        assert!(store
            .transition_from_running(&session.id, SessionStatus::Disconnected, None, None)
            .unwrap());
        let disconnected = store.get_session(&session.id).unwrap().unwrap();
        assert!(store
            .transition_from_disconnected_observed_at(
                &session.id,
                disconnected.updated_at,
                SessionStatus::Completed,
                None,
                None,
            )
            .unwrap());
        let app = mock_app();
        let events = status_events(&app);

        recover_dead_session(&store, app.handle(), &session);

        let row = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Completed);
        assert!(events.lock().unwrap().is_empty());
    }

    /// A mock app so the refusal's status event has somewhere to go. Nothing
    /// listens, so the emit is a no-op — the point is to drive the real
    /// refusal rather than a test-only copy of it.
    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("failed to build mock app")
    }

    /// The whole refusal against a real store. Every caller has already moved
    /// its row to `running` by the time it gets here, and the refusal writes
    /// the terminal state itself rather than leaving it to the quit sweep —
    /// the sweep is `shutdown_cleanup`'s last step, so a refusal landing after
    /// it would strand a `running` row under our pid for the next launch to
    /// report as an errored session.
    #[test]
    fn a_refused_start_records_the_row_the_sweep_would_have() {
        let store = Store::in_memory().unwrap();
        let session = crate::store::Session::new_running("prompt", &PathBuf::from("/tmp"));
        store.create_session(&session).unwrap();
        let app = mock_app();

        let outcome = refuse_start_during_shutdown(
            &session.id,
            None,
            None,
            &store,
            app.handle(),
            &SessionRegistry::new(),
        );

        assert_eq!(outcome, SessionStartOutcome::RefusedShuttingDown);
        assert!(
            !outcome.started(),
            "a refusal must not read as a start to the queue drain"
        );
        let row = store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Cancelled);
        assert_eq!(row.completion_reason, Some(CompletionReason::AppQuit));
    }

    fn make_git_repo(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "staged-{test_name}-{}",
            crate::store::now_timestamp()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init"]);
        run_git(&dir, &["config", "user.email", "test@example.com"]);
        run_git(&dir, &["config", "user.name", "Test User"]);
        std::fs::write(dir.join("file.txt"), "one\n").unwrap();
        run_git(&dir, &["add", "file.txt"]);
        run_git(&dir, &["commit", "-m", "initial"]);
        dir
    }

    fn run_git(dir: &std::path::Path, args: &[&str]) -> String {
        let mut command = std::process::Command::new("git");
        command.args(args).current_dir(dir);
        strip_git_env(&mut command);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn rebase_pipeline_config(
        session_id: &str,
        repo: &std::path::Path,
        pre_head: &str,
    ) -> PipelineConfig {
        PipelineConfig {
            session_id: session_id.to_string(),
            prompt: "Rebase branch".to_string(),
            steps: vec![],
            pipeline: PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase),
            working_dir: repo.to_path_buf(),
            pre_head_sha: Some(pre_head.to_string()),
            provider: None,
            workspace_name: None,
            remote_working_dir: None,
            branch_id: None,
            project_id: None,
        }
    }

    #[test]
    fn rebase_pipeline_completion_updates_pending_commit_when_head_changes() {
        let repo = make_git_repo("rebase-updates-pending");
        let pre_head = run_git(&repo, &["rev-parse", "HEAD"]);
        let store = Store::in_memory().unwrap();
        let project = crate::store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = crate::store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        let mut session = crate::store::Session::new_running("Rebase branch", &repo);
        session.pipeline = Some(PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase));
        store.create_session(&session).unwrap();
        let commit = crate::store::Commit::new_pending(&branch.id).with_session(&session.id);
        store.create_commit(&commit).unwrap();

        std::fs::write(repo.join("file.txt"), "two\n").unwrap();
        run_git(&repo, &["add", "file.txt"]);
        run_git(&repo, &["commit", "-m", "second"]);
        let new_head = run_git(&repo, &["rev-parse", "HEAD"]);

        finalize_rebase_pipeline_without_ai(
            &rebase_pipeline_config(&session.id, &repo, &pre_head),
            &store,
        );

        let updated = store.get_commit(&commit.id).unwrap().unwrap();
        assert_eq!(updated.sha.as_deref(), Some(new_head.as_str()));
        let _ = std::fs::remove_dir_all(repo);
    }

    /// A session whose pipeline recorded `step_index` as failed, mirroring what
    /// `run_pipeline` persists before it returns `Aborted`.
    fn store_with_aborted_pipeline(
        kind: PipelineKind,
        steps: &[PipelineStep],
        step_index: usize,
        output: Option<&str>,
    ) -> (Store, PipelineConfig) {
        let store = Store::in_memory().unwrap();
        let mut pipeline = PipelineExecution::from_steps(steps).with_kind(kind.clone());
        pipeline.steps[step_index].status = StepStatus::Failed;
        pipeline.steps[step_index].output = output.map(str::to_string);
        let mut session =
            crate::store::Session::new_running("git pipeline", std::path::Path::new("/tmp"));
        session.pipeline = Some(pipeline.clone());
        store.create_session(&session).unwrap();

        let config = PipelineConfig {
            session_id: session.id,
            prompt: "git pipeline".to_string(),
            steps: steps.to_vec(),
            pipeline,
            working_dir: PathBuf::from("/tmp"),
            pre_head_sha: None,
            provider: None,
            workspace_name: None,
            remote_working_dir: None,
            branch_id: None,
            project_id: None,
        };
        (store, config)
    }

    fn abort_step(label: &str) -> PipelineStep {
        PipelineStep::Command {
            label: label.to_string(),
            command: "true".to_string(),
            on_failure: FailureStrategy::Abort { marker: None },
        }
    }

    #[test]
    fn aborted_pull_reports_the_failing_step_and_its_output() {
        let steps = vec![
            abort_step("Fetch origin/feature"),
            abort_step("Fast-forward"),
        ];
        let (store, config) = store_with_aborted_pipeline(
            PipelineKind::Pull,
            &steps,
            1,
            Some("  fatal: Not possible to fast-forward, aborting.\n"),
        );

        let error = aborted_pipeline_error(&config, &store, 1).unwrap();

        assert_eq!(
            error,
            "Fast-forward failed:\n\nfatal: Not possible to fast-forward, aborting."
        );
    }

    #[test]
    fn aborted_pull_without_step_output_still_names_the_step() {
        let steps = vec![abort_step("Fetch origin/feature")];
        let (store, config) = store_with_aborted_pipeline(PipelineKind::Pull, &steps, 0, None);

        assert_eq!(
            aborted_pipeline_error(&config, &store, 0).unwrap(),
            "Fetch origin/feature failed"
        );
    }

    #[test]
    fn aborted_push_completes_without_an_error_message() {
        // A push rejected as non-fast-forward is an expected outcome: the session
        // completes so the frontend can offer a force push.
        let steps = vec![abort_step("Push to remote")];
        let (store, config) = store_with_aborted_pipeline(
            PipelineKind::Push,
            &steps,
            0,
            Some("! [rejected] feature -> feature (non-fast-forward)"),
        );

        assert_eq!(aborted_pipeline_error(&config, &store, 0), None);
    }

    #[test]
    fn rebase_pipeline_completion_deletes_noop_pending_commit() {
        let repo = make_git_repo("rebase-noop");
        let pre_head = run_git(&repo, &["rev-parse", "HEAD"]);
        let store = Store::in_memory().unwrap();
        let project = crate::store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = crate::store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        let mut session = crate::store::Session::new_running("Rebase branch", &repo);
        session.pipeline = Some(PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase));
        store.create_session(&session).unwrap();
        let commit = crate::store::Commit::new_pending(&branch.id).with_session(&session.id);
        store.create_commit(&commit).unwrap();

        finalize_rebase_pipeline_without_ai(
            &rebase_pipeline_config(&session.id, &repo, &pre_head),
            &store,
        );

        assert!(store.get_commit(&commit.id).unwrap().is_none());
        let _ = std::fs::remove_dir_all(repo);
    }

    /// A branch whose two commits were authored by Staged sessions (with a
    /// review on the head one) and have just been rebased onto a moved base by
    /// a rebase-pipeline session, leaving every row orphaned.
    struct RebasedBranch {
        repo: crate::test_utils::TempGitRepo,
        store: Arc<Store>,
        /// `(session_id, commit_row_id)` for the authoring sessions, oldest first.
        authored: Vec<(String, String)>,
        pending_id: String,
        review_id: String,
        rebase_session_id: String,
        old_head: String,
        new_head: String,
        new_first: String,
    }

    fn rebased_branch() -> RebasedBranch {
        use crate::store::{Commit, Review, ReviewScope, Session};

        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("base.txt", "base\n");
        let base_sha = repo.commit("chore: base");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &base_sha]);

        repo.run_git(&["checkout", "-b", "feature"]);
        repo.write_file("parser.txt", "parser\n");
        let old_first = repo.commit("feat: parser");
        repo.write_file("lexer.txt", "lexer\n");
        let old_head = repo.commit("fix: lexer");

        let store = Arc::new(Store::in_memory().unwrap());
        let project = crate::store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = crate::store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();

        // The two sessions that authored the branch, oldest first.
        let mut authored = Vec::new();
        for (index, (prompt, sha)) in [("Add parser", &old_first), ("Fix lexer", &old_head)]
            .into_iter()
            .enumerate()
        {
            let session = Session::new_running(prompt, repo.path());
            store.create_session(&session).unwrap();
            let mut row = Commit::new_with_sha(&branch.id, sha).with_session(&session.id);
            row.created_at = 1_000 + index as i64;
            row.updated_at = row.created_at;
            store.create_commit(&row).unwrap();
            authored.push((session.id, row.id));
        }
        let review = Review::new(&branch.id, &old_head, ReviewScope::Commit);
        store.create_review(&review).unwrap();

        let mut rebase_session = Session::new_running("Rebase branch", repo.path());
        rebase_session.pipeline =
            Some(PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase));
        store.create_session(&rebase_session).unwrap();
        let mut pending = Commit::new_pending(&branch.id).with_session(&rebase_session.id);
        pending.created_at = 2_000;
        pending.updated_at = pending.created_at;
        store.create_commit(&pending).unwrap();

        // Move the base out from under the branch, then really rebase onto it.
        repo.run_git(&["checkout", "main"]);
        repo.write_file("moved.txt", "moved\n");
        let moved_base = repo.commit("chore: move base");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &moved_base]);
        repo.run_git(&["checkout", "feature"]);
        repo.run_git(&["rebase", "--signoff", "origin/main"]);
        let new_head = repo.run_git(&["rev-parse", "HEAD"]).trim().to_string();
        let new_first = repo.run_git(&["rev-parse", "HEAD~1"]).trim().to_string();
        assert_ne!(new_head, old_head, "the rebase must rewrite the SHAs");

        RebasedBranch {
            repo,
            store,
            authored,
            pending_id: pending.id,
            review_id: review.id,
            rebase_session_id: rebase_session.id,
            old_head,
            new_head,
            new_first,
        }
    }

    fn assert_reassociated(fixture: &RebasedBranch) {
        let store = &fixture.store;

        let first = store.get_commit(&fixture.authored[0].1).unwrap().unwrap();
        assert_eq!(first.sha.as_deref(), Some(fixture.new_first.as_str()));
        let head = store.get_commit(&fixture.authored[1].1).unwrap().unwrap();
        assert_eq!(head.sha.as_deref(), Some(fixture.new_head.as_str()));
        assert_eq!(
            head.session_id.as_deref(),
            Some(fixture.authored[1].0.as_str()),
            "the head commit must keep its authoring session, not the rebase one"
        );

        // The rebase session's pending row loses the race for the new HEAD and
        // is dropped, exactly as it is for a no-op rebase today.
        assert!(store.get_commit(&fixture.pending_id).unwrap().is_none());

        // The review followed its commit, so it stays visible in the timeline.
        let review = store.get_review(&fixture.review_id).unwrap().unwrap();
        assert_eq!(review.commit_sha, fixture.new_head);
    }

    /// A rebase rewrites every SHA on the branch. Finalizing the pipeline must
    /// move the pre-existing commit rows (and their reviews) onto the new SHAs.
    #[test]
    fn rebase_pipeline_completion_reassociates_pre_existing_commits() {
        let fixture = rebased_branch();

        finalize_rebase_pipeline_without_ai(
            &rebase_pipeline_config(
                &fixture.rebase_session_id,
                fixture.repo.path(),
                &fixture.old_head,
            ),
            &fixture.store,
        );

        assert_reassociated(&fixture);
    }

    /// When the pipeline hands off to AI (conflicts, a failed fetch), the agent
    /// finishes the rebase and the post-completion hooks run instead — they
    /// have to reassociate too.
    #[test]
    fn rebase_handoff_post_completion_reassociates_pre_existing_commits() {
        let fixture = rebased_branch();

        run_post_completion_hooks(
            &fixture.rebase_session_id,
            fixture.repo.path(),
            Some(&fixture.old_head),
            None,
            None,
            &fixture.store,
            0,
        );

        assert_reassociated(&fixture);
    }

    /// A branch like [`RebasedBranch`], except the rebase is still in flight:
    /// the first commit rebased cleanly, the second stopped on a conflict,
    /// leaving HEAD detached on the partially applied rewrite.
    struct ConflictedRebase {
        repo: crate::test_utils::TempGitRepo,
        store: Arc<Store>,
        /// `(session_id, commit_row_id)` for the authoring sessions, oldest first.
        authored: Vec<(String, String)>,
        pending_id: String,
        review_id: String,
        rebase_session_id: String,
        old_first: String,
        old_head: String,
    }

    fn conflicted_rebase() -> ConflictedRebase {
        use crate::store::{Commit, Review, ReviewScope, Session};

        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("shared.txt", "base\n");
        let base_sha = repo.commit("chore: base");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &base_sha]);

        // Two commits: the first rebases cleanly, the second conflicts — so the
        // stopped rebase leaves a rewritten commit on a detached HEAD.
        repo.run_git(&["checkout", "-b", "feature"]);
        repo.write_file("parser.txt", "parser\n");
        let old_first = repo.commit("feat: parser");
        repo.write_file("shared.txt", "feature\n");
        let old_head = repo.commit("fix: lexer");

        let store = Arc::new(Store::in_memory().unwrap());
        let project = crate::store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = crate::store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();

        let mut authored = Vec::new();
        for (index, sha) in [&old_first, &old_head].into_iter().enumerate() {
            let session = Session::new_running("Author a commit", repo.path());
            store.create_session(&session).unwrap();
            let mut row = Commit::new_with_sha(&branch.id, sha).with_session(&session.id);
            row.created_at = 1_000 + index as i64;
            row.updated_at = row.created_at;
            store.create_commit(&row).unwrap();
            authored.push((session.id, row.id));
        }
        let review = Review::new(&branch.id, &old_head, ReviewScope::Commit);
        store.create_review(&review).unwrap();

        let mut rebase_session = Session::new_running("Rebase branch", repo.path());
        rebase_session.pipeline =
            Some(PipelineExecution::from_steps(&[]).with_kind(PipelineKind::Rebase));
        store.create_session(&rebase_session).unwrap();
        let pending = Commit::new_pending(&branch.id).with_session(&rebase_session.id);
        store.create_commit(&pending).unwrap();

        // Move the base with a conflicting change, then rebase into the conflict.
        repo.run_git(&["checkout", "main"]);
        repo.write_file("shared.txt", "moved\n");
        let moved_base = repo.commit("chore: move base");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &moved_base]);
        repo.run_git(&["checkout", "feature"]);
        assert!(
            repo.try_run_git(&["rebase", "origin/main"]).is_err(),
            "the rebase must stop on the conflict"
        );
        assert_ne!(
            repo.run_git(&["rev-parse", "HEAD"]).trim(),
            old_head,
            "the stopped rebase must have moved HEAD off the branch"
        );

        ConflictedRebase {
            repo,
            store,
            authored,
            pending_id: pending.id,
            review_id: review.id,
            rebase_session_id: rebase_session.id,
            old_first,
            old_head,
        }
    }

    /// End the handoff turn with the rebase still stopped on the conflict.
    fn end_turn_mid_rebase(fixture: &ConflictedRebase) {
        run_post_completion_hooks(
            &fixture.rebase_session_id,
            fixture.repo.path(),
            Some(&fixture.old_head),
            None,
            None,
            &fixture.store,
            0,
        );
    }

    /// A mid-rebase state must not be claimed as a completed commit: the
    /// rebase session's pending row has to come out of the turn still
    /// `sha IS NULL`.
    fn assert_pending_unclaimed(fixture: &ConflictedRebase) {
        let pending = fixture
            .store
            .get_commit(&fixture.pending_id)
            .unwrap()
            .unwrap();
        assert!(
            pending.sha.is_none(),
            "the pending row must not claim the detached mid-rebase SHA"
        );
    }

    fn assert_untouched(fixture: &ConflictedRebase) {
        let store = &fixture.store;
        assert_eq!(
            store
                .get_commit(&fixture.authored[0].1)
                .unwrap()
                .unwrap()
                .sha
                .as_deref(),
            Some(fixture.old_first.as_str()),
            "the first commit's row must keep the SHA an abort would restore"
        );
        assert_eq!(
            store
                .get_commit(&fixture.authored[1].1)
                .unwrap()
                .unwrap()
                .sha
                .as_deref(),
            Some(fixture.old_head.as_str())
        );
        assert_eq!(
            store
                .get_review(&fixture.review_id)
                .unwrap()
                .unwrap()
                .commit_sha,
            fixture.old_head
        );
    }

    /// The handoff also runs when the agent's turn ends with the rebase still
    /// stopped on a conflict. HEAD is detached on a partially applied rewrite
    /// there, and those SHAs only survive until someone runs `git rebase
    /// --abort` — which restores the originals — so neither the authored rows
    /// nor the rebase session's pending row may take one. The rows have to
    /// stay put until the rebase finishes.
    #[test]
    fn rebase_stopped_on_a_conflict_leaves_the_rows_alone() {
        let fixture = conflicted_rebase();

        end_turn_mid_rebase(&fixture);

        assert_untouched(&fixture);
        assert_pending_unclaimed(&fixture);
    }

    /// The deferred pending row resolves on the next turn: the resumed session
    /// re-captures HEAD (now the detached mid-rebase commit), the conflict is
    /// resolved, and `--continue` finishes the rebase. The authored rows claim
    /// the rewritten SHAs first, so the pending row drops as a duplicate —
    /// the same end state as a rebase that never conflicted.
    #[test]
    fn rebase_resumed_and_finished_resolves_the_deferred_pending_row() {
        let fixture = conflicted_rebase();
        end_turn_mid_rebase(&fixture);
        assert_pending_unclaimed(&fixture);

        let repo = &fixture.repo;
        let detached_head = repo.run_git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.write_file("shared.txt", "resolved\n");
        repo.run_git(&["add", "shared.txt"]);
        repo.run_git(&["-c", "core.editor=true", "rebase", "--continue"]);
        let new_head = repo.run_git(&["rev-parse", "HEAD"]).trim().to_string();
        let new_first = repo.run_git(&["rev-parse", "HEAD~1"]).trim().to_string();

        run_post_completion_hooks(
            &fixture.rebase_session_id,
            repo.path(),
            Some(&detached_head),
            None,
            None,
            &fixture.store,
            0,
        );

        let store = &fixture.store;
        let first = store.get_commit(&fixture.authored[0].1).unwrap().unwrap();
        assert_eq!(first.sha.as_deref(), Some(new_first.as_str()));
        let head = store.get_commit(&fixture.authored[1].1).unwrap().unwrap();
        assert_eq!(head.sha.as_deref(), Some(new_head.as_str()));
        assert_eq!(
            head.session_id.as_deref(),
            Some(fixture.authored[1].0.as_str()),
            "the head commit must keep its authoring session, not the rebase one"
        );
        assert_eq!(
            store
                .get_review(&fixture.review_id)
                .unwrap()
                .unwrap()
                .commit_sha,
            new_head
        );
        assert!(
            store.get_commit(&fixture.pending_id).unwrap().is_none(),
            "the deferred pending row must drop as a duplicate of the reclaimed head"
        );
    }

    /// The other way out of the conflict: `--abort` restores the original
    /// SHAs. The next turn sees HEAD attached again, reassociation finds no
    /// orphans, and the pending row's claim on the old head hits the same
    /// duplicate-resolution branch — dropped cleanly, rows untouched.
    #[test]
    fn rebase_resumed_and_aborted_drops_the_deferred_pending_row() {
        let fixture = conflicted_rebase();
        end_turn_mid_rebase(&fixture);
        assert_pending_unclaimed(&fixture);

        let repo = &fixture.repo;
        let detached_head = repo.run_git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.run_git(&["rebase", "--abort"]);

        run_post_completion_hooks(
            &fixture.rebase_session_id,
            repo.path(),
            Some(&detached_head),
            None,
            None,
            &fixture.store,
            0,
        );

        assert_untouched(&fixture);
        assert!(
            fixture
                .store
                .get_commit(&fixture.pending_id)
                .unwrap()
                .is_none(),
            "the deferred pending row must drop as a duplicate of the restored head"
        );
    }

    // ── find_closing_fence ──────────────────────────────────────────────

    #[test]
    fn closing_fence_simple() {
        let text = "some json\n```\n";
        assert_eq!(find_closing_fence(text), Some(10));
    }

    #[test]
    fn closing_fence_at_eof_without_newline() {
        let text = "some json\n```";
        assert_eq!(find_closing_fence(text), Some(10));
    }

    #[test]
    fn closing_fence_skips_opening_fences() {
        // ```rust is an opening fence (has info-string), should be skipped
        let text = "before\n```rust\ncode\n```\nafter";
        assert_eq!(find_closing_fence(text), Some(20));
    }

    #[test]
    fn closing_fence_skips_mid_line_backticks() {
        // ``` appearing mid-line (not at column 0) should be skipped
        let text = "some text ``` not a fence\n```\n";
        assert_eq!(find_closing_fence(text), Some(26));
    }

    #[test]
    fn closing_fence_none_when_missing() {
        let text = "no closing fence here\n```rust\ncode";
        assert_eq!(find_closing_fence(text), None);
    }

    #[test]
    fn closing_fence_with_trailing_whitespace() {
        let text = "json\n```  \nmore";
        assert_eq!(find_closing_fence(text), Some(5));
    }

    // ── extract_review_title ──────────────────────────────────────────

    #[test]
    fn extract_title_simple() {
        let text = "Here is my review:\n\n```review-title\nSolid refactor with one edge case\n```\n\nAnd comments...";
        assert_eq!(
            extract_review_title(text),
            Some("Solid refactor with one edge case".to_string())
        );
    }

    #[test]
    fn extract_title_trims_whitespace() {
        let text = "```review-title\n  Clean changes, no concerns  \n```\n";
        assert_eq!(
            extract_review_title(text),
            Some("Clean changes, no concerns".to_string())
        );
    }

    #[test]
    fn extract_title_none_when_missing() {
        let text = "Just a normal message with no review title.";
        assert_eq!(extract_review_title(text), None);
    }

    #[test]
    fn extract_title_none_when_empty() {
        let text = "```review-title\n\n```\n";
        assert_eq!(extract_review_title(text), None);
    }

    #[test]
    fn extract_title_with_review_comments() {
        let text = r#"Here is my review:

```review-title
Risky changes to auth flow need closer look
```

```review-comments
[
  {
    "path": "src/auth.rs",
    "span": { "start": 10, "end": 15 },
    "content": "Missing validation."
  }
]
```
"#;
        assert_eq!(
            extract_review_title(text),
            Some("Risky changes to auth flow need closer look".to_string())
        );
    }

    #[test]
    fn extract_title_ignores_llm_preamble() {
        let text = r#"I now have a complete picture of the changes. Let me produce the review.

```review-title
Clean, well-tested feature addition with good backward compatibility
```

```review-comments
[
  {
    "path": "src/foo.rs",
    "span": { "start": 10, "end": 15 },
    "type": "suggestion",
    "content": "Consider adding a test."
  }
]
```
"#;
        assert_eq!(
            extract_review_title(text),
            Some(
                "Clean, well-tested feature addition with good backward compatibility".to_string()
            )
        );
    }

    #[test]
    fn extract_title_marker_mentioned_in_preamble() {
        // The LLM discusses the ```review-title marker in its preamble before
        // actually producing the fenced block.  The extractor must skip the
        // mid-line mention and find the real opening fence.
        let text = r#"Let me check whether `"```review-title"` would match inside `"```review-title-v2"`:
I now have a complete picture. Let me produce the review.

```review-title
Solid changes with minor nit
```

```review-comments
[]
```
"#;
        assert_eq!(
            extract_review_title(text),
            Some("Solid changes with minor nit".to_string())
        );
    }

    // ── end-of-session ACP title fallbacks ──────────────────────────────

    fn store_with_session(prompt: &str, acp_title: Option<&str>) -> (Arc<Store>, String) {
        let store = Arc::new(Store::in_memory().unwrap());
        let mut session = crate::store::Session::new_running(prompt, std::path::Path::new("/tmp"));
        session.acp_title = acp_title.map(str::to_string);
        store.create_session(&session).unwrap();
        (store, session.id)
    }

    #[test]
    fn note_title_prefers_h1_over_acp_title() {
        let (store, session_id) = store_with_session("Write a note", Some("ACP session title"));
        let (title, body) =
            resolve_note_title_and_body("# Heading From Note\n\nBody text", &store, &session_id);
        assert_eq!(title, "Heading From Note");
        assert_eq!(body, "Body text");
    }

    #[test]
    fn note_title_falls_back_to_acp_title_when_no_h1() {
        let (store, session_id) =
            store_with_session("Write a note", Some("Investigate flaky login test"));
        let (title, body) =
            resolve_note_title_and_body("Body without a heading", &store, &session_id);
        assert_eq!(title, "Investigate flaky login test");
        assert_eq!(body, "Body without a heading");
    }

    #[test]
    fn note_title_falls_back_to_prompt_without_h1_or_acp_title() {
        let (store, session_id) = store_with_session("Write a note", None);
        let (title, _) = resolve_note_title_and_body("Body without a heading", &store, &session_id);
        assert_eq!(title, "Write a note");
    }

    #[test]
    fn note_title_skips_blank_acp_title() {
        let (store, session_id) = store_with_session("Write a note", Some("   "));
        let (title, _) = resolve_note_title_and_body("Body without a heading", &store, &session_id);
        assert_eq!(title, "Write a note");
    }

    fn create_review_for_session(store: &Store, session_id: &str) -> String {
        let project = crate::store::Project::new("test-owner/test-repo");
        store.create_project(&project).unwrap();
        let branch = crate::store::Branch::new(&project.id, "feature", "main");
        store.create_branch(&branch).unwrap();
        let review =
            crate::store::Review::new(&branch.id, "abc123", crate::store::ReviewScope::Branch)
                .with_session(session_id);
        store.create_review(&review).unwrap();
        review.id
    }

    #[test]
    fn review_title_falls_back_to_acp_title_when_no_title_block() {
        let (store, session_id) =
            store_with_session("Review this", Some("Solid refactor of the parser"));
        let review_id = create_review_for_session(&store, &session_id);
        store
            .add_session_message(
                &session_id,
                MessageRole::Assistant,
                "Looked at everything; no fenced blocks here.",
            )
            .unwrap();

        run_post_completion_hooks(
            &session_id,
            std::path::Path::new("/tmp"),
            None,
            None,
            None,
            &store,
            0,
        );

        let review = store.get_review(&review_id).unwrap().unwrap();
        assert_eq!(
            review.title.as_deref(),
            Some("Solid refactor of the parser")
        );
        assert!(review.completed_at.is_some());
    }

    #[test]
    fn review_title_block_wins_over_acp_title() {
        let (store, session_id) = store_with_session("Review this", Some("ACP session title"));
        let review_id = create_review_for_session(&store, &session_id);
        store
            .add_session_message(
                &session_id,
                MessageRole::Assistant,
                "```review-title\nExtracted block title\n```\n",
            )
            .unwrap();

        run_post_completion_hooks(
            &session_id,
            std::path::Path::new("/tmp"),
            None,
            None,
            None,
            &store,
            0,
        );

        let review = store.get_review(&review_id).unwrap().unwrap();
        assert_eq!(review.title.as_deref(), Some("Extracted block title"));
    }

    #[test]
    fn review_without_title_block_or_acp_title_stays_untitled_but_completes() {
        let (store, session_id) = store_with_session("Review this", None);
        let review_id = create_review_for_session(&store, &session_id);
        store
            .add_session_message(&session_id, MessageRole::Assistant, "No blocks at all.")
            .unwrap();

        run_post_completion_hooks(
            &session_id,
            std::path::Path::new("/tmp"),
            None,
            None,
            None,
            &store,
            0,
        );

        let review = store.get_review(&review_id).unwrap().unwrap();
        assert_eq!(review.title, None);
        assert!(review.completed_at.is_some());
    }

    // ── extract_review_comments ─────────────────────────────────────────

    #[test]
    fn extract_simple_comments() {
        let text = r#"Here is my review:

```review-comments
[
  {
    "path": "src/main.rs",
    "span": { "start": 10, "end": 15 },
    "content": "This function is missing error handling."
  }
]
```

That's all!"#;

        let comments = extract_review_comments(text);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].path, "src/main.rs");
        assert_eq!(comments[0].span.start, 10);
        assert_eq!(comments[0].span.end, 15);
        assert_eq!(
            comments[0].content,
            "This function is missing error handling."
        );
    }

    #[test]
    fn extract_comments_with_embedded_code_blocks() {
        // This is the exact bug scenario: JSON content contains markdown
        // code blocks with triple backticks. The old parser would match the
        // first ``` inside the JSON string, truncating the JSON.
        let text = r#"Review:

```review-comments
[
  {
    "path": "src/store/models.rs",
    "span": { "start": 82, "end": 91 },
    "content": "Consider implementing `FromStr`:\n```rust\nimpl FromStr for Status {\n    type Err = Error;\n    fn from_str(s: &str) -> Result<Self, Self::Err> {\n        match s {\n            \"ready\" => Ok(Self::Ready),\n            _ => Err(Error::InvalidStatus(s.to_string())),\n        }\n    }\n}\n```\nThis would be more idiomatic."
  },
  {
    "path": "src/store/pool.rs",
    "span": { "start": 10, "end": 25 },
    "content": "Missing validation for worktree path."
  }
]
```

Done."#;

        let comments = extract_review_comments(text);
        assert_eq!(comments.len(), 2, "should parse both comments");
        assert_eq!(comments[0].path, "src/store/models.rs");
        assert!(comments[0].content.contains("FromStr"));
        assert_eq!(comments[1].path, "src/store/pool.rs");
    }

    #[test]
    fn extract_multiple_review_blocks() {
        let text = r#"First batch:

```review-comments
[
  {
    "path": "a.rs",
    "span": { "start": 1, "end": 2 },
    "content": "Issue A"
  }
]
```

Second batch:

```review-comments
[
  {
    "path": "b.rs",
    "span": { "start": 3, "end": 4 },
    "content": "Issue B"
  }
]
```
"#;

        let comments = extract_review_comments(text);
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].path, "a.rs");
        assert_eq!(comments[1].path, "b.rs");
    }

    #[test]
    fn extract_no_review_block() {
        let text = "Just a normal message with no review comments.";
        let comments = extract_review_comments(text);
        assert!(comments.is_empty());
    }

    #[test]
    fn extract_skips_empty_path_or_content() {
        let text = r#"```review-comments
[
  {
    "path": "",
    "span": { "start": 0, "end": 0 },
    "content": "Has content but no path"
  },
  {
    "path": "file.rs",
    "span": { "start": 0, "end": 0 },
    "content": ""
  }
]
```"#;

        let comments = extract_review_comments(text);
        assert!(
            comments.is_empty(),
            "should skip entries with empty path or content"
        );
    }

    #[test]
    fn extract_comment_types() {
        let text = r#"```review-comments
[
  {
    "path": "src/main.rs",
    "span": { "start": 1, "end": 5 },
    "content": "FYI: this is informational.",
    "type": "information"
  },
  {
    "path": "src/main.rs",
    "span": { "start": 10, "end": 12 },
    "content": "Consider renaming this variable.",
    "type": "suggestion"
  },
  {
    "path": "src/lib.rs",
    "span": { "start": 20, "end": 25 },
    "content": "This could panic at runtime.",
    "type": "warning"
  },
  {
    "path": "src/lib.rs",
    "span": { "start": 30, "end": 35 },
    "content": "Off-by-one error here.",
    "type": "issue"
  },
  {
    "path": "src/lib.rs",
    "span": { "start": 40, "end": 45 },
    "content": "No type field at all."
  }
]
```"#;

        let comments = extract_review_comments(text);
        assert_eq!(comments.len(), 5);

        assert_eq!(comments[0].comment_type, Some(CommentType::Information));
        assert_eq!(comments[1].comment_type, Some(CommentType::Suggestion));
        assert_eq!(comments[2].comment_type, Some(CommentType::Warning));
        assert_eq!(comments[3].comment_type, Some(CommentType::Issue));
        assert_eq!(
            comments[4].comment_type, None,
            "missing type field should result in None"
        );
    }

    // ── extract_note_content ────────────────────────────────────────────

    #[test]
    fn note_content_after_hr() {
        let text = "Preamble\n---\n# My Note\nBody here.";
        let content = extract_note_content(text);
        assert_eq!(content, Some("# My Note\nBody here.".to_string()));
    }

    #[test]
    fn note_content_none_without_hr() {
        let text = "Just some text without a horizontal rule.";
        assert_eq!(extract_note_content(text), None);
    }

    #[test]
    fn note_content_inline_hr_before_h1() {
        let text =
            "I gathered enough context.---\n# Repo Purpose\nThis repo ships desktop tooling.";
        let content = extract_note_content(text);
        assert_eq!(
            content,
            Some("# Repo Purpose\nThis repo ships desktop tooling.".to_string())
        );
    }

    #[test]
    fn note_content_strips_inline_suggested_steps_before_hr_with_code_fence_body() {
        let text = r#"I focused the plan on the parser and tests.```suggested-next-steps
{"suggestedNextCommitStep":"Fix note parsing","suggestedNextNoteStep":null}
```
---
# Harden Note Detection
Strip metadata before scanning for the note separator.

```rust
fn example() {}
```

Keep normal markdown fences in the note body."#;
        let content = extract_note_content(text);
        assert_eq!(
            content,
            Some(
                "# Harden Note Detection\nStrip metadata before scanning for the note separator.\n\n```rust\nfn example() {}\n```\n\nKeep normal markdown fences in the note body.".to_string()
            )
        );
    }

    #[test]
    fn note_content_inline_hr_without_h1_is_ignored() {
        let text = "Two reasons:--- this session is read-only.";
        assert_eq!(extract_note_content(text), None);
    }

    #[test]
    fn note_content_first_hr_wins_within_message() {
        // Within a single message the first --- is the note separator.
        // The caller is responsible for picking the right message.
        let text = "Here is the format:\n---\n# <Title>\n<Body>\n\nNow here is my actual note:\n---\n# Real Title\nReal body.";
        let content = extract_note_content(text);
        assert_eq!(
            content,
            Some(
                "# <Title>\n<Body>\n\nNow here is my actual note:\n---\n# Real Title\nReal body."
                    .to_string()
            ),
        );
    }

    #[test]
    fn note_content_hr_inside_code_fence_is_skipped() {
        // A --- inside a fenced code block should not be treated as a note separator.
        let text = "Here is an example:\n```\n---\n# <Title>\n<Body>\n```\n---\n# Actual Note\nActual body.";
        let content = extract_note_content(text);
        assert_eq!(content, Some("# Actual Note\nActual body.".to_string()));
    }

    #[test]
    fn note_content_hr_inside_tilde_fence_is_skipped() {
        let text = "Example:\n~~~\n---\n# Fake\n~~~\n---\n# Real\nBody.";
        let content = extract_note_content(text);
        assert_eq!(content, Some("# Real\nBody.".to_string()));
    }

    #[test]
    fn note_content_only_hr_is_inside_code_fence() {
        // If the only --- is inside a code fence, no note should be detected.
        let text = "Some reasoning:\n```\n---\n# Title\nBody\n```\nDone.";
        assert_eq!(extract_note_content(text), None);
    }

    #[test]
    fn note_content_multiple_hrs_picks_first() {
        // Multiple standalone HRs — the first one delimits the note.
        let text = "Section 1\n---\nSection 2\n---\n# Final Note\nFinal body.";
        let content = extract_note_content(text);
        assert_eq!(
            content,
            Some("Section 2\n---\n# Final Note\nFinal body.".to_string()),
        );
    }

    // ── extract_note_title ──────────────────────────────────────────────

    #[test]
    fn note_title_from_h1() {
        let (title, body) = extract_note_title("# My Title\nBody text.");
        assert_eq!(title, "My Title");
        assert_eq!(body, "Body text.");
    }

    #[test]
    fn note_title_empty_when_no_h1() {
        let (title, body) = extract_note_title("No heading here.\nJust text.");
        assert!(title.is_empty());
        assert_eq!(body, "No heading here.\nJust text.");
    }

    // ── strip_action_wrapper ────────────────────────────────────────────

    #[test]
    fn strip_action_no_wrapper() {
        assert_eq!(strip_action_wrapper("plain prompt"), "plain prompt");
    }

    #[test]
    fn strip_action_normal_wrapper() {
        let input = "<action>\nSome injected context\n</action>\nActual user prompt";
        assert_eq!(strip_action_wrapper(input), "Actual user prompt");
    }

    #[test]
    fn strip_action_wrapper_with_leading_whitespace() {
        let input = "  \n<action>injected</action>  user text";
        assert_eq!(strip_action_wrapper(input), "user text");
    }

    #[test]
    fn strip_action_missing_closing_tag() {
        // If the closing tag is absent, return the original prompt unchanged.
        let input = "<action>unclosed block\nuser text";
        assert_eq!(strip_action_wrapper(input), input);
    }

    #[test]
    fn strip_action_whitespace_only_after_stripping() {
        // After stripping the wrapper, only whitespace remains — should trim to empty.
        let input = "<action>stuff</action>   \n  ";
        assert_eq!(strip_action_wrapper(input), "");
    }

    #[test]
    fn strip_action_nested_action_tags() {
        // The function uses the *first* </action> it finds, so a nested
        // <action> inside the wrapper content would split there.
        let input = "<action>outer <action>inner</action> leftover</action>\nreal prompt";
        assert_eq!(
            strip_action_wrapper(input),
            "leftover</action>\nreal prompt"
        );
    }

    // ── extract_suggested_next_steps ────────────────────────────────────

    #[test]
    fn extract_steps_valid_both_fields() {
        let text = r#"Here are the notes.

```suggested-next-steps
{"suggestedNextCommitStep": "Implement the plan", "suggestedNextNoteStep": "Research alternatives"}
```

---
# Title
Body
"#;
        let steps = extract_suggested_next_steps(text).unwrap();
        assert_eq!(
            steps.suggested_next_commit_step.as_deref(),
            Some("Implement the plan")
        );
        assert_eq!(
            steps.suggested_next_note_step.as_deref(),
            Some("Research alternatives")
        );
    }

    #[test]
    fn extract_steps_valid_inline_opening_fence() {
        let text = "Ready to return the note.```suggested-next-steps\n{\"suggestedNextCommitStep\": \"Reduce churn\", \"suggestedNextNoteStep\": \"Plan IPC fix\"}\n```\n";
        let steps = extract_suggested_next_steps(text).unwrap();
        assert_eq!(
            steps.suggested_next_commit_step.as_deref(),
            Some("Reduce churn")
        );
        assert_eq!(
            steps.suggested_next_note_step.as_deref(),
            Some("Plan IPC fix")
        );
    }

    #[test]
    fn extract_steps_null_fields() {
        let text = "```suggested-next-steps\n{\"suggestedNextCommitStep\": null, \"suggestedNextNoteStep\": null}\n```\n";
        let steps = extract_suggested_next_steps(text).unwrap();
        assert_eq!(steps.suggested_next_commit_step, None);
        assert_eq!(steps.suggested_next_note_step, None);
    }

    #[test]
    fn extract_steps_partial_fields() {
        let text = "```suggested-next-steps\n{\"suggestedNextCommitStep\": \"Fix the bug\", \"suggestedNextNoteStep\": null}\n```\n";
        let steps = extract_suggested_next_steps(text).unwrap();
        assert_eq!(
            steps.suggested_next_commit_step.as_deref(),
            Some("Fix the bug")
        );
        assert_eq!(steps.suggested_next_note_step, None);
    }

    #[test]
    fn extract_steps_missing_block() {
        let text = "Just a normal assistant message with no fenced blocks.";
        assert!(extract_suggested_next_steps(text).is_none());
    }

    #[test]
    fn extract_steps_malformed_json() {
        let text = "```suggested-next-steps\n{not valid json}\n```\n";
        assert!(extract_suggested_next_steps(text).is_none());
    }
}
