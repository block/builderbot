//! Reconciler for the Staged-managed ACP bridges.
//!
//! Spawned from app setup: installs or upgrades every managed bridge
//! ([`crate::managed_acp_tools::MANAGED_TOOLS`]) to the latest published
//! version on launch, and then re-runs once a day for the lifetime of the
//! process, so a new bridge release ships to users the next time Staged
//! starts *and* a Staged instance left running for days keeps its private
//! npm packages current without a restart. Each install runs a floating
//! `npm install <pkg>@latest` onto the Staged-managed Node runtime in
//! `~/.staged/packages`. Failures are logged, recorded in `state.json`, and
//! retried on the next daily pass or launch; a previously installed version
//! keeps working in the meantime, so an offline launch never removes a
//! working bridge. Superseded managed Node runtimes
//! are pruned only in the epilogue of a fully-successful run — every bridge
//! shim execs its Node by absolute versioned path, so an old runtime must
//! outlive the last shim that references it.
//!
//! Silent when there is nothing to manage: the `STAGED_ACP_TOOLS_DIR` dev
//! override is active, the `no-managed-acp-tools` build feature is set, or
//! the target is unsupported.
//!
//! Completion is broadcast to the renderer as [`ACP_TOOLS_RECONCILED_EVENT`]:
//! on a fresh profile the frontend caches its doctor report and provider
//! discovery (bridges missing) long before the reconciler finishes
//! downloading Node and installing the bridges, and nothing re-probes on its
//! own — without a signal the agent picker keeps reporting missing bridges
//! that are already installed until the user manually refreshes or restarts.

use std::time::Duration;

use tauri::AppHandle;

use crate::managed_acp_tools;

/// Emitted after every reconcile pass finishes, successful or not — a partial
/// failure still installs the other bridge, so the renderer should re-probe
/// either way. Mirrored in `src/lib/listeners/acpToolsListener.ts`.
pub const ACP_TOOLS_RECONCILED_EVENT: &str = "acp-tools-reconciled";

/// Re-run cadence beyond the launch pass. The bridges float to `@latest`, so a
/// daily sweep keeps a long-running Staged instance's private npm packages
/// current without hammering the registry.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AcpToolsReconciledPayload {
    ok: bool,
    /// Managed tool ids (`claude-acp`, `codex-acp`).
    provider_ids: Vec<&'static str>,
}

pub fn spawn_reconcile_loop(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        reconcile_loop(app).await;
    });
}

/// Reconcile at launch, then once a day for the lifetime of the process.
async fn reconcile_loop(app: AppHandle) {
    // Nothing to manage on this build/target/override — don't spin a daily
    // timer that would only no-op. `reconcile` re-checks the same predicate,
    // so a build that does manage bridges is unaffected.
    if managed_acp_tools::managed_tools().is_empty() {
        return;
    }

    let mut interval = tokio::time::interval(RECONCILE_INTERVAL);
    // The first `interval.tick()` resolves immediately, so the launch pass runs
    // with no delay. `Skip` collapses the catch-up burst after the machine
    // wakes from a multi-day sleep into a single reconcile rather than one per
    // missed day.
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        reconcile(app.clone()).await;
    }
}

async fn reconcile(app: AppHandle) {
    let tools = managed_acp_tools::managed_tools();
    if tools.is_empty() {
        return;
    }

    let mut errors = Vec::new();
    for tool in &tools {
        let log_prefix = format!("[acp-tools reconcile {}]", tool.id);
        let on_line = |line: &str| log::info!("{log_prefix} {line}");
        match managed_acp_tools::install_managed_tool(tool.id, &on_line).await {
            Ok(()) => log::info!("{log_prefix} {} is up to date", tool.package),
            Err(error) => {
                log::warn!("{log_prefix} install failed (will retry next launch): {error}");
                errors.push(format!("{}: {error}", tool.id));
            }
        }
    }
    let ok = errors.is_empty();
    managed_acp_tools::finish_reconcile(&tools, errors).await;

    // Through `emit_to_all` rather than a bare Tauri emit so web-mode
    // browser clients get the refresh signal over the WebSocket fanout too.
    crate::web_server::emit_to_all(
        &app,
        ACP_TOOLS_RECONCILED_EVENT,
        AcpToolsReconciledPayload {
            ok,
            provider_ids: tools.iter().map(|tool| tool.id).collect(),
        },
    );
}
