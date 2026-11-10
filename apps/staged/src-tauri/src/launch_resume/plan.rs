//! What launch resume does with each interrupted session, decided from the
//! rows alone so the ordering and the exclusions are testable without a
//! runner.

use crate::session_commands::{project_active_session, resumable_agent_session_id};
use crate::store::{Session, Store};

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
pub(crate) const PIPELINE_NOT_RESUMED_REASON: &str =
    "pipeline sessions are not resumed automatically; run the push or PR again from the branch";
pub(crate) const PROJECT_NEVER_STARTED_REASON: &str =
    "the project session never reached its agent; start a new project session instead";
pub(crate) const INTERNAL_SESSION_REASON: &str = "internal session; its parent re-runs it";

/// What kind of row a candidate is, as far as launch resume cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateKind {
    /// A branch-scoped session: commit, note, review, or a pr/push/pull
    /// pipeline row.
    Repo { pipeline: bool },
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
        CandidateKind::Repo {
            pipeline: session.pipeline.is_some(),
        }
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
    }
}

/// Decide the action for one candidate.
pub(crate) fn action_for(candidate: &LaunchCandidate) -> LaunchAction {
    match candidate.kind {
        CandidateKind::Repo { pipeline: true } => LaunchAction::Skip {
            reason: PIPELINE_NOT_RESUMED_REASON,
        },
        CandidateKind::Repo { pipeline: false } if candidate.agent_resumable => {
            LaunchAction::Resume
        }
        CandidateKind::Repo { pipeline: false } => LaunchAction::Requeue,
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
            CandidateKind::Repo { .. } => repo.push(candidate),
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
