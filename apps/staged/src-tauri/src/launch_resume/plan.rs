//! What launch resume does with each interrupted session, decided from the
//! rows alone so the ordering and the exclusions are testable without a
//! runner.

use crate::session_commands::{project_active_session, resumable_agent_session_id};
use crate::store::{PipelineKind, Session, Store};

/// The next user turn a resumed repo session gets. Resume is SDK-native
/// (`load_session` restores the conversation), so this only has to say what
/// happened and what to check first.
pub(crate) const REPO_SESSION_RESUME_PROMPT: &str = "Staged was closed and relaunched while you \
     were working, so your previous turn was interrupted. Check the working tree (for example \
     `git status` and `git log -1`) before continuing, then continue where you left off.";

/// The next user turn a resumed project session gets. Its children were
/// restarted (or re-queued) in the phase before it, so the handles it already
/// holds are worth polling again; the ones that could not be restarted still
/// read as ended by `app_quit`.
pub(crate) const PROJECT_SESSION_RESUME_PROMPT: &str = "Staged was closed and relaunched while \
     you were working. The repo sessions you started were restarted where possible; call \
     `wait_for_repo_session` again on any handle you were waiting on. A handle that reports \
     `cancelled` or `failed` with reason `app_quit` could not be restarted and may be continued \
     with `continue_repo_session` to finish its own goal, or started again. Then continue where \
     you left off.";

/// Why a session is listed but not restarted automatically.
pub(crate) const PROJECT_NEVER_STARTED_REASON: &str =
    "the project session never reached its agent; start a new project session instead";
pub(crate) const INTERNAL_SESSION_REASON: &str = "internal session; its parent re-runs it";
/// A rebase or squash with no conversation that had started running. See
/// [`restart_blocked_reason`] for why neither is run again.
/// Shown as plain text in the dialog, so no markup.
pub(crate) const REBASE_CUT_OFF_REASON: &str =
    "the rebase was cut off part-way through; run git status in the worktree, finish or abort \
     a rebase still in progress there, then rebase again if it is still needed";
pub(crate) const SQUASH_CUT_OFF_REASON: &str =
    "the squash was cut off part-way through; run git status in the worktree: if the branch's \
     commits were already reset, their changes are staged and need committing by hand, \
     otherwise squash again";
/// A rebase or squash the quit stopped before its first step, whose pending
/// commit went with it, so the branch queue has nothing to schedule it by.
pub(crate) const REBASE_NOT_STARTED_REASON: &str =
    "the rebase was stopped before it ran; rebase again";
pub(crate) const SQUASH_NOT_STARTED_REASON: &str =
    "the squash was stopped before it ran; squash again";
/// A create-PR pipeline cut off before its agent had a conversation.
pub(crate) const PR_NOT_CREATED_REASON: &str =
    "cut off before the pull request was created; create it again";

/// What kind of row a candidate is, as far as launch resume cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateKind {
    /// A branch-scoped session: commit, note, or review, or a command
    /// pipeline row (rebase, squash, push, pull, PR). A pipeline row whose
    /// agent turn the quit cut off continues that turn like the rest, as the
    /// chat pane's Resume does, and the drain resumes a re-queued one rather
    /// than running its steps again (`session_commands::QueuedStartKind`). One
    /// with no conversation is only re-queued when running it from the start
    /// is safe (see [`restart_blocked_reason`]).
    Repo,
    /// A project-level session (linked from `project_notes`).
    Project,
    /// Neither: a Pikchr child session or a row whose links are gone.
    Internal,
}

/// One interrupted row, reduced to what the plan needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchCandidate {
    pub(crate) session_id: String,
    pub(crate) branch_id: Option<String>,
    pub(crate) created_at: i64,
    pub(crate) kind: CandidateKind,
    /// The row carries a real ACP session id (not the immediate-start
    /// placeholder), so `load_session` can restore its conversation.
    pub(crate) agent_resumable: bool,
    /// Why a row with no conversation must not be put back on its branch
    /// queue, or `None` when re-queueing it is a safe restart
    /// ([`restart_blocked_reason`]). Not consulted for a row that resumes.
    pub(crate) restart_blocked: Option<&'static str>,
}

/// What launch resume does with one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchAction {
    /// `continue_resume_gate` then `resume_claimed_session_for_store`.
    Resume,
    /// Flip back to `queued` and kick the branch; the drain rebuilds its
    /// context as if it had never been touched. (A `Resume` row the gate
    /// blocks is re-queued the same way, but keeps its agent id, so the drain
    /// resumes its conversation in turn — see
    /// `session_commands::QueuedStartKind`.)
    Requeue,
    /// Left as it is, with the reason shown to the user.
    Skip { reason: &'static str },
}

impl LaunchAction {
    /// The wire spelling shown in the dialog.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            LaunchAction::Resume => "resume",
            LaunchAction::Requeue => "restart",
            LaunchAction::Skip { .. } => "manual",
        }
    }

    pub(crate) fn skip_reason(&self) -> Option<&'static str> {
        match self {
            LaunchAction::Skip { reason } => Some(reason),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedSession {
    pub(crate) session_id: String,
    pub(crate) branch_id: Option<String>,
    pub(crate) action: LaunchAction,
}

/// The two phases, each in the order it runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LaunchResumePlan {
    /// Repo sessions grouped by branch, each branch in `created_at` order.
    pub(crate) repo: Vec<PlannedSession>,
    /// Project sessions in `created_at` order.
    pub(crate) project: Vec<PlannedSession>,
}

/// Reduce a row to a candidate. The branch comes from the row's artifact
/// first and its own `branch_id` second (as the busy-state projection does),
/// the project link from `project_notes`; a row with neither is internal.
pub(crate) fn classify_candidate(store: &Store, session: &Session) -> LaunchCandidate {
    let info = project_active_session(store, session);
    let is_project_session = store
        .get_project_note_by_session(&session.id)
        .ok()
        .flatten()
        .is_some();
    let kind = if info.branch_id.is_some() {
        CandidateKind::Repo
    } else if is_project_session {
        CandidateKind::Project
    } else {
        CandidateKind::Internal
    };
    LaunchCandidate {
        session_id: session.id.clone(),
        branch_id: info.branch_id,
        created_at: session.created_at,
        kind,
        agent_resumable: resumable_agent_session_id(session.agent_id.as_deref()).is_some(),
        restart_blocked: restart_blocked_reason(store, session),
    }
}

/// Why re-queueing `session` would not be a safe restart, or `None` when it
/// is. Only pipeline rows are ever blocked: a commit, note or review session
/// with no conversation never reached its agent, and the drain builds its
/// prompt again from the row.
///
/// A re-queued pipeline row is rebuilt by the drain and run from its first
/// step (`prs::start_queued_commit_pipeline_for_branch`,
/// `prs::start_queued_git_pipeline_for_branch`). Whether that is safe depends
/// on the kind:
///
/// - **Push, pull**: safe wherever the quit landed. `git push` either updated
///   the remote ref or did not, and pushing again is a no-op or the same push;
///   `--force-with-lease` refuses rather than overwrites if the first attempt
///   landed without updating the tracking ref. A pull is `git fetch` then
///   `git merge --ff-only`, which refuses a worktree the first attempt left
///   half-updated. A force push that was waiting on a rebase is still checked
///   against that rebase by the drain (`pipeline_dependency`).
/// - **Rebase, squash**: only when no step had started and the pending
///   commit is still there, which is a row the quit caught while it was still
///   queued. Otherwise it is left alone, for two reasons found in the runner:
///   - The quit cancels a running command step by killing its process group
///     (`session_runner::run_pipeline_command`), so `git rebase` can die
///     between picks, or stop on a conflict before the hand-off's agent has a
///     conversation, leaving `.git/rebase-merge` and a detached HEAD. A rerun
///     would record that detached HEAD as the pre-rebase head, fail in
///     `git rebase` ("already a rebase-merge directory") and reach the agent
///     only through the failure hand-off, whose prompt assumes a rebase
///     stopped on a conflict. A squash cut off after its soft reset would
///     rerun `git log origin/<base>..HEAD` against the reset branch and hand
///     the agent an empty history to write the message from.
///   - A pipeline that ends without its agent deletes its pending commit
///     (`session_runner::resolve_pipeline_artifacts_without_ai`), and that
///     commit is the only thing the branch queue schedules a rebase or squash
///     by. Re-queued without it the row is stepped over on every drain
///     (`BranchSessionLink::Unlinked`) and stays `queued` for good.
/// - **No kind** (the create-PR pipeline): it has no queued form at all. The
///   drain cannot schedule it, so it would also stay `queued` for good.
pub(crate) fn restart_blocked_reason(store: &Store, session: &Session) -> Option<&'static str> {
    let pipeline = session.pipeline.as_ref()?;
    let Some(kind) = pipeline.kind.as_ref() else {
        return Some(PR_NOT_CREATED_REASON);
    };
    let (cut_off, not_started) = match kind {
        PipelineKind::Push | PipelineKind::Pull => return None,
        PipelineKind::Rebase => (REBASE_CUT_OFF_REASON, REBASE_NOT_STARTED_REASON),
        PipelineKind::Squash => (SQUASH_CUT_OFF_REASON, SQUASH_NOT_STARTED_REASON),
    };
    // `started_at` is stamped when the runner begins a step and survives the
    // cancel, which marks the step and the ones after it `skipped`.
    if pipeline.steps.iter().any(|step| step.started_at.is_some()) {
        return Some(cut_off);
    }
    match store.get_commit_by_session(&session.id) {
        Ok(Some(_)) => None,
        Ok(None) | Err(_) => Some(not_started),
    }
}

/// Decide the action for one candidate.
pub(crate) fn action_for(candidate: &LaunchCandidate) -> LaunchAction {
    match candidate.kind {
        CandidateKind::Repo if candidate.agent_resumable => LaunchAction::Resume,
        CandidateKind::Repo => match candidate.restart_blocked {
            Some(reason) => LaunchAction::Skip { reason },
            None => LaunchAction::Requeue,
        },
        CandidateKind::Project if candidate.agent_resumable => LaunchAction::Resume,
        CandidateKind::Project => LaunchAction::Skip {
            reason: PROJECT_NEVER_STARTED_REASON,
        },
        CandidateKind::Internal => LaunchAction::Skip {
            reason: INTERNAL_SESSION_REASON,
        },
    }
}

/// Order the candidates into the two phases.
///
/// Repo sessions go first, grouped by branch and oldest-created first within
/// a branch: that is the order the branch drain uses, so re-queued rows keep
/// their place and a session with a conversation to resume is tried before
/// the rows queued behind it. Project sessions follow, oldest first; they are
/// independent of one another.
pub(crate) fn plan_launch_resume(candidates: Vec<LaunchCandidate>) -> LaunchResumePlan {
    let mut repo: Vec<&LaunchCandidate> = Vec::new();
    let mut project: Vec<&LaunchCandidate> = Vec::new();
    for candidate in &candidates {
        match candidate.kind {
            CandidateKind::Repo => repo.push(candidate),
            CandidateKind::Project => project.push(candidate),
            CandidateKind::Internal => {}
        }
    }
    repo.sort_by(|a, b| {
        a.branch_id
            .cmp(&b.branch_id)
            .then(a.created_at.cmp(&b.created_at))
            .then(a.session_id.cmp(&b.session_id))
    });
    project.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then(a.session_id.cmp(&b.session_id))
    });

    let planned = |candidate: &LaunchCandidate| PlannedSession {
        session_id: candidate.session_id.clone(),
        branch_id: candidate.branch_id.clone(),
        action: action_for(candidate),
    };
    LaunchResumePlan {
        repo: repo.into_iter().map(planned).collect(),
        project: project.into_iter().map(planned).collect(),
    }
}
