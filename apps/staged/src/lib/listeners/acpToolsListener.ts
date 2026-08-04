/**
 * Listener for the backend's ACP tools reconcile completion.
 *
 * The backend installs/upgrades the Staged-managed ACP bridges (claude, codex)
 * in the background at launch and once a day thereafter
 * (`acp_tools_reconciler.rs`). On a fresh profile, provider discovery and any
 * doctor report are cached long before the launch pass finishes, and nothing
 * re-probes on its own — without this signal the agent picker keeps reporting
 * missing bridges that are already installed until a manual refresh or restart;
 * the daily pass likewise surfaces a freshly-published bridge version without a
 * restart. The event also fires on partial failure: the bridges that did land
 * should become selectable.
 */

import { listenToEvent, type UnlistenFn } from '../transport';
import { refreshProviders } from '../features/agents/agent.svelte';
import { doctorState, runChecks } from '../features/doctor/doctor.svelte';

/** Mirrors `ACP_TOOLS_RECONCILED_EVENT` in `acp_tools_reconciler.rs`. */
interface AcpToolsReconciledEvent {
  /** False when at least one managed bridge install failed this pass. */
  ok: boolean;
  /** Managed tool ids the reconciler handled (e.g. `claude-acp`). */
  providerIds: string[];
}

export function listenForAcpToolsReconciled(): UnlistenFn {
  return listenToEvent<AcpToolsReconciledEvent>('acp-tools-reconciled', () => {
    // Force: discover_acp_providers sits behind a 30-minute SWR cache, and
    // the pre-reconcile discovery it holds is exactly what is stale now.
    void refreshProviders({ force: true });
    // Re-run doctor checks only when a report has been loaded — running them
    // just paints doctorState for the settings panel, so there is nothing to
    // refresh before the user first opens it.
    if (doctorState.report) void runChecks();
  });
}
