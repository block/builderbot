//! Bringing a workstation up for a branch: `sq blox ws start`, and the
//! bounded wait for one that is on its way.

use std::sync::Arc;

use crate::blox::BloxError;
use crate::branches;
use crate::store::{self, Store, WorkspaceStatus};

use super::host::{reports_running, Checkout, SetupHost};

/// `sq blox ws start`, and when that fails without ruling the workstation
/// out, a bounded wait for it to come up anyway.
pub(super) async fn start_workstation(
    store: &Arc<Store>,
    host: &dyn SetupHost,
    branch: &store::Branch,
    checkout: &Checkout,
) -> Result<(), String> {
    let workspace = checkout.workspace.as_str();
    let source = format!(
        "https://github.com/{}.git?ref={}",
        checkout.repo_slug, checkout.base_ref
    );
    log::info!(
        "[workspace_setup] branch={} workspace={workspace} starting from {source}",
        branch.id
    );
    let start_error = match host.start_workspace(workspace, &source).await {
        Ok(()) => return Ok(()),
        Err(BloxError::NotAuthenticated) => {
            return Err("Not authenticated with Blox. Run: sq login".to_string());
        }
        // Definitive: keep the CLI's text, which carries the onboarding URL
        // and the action it wants.
        Err(e) if branches::is_blox_onboarding_precondition_error(&e) => {
            return Err(e.to_string());
        }
        Err(e) => e,
    };

    // `ws start` can fail — its own 120 s limit, a dropped connection — after
    // Blox created the workstation, which then keeps booting. Returning here
    // would leave the branch with a running workstation and no checkout, so
    // wait for it and carry on, or fail where the user can see it.
    log::warn!(
        "[workspace_setup] branch={} workspace={workspace} ws start failed, waiting for the workstation: {start_error}",
        branch.id
    );
    wait_until_running(
        store,
        host,
        branch,
        workspace,
        &format!("`sq blox ws start` failed: {start_error}"),
    )
    .await
}

/// Wait, within [`SetupHost::ready_wait`], for the workstation to report
/// running. `why` says what the wait was for, for the error when it never
/// does.
pub(super) async fn wait_until_running(
    store: &Arc<Store>,
    host: &dyn SetupHost,
    branch: &store::Branch,
    workspace: &str,
    why: &str,
) -> Result<(), String> {
    let wait = host.ready_wait();
    let started = tokio::time::Instant::now();
    loop {
        match host.workspace_status(workspace).await {
            Ok(status) if reports_running(status.as_deref()) => return Ok(()),
            Ok(status)
                if branches::map_blox_status_to_workspace_status(
                    status.as_deref(),
                    Some(&WorkspaceStatus::Starting),
                ) == WorkspaceStatus::Error =>
            {
                return Err(format!(
                    "Workspace '{workspace}' failed to start (Blox reports it as {}); {why}",
                    status.as_deref().unwrap_or("error")
                ));
            }
            Err(BloxError::NotAuthenticated) => {
                return Err("Not authenticated with Blox. Run: sq login".to_string());
            }
            // Still booting, or not listed yet.
            Ok(_) | Err(_) => {}
        }
        // Deleted while it waited: nothing left to set up.
        if store.get_branch(&branch.id).ok().flatten().is_none() {
            return Err(format!("Branch not found: {}", branch.id));
        }
        if started.elapsed() >= wait.timeout {
            return Err(format!(
                "Workspace '{workspace}' did not come up within {}s; {why}",
                wait.timeout.as_secs()
            ));
        }
        tokio::time::sleep(wait.interval).await;
    }
}
