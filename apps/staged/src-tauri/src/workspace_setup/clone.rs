//! The clone a branch is checked out in: which one it is, who else is in
//! it, and the checkout itself with the clone held.
//!
//! "The same clone" is the directory on the workstation. Every remote
//! branch whose checkout is that directory counts, whichever project it
//! belongs to: the directory comes from the repository's name alone.

use std::sync::Arc;

use crate::branches;
use crate::store::{self, Store};

use super::checkout::CheckoutError;
use super::guard::SetupGuard;
use super::host::{Checkout, SetupHost};
use super::verify;
use super::SetupRequest;

/// Where `branch` is checked out and what its clone should be on, from its
/// row and its project's.
pub(crate) fn checkout_for(store: &Arc<Store>, branch: &store::Branch) -> Result<Checkout, String> {
    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;
    let workspace = branch
        .workspace_name
        .as_deref()
        .ok_or("Branch has no workspace name")?;
    let clone_dir = branches::resolve_branch_clone_dir(store, branch)?;
    let base_ref = branches::normalize_branch_ref(&branch.base_branch);
    let base_is_worked_on = branches_in_clone(store, workspace, clone_dir.as_deref())?
        .iter()
        .any(|other| other.id != branch.id && other.branch_name == base_ref);
    Ok(Checkout {
        workspace: workspace.to_string(),
        clone_dir,
        repo_slug: branches::resolve_branch_repo_slug(store, &project, branch)?,
        base_ref,
        branch_name: branch.branch_name.clone(),
        base_is_worked_on,
    })
}

/// Every remote branch whose checkout is the clone at `clone_dir` on
/// `workspace`, whichever project it belongs to. The clone directory comes
/// from the repository's name alone, so the project is not what decides
/// whether two branches share one.
pub(crate) fn branches_in_clone(
    store: &Store,
    workspace: &str,
    clone_dir: Option<&str>,
) -> Result<Vec<store::Branch>, String> {
    let mut sharing = Vec::new();
    for project in store.list_projects().map_err(|e| e.to_string())? {
        for branch in store
            .list_branches_for_project(&project.id)
            .map_err(|e| e.to_string())?
        {
            if branch.branch_type == store::BranchType::Remote
                && branch.workspace_name.as_deref() == Some(workspace)
                && branches::resolve_branch_clone_dir(store, &branch)?.as_deref() == clone_dir
            {
                sharing.push(branch);
            }
        }
    }
    Ok(sharing)
}

/// The checkout, and the read that confirms it, with the clone to itself.
///
/// The clone's gate is held from before the look for live sessions until the
/// clone has been read back ([`SetupGuard::clone_gate`]). An agent start
/// holds the same gate from its own read of the clone until its session is
/// live, so the two can't interleave: either the start is seen here as a
/// live session, or it reads the clone after this checkout is done.
///
/// A clone a live session is working in is never touched. If it already
/// reads as on the branch there is nothing to do, and that read is the
/// verification; if not, the checkout is refused rather than run under an
/// agent, whose commits would land on the wrong branch.
pub(super) async fn check_out_in_clone(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &dyn SetupHost,
    checkout: &Checkout,
    request: &SetupRequest,
) -> Result<(), CheckoutError> {
    let failed = CheckoutError::Failed;
    // Before the gate: this can wait on the reconcile service.
    settle_disconnected_sessions(store, host, checkout, request).await;

    let _gate = guard
        .clone_gate(&checkout.workspace, checkout.clone_dir.as_deref())
        .await;
    if let Some(live) = live_session_in_clone(store, checkout, request).map_err(failed)? {
        let (workspace, expected) = (&checkout.workspace, &checkout.branch_name);
        return match verify::read_head(host, checkout).await {
            Ok(head) if verify::is_on_branch(&head, checkout) => Ok(()),
            Ok(head) => Err(CheckoutError::Blocked(format!(
                "Workspace '{workspace}' has {head} checked out, where branch '{expected}' was \
                 expected. It was not switched because {live}; try again once that session has \
                 finished."
            ))),
            Err(e) => Err(failed(verify::unverified(checkout, &e))),
        };
    }

    host.check_out(checkout).await?;

    // The checkout said it worked. `Running` rests on where the clone is,
    // so that is read, not inferred.
    verify::confirm_checked_out(host, checkout)
        .await
        .map_err(failed)
}

/// A session found working in a clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveSession {
    pub(crate) branch_name: String,
    /// `disconnected`: its turn may still be going on the workstation with
    /// no Staged process attached.
    pub(crate) disconnected: bool,
}

impl std::fmt::Display for LiveSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.disconnected {
            write!(
                f,
                "a session of branch '{}' lost its connection and may still be working in the \
                 same clone (Staged is checking on it)",
                self.branch_name
            )
        } else {
            write!(
                f,
                "a session of branch '{}' is running in the same clone",
                self.branch_name
            )
        }
    }
}

/// The sessions occupying `checkout`'s clone, besides the one the request
/// was made for: `running`, or `disconnected` with their remote turn
/// possibly still going. Each with the name of its branch.
///
/// Scoped to the clone, not the project: every remote branch whose checkout
/// is this directory on this workstation counts ([`branches_in_clone`]),
/// this branch included.
fn sessions_in_clone(
    store: &Store,
    checkout: &Checkout,
    request: &SetupRequest,
) -> Result<Vec<(String, store::Session)>, String> {
    let mut found = Vec::new();
    for branch in branches_in_clone(store, &checkout.workspace, checkout.clone_dir.as_deref())? {
        let busy = store
            .get_busy_sessions_for_branch(&branch.id)
            .map_err(|e| e.to_string())?;
        for session in busy {
            if request.session.as_deref() != Some(session.id.as_str()) {
                found.push((branch.branch_name.clone(), session));
            }
        }
    }
    Ok(found)
}

/// The first live session in `checkout`'s clone, a `running` one before a
/// `disconnected` one. A checkout under a live agent would move the commits
/// it goes on to make onto another branch.
pub(crate) fn live_session_in_clone(
    store: &Store,
    checkout: &Checkout,
    request: &SetupRequest,
) -> Result<Option<LiveSession>, String> {
    let sessions = sessions_in_clone(store, checkout, request)?;
    let live = |disconnected: bool| {
        sessions
            .iter()
            .find(|(_, session)| {
                (session.status == store::SessionStatus::Disconnected) == disconnected
            })
            .map(|(branch_name, _)| LiveSession {
                branch_name: branch_name.clone(),
                disconnected,
            })
    };
    Ok(live(false).or_else(|| live(true)))
}

/// Give the `disconnected` sessions in `checkout`'s clone a chance to be
/// settled before they count as live.
///
/// A `disconnected` row holds its place until the reconcile service has
/// asked the workstation what became of its turn, which it does on a backoff
/// of up to half an hour, so it could keep a clone from being repaired for
/// as long. Here the service is asked to look now, and the rows are given a
/// bounded time to leave `disconnected`: a turn that is over is settled
/// within seconds and stops counting; one that is still going is claimed
/// `running` and counts for what it is. A row still `disconnected` after
/// the wait is treated as live, and the service goes on settling it within
/// its own bounds.
async fn settle_disconnected_sessions(
    store: &Arc<Store>,
    host: &dyn SetupHost,
    checkout: &Checkout,
    request: &SetupRequest,
) {
    let disconnected = |store: &Store| -> Vec<String> {
        sessions_in_clone(store, checkout, request)
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, session)| session.status == store::SessionStatus::Disconnected)
            .map(|(_, session)| session.id)
            .collect()
    };
    let waiting_on = disconnected(store);
    if waiting_on.is_empty() {
        return;
    }
    for session_id in &waiting_on {
        host.check_disconnected_session(session_id);
    }
    let wait = host.disconnected_session_wait();
    let started = tokio::time::Instant::now();
    while started.elapsed() < wait.timeout {
        tokio::time::sleep(wait.interval).await;
        if disconnected(store).is_empty() {
            return;
        }
    }
    log::info!(
        "[workspace_setup] {} disconnected session(s) in the clone of '{}' on '{}' were not \
         settled within {}s",
        waiting_on.len(),
        checkout.branch_name,
        checkout.workspace,
        wait.timeout.as_secs()
    );
}
