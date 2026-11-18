//! Whether a workstation's clone is on the branch it belongs to.
//!
//! `Running` says two things about a remote branch: its workstation takes
//! commands, and the clone there is on the branch. Blox vouches for the
//! first. The second is read here, off the clone itself, at the two points
//! that rely on it:
//!
//! - at the end of setup, before `Running` is written
//!   ([`confirm_checked_out`]);
//! - at every start on a remote branch of something that runs git in the
//!   clone, before it is launched ([`ensure_session_checkout`]): an agent
//!   session, fresh or resumed, and a pipeline session (commit, push,
//!   rebase, squash, PR). A branch can be `Running` over a clone that has
//!   since moved: two branches of one repo share a clone, someone switches
//!   branches in a terminal.
//!
//! The read is one transient exec and no fetch. It answers through stdout
//! ([`CLONE_HEAD_PROBE_SCRIPT`]), so an exec that failed is not an answer:
//! it is never "wrong branch" (which would run a checkout the clone may not
//! need) and never "right branch" (which would start the agent unchecked).
//! [`read_head`] tries once more after a short pause and then fails.
//!
//! A start that finds the clone somewhere else repairs it rather than
//! failing: it runs the shared setup under the guard
//! ([`SetupTrigger::Repair`]) and starts once the clone reads right. The
//! repair moves the row to `Starting` for as long as it runs, so the session
//! queue's own gate holds whatever else is queued. A repair that fails
//! leaves the row in `Error`, with its reason and a retry timer; one that is
//! refused, because switching would lose or move work or because another
//! live session is working in the clone, puts the row back to `Running` and
//! fails only the session that asked.
//!
//! The read and the start it allows are one step as far as other starts and
//! checkouts in the same clone go: the caller holds the clone's gate
//! ([`CheckoutHold`]) from the read until its session row is `running`.
//! Setup takes the same gate around its look for live sessions and its
//! checkout, so it either sees this session as live and leaves the clone
//! alone, or finishes before this session reads the clone.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::OwnedMutexGuard;

use crate::branches;
use crate::store::{self, Store};

use super::clone::checkout_for;
use super::guard::SetupGuard;
use super::host::{blox_host, Checkout, SetupHost};
use super::{run_guarded, SetupRequest};

/// What a clone has checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloneHead {
    /// On this local branch.
    Branch(String),
    /// A repository whose HEAD is not on a local branch.
    Detached,
    /// No repository where the clone should be.
    NoRepo,
}

impl std::fmt::Display for CloneHead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloneHead::Branch(name) => write!(f, "branch '{name}'"),
            CloneHead::Detached => f.write_str("a detached HEAD"),
            CloneHead::NoRepo => f.write_str("no repository"),
        }
    }
}

/// Prints what the repository at `$1` has checked out: `branch <ref>`,
/// `detached`, or `norepo`.
///
/// Every outcome is a line on stdout from a script that exits 0. Asked as a
/// bare `git symbolic-ref`, a detached HEAD and a missing clone would be
/// non-zero exits, which over `ws exec` look the same as a workstation that
/// never ran the command.
pub(crate) const CLONE_HEAD_PROBE_SCRIPT: &str = r#"if ref=$(git -C "$1" symbolic-ref -q HEAD 2>/dev/null); then printf 'branch %s\n' "$ref"; elif git -C "$1" rev-parse --git-dir >/dev/null 2>&1; then echo detached; else echo norepo; fi"#;

/// Read [`CLONE_HEAD_PROBE_SCRIPT`]'s answer. Anything the script doesn't
/// print (an empty reply, a truncated one) is an error, not a guess.
pub(crate) fn parse_clone_head(output: &str) -> Result<CloneHead, String> {
    let answer = output.trim();
    if answer == "detached" {
        return Ok(CloneHead::Detached);
    }
    if answer == "norepo" {
        return Ok(CloneHead::NoRepo);
    }
    match answer.strip_prefix("branch ") {
        Some(head_ref) => Ok(match head_ref.trim().strip_prefix("refs/heads/") {
            Some(name) if !name.is_empty() => CloneHead::Branch(name.to_string()),
            // HEAD names a ref that is not a local branch.
            _ => CloneHead::Detached,
        }),
        None => Err(format!(
            "unexpected answer from the checked-out-branch probe: {answer:?}"
        )),
    }
}

/// What the clone at `clone_dir` on `workspace` has checked out: one
/// transient exec. `clone_dir` is the `home:<clone>` form, or `None` for a
/// branch with no project repo, whose git runs in the workstation's default
/// directory.
pub(crate) async fn read_clone_head(
    workspace: &str,
    clone_dir: Option<&str>,
) -> Result<CloneHead, String> {
    let dir = match clone_dir {
        Some(dir) => branches::resolve_workspace_repo_path(workspace, dir)
            .map_err(|e| format!("could not resolve workspace repo path '{dir}': {e}"))?,
        None => ".".to_string(),
    };
    let workspace = workspace.to_string();
    let output = branches::run_blox_blocking(move || {
        branches::run_workspace_shell(&workspace, CLONE_HEAD_PROBE_SCRIPT, &[&dir])
    })
    .await
    .map_err(|e| e.to_string())?;
    parse_clone_head(&output)
}

/// How [`read_head`] treats a read that doesn't answer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeadRead {
    /// How long one read may take. The exec itself gives up much later
    /// (`blox-cli`'s 300 s); a session start doesn't wait that long for one
    /// `symbolic-ref`.
    pub(crate) attempt_cap: Duration,
    /// The pause before the second, and last, read.
    pub(crate) retry_after: Duration,
}

impl HeadRead {
    pub(crate) const DEFAULT: HeadRead = HeadRead {
        attempt_cap: Duration::from_secs(30),
        retry_after: Duration::from_secs(2),
    };
}

/// Read what `checkout`'s clone is on, trying a second time when the first
/// read fails or outlives its cap.
///
/// One retry, then an error. A transient exec fails at once on a workstation
/// that has just dropped its connection and usually works a moment later, so
/// one more read saves a session from a blip. Past that the failure is
/// returned: the caller must not treat it as either answer, and a queued
/// start already gets the session queue's own bounded retries on top.
pub(crate) async fn read_head(
    host: &dyn SetupHost,
    checkout: &Checkout,
) -> Result<CloneHead, String> {
    let policy = host.head_read();
    let first = match read_head_once(host, checkout, policy.attempt_cap).await {
        Ok(head) => return Ok(head),
        Err(e) => e,
    };
    log::warn!(
        "[workspace_setup] could not read HEAD of '{}' in workspace '{}', reading once more: {first}",
        checkout.branch_name,
        checkout.workspace
    );
    tokio::time::sleep(policy.retry_after).await;
    read_head_once(host, checkout, policy.attempt_cap).await
}

async fn read_head_once(
    host: &dyn SetupHost,
    checkout: &Checkout,
    cap: Duration,
) -> Result<CloneHead, String> {
    // A read that outlives the cap is dropped. It only reads, so there is
    // nothing to undo; the exec finishes on its blocking thread and its
    // answer is discarded.
    match tokio::time::timeout(cap, host.clone_head(checkout)).await {
        Ok(result) => result,
        Err(_) => Err(format!("no answer within {}s", cap.as_secs())),
    }
}

pub(super) fn is_on_branch(head: &CloneHead, checkout: &Checkout) -> bool {
    matches!(head, CloneHead::Branch(name) if *name == checkout.branch_name)
}

pub(super) fn unverified(checkout: &Checkout, error: &str) -> String {
    format!(
        "Could not verify that branch '{}' is checked out in workspace '{}': {error}",
        checkout.branch_name, checkout.workspace
    )
}

/// The last step of setup: the clone must read as on the branch before the
/// row may say `Running`. A checkout that reported success and left the
/// clone elsewhere is a setup failure, and so is a read that never answers.
pub(crate) async fn confirm_checked_out(
    host: &dyn SetupHost,
    checkout: &Checkout,
) -> Result<(), String> {
    match read_head(host, checkout).await {
        Ok(head) if is_on_branch(&head, checkout) => Ok(()),
        Ok(head) => Err(format!(
            "Workspace '{}' has {head} checked out after setup, where branch '{}' was expected",
            checkout.workspace, checkout.branch_name
        )),
        Err(e) => Err(unverified(checkout, &e)),
    }
}

/// The clone's gate, held by a start that has read the clone as on its
/// branch. While it is held no checkout runs in that clone and no other
/// start reads it.
///
/// Keep it until the session's row is `running`, which is what setup looks
/// for before it touches a clone, and drop it then. Nothing may ask for
/// setup while holding one ([`SetupGuard::clone_gate`]'s lock order).
#[must_use = "the clone is only held while this is alive; bind it until the session row is running"]
pub(crate) struct CheckoutHold(#[allow(dead_code)] Option<OwnedMutexGuard<()>>);

impl CheckoutHold {
    /// For a start with no remote clone to hold: a local branch.
    pub(crate) fn none() -> Self {
        CheckoutHold(None)
    }
}

/// Make sure `branch`'s clone is on the branch before `session_id` starts
/// running git in it. A no-op for a local branch.
///
/// Call it before anything else reads the clone for the session (the branch
/// context in its prompt, its pre-run HEAD) and before the row is claimed:
/// it can wait on a repair. Bind what it returns until the row is `running`
/// ([`CheckoutHold`]).
pub(crate) async fn ensure_session_checkout<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    store: &Arc<Store>,
    branch: &store::Branch,
    session_id: &str,
) -> Result<CheckoutHold, String> {
    if branch.branch_type != store::BranchType::Remote || branch.workspace_name.is_none() {
        return Ok(CheckoutHold::none());
    }
    let host = blox_host(app_handle, store);
    verify_session_checkout(SetupGuard::global(), store, &host, branch, session_id).await
}

/// [`ensure_session_checkout`] for a remote branch, against any host.
///
/// Returns `Ok` only after a read, made with the clone's gate held, that
/// says the clone is on the branch: the first one, or the one after a
/// repair.
pub(crate) async fn verify_session_checkout(
    guard: &Arc<SetupGuard>,
    store: &Arc<Store>,
    host: &Arc<dyn SetupHost>,
    branch: &store::Branch,
    session_id: &str,
) -> Result<CheckoutHold, String> {
    let checkout = checkout_for(store, branch)?;
    let clone_dir = checkout.clone_dir.as_deref();
    let (workspace, expected) = (&checkout.workspace, &checkout.branch_name);

    let found = {
        let gate = guard.clone_gate(workspace, clone_dir).await;
        match read_head(host.as_ref(), &checkout).await {
            Ok(head) if is_on_branch(&head, &checkout) => return Ok(CheckoutHold(Some(gate))),
            Ok(head) => head,
            Err(e) => return Err(unverified(&checkout, &e)),
        }
        // The gate is let go here: the repair takes it for its checkout.
    };
    log::warn!(
        "[workspace_setup] session {session_id}: workspace '{workspace}' has {found} checked \
         out, where branch '{expected}' was expected; checking the branch out before the \
         session starts"
    );

    let repair = SetupRequest::repair_for(session_id);
    run_guarded(guard, store, host, &branch.id, repair)
        .await
        .map_err(|e| {
            format!(
                "Workspace '{workspace}' had {found} checked out, where this session's branch \
             '{expected}' was expected, and it could not be switched: {e}"
            )
        })?;

    // The repair's own word is not taken for it: a run this one joined, or a
    // row that had left `Running` and so had nothing to repair, says nothing
    // about where the clone is now. Neither does a repair that did its work:
    // the gate was free between its checkout and this read.
    let gate = guard.clone_gate(workspace, clone_dir).await;
    match read_head(host.as_ref(), &checkout).await {
        Ok(head) if is_on_branch(&head, &checkout) => Ok(CheckoutHold(Some(gate))),
        Ok(head) => Err(format!(
            "Workspace '{workspace}' has {head} checked out, where this session's branch \
             '{expected}' was expected"
        )),
        Err(e) => Err(unverified(&checkout, &e)),
    }
}
