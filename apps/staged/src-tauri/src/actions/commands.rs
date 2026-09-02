//! Tauri commands for action execution and detection

use anyhow::Result;
use builderbot_actions::{
    ActionDetector, ActionExecutor, ActionMetadata, ActionType, FileExplorationMode,
    RunDetectionMode, StopOptions, SuggestedAction,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};
use tokio::sync::watch;

use crate::store::Store;

use super::ai_provider::AcpAiProvider;
use super::events::{emit_run_phase_changed, RunPhaseChangedEvent, TauriExecutionListener};
use super::registry::{ActionRegistry, RunPhase, RunningActionInfo};
use super::run_detector;

/// Helper to get store from Mutex<Option<Arc<Store>>>
fn get_store(store: &State<'_, Mutex<Option<Arc<Store>>>>) -> Result<Arc<Store>, String> {
    store
        .lock()
        .unwrap()
        .as_ref()
        .ok_or_else(|| "Store not initialized".to_string())
        .cloned()
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DetectingActionsEvent {
    github_repo: String,
    subpath: Option<String>,
    detecting: bool,
}

/// Build an [`AcpAiProvider`] for action detection, honoring the user's
/// preferred agent when `provider_id` is `None`.
///
/// An explicit `provider_id` always wins. When it is `None` — which the
/// automatic first-touch worktree setup and the project-MCP `add_project_repo`
/// path both pass — detection resolves the user's most-recently-used available
/// agent via
/// [`discover_preferred_provider_id`](crate::session_commands::discover_preferred_provider_id),
/// the shared helper behind the badge and action-detection fallbacks, instead
/// of silently picking the first installed agent in `KNOWN_AGENTS` order
/// (Goose).
///
/// Falls back to [`AcpAiProvider::new`] (first installed agent) only when no
/// provider can be resolved at all — i.e. no agents are installed, in which
/// case construction would fail regardless.
pub(crate) async fn build_action_provider(
    provider_id: Option<&str>,
    working_dir: PathBuf,
) -> Result<AcpAiProvider> {
    let provider = match crate::session_commands::discover_preferred_provider_id(provider_id) {
        Some(id) => AcpAiProvider::with_agent(&id, working_dir),
        None => AcpAiProvider::new(working_dir),
    }?;
    let home_snapshot = crate::shell_env::home_env_vars_with_extended_path(
        crate::session_runner::shell_env_cache().as_ref(),
    )
    .await;

    Ok(provider.with_interpreter_env_snapshot(home_snapshot))
}

pub(crate) async fn detect_actions_for_repo_context(
    github_repo: &str,
    subpath: Option<&str>,
    provider_id: Option<&str>,
) -> Result<Vec<SuggestedAction>, String> {
    // Check whether a local clone already exists on disk.
    let local_clone = crate::paths::repos_dir()
        .map(|d| d.join(github_repo))
        .filter(|p| p.exists());

    // If we have a local clone, update its working tree to the latest remote
    // default branch so that action detection sees the current file layout.
    // This is essential when the upstream repo has been restructured (e.g. a
    // subpath moved) — without this the stale working tree would be missing
    // the expected directories. Only the main checkout is affected; worktrees
    // are separate directories and remain untouched.
    if let Some(clone_path) = &local_clone {
        crate::git::update_clone_to_remote_head(clone_path, github_repo);
    }

    // Pick the right AI provider working directory. When we have a local
    // clone we point the provider at it; otherwise we use a temp dir (the
    // provider only needs a cwd for spawning processes, not for file access).
    let provider_dir = match &local_clone {
        Some(clone_path) => match subpath {
            Some(subpath) => clone_path.join(subpath),
            None => clone_path.clone(),
        },
        None => std::env::temp_dir(),
    };

    let provider = build_action_provider(provider_id, provider_dir.clone())
        .await
        .map_err(|e| format!("Failed to create AI provider: {e}"))?;

    let detector = ActionDetector::new(Box::new(provider));

    let mode = match local_clone {
        Some(_) => FileExplorationMode::Local {
            working_dir: provider_dir,
        },
        None => FileExplorationMode::GitHub {
            repo: github_repo.to_string(),
            subpath: subpath.map(str::to_string),
        },
    };

    detector
        .detect_actions_with_mode(mode)
        .await
        .map_err(|e| format!("Action detection failed: {e}"))
}

fn resolve_branch_repo_context(
    store: &Store,
    branch: &crate::store::Branch,
    project: &crate::store::Project,
) -> Result<(String, Option<String>), String> {
    if let Some(project_repo_id) = &branch.project_repo_id {
        let project_repo = store
            .get_project_repo(project_repo_id)
            .map_err(|e| format!("Failed to get project repo: {e}"))?
            .ok_or_else(|| format!("Project repo not found: {project_repo_id}"))?;
        return Ok((project_repo.github_repo, project_repo.subpath));
    }

    let repo = project
        .primary_repo()
        .ok_or_else(|| "Project has no repository attached".to_string())?;
    Ok((repo.to_string(), project.subpath.clone()))
}

/// Persist detected suggestions into an action context, skipping commands the
/// context already has and continuing its sort order.
///
/// A context that had no run action at all before this call gets the first
/// run-type suggestion pinned, so a freshly detected repo arrives with the play
/// button in its card header that detection has always implied. The gate is
/// "had no run actions", not "has nothing pinned": a user who deliberately
/// unpins their run action would otherwise have it pinned right back by the
/// next re-detect. Contexts that predate pinning are covered by the 0028
/// migration instead.
///
/// Persistence belongs inside the detection window: every surface treats the
/// `detecting: false` half of the `repo-actions-detection` broadcast as "this
/// context's action list is final", so a caller that detects here and persists
/// afterwards — as the repo card's Detect Actions button used to — reopens its
/// own in-progress guard while the writes are still landing, and a second run
/// dedupes against a list the first one hasn't finished writing. Its sole
/// caller is [`finish_detection_window`], which runs it with the flag still
/// set.
pub(crate) fn persist_suggested_actions(
    store: &Store,
    context_id: &str,
    suggestions: Vec<SuggestedAction>,
) -> Result<(), String> {
    let existing_actions = store
        .list_repo_actions(context_id)
        .map_err(|e| format!("Failed to list actions: {e}"))?;
    let mut existing_commands: std::collections::HashSet<String> =
        existing_actions.iter().map(|a| a.command.clone()).collect();
    let mut next_sort_order = existing_actions
        .iter()
        .map(|a| a.sort_order)
        .max()
        .unwrap_or(-1)
        + 1;
    let mut pin_next_run_action = !existing_actions
        .iter()
        .any(|a| a.action_type == ActionType::Run);

    for suggestion in suggestions {
        if existing_commands.contains(&suggestion.command) {
            continue;
        }
        existing_commands.insert(suggestion.command.clone());
        let pinned = pin_next_run_action && suggestion.action_type == ActionType::Run;
        pin_next_run_action &= !pinned;
        let action = crate::store::RepoAction::new(
            context_id.to_string(),
            suggestion.name,
            suggestion.command,
            suggestion.action_type,
            next_sort_order,
        )
        .with_auto_commit(suggestion.auto_commit)
        .with_pinned(pinned);
        store
            .create_repo_action(&action)
            .map_err(|e| format!("Failed to create detected action: {e}"))?;
        next_sort_order += 1;
    }

    Ok(())
}

/// Why a detection window handed back no action list.
///
/// The two arms mean opposite things to the caller. `Failed` says detection
/// ran and came back empty-handed, so the context's current list is all there
/// is going to be. `InProgress` says the list the caller wants is *about to
/// exist*: another window owns the context and persists into it before it
/// closes. Collapsing both into one string is what made the prerun paths treat
/// the second as the first — see [`ensure_actions_detected`], which waits it
/// out. The repo card's button renders either as text, via [`Display`].
///
/// [`Display`]: std::fmt::Display
#[derive(Debug)]
enum DetectionError {
    /// A window is already open for this context — here, or in another Staged
    /// instance sharing the database.
    InProgress,
    /// Detection failed, or its results could not be persisted or read back.
    Failed(String),
}

impl std::fmt::Display for DetectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The string the Detect Actions button has always surfaced.
            Self::InProgress => write!(f, "Detection is already in progress for this repository"),
            Self::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// Run one complete detection window for an action context: claim the
/// `detecting_actions` flag, broadcast `detecting: true`, detect, persist the
/// suggestions, and return the context's resulting action list.
///
/// This is the one detection window in the app. The repo card's Detect Actions
/// button ([`detect_repo_actions_impl`]) and both prerun-actions paths (via
/// [`ensure_actions_detected`]) all route through here, so the claim also
/// serializes them against each other: a branch created while a card's
/// detection is in flight is told detection is already in progress instead of
/// launching a second AI call whose dedupe reads a list the first run hasn't
/// finished writing. What a caller does with that rejection is its own
/// business — the button surfaces it, prerun waits it out.
///
/// One of the two ways into [`run_claimed_detection_window`], and the only one
/// that opens the window: the other is [`wait_for_detection_window`] taking
/// over a claim whose owner died, which arrives holding one already.
async fn detect_and_persist_repo_actions(
    app: &AppHandle,
    store: &Store,
    context: &crate::store::ActionContext,
    provider_id: Option<&str>,
) -> Result<Vec<crate::store::RepoAction>, DetectionError> {
    let claimed = store
        .claim_action_context_detection(&context.id, std::process::id())
        .map_err(|e| DetectionError::Failed(format!("Failed to set detection status: {e}")))?;
    if !claimed {
        return Err(DetectionError::InProgress);
    }
    run_claimed_detection_window(app, store, context, provider_id)
        .await
        .map_err(DetectionError::Failed)
}

/// The body of a detection window whose claim this caller already holds:
/// broadcast `detecting: true`, detect, persist, and close the window.
///
/// Detection and persistence both happen while the flag is set, so the flag —
/// and the `detecting: false` event that clears it — only drop once the list
/// callers are about to load is complete. Once the window is open, *every*
/// exit closes it: a detection failure, a persist failure, and a mark failure
/// all clear the flag and emit `detecting: false` before returning. Leaving
/// either half behind wedges the repo: surfaces spin on a run that is over,
/// and a flag still set in SQLite makes [`Store::claim_action_context_detection`]
/// reject every later detection for this context — across restarts, with no UI
/// path to clear it. ([`recover_orphaned_detection_claims`] is the backstop for
/// the one exit no code here runs on, a hard kill, and it only heals at the
/// next startup.)
///
/// Split from [`detect_and_persist_repo_actions`] so a waiter that takes over
/// an orphaned window can run it without re-claiming: its takeover already
/// moved the claim, in the single UPDATE that keeps the flag set throughout.
async fn run_claimed_detection_window(
    app: &AppHandle,
    store: &Store,
    context: &crate::store::ActionContext,
    provider_id: Option<&str>,
) -> Result<Vec<crate::store::RepoAction>, String> {
    let event = |detecting: bool| DetectingActionsEvent {
        github_repo: context.github_repo.clone(),
        subpath: context.subpath.clone(),
        detecting,
    };
    crate::web_server::emit_to_all(app, "repo-actions-detection", event(true));

    let detected = detect_actions_for_repo_context(
        &context.github_repo,
        context.subpath.as_deref(),
        provider_id,
    )
    .await;
    if let Err(ref e) = detected {
        log::warn!("Action detection failed for {}: {e}", context.github_repo);
    }

    let result = finish_detection_window(store, &context.id, detected);
    crate::web_server::emit_to_all(app, "repo-actions-detection", event(false));
    result
}

/// Store-side close-out for a detection window: persist the suggestions and
/// read back the context's action list, then mark the context detected —
/// which is also what drops the `detecting_actions` flag.
///
/// The mark runs on every path, including a failed detection: a context that
/// detection could not read stays marked detected so prerun doesn't retry it
/// for every branch. Should the mark itself fail, fall back to clearing just
/// the flag, since a flag left set is what rejects all later detection.
fn finish_detection_window(
    store: &Store,
    context_id: &str,
    detected: Result<Vec<SuggestedAction>, String>,
) -> Result<Vec<crate::store::RepoAction>, String> {
    let result = detected.and_then(|suggestions| {
        persist_suggested_actions(store, context_id, suggestions)?;
        store
            .list_repo_actions(context_id)
            .map_err(|e| format!("Failed to list actions: {e}"))
    });

    if let Err(e) = store.mark_action_context_detected(context_id) {
        log::error!("Failed to mark action context {context_id} detected after detection: {e}");
        if let Err(e) = store.clear_action_context_detection(context_id) {
            log::error!(
                "Failed to clear the detecting flag for action context {context_id}: {e} — \
                 further detection for this repo will be rejected as already in progress"
            );
        }
    }
    result
}

/// How a caller waits out a detection window someone else owns: how often it
/// re-reads the claim, how often it re-checks the claim's owner, and how long
/// it waits before giving up.
///
/// The AI call inside a window has no timeout of its own, so the wait needs a
/// cap — an owner that never finishes must not hold branch setup open forever.
/// All three knobs are injected so the tests can drive the loop tick by tick
/// instead of by the clock.
#[derive(Clone, Copy)]
struct DetectionWaitPolicy {
    interval: std::time::Duration,
    max_wait: std::time::Duration,
    liveness_probe_interval: std::time::Duration,
}

impl Default for DetectionWaitPolicy {
    fn default() -> Self {
        Self {
            interval: std::time::Duration::from_secs(1),
            // Generous next to a detection that takes tens of seconds: waiting
            // costs this caller no more than winning the claim would have,
            // since that path awaits the same AI call.
            max_wait: std::time::Duration::from_secs(300),
            // Reading the claim is a SQLite query; probing its owner spawns a
            // `kill -0` subprocess, so it runs on its own, much slower clock —
            // at most 20 probes across the cap rather than one per tick. See
            // [`OwnerLiveness`].
            liveness_probe_interval: std::time::Duration::from_secs(15),
        }
    }
}

/// The window owner's liveness, remembered between ticks.
///
/// [`crate::session_runner::is_process_alive`] shells out to `kill -0` and
/// blocks the thread on `Command::status()`. That is fine for the startup
/// sweep, which visits each claim once, but this wait re-runs the same test
/// every tick for up to [`DetectionWaitPolicy::max_wait`] — a subprocess spawn
/// per second, on a tokio worker, for five minutes, to re-answer a question
/// whose answer barely moves. So the verdict is cached for
/// [`DetectionWaitPolicy::liveness_probe_interval`].
///
/// The cache is keyed on the pid, and a different pid is always probed afresh:
/// a window that changed hands mid-wait has a new owner the previous verdict
/// says nothing about, and reusing a dead reading there would take a *live*
/// owner's window over.
struct OwnerLiveness<F> {
    is_alive: F,
    ttl: std::time::Duration,
    /// The pid last probed, what it answered, and when.
    last: Option<(u32, bool, tokio::time::Instant)>,
}

impl<F: Fn(u32) -> bool> OwnerLiveness<F> {
    fn new(is_alive: F, ttl: std::time::Duration) -> Self {
        Self {
            is_alive,
            ttl,
            last: None,
        }
    }

    fn alive(&mut self, pid: u32) -> bool {
        let now = tokio::time::Instant::now();
        if let Some((probed, alive, at)) = self.last {
            if probed == pid && now.duration_since(at) < self.ttl {
                return alive;
            }
        }
        let alive = (self.is_alive)(pid);
        self.last = Some((pid, alive, now));
        alive
    }
}

/// How [`wait_for_detection_window`] stopped waiting.
#[derive(Debug, PartialEq, Eq)]
enum DetectionWait {
    /// The window closed. Its owner persisted inside it, so the context's
    /// action list is final.
    Closed,
    /// The window's owner was gone, so the waiter took the claim over — with
    /// the flag never unset in between — and now holds the window itself. It
    /// owes the context a detection, and the window it inherited a close.
    TookOver,
    /// Gave up with the window still open — the cap expired, or the claim
    /// couldn't be read. The caller proceeds with the list it can see.
    GaveUp,
}

/// Wait for a detection window owned by someone else to close.
///
/// The owner can be another Staged instance — `~/.staged/data.db` is shared,
/// which is the entire reason `detecting_pid` exists — so this polls SQLite
/// rather than waiting on an in-process primitive; a `Notify` keyed by context
/// id would miss a foreign owner, and the `repo-actions-detection` broadcast is
/// frontend-bound. [`Store::list_detecting_action_contexts`] answers both
/// halves of a tick in one query: whether the flag is still set, and who owns
/// it.
///
/// Each tick also runs the startup sweep's orphan test
/// ([`recover_orphaned_detection_claims`]), with one inversion: mid-session a
/// claim carrying *our own pid* is live — another task in this process owns
/// that window — so it is waited on rather than taken over. An owner that is
/// gone leaves nothing to wait for, so its claim is moved to this process and
/// the waiter is told to detect the context itself. That is the "steal a dead
/// owner's window, wedged until the next attempt rather than until the next
/// launch" half the startup sweep deliberately left out. Only the claim itself
/// is re-read every tick; its owner's liveness is a subprocess spawn away and
/// runs on the slower clock [`OwnerLiveness`] keeps.
///
/// Where the sweep releases such a claim, this *takes it over* — one UPDATE
/// that swaps the owner with the flag still set
/// ([`Store::take_over_detection_claim`]), rather than a release followed by a
/// fresh claim. The two-statement version reopens the read-then-write gap the
/// single-statement claim exists to close, and it falls either way onto the
/// silently skipped prerun this wait exists to prevent: another waiter ticking
/// in the gap sees no window and takes this context's undetected action list
/// for final, and a claim landing in the gap sends the taker-over back an
/// in-progress rejection for a window it had just won.
async fn wait_for_detection_window(
    store: &Store,
    context_id: &str,
    policy: DetectionWaitPolicy,
    is_alive: impl Fn(u32) -> bool,
) -> DetectionWait {
    let deadline = tokio::time::Instant::now() + policy.max_wait;
    let mut owner = OwnerLiveness::new(is_alive, policy.liveness_probe_interval);
    loop {
        let claims = match store.list_detecting_action_contexts() {
            Ok(claims) => claims,
            Err(e) => {
                log::warn!(
                    "[actions] Failed to read the detection claim on action context {context_id}: {e}"
                );
                return DetectionWait::GaveUp;
            }
        };
        let Some((_, pid)) = claims.into_iter().find(|(id, _)| id == context_id) else {
            return DetectionWait::Closed;
        };

        let orphaned = match pid {
            None => true,
            Some(pid) if pid == std::process::id() => false,
            Some(pid) => !owner.alive(pid),
        };
        if orphaned {
            // One UPDATE that moves the owner rather than a release and a
            // fresh claim: the flag never drops, so a second waiter ticking in
            // between can't read "no window open" and take this context's
            // undetected action list for final, and nothing can slip a claim
            // into the gap and hand this caller an in-progress rejection for a
            // window it just won.
            //
            // Guarded on the pid just read, so a window that changed hands
            // between the read and here is left to its new owner and waited
            // out on the next tick rather than stolen.
            match store.take_over_detection_claim(context_id, pid, std::process::id()) {
                Ok(true) => {
                    log::info!(
                        "[actions] Took over the detection claim on action context {context_id}: its owner ({pid:?}) is gone"
                    );
                    return DetectionWait::TookOver;
                }
                Ok(false) => {}
                Err(e) => log::warn!(
                    "[actions] Failed to take over the orphaned detection claim on action context {context_id}: {e}"
                ),
            }
        }

        if tokio::time::Instant::now() >= deadline {
            return DetectionWait::GaveUp;
        }
        tokio::time::sleep(policy.interval).await;
    }
}

/// The prerun paths' way in to detection: make sure this context has been
/// detected, then hand back its actions.
///
/// The invariant that justifies the wait below belongs to one of its two
/// callers: [`crate::branches::claim_and_run_prerun_actions`] runs once per
/// branch, behind the one-shot atomic `mark_branch_setup_complete` claim, so
/// the list it gets here is the only one that branch's prerun will ever see —
/// a miss is permanent for that worktree, which never gets its setup actions,
/// and nothing retries. The other caller, [`run_prerun_actions_impl`] (the
/// `run_prerun_actions` command), takes no claim: a miss there costs one
/// invocation, which the caller can simply repeat.
///
/// The wait is why prerun must not sit on a caller's critical path; the three
/// entry points whose callers were on a clock now detach it
/// ([`crate::branches::spawn_prerun_actions`]), so the only thing that ever
/// spends this cap is a background task.
///
/// - Already detected → list and return.
/// - Claim won → detect best-effort. A failure is logged and prerun continues
///   with whatever the context does have, rather than blocking a branch on a
///   missing agent.
/// - Claim lost → **wait**. The rejection means the actions aren't missing,
///   they're about to exist: the winner persists inside its window, so listing
///   now reads the pre-detection (typically empty) list, finds no prerun
///   actions, and silently skips this branch's setup. No re-detection once the
///   window closes — the winner marks the context detected on every exit,
///   including a failed detection. Should the wait find the window's owner
///   gone, it takes the claim over rather than releasing it, and what runs here
///   is the body of that inherited window — never a second claim.
pub(crate) async fn ensure_actions_detected(
    app: &AppHandle,
    store: &Store,
    context: &crate::store::ActionContext,
    provider_id: Option<&str>,
) -> Result<Vec<crate::store::RepoAction>, String> {
    let list = || {
        store
            .list_repo_actions(&context.id)
            .map_err(|e| format!("Failed to list actions: {e}"))
    };
    if context.has_detected_actions {
        return list();
    }

    let waited = match detect_and_persist_repo_actions(app, store, context, provider_id).await {
        Ok(actions) => return Ok(actions),
        Err(DetectionError::Failed(e)) => {
            log::warn!(
                "[actions] Detection failed for {} (subpath: {:?}): {e} — running prerun with the context's current action list",
                context.github_repo,
                context.subpath
            );
            return list();
        }
        Err(DetectionError::InProgress) => {
            wait_for_detection_window(
                store,
                &context.id,
                DetectionWaitPolicy::default(),
                crate::session_runner::is_process_alive,
            )
            .await
        }
    };

    match waited {
        DetectionWait::Closed => list(),
        // The wait took the claim over rather than releasing it, so the window
        // is ours already — run its body directly instead of claiming a second
        // time, which would leave the flag unset in between and could come back
        // rejected for a window we hold.
        DetectionWait::TookOver => {
            match run_claimed_detection_window(app, store, context, provider_id).await {
                Ok(actions) => Ok(actions),
                Err(e) => {
                    log::warn!(
                        "[actions] Detection failed for {} (subpath: {:?}) after taking over an orphaned window: {e}",
                        context.github_repo,
                        context.subpath
                    );
                    list()
                }
            }
        }
        DetectionWait::GaveUp => {
            log::warn!(
                "[actions] Gave up waiting for an in-progress detection of {} (subpath: {:?}) — running prerun with the context's current action list",
                context.github_repo,
                context.subpath
            );
            list()
        }
    }
}

/// On startup, release detection windows whose owner process is no longer
/// alive; returns how many were released.
///
/// Every path through [`detect_and_persist_repo_actions`] closes its own
/// window, so the only way one outlives its process is a hard kill during the
/// AI call — tens of seconds, and it runs during first-touch branch setup.
/// The flag it leaves behind is durable: the claim then rejects the Detect
/// Actions button on every surface and skips prerun detection for every
/// branch on that repo, forever, with nothing in the UI to explain it.
///
/// A blanket `UPDATE action_contexts SET detecting_actions = 0` would do it if
/// the database belonged to this process, but `~/.staged/data.db` is shared —
/// nothing stops two Staged instances from opening it, which is why `sessions`
/// and `queued_session_messages` carry `owner_pid` for exactly this recovery.
/// Clearing a live foreign claim would let a second instance start the
/// concurrent detection the claim exists to prevent, so this checks the owner,
/// mirroring [`crate::session_runner::recover_dead_sessions`]:
///
/// - `None` → release; a row written before `detecting_pid` existed.
/// - our own pid → release; at startup that can only be a dead process whose
///   pid we inherited.
/// - a dead pid → release.
/// - a live pid → leave it; another Staged instance owns that window.
///
/// The own-pid arm is why this is startup-only: mid-session that same claim is
/// live, held by another task in this process. [`wait_for_detection_window`]
/// runs the same test with that one arm inverted.
///
/// `is_alive` is injected so the sweep is testable without spawning processes.
/// It shells out per row, which is fine *here*: this loop runs once and visits
/// only rows with the flag set, normally none. The same test on a loop that
/// runs for minutes needs [`OwnerLiveness`] in front of it.
pub fn recover_orphaned_detection_claims(store: &Store, is_alive: impl Fn(u32) -> bool) -> usize {
    let claims = match store.list_detecting_action_contexts() {
        Ok(claims) => claims,
        Err(e) => {
            log::warn!("[actions] Failed to query in-progress action detections: {e}");
            return 0;
        }
    };

    let mut released = 0;
    for (context_id, pid) in claims {
        let orphaned = match pid {
            None => true,
            Some(pid) if pid == std::process::id() => true,
            Some(pid) => !is_alive(pid),
        };
        if !orphaned {
            continue;
        }
        // Guarded on the pid we just read, so a claim that changed hands in
        // between is left alone rather than clobbered.
        match store.release_detection_claim(&context_id, pid) {
            Ok(true) => released += 1,
            Ok(false) => {}
            Err(e) => log::warn!(
                "[actions] Failed to release the orphaned detection claim on action context {context_id}: {e}"
            ),
        }
    }
    released
}

/// The repo card's Detect Actions button: resolve (or create) the context for
/// a repo+subpath, then run one detection window over it.
///
/// Unlike the prerun paths, this one propagates the window's error to the
/// caller — the button surfaces it — including the already-in-progress
/// rejection. Waiting the way [`ensure_actions_detected`] does would freeze the
/// button for the length of someone else's window, and there is nothing to wait
/// for anyway: the spinner it drives is already running off the `detecting:
/// true` event the winning window broadcast.
pub(crate) async fn detect_repo_actions_impl(
    github_repo: String,
    subpath: Option<String>,
    provider: Option<String>,
    app: AppHandle,
    store: Arc<Store>,
) -> Result<Vec<crate::store::RepoAction>, String> {
    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| format!("Failed to get action context: {e}"))?;
    detect_and_persist_repo_actions(&app, &store, &context, provider.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// Detect available actions for a specific repo+subpath context using AI and
/// persist them; returns the context's actions afterwards.
#[tauri::command(rename_all = "camelCase")]
pub async fn detect_repo_actions(
    github_repo: String,
    subpath: Option<String>,
    provider: Option<String>,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
) -> Result<Vec<crate::store::RepoAction>, String> {
    let store = get_store(&store)?;
    detect_repo_actions_impl(github_repo, subpath, provider, app, store).await
}

/// Wire up run detection for a just-started Run-type action execution.
///
/// `scope_id` is the routing id echoed into run-phase events — a branch id
/// for branch runs, or the synthetic id from [`repo_action_scope_id`] for
/// repo runs; the registry and event stream treat it as an opaque string.
/// `working_dir` is the local directory the autodetect poller inspects
/// (empty for remote executions, where detection gracefully degrades).
#[allow(clippy::too_many_arguments)]
fn wire_run_detection(
    app: AppHandle,
    store: Arc<Store>,
    registry: Arc<ActionRegistry>,
    execution_id: String,
    scope_id: String,
    action: &crate::store::RepoAction,
    working_dir: String,
    provider_id: Option<String>,
) {
    // Ensure the output buffer for this execution_id exists so the
    // regex matcher can obtain a reference to it.
    registry.register_output_buffer(&execution_id);

    match action.run_detection_mode.clone() {
        Some(RunDetectionMode::EndpointRegex { pattern }) => {
            let (cancel_tx, cancel_rx) = watch::channel(false);
            registry.store_cancel_sender(&execution_id, cancel_tx);
            run_detector::spawn_regex_matcher(
                app,
                registry,
                execution_id,
                scope_id,
                action.name.clone(),
                pattern,
                true,
                cancel_rx,
            );
        }
        Some(RunDetectionMode::RunningRegex { pattern }) => {
            let (cancel_tx, cancel_rx) = watch::channel(false);
            registry.store_cancel_sender(&execution_id, cancel_tx);
            run_detector::spawn_regex_matcher(
                app,
                registry,
                execution_id,
                scope_id,
                action.name.clone(),
                pattern,
                false,
                cancel_rx,
            );
        }
        Some(RunDetectionMode::NoDetection) => {
            registry.set_run_phase(&execution_id, RunPhase::NoDetection);
            emit_run_phase_changed(
                &app,
                RunPhaseChangedEvent {
                    execution_id,
                    branch_id: scope_id,
                    action_name: action.name.clone(),
                    phase: RunPhase::NoDetection,
                },
            );
        }
        Some(RunDetectionMode::Autodetect) | None => {
            registry.set_run_phase(&execution_id, RunPhase::AutodetectPending);
            emit_run_phase_changed(
                &app,
                RunPhaseChangedEvent {
                    execution_id: execution_id.clone(),
                    branch_id: scope_id.clone(),
                    action_name: action.name.clone(),
                    phase: RunPhase::AutodetectPending,
                },
            );

            let (cancel_tx, cancel_rx) = watch::channel(false);
            registry.store_cancel_sender(&execution_id, cancel_tx);
            run_detector::spawn_autodetect_poller(
                app,
                store,
                registry,
                execution_id,
                scope_id,
                action.id.clone(),
                action.name.clone(),
                action.command.clone(),
                std::path::PathBuf::from(&working_dir),
                provider_id,
                cancel_rx,
            );
        }
    }
}

pub(crate) async fn run_branch_action_impl(
    branch_id: String,
    action_id: String,
    provider_id: Option<String>,
    app: AppHandle,
    store: Arc<Store>,
    executor: Arc<ActionExecutor>,
    registry: Arc<ActionRegistry>,
) -> Result<String, String> {
    // Get the action
    let action = store
        .get_repo_action(&action_id)
        .map_err(|e| format!("Failed to get action: {e}"))?
        .ok_or_else(|| "Action not found".to_string())?;

    // Get the branch and its project (for repo context + subpath)
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| format!("Failed to get branch: {e}"))?
        .ok_or_else(|| "Branch not found".to_string())?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| format!("Failed to get project: {e}"))?
        .ok_or_else(|| "Project not found".to_string())?;

    let (github_repo, subpath) = resolve_branch_repo_context(&store, &branch, &project)?;
    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| format!("Failed to get action context: {e}"))?;
    if action.context_id != context.id {
        return Err("Action does not belong to this repo/subpath context".to_string());
    }

    let is_remote = branch.branch_type == crate::store::BranchType::Remote;

    // Create event listener
    let listener = Arc::new(TauriExecutionListener::new(
        app.clone(),
        branch_id.clone(),
        action_id.clone(),
        action.name.clone(),
        action.action_type.as_str().to_string(),
        Arc::clone(&registry),
    ));

    // Create metadata
    let metadata = ActionMetadata {
        action_id: action.id.clone(),
        action_name: action.name.clone(),
        auto_commit: action.auto_commit,
    };

    // Execute the action — local vs remote paths
    let (execution_id, working_dir_for_detection) = if is_remote {
        // Remote branch: execute via `sq blox ws exec`
        let workspace_name = branch
            .workspace_name
            .as_deref()
            .ok_or_else(|| "Remote branch has no workspace name".to_string())?;

        // Check workspace status before running. A `None` status means
        // the workspace hasn't been polled yet — treat it as an error to
        // avoid a confusing `sq blox ws exec` failure.
        match branch.workspace_status {
            Some(crate::store::WorkspaceStatus::Running) => {} // OK
            Some(crate::store::WorkspaceStatus::Starting) => {
                return Err(
                    "Workspace is still starting. Please wait until it is running.".to_string(),
                );
            }
            Some(crate::store::WorkspaceStatus::Stopped) => {
                return Err(
                    "Workspace is stopped. Please restart it before running actions.".to_string(),
                );
            }
            Some(crate::store::WorkspaceStatus::Suspended) => {
                return Err(
                    "Workspace is suspended. Please resume it before running actions.".to_string(),
                );
            }
            Some(crate::store::WorkspaceStatus::Error) => {
                return Err("Workspace is in an error state.".to_string());
            }
            None => {
                return Err(
                    "Workspace status is unknown. Please wait for status to be determined."
                        .to_string(),
                );
            }
        }

        let repo_subpath = crate::branches::resolve_branch_workspace_subpath(&store, &branch)
            .map_err(|e| format!("Failed to resolve workspace subpath: {e}"))?;

        // Resolve the full path inside the workspace for this repo+subpath.
        let resolved_repo_path = match &repo_subpath {
            Some(subpath) => Some(
                crate::branches::resolve_workspace_repo_path(workspace_name, subpath)
                    .map_err(|e| format!("Failed to resolve workspace repo path: {e}"))?,
            ),
            None => None,
        };

        // Build the shell command to run inside the workspace.
        // If there's a subpath, cd into it first.
        // Note: `action.command` comes from the action config (not user input)
        // so it is trusted. The `resolved` path is shell-escaped for safety.
        let shell_command = match &resolved_repo_path {
            Some(resolved) => {
                format!(
                    "cd '{}' && {}",
                    resolved.replace('\'', "'\\''"),
                    action.command
                )
            }
            None => action.command.clone(),
        };

        // Find the sq binary and build args for `sq blox ws exec`
        let sq_binary = blox_cli::find_sq_binary().ok_or_else(|| {
            "Could not find `sq` binary. Is it installed and on your PATH?".to_string()
        })?;

        let args = vec![
            "blox".to_string(),
            "ws".to_string(),
            "exec".to_string(),
            workspace_name.to_string(),
            "--".to_string(),
            "sh".to_string(),
            "-lc".to_string(),
            shell_command,
        ];

        // Provide auto-commit context so that after a successful action,
        // git commands run on the remote workspace via `sq blox ws exec`.
        // When there's no resolved path we can't determine the git working
        // directory, so auto-commit is skipped (unlikely for remote branches).
        let auto_commit_info = resolved_repo_path
            .map(|resolved| (sq_binary.clone(), workspace_name.to_string(), resolved));

        let eid = executor
            .execute_remote(sq_binary, args, metadata, listener, auto_commit_info)
            .await
            .map_err(|e| format!("Failed to execute remote action: {e}"))?;

        // Remote actions don't have a local working dir for autodetect polling.
        // Use an empty string as a placeholder — Run detection that needs a
        // local path will gracefully degrade.
        (eid, String::new())
    } else {
        // Local branch: resolve worktree path
        let workdir = store
            .get_workdir_for_branch(&branch_id)
            .map_err(|e| format!("Failed to get workdir: {e}"))?
            .ok_or_else(|| "No worktree found for branch".to_string())?;

        let working_dir = if let Some(subpath) = &subpath {
            let path = std::path::PathBuf::from(&workdir.path).join(subpath);
            path.to_string_lossy().to_string()
        } else {
            workdir.path
        };

        let wd = working_dir.clone();
        let eid = executor
            .execute(action.command.clone(), working_dir, metadata, listener)
            .await
            .map_err(|e| format!("Failed to execute action: {e}"))?;

        (eid, wd)
    };

    // --- Run detection wiring (only for Run actions) ---
    if matches!(action.action_type, ActionType::Run) {
        wire_run_detection(
            app,
            store,
            registry,
            execution_id.clone(),
            branch_id,
            &action,
            working_dir_for_detection,
            provider_id,
        );
    }

    Ok(execution_id)
}

/// Run an action for a branch
#[tauri::command]
pub async fn run_branch_action(
    branch_id: String,
    action_id: String,
    provider: Option<String>,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    executor: State<'_, Arc<ActionExecutor>>,
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<String, String> {
    let store = get_store(&store)?;
    run_branch_action_impl(
        branch_id,
        action_id,
        provider,
        app,
        store,
        executor.inner().clone(),
        registry.inner().clone(),
    )
    .await
}

/// Build the synthetic scope id under which repo-scoped executions are
/// routed: `repo:{github_repo}` or `repo:{github_repo}:{subpath}`.
///
/// The registry, execution events, and running-actions queries all treat
/// the branch id as an opaque routing string, so repo runs reuse them
/// untouched by passing this id where branch runs pass a branch id. The
/// frontend mirrors this format in `repoActionScopeId` (src/lib/commands.ts).
pub(crate) fn repo_action_scope_id(github_repo: &str, subpath: Option<&str>) -> String {
    match subpath.filter(|s| !s.is_empty()) {
        Some(subpath) => format!("repo:{github_repo}:{subpath}"),
        None => format!("repo:{github_repo}"),
    }
}

/// Check that `action` belongs to the (`github_repo`, `subpath`) context the
/// caller claims it does.
///
/// The lookup is read-only on purpose: a context that does not exist cannot own
/// the action, so a missing one is rejected exactly like an unrelated one.
/// Getting-or-creating here would insert an empty context row for the repo on
/// the way to refusing the run — the same reason `list_all_repo_actions` only
/// reads.
fn validate_repo_action_context(
    store: &Store,
    action: &crate::store::RepoAction,
    github_repo: &str,
    subpath: Option<&str>,
) -> Result<(), String> {
    let owns_action = store
        .get_action_context_by_repo_and_subpath(github_repo, subpath)
        .map_err(|e| format!("Failed to get action context: {e}"))?
        .is_some_and(|context| context.id == action.context_id);
    if !owns_action {
        return Err("Action does not belong to this repo/subpath context".to_string());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_repo_action_impl(
    github_repo: String,
    subpath: Option<String>,
    action_id: String,
    provider_id: Option<String>,
    app: AppHandle,
    store: Arc<Store>,
    executor: Arc<ActionExecutor>,
    registry: Arc<ActionRegistry>,
) -> Result<String, String> {
    // Normalize empty subpaths so the context lookup and scope id agree.
    let subpath = subpath.filter(|s| !s.is_empty());

    // Get the action and validate it belongs to this repo+subpath context
    let action = store
        .get_repo_action(&action_id)
        .map_err(|e| format!("Failed to get action: {e}"))?
        .ok_or_else(|| "Action not found".to_string())?;

    validate_repo_action_context(&store, &action, &github_repo, subpath.as_deref())?;

    // Repo runs execute against the repo's main local clone; unlike branch
    // runs there is no worktree or remote-workspace fallback, so the clone
    // must already exist on disk.
    let clone_path = crate::paths::clone_path_for(&github_repo)
        .ok_or_else(|| "Cannot determine clone path (no home directory)".to_string())?;
    if !clone_path.exists() {
        return Err(format!(
            "Repository {github_repo} has not been cloned locally. Clone it before running actions."
        ));
    }

    let working_dir_path = match subpath.as_deref() {
        Some(subpath) => clone_path.join(subpath),
        None => clone_path,
    };
    if !working_dir_path.exists() {
        return Err(format!(
            "Path {} does not exist in the local clone",
            working_dir_path.to_string_lossy()
        ));
    }
    let working_dir = working_dir_path.to_string_lossy().to_string();

    let scope_id = repo_action_scope_id(&github_repo, subpath.as_deref());

    let listener = Arc::new(TauriExecutionListener::new(
        app.clone(),
        scope_id.clone(),
        action.id.clone(),
        action.name.clone(),
        action.action_type.as_str().to_string(),
        Arc::clone(&registry),
    ));

    // Auto-commit is always stripped for repo runs: the executor would
    // commit into the working dir, which here is the user's default-branch
    // checkout rather than a disposable worktree.
    let metadata = ActionMetadata {
        action_id: action.id.clone(),
        action_name: action.name.clone(),
        auto_commit: false,
    };

    let execution_id = executor
        .execute(
            action.command.clone(),
            working_dir.clone(),
            metadata,
            listener,
        )
        .await
        .map_err(|e| format!("Failed to execute action: {e}"))?;

    // --- Run detection wiring (only for Run actions) ---
    if matches!(action.action_type, ActionType::Run) {
        wire_run_detection(
            app,
            store,
            registry,
            execution_id.clone(),
            scope_id,
            &action,
            working_dir,
            provider_id,
        );
    }

    Ok(execution_id)
}

/// Run a repo-scoped action against the repo's local clone.
#[tauri::command(rename_all = "camelCase")]
#[allow(clippy::too_many_arguments)]
pub async fn run_repo_action(
    github_repo: String,
    subpath: Option<String>,
    action_id: String,
    provider: Option<String>,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    executor: State<'_, Arc<ActionExecutor>>,
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<String, String> {
    let store = get_store(&store)?;
    run_repo_action_impl(
        github_repo,
        subpath,
        action_id,
        provider,
        app,
        store,
        executor.inner().clone(),
        registry.inner().clone(),
    )
    .await
}

pub(crate) fn stop_branch_action_impl(
    execution_id: String,
    executor: &ActionExecutor,
) -> Result<(), String> {
    executor
        .stop(&execution_id)
        .map_err(|e| format!("Failed to stop action: {e}"))
}

/// Stop a running action
#[tauri::command]
pub fn stop_branch_action(
    execution_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<(), String> {
    stop_branch_action_impl(execution_id, &executor)
}

/// Stop all running actions for the given branch IDs (best-effort).
pub fn stop_actions_for_branches(
    executor: &ActionExecutor,
    registry: &ActionRegistry,
    branch_ids: &[&str],
) {
    for branch_id in branch_ids {
        for info in registry.get_running_for_branch(branch_id) {
            if executor.is_running(&info.execution_id) {
                if let Err(e) = executor.stop(&info.execution_id) {
                    log::warn!("Failed to stop action {}: {e}", info.execution_id);
                }
            }
        }
    }
}

/// Stop all running actions across all branches (best-effort).
pub fn stop_all_actions(
    executor: &ActionExecutor,
    registry: &ActionRegistry,
    stop_options: StopOptions,
) -> Vec<String> {
    let mut stopped_execution_ids = Vec::new();

    for info in registry.get_all_running() {
        if executor.is_running(&info.execution_id) {
            if let Err(e) = executor.stop_with_options(&info.execution_id, stop_options) {
                log::warn!("Failed to stop action {}: {e}", info.execution_id);
            } else {
                stopped_execution_ids.push(info.execution_id);
            }
        }
    }

    stopped_execution_ids
}

pub(crate) fn get_running_branch_actions_impl(
    branch_id: String,
    executor: &ActionExecutor,
    registry: &ActionRegistry,
) -> Result<Vec<RunningActionInfo>, String> {
    // Get running actions from registry for this branch
    let running_actions = registry.get_running_for_branch(&branch_id);

    // Filter to only actions that are still actually running in the executor
    let executor_ids: std::collections::HashSet<String> =
        executor.get_running_ids().into_iter().collect();

    let active_actions: Vec<RunningActionInfo> = running_actions
        .into_iter()
        .filter(|info| executor_ids.contains(&info.execution_id))
        .collect();

    Ok(active_actions)
}

/// Get all currently running actions for a branch
#[tauri::command]
pub fn get_running_branch_actions(
    branch_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<Vec<RunningActionInfo>, String> {
    get_running_branch_actions_impl(branch_id, &executor, &registry)
}

/// A live execution paired with its current run phase. Carrying the phase
/// inline spares callers a `get_run_phase` round trip per execution.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningActionSnapshot {
    #[serde(flatten)]
    pub info: RunningActionInfo,
    pub phase: Option<RunPhase>,
}

/// Pair each registry entry that is still live in the executor with its run
/// phase. `live_execution_ids` is the executor's liveness set, passed in so the
/// filter and phase join are testable without a real execution.
fn snapshot_running_actions(
    registry: &ActionRegistry,
    live_execution_ids: &std::collections::HashSet<String>,
) -> Vec<RunningActionSnapshot> {
    registry
        .get_all_running()
        .into_iter()
        .filter(|info| live_execution_ids.contains(&info.execution_id))
        .map(|info| RunningActionSnapshot {
            phase: registry.get_run_phase(&info.execution_id),
            info,
        })
        .collect()
}

pub(crate) fn get_all_running_actions_impl(
    executor: &ActionExecutor,
    registry: &ActionRegistry,
) -> Result<Vec<RunningActionSnapshot>, String> {
    let live_execution_ids: std::collections::HashSet<String> =
        executor.get_running_ids().into_iter().collect();
    Ok(snapshot_running_actions(registry, &live_execution_ids))
}

/// Get every currently running action across all scopes, each with its run
/// phase. Cards slice the result by their own scope id, so a surface rendering
/// N of them hydrates from one call instead of one (plus a phase call per
/// execution) each.
#[tauri::command]
pub fn get_all_running_actions(
    executor: State<'_, Arc<ActionExecutor>>,
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<Vec<RunningActionSnapshot>, String> {
    get_all_running_actions_impl(&executor, &registry)
}

pub(crate) fn get_action_output_buffer_impl(
    execution_id: String,
    executor: &ActionExecutor,
) -> Result<Option<Vec<builderbot_actions::OutputChunk>>, String> {
    Ok(executor.get_buffered_output(&execution_id))
}

/// Get buffered output for an action execution
#[tauri::command]
pub fn get_action_output_buffer(
    execution_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<Option<Vec<builderbot_actions::OutputChunk>>, String> {
    get_action_output_buffer_impl(execution_id, &executor)
}

pub(crate) fn clear_action_execution_impl(
    execution_id: String,
    executor: &ActionExecutor,
) -> Result<bool, String> {
    Ok(executor.clear_execution(&execution_id))
}

/// Clear buffered output for a completed execution
#[tauri::command]
pub fn clear_action_execution(
    execution_id: String,
    executor: State<'_, Arc<ActionExecutor>>,
) -> Result<bool, String> {
    clear_action_execution_impl(execution_id, &executor)
}

pub(crate) async fn run_prerun_actions_impl(
    branch_id: String,
    provider_id: Option<String>,
    app: AppHandle,
    store: Arc<Store>,
    executor: Arc<ActionExecutor>,
    registry: Arc<ActionRegistry>,
) -> Result<Vec<String>, String> {
    // Get the branch and project (for repo context + subpath)
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| format!("Failed to get branch: {e}"))?
        .ok_or_else(|| "Branch not found".to_string())?;

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| format!("Failed to get project: {e}"))?
        .ok_or_else(|| "Project not found".to_string())?;

    let (github_repo, subpath) = resolve_branch_repo_context(&store, &branch, &project)?;
    let context = store
        .get_or_create_action_context(&github_repo, subpath.as_deref())
        .map_err(|e| format!("Failed to get action context: {e}"))?;

    // First time we see this repo+subpath, detect actions before running
    // prerun — waiting out another caller's detection rather than reading a
    // list it hasn't finished writing.
    let actions = ensure_actions_detected(&app, &store, &context, provider_id.as_deref()).await?;

    // Filter to prerun actions
    let prerun_actions = actions
        .into_iter()
        .filter(|a| matches!(a.action_type, builderbot_actions::ActionType::Prerun))
        .collect::<Vec<_>>();

    // Get the worktree path for this branch, then apply the repo subpath
    let workdir = store
        .get_workdir_for_branch(&branch_id)
        .map_err(|e| format!("Failed to get workdir: {e}"))?
        .ok_or_else(|| "No worktree found for branch".to_string())?;

    let working_dir = if let Some(subpath) = &subpath {
        let path = std::path::PathBuf::from(&workdir.path).join(subpath);
        path.to_string_lossy().to_string()
    } else {
        workdir.path
    };

    // Execute each prerun action sequentially, waiting for each to complete
    // before starting the next one
    let mut execution_ids = Vec::new();
    for action in prerun_actions {
        let listener = Arc::new(TauriExecutionListener::new(
            app.clone(),
            branch_id.clone(),
            action.id.clone(),
            action.name.clone(),
            action.action_type.as_str().to_string(),
            Arc::clone(&registry),
        ));

        let metadata = ActionMetadata {
            action_id: action.id.clone(),
            action_name: action.name.clone(),
            auto_commit: action.auto_commit,
        };

        let execution_id = executor
            .execute_and_wait(action.command, working_dir.clone(), metadata, listener)
            .await
            .map_err(|e| format!("Failed to execute prerun action: {e}"))?;

        execution_ids.push(execution_id);
    }

    Ok(execution_ids)
}

/// Run all prerun actions for a branch after creation.
///
/// The setup paths detach prerun ([`crate::branches::spawn_prerun_actions`])
/// because they discard its result; this one *is* its result — the execution
/// ids — so it can't be, and a caller asking to run prerun now is asking to
/// wait. It therefore carries the full exposure the setup paths shed: before
/// the first action starts, [`ensure_actions_detected`] can spend up to five
/// minutes waiting out another caller's detection window, and each action then
/// runs to completion in turn. There is no frontend caller today; this exists
/// as an API surface reachable over HTTP.
#[tauri::command]
pub async fn run_prerun_actions(
    branch_id: String,
    provider: Option<String>,
    app: AppHandle,
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    executor: State<'_, Arc<ActionExecutor>>,
    registry: State<'_, Arc<ActionRegistry>>,
) -> Result<Vec<String>, String> {
    let store = get_store(&store)?;
    run_prerun_actions_impl(
        branch_id,
        provider,
        app,
        store,
        executor.inner().clone(),
        registry.inner().clone(),
    )
    .await
}

// =============================================================================
// Run detection commands
// =============================================================================

pub(crate) fn get_run_phase_impl(
    registry: &ActionRegistry,
    execution_id: String,
) -> Result<Option<RunPhase>, String> {
    Ok(registry.get_run_phase(&execution_id))
}

/// Get the current run phase for an execution.
#[tauri::command]
pub async fn get_run_phase(
    registry: State<'_, Arc<ActionRegistry>>,
    execution_id: String,
) -> Result<Option<RunPhase>, String> {
    get_run_phase_impl(&registry, execution_id)
}

pub(crate) fn update_run_detection_mode_impl(
    store: Arc<Store>,
    action_id: String,
    mode: RunDetectionMode,
) -> Result<(), String> {
    let mut action = store
        .get_repo_action(&action_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Action not found".to_string())?;
    action.run_detection_mode = Some(mode);
    store
        .update_repo_action(&action)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Update the run detection mode for a repo action.
#[tauri::command]
pub async fn update_run_detection_mode(
    store: State<'_, Mutex<Option<Arc<Store>>>>,
    action_id: String,
    mode: RunDetectionMode,
) -> Result<(), String> {
    let store = get_store(&store)?;
    update_run_detection_mode_impl(store, action_id, mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn register(registry: &ActionRegistry, execution_id: &str, scope_id: &str, action_type: &str) {
        registry.register(
            execution_id.to_string(),
            scope_id.to_string(),
            format!("action-{execution_id}"),
            format!("Action {execution_id}"),
            action_type.to_string(),
            0,
        );
    }

    #[test]
    fn snapshot_running_actions_drops_dead_executions_and_joins_phases() {
        let registry = ActionRegistry::new();
        let repo_scope = repo_action_scope_id("block/builderbot", Some("apps/staged"));
        register(&registry, "live-run", &repo_scope, "run");
        register(&registry, "live-test", "branch-1", "test");
        register(&registry, "dead", "branch-1", "build");
        registry.set_run_phase(
            "live-run",
            RunPhase::Running {
                endpoint: Some("http://localhost:5173".to_string()),
            },
        );

        let live: HashSet<String> = ["live-run", "live-test"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut snapshots = snapshot_running_actions(&registry, &live);
        snapshots.sort_by(|a, b| a.info.execution_id.cmp(&b.info.execution_id));

        // "dead" is still registered but no longer live in the executor.
        assert_eq!(
            snapshots
                .iter()
                .map(|s| s.info.execution_id.as_str())
                .collect::<Vec<_>>(),
            vec!["live-run", "live-test"]
        );
        assert!(matches!(
            &snapshots[0].phase,
            Some(RunPhase::Running { endpoint: Some(e) }) if e == "http://localhost:5173"
        ));
        assert!(snapshots[1].phase.is_none());
        // Repo- and branch-scoped executions come back together; callers slice
        // the result by their own scope id.
        assert_eq!(snapshots[0].info.branch_id, repo_scope);
        assert_eq!(snapshots[1].info.branch_id, "branch-1");

        // The info fields flatten alongside the phase, so a snapshot reads like
        // a RunningActionInfo with one extra key.
        let json = serde_json::to_value(&snapshots[0]).unwrap();
        assert_eq!(json["executionId"], "live-run");
        assert_eq!(json["actionType"], "run");
        assert_eq!(json["phase"]["type"], "running");
    }

    #[test]
    fn get_all_running_actions_is_empty_when_the_executor_runs_nothing() {
        let registry = ActionRegistry::new();
        register(&registry, "stale", "branch-1", "test");

        let executor = ActionExecutor::new();
        assert!(get_all_running_actions_impl(&executor, &registry)
            .unwrap()
            .is_empty());
    }

    fn suggestion(name: &str, command: &str, action_type: ActionType) -> SuggestedAction {
        SuggestedAction {
            name: name.to_string(),
            command: command.to_string(),
            action_type,
            auto_commit: false,
            source: "justfile".to_string(),
        }
    }

    #[test]
    fn persist_suggested_actions_skips_known_commands_and_continues_sort_order() {
        let store = Store::in_memory().unwrap();
        let context = store
            .get_or_create_action_context("block/builderbot", Some("apps/staged"))
            .unwrap();

        persist_suggested_actions(
            &store,
            &context.id,
            vec![
                suggestion("Dev", "just dev", ActionType::Run),
                suggestion("Test", "just test", ActionType::Test),
            ],
        )
        .unwrap();

        // A re-detection that turns up one known command and one new one only
        // writes the new one, appended after the existing sort orders.
        persist_suggested_actions(
            &store,
            &context.id,
            vec![
                suggestion("Test (renamed)", "just test", ActionType::Test),
                suggestion("Build", "just build", ActionType::Build),
            ],
        )
        .unwrap();

        let actions = store.list_repo_actions(&context.id).unwrap();
        assert_eq!(
            actions
                .iter()
                .map(|a| (a.name.as_str(), a.sort_order))
                .collect::<Vec<_>>(),
            vec![("Dev", 0), ("Test", 1), ("Build", 2)]
        );

        // The context started with no run action, so the first run suggestion
        // is what the card header ends up showing — and only that one.
        assert_eq!(
            actions
                .iter()
                .filter(|a| a.pinned)
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Dev"]
        );
        // Detection never picks an icon; NULL means the action type's default.
        assert!(actions.iter().all(|a| a.icon.is_none()));
    }

    #[test]
    fn persist_suggested_actions_pins_only_the_first_run_action_of_a_fresh_context() {
        let store = Store::in_memory().unwrap();
        let context = store
            .get_or_create_action_context("block/builderbot", Some("apps/staged"))
            .unwrap();

        persist_suggested_actions(
            &store,
            &context.id,
            vec![
                suggestion("Build", "just build", ActionType::Build),
                suggestion("Dev", "just dev", ActionType::Run),
                suggestion("Storybook", "just storybook", ActionType::Run),
            ],
        )
        .unwrap();

        let pinned = |store: &Store| -> Vec<String> {
            store
                .list_repo_actions(&context.id)
                .unwrap()
                .into_iter()
                .filter(|a| a.pinned)
                .map(|a| a.name)
                .collect()
        };
        assert_eq!(pinned(&store), vec!["Dev".to_string()]);

        // Unpinning is a deliberate choice, so a later re-detect that turns up
        // another run action leaves the header empty rather than re-pinning.
        let dev = store
            .list_repo_actions(&context.id)
            .unwrap()
            .into_iter()
            .find(|a| a.name == "Dev")
            .unwrap();
        store.update_repo_action(&dev.with_pinned(false)).unwrap();

        persist_suggested_actions(
            &store,
            &context.id,
            vec![suggestion("Preview", "just preview", ActionType::Run)],
        )
        .unwrap();
        assert!(pinned(&store).is_empty());
    }

    /// A context mid-detection: the flag claimed by `pid`, nothing marked yet.
    fn detecting_context(store: &Store, pid: u32) -> crate::store::ActionContext {
        let context = store
            .get_or_create_action_context("block/builderbot", Some("apps/staged"))
            .unwrap();
        assert!(store
            .claim_action_context_detection(&context.id, pid)
            .unwrap());
        context
    }

    #[test]
    fn finish_detection_window_persists_and_closes_the_window() {
        let store = Store::in_memory().unwrap();
        let context = detecting_context(&store, std::process::id());

        let actions = finish_detection_window(
            &store,
            &context.id,
            Ok(vec![
                suggestion("Dev", "just dev", ActionType::Run),
                suggestion("Test", "just test", ActionType::Test),
            ]),
        )
        .unwrap();

        assert_eq!(
            actions.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["Dev", "Test"]
        );
        let context = store.get_action_context(&context.id).unwrap().unwrap();
        assert!(!context.detecting_actions);
        assert!(context.has_detected_actions);
    }

    #[test]
    fn finish_detection_window_closes_the_window_when_detection_failed() {
        let store = Store::in_memory().unwrap();
        let context = detecting_context(&store, std::process::id());

        let err = finish_detection_window(&store, &context.id, Err("no agent installed".into()))
            .unwrap_err();
        assert_eq!(err, "no agent installed");

        // The error reaches the caller, but the window still closed: a flag
        // left set would reject every later detection for this repo — with no
        // UI path to clear it — while every surface spins on a run that is over.
        let context = store.get_action_context(&context.id).unwrap().unwrap();
        assert!(!context.detecting_actions);
        assert!(context.has_detected_actions);
    }

    #[test]
    fn a_claimed_detection_window_rejects_a_second_claim_until_it_closes() {
        let store = Store::in_memory().unwrap();
        let pid = std::process::id();
        let context = detecting_context(&store, pid);

        // The check-and-set is one statement, so the racing caller loses.
        assert!(!store
            .claim_action_context_detection(&context.id, pid)
            .unwrap());

        finish_detection_window(&store, &context.id, Ok(Vec::new())).unwrap();
        assert!(store
            .claim_action_context_detection(&context.id, pid)
            .unwrap());
    }

    #[test]
    fn the_sweep_releases_a_window_whose_owner_died() {
        let store = Store::in_memory().unwrap();
        // A process killed mid-detection: the flag is set, the owner is gone.
        let context = detecting_context(&store, 4242);

        assert_eq!(recover_orphaned_detection_claims(&store, |_| false), 1);

        // The regression: without the sweep this claim — and so every later
        // detection for the repo, on every surface, across restarts — is
        // rejected as already in progress.
        assert!(store
            .claim_action_context_detection(&context.id, std::process::id())
            .unwrap());
    }

    #[test]
    fn the_sweep_leaves_a_window_owned_by_a_live_process_alone() {
        let store = Store::in_memory().unwrap();
        // Another Staged instance, mid-detection on the shared database.
        let context = detecting_context(&store, 4242);

        assert_eq!(recover_orphaned_detection_claims(&store, |_| true), 0);

        // Releasing it would let this instance start the concurrent detection
        // the claim exists to prevent.
        assert!(!store
            .claim_action_context_detection(&context.id, std::process::id())
            .unwrap());
    }

    #[test]
    fn the_sweep_releases_a_window_with_no_recorded_owner() {
        let store = Store::in_memory().unwrap();
        let context = store
            .get_or_create_action_context("block/builderbot", Some("apps/staged"))
            .unwrap();
        // A row claimed before `detecting_pid` existed.
        store
            .claim_action_context_detection_without_owner(&context.id)
            .unwrap();

        assert_eq!(recover_orphaned_detection_claims(&store, |_| true), 1);
        assert!(store
            .claim_action_context_detection(&context.id, std::process::id())
            .unwrap());
    }

    #[test]
    fn the_sweep_releases_a_window_carrying_our_own_pid() {
        let store = Store::in_memory().unwrap();
        // At startup our pid on a claim can only be a dead process we
        // inherited it from, so it goes even though the pid reads as live.
        let context = detecting_context(&store, std::process::id());

        assert_eq!(recover_orphaned_detection_claims(&store, |_| true), 1);
        assert!(store
            .claim_action_context_detection(&context.id, std::process::id())
            .unwrap());
    }

    /// Poll without pausing between ticks, with a cap only the loop's own
    /// exits reach — so a test ends on what it arranges, not on the clock.
    /// Probes every tick, since these tests use the probe as their hook into
    /// the loop.
    const EAGER: DetectionWaitPolicy = DetectionWaitPolicy {
        interval: std::time::Duration::ZERO,
        max_wait: std::time::Duration::from_secs(30),
        liveness_probe_interval: std::time::Duration::ZERO,
    };

    /// Poll once, then give up: stands in for a window still held long after
    /// the cap.
    const ONE_TICK: DetectionWaitPolicy = DetectionWaitPolicy {
        interval: std::time::Duration::ZERO,
        max_wait: std::time::Duration::ZERO,
        liveness_probe_interval: std::time::Duration::ZERO,
    };

    /// Many ticks, each far shorter than the probe interval — the shipped
    /// policy's shape (300 ticks, 20 probes) compressed into milliseconds, so
    /// the tests below can count probes against ticks without a clock of their
    /// own. How many ticks actually fit is up to the machine; the assertions
    /// don't depend on it.
    const MANY_TICKS: DetectionWaitPolicy = DetectionWaitPolicy {
        interval: std::time::Duration::from_millis(1),
        max_wait: std::time::Duration::from_millis(50),
        liveness_probe_interval: std::time::Duration::from_secs(60),
    };

    #[tokio::test]
    async fn the_wait_returns_once_a_live_window_closes() {
        let store = Store::in_memory().unwrap();
        let context = detecting_context(&store, 4242);

        // The window's owner finishes between two ticks: still alive when the
        // first tick probes it, done persisting by the second.
        let owner_finishes = |_pid| {
            finish_detection_window(
                &store,
                &context.id,
                Ok(vec![suggestion(
                    "Install",
                    "just install",
                    ActionType::Prerun,
                )]),
            )
            .unwrap();
            true
        };

        let waited = wait_for_detection_window(&store, &context.id, EAGER, owner_finishes).await;

        assert_eq!(waited, DetectionWait::Closed);
        // The regression: a caller that listed on the rejection instead of
        // waiting reads the empty pre-detection list, finds no prerun actions,
        // and skips this branch's setup for good.
        assert_eq!(
            store
                .list_repo_actions(&context.id)
                .unwrap()
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Install"]
        );
    }

    #[tokio::test]
    async fn the_wait_takes_over_a_window_whose_owner_died() {
        let store = Store::in_memory().unwrap();
        // A process killed mid-detection: nothing is going to persist into
        // this context, so there is nothing to wait for.
        let context = detecting_context(&store, 4242);

        let waited = wait_for_detection_window(&store, &context.id, EAGER, |_| false).await;

        assert_eq!(waited, DetectionWait::TookOver);
        // Taken over rather than released, so the waiter detects the context
        // itself instead of waiting out the cap for an owner that is gone —
        // and the flag never drops on the way. A window that blinked closed
        // here would let a second waiter's tick read this context's
        // pre-detection list as final, and let a claim land in the gap and
        // reject the caller that just won the window.
        assert_eq!(
            store.list_detecting_action_contexts().unwrap(),
            vec![(context.id.clone(), Some(std::process::id()))]
        );
        assert!(!store
            .claim_action_context_detection(&context.id, std::process::id())
            .unwrap());
    }

    #[tokio::test]
    async fn the_wait_leaves_a_window_that_changed_hands_to_its_new_owner() {
        let store = Store::in_memory().unwrap();
        let context = detecting_context(&store, 4242);

        // The dead owner's window closed and another process opened a new one
        // between the tick's read and its takeover. The takeover is guarded on
        // the pid just read, so it matches nothing and the new owner keeps its
        // window — the next tick (here, the cap) would find it alive and wait.
        let reclaimed = std::cell::Cell::new(false);
        let owner_changes_hands = |_pid| {
            if !reclaimed.replace(true) {
                store.mark_action_context_detected(&context.id).unwrap();
                assert!(store
                    .claim_action_context_detection(&context.id, 9999)
                    .unwrap());
                return false;
            }
            true
        };

        let waited =
            wait_for_detection_window(&store, &context.id, ONE_TICK, owner_changes_hands).await;

        assert_eq!(waited, DetectionWait::GaveUp);
        assert_eq!(
            store.list_detecting_action_contexts().unwrap(),
            vec![(context.id.clone(), Some(9999))]
        );
    }

    #[tokio::test]
    async fn the_wait_gives_up_on_a_window_that_never_closes() {
        let store = Store::in_memory().unwrap();
        // Another live Staged instance, wedged mid-detection.
        let context = detecting_context(&store, 4242);

        let waited = wait_for_detection_window(&store, &context.id, ONE_TICK, |_| true).await;

        // The AI call has no timeout of its own, so the cap is what keeps a
        // wedged owner from holding branch setup open forever. Its claim is
        // left alone; the caller proceeds with the list it can see.
        assert_eq!(waited, DetectionWait::GaveUp);
        assert!(!store
            .claim_action_context_detection(&context.id, std::process::id())
            .unwrap());
    }

    #[tokio::test]
    async fn the_wait_treats_a_window_owned_by_this_process_as_live() {
        let store = Store::in_memory().unwrap();
        // The startup sweep releases our own pid — there it can only be a dead
        // process whose pid we inherited. Mid-session it is the opposite:
        // another task in this process owns the window, so it gets waited on
        // even though the pid probe would call it dead.
        let context = detecting_context(&store, std::process::id());

        let waited = wait_for_detection_window(&store, &context.id, ONE_TICK, |_| false).await;

        assert_eq!(waited, DetectionWait::GaveUp);
        assert!(!store
            .claim_action_context_detection(&context.id, std::process::id())
            .unwrap());
    }

    #[tokio::test]
    async fn the_wait_probes_the_owner_once_across_many_ticks() {
        let store = Store::in_memory().unwrap();
        // Another live Staged instance, wedged mid-detection: the wait re-reads
        // its claim until the cap, and every read used to re-probe the owner —
        // a `kill -0` subprocess spawn, blocking a tokio worker, up to 300
        // times for one waiting caller.
        let context = detecting_context(&store, 4242);

        let probes = std::cell::Cell::new(0);
        let counting_probe = |_pid| {
            probes.set(probes.get() + 1);
            true
        };

        let waited =
            wait_for_detection_window(&store, &context.id, MANY_TICKS, counting_probe).await;

        assert_eq!(waited, DetectionWait::GaveUp);
        // Every tick re-read the claim (a SQLite query, which is the cheap
        // half) and none of them re-probed: the probe interval outlasts the
        // whole wait here, so the first verdict stands for all of it.
        assert_eq!(probes.get(), 1);
    }

    #[tokio::test]
    async fn the_wait_reprobes_an_owner_it_has_not_seen_before() {
        let store = Store::in_memory().unwrap();
        let context = detecting_context(&store, 4242);

        // The first owner finishes and a second claims the context, well
        // inside the probe interval. A cached verdict is about the pid it was
        // taken on and can't stand in for a new owner's: here that would mean
        // waiting out the cap on a window whose owner is already gone, and
        // (with the verdicts the other way around) taking a live owner's
        // window over.
        let probed = std::cell::RefCell::new(Vec::new());
        let owner_changes_hands = |pid| {
            probed.borrow_mut().push(pid);
            if pid == 4242 {
                store.mark_action_context_detected(&context.id).unwrap();
                assert!(store
                    .claim_action_context_detection(&context.id, 9999)
                    .unwrap());
                return true;
            }
            false
        };

        let waited =
            wait_for_detection_window(&store, &context.id, MANY_TICKS, owner_changes_hands).await;

        assert_eq!(waited, DetectionWait::TookOver);
        assert_eq!(*probed.borrow(), vec![4242, 9999]);
    }

    #[test]
    fn a_liveness_verdict_stands_until_the_probe_interval_expires() {
        let probes = std::cell::Cell::new(0);
        let mut owner = OwnerLiveness::new(
            |_pid| {
                probes.set(probes.get() + 1);
                true
            },
            std::time::Duration::from_secs(60),
        );

        assert!(owner.alive(4242));
        assert!(owner.alive(4242));
        assert!(owner.alive(4242));
        assert_eq!(probes.get(), 1);

        // A zero interval is every-tick probing, which is what the tests that
        // use the probe as their hook into the wait loop rely on.
        let mut eager = OwnerLiveness::new(
            |_pid| {
                probes.set(probes.get() + 1);
                true
            },
            std::time::Duration::ZERO,
        );
        assert!(eager.alive(4242));
        assert!(eager.alive(4242));
        assert_eq!(probes.get(), 3);
    }

    #[test]
    fn a_liveness_verdict_never_carries_over_to_another_pid() {
        let probed = std::cell::RefCell::new(Vec::new());
        let mut owner = OwnerLiveness::new(
            |pid| {
                probed.borrow_mut().push(pid);
                // Only the second owner is alive. Carrying the first one's
                // verdict over would take a live window from underneath it.
                pid == 9999
            },
            std::time::Duration::from_secs(60),
        );

        assert!(!owner.alive(4242));
        assert!(owner.alive(9999));
        assert!(owner.alive(9999));
        assert_eq!(*probed.borrow(), vec![4242, 9999]);
    }

    #[test]
    fn the_in_progress_rejection_reads_the_way_the_button_has_always_shown_it() {
        assert_eq!(
            DetectionError::InProgress.to_string(),
            "Detection is already in progress for this repository"
        );
        assert_eq!(
            DetectionError::Failed("no agent installed".into()).to_string(),
            "no agent installed"
        );
    }

    #[test]
    fn validating_a_repo_action_context_never_creates_one() {
        let store = Store::in_memory().unwrap();
        let context = store
            .get_or_create_action_context("block/builderbot", Some("apps/staged"))
            .unwrap();
        let action = crate::store::RepoAction::new(
            context.id.clone(),
            "Dev".to_string(),
            "just dev".to_string(),
            ActionType::Run,
            0,
        );
        store.create_repo_action(&action).unwrap();

        validate_repo_action_context(&store, &action, "block/builderbot", Some("apps/staged"))
            .unwrap();

        // A repo with no context of its own is rejected without minting one,
        // and so is the same repo at a different subpath.
        assert!(validate_repo_action_context(&store, &action, "block/goose", None).is_err());
        assert!(validate_repo_action_context(
            &store,
            &action,
            "block/builderbot",
            Some("apps/other")
        )
        .is_err());
        assert_eq!(
            store.count_action_contexts_for_repo("block/goose").unwrap(),
            0
        );
        assert_eq!(
            store
                .count_action_contexts_for_repo("block/builderbot")
                .unwrap(),
            1
        );
    }

    #[test]
    fn repo_action_scope_id_includes_subpath_when_present() {
        assert_eq!(
            repo_action_scope_id("block/builderbot", Some("apps/staged")),
            "repo:block/builderbot:apps/staged"
        );
        assert_eq!(
            repo_action_scope_id("block/goose", None),
            "repo:block/goose"
        );
        // Empty subpaths normalize to the no-subpath form.
        assert_eq!(
            repo_action_scope_id("block/goose", Some("")),
            "repo:block/goose"
        );
    }
}
