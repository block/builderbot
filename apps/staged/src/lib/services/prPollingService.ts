/**
 * PR status polling — frontend interest/hint layer.
 *
 * The backend owns PR-polling cadence and concurrency (see
 * `src-tauri/src/pr_poll_scheduler.rs`): it derives the project list from the
 * DB, decides what is due across the pending/selected/background tiers, dedups
 * in-flight work, and backs off failures — all on a single bounded pool.
 *
 * This module is now a thin shim that:
 *   - forwards UI interest to the backend as hints (selected project, pending
 *     checks, window focus, explicit refresh nudges); and
 *   - re-broadcasts the backend's per-project refresh/stale lifecycle events to
 *     local subscribers, preserving the `onRefreshing` / `onStale` /
 *     `isRefreshing` API that `BranchCardPrButton` relies on.
 *
 * The backend already emits per-branch `pr-status-changed` and a final
 * `pr-statuses-refreshed`; components subscribe to those directly.
 */

import { isTauri, listenToEvent, type UnlistenFn } from '../transport';
import {
  setForegroundProject,
  setPrPollFocus,
  setBranchPending,
  refreshPrStatusesNow,
  disconnectPrPollClient,
} from '../commands';

// ---------------------------------------------------------------------------
// Client identity
// ---------------------------------------------------------------------------
//
// The backend tracks PR-poll interest per connected client and unions the
// cadence across them (see src-tauri/src/pr_poll_scheduler.rs). This module
// owns a stable id for the lifetime of the page that is threaded through every
// interest hint.
//
//   - Native (Tauri): the fixed well-known id `tauri-main` (must match
//     `TAURI_CLIENT_ID` in pr_poll_scheduler.rs), so single-client behaviour is
//     identical to before per-client interest existed.
//   - Web (browser): a fresh UUID per page load (per tab). The same value must
//     be appended to the WS connect URL (`?clientId=<id>`) by the web transport
//     so the backend correlates this client's interest (invoke channel) with
//     its disconnect (WS close). See `getPrPollClientId`.

const TAURI_CLIENT_ID = 'tauri-main';

const clientId: string = isTauri ? TAURI_CLIENT_ID : crypto.randomUUID();

/**
 * This client's PR-poll id, stable for the page's lifetime. Exposed so the web
 * transport can append the *same* id to the events WebSocket connect query
 * (`?clientId=<id>`) — the backend↔frontend contract requires the same id on
 * both the invoke and WS channels.
 */
export function getPrPollClientId(): string {
  return clientId;
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type StaleCallback = (projectId: string, isStale: boolean) => void;
type RefreshingCallback = (projectId: string, isRefreshing: boolean) => void;

interface PendingBranchHint {
  branchId: string;
  projectId: string;
}

interface PrRefreshStateEvent {
  projectId: string;
  refreshing: boolean;
}

interface PrStaleEvent {
  projectId: string;
  stale: boolean;
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/** Registered stale-data callbacks. */
const staleCallbacks = new Set<StaleCallback>();

/** Registered refresh-state callbacks. */
const refreshingCallbacks = new Set<RefreshingCallback>();

/** Projects the backend currently reports as refreshing. */
const refreshingProjects = new Set<string>();

let selectedProjectId: string | null = null;
const pendingBranchHints = new Map<string, PendingBranchHint>();

let initialized = false;
let unlistenRefreshState: UnlistenFn | null = null;
let unlistenStale: UnlistenFn | null = null;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

function notifyStale(projectId: string, isStale: boolean) {
  for (const cb of staleCallbacks) {
    try {
      cb(projectId, isStale);
    } catch {
      // ignore callback errors
    }
  }
}

function notifyRefreshing(projectId: string, isRefreshing: boolean) {
  for (const cb of refreshingCallbacks) {
    try {
      cb(projectId, isRefreshing);
    } catch {
      // ignore callback errors
    }
  }
}

function setProjectRefreshing(projectId: string, isRefreshing: boolean) {
  const wasRefreshing = refreshingProjects.has(projectId);
  if (isRefreshing) {
    refreshingProjects.add(projectId);
  } else {
    refreshingProjects.delete(projectId);
  }
  if (wasRefreshing !== isRefreshing) {
    notifyRefreshing(projectId, isRefreshing);
  }
}

// ---------------------------------------------------------------------------
// Focus / blur handlers
// ---------------------------------------------------------------------------

function handleFocus() {
  void setPrPollFocus(clientId, true).catch(() => {});
}

function handleBlur() {
  void setPrPollFocus(clientId, false).catch(() => {});
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/**
 * Wire up the interest/hint layer: forward window focus to the backend and
 * subscribe to its refresh/stale lifecycle events. Idempotent; call once at app
 * start.
 */
export function init(): void {
  if (initialized) return;
  initialized = true;

  window.addEventListener('focus', handleFocus);
  window.addEventListener('blur', handleBlur);
  // Seed the backend with the current focus state (it defaults to focused, so
  // the initial poll already ran; this corrects it if we launched unfocused).
  void setPrPollFocus(clientId, document.hasFocus()).catch(() => {});

  unlistenRefreshState = listenToEvent<PrRefreshStateEvent>('pr-refresh-state', (payload) => {
    setProjectRefreshing(payload.projectId, payload.refreshing);
  });
  unlistenStale = listenToEvent<PrStaleEvent>('pr-status-stale', (payload) => {
    notifyStale(payload.projectId, payload.stale);
  });
}

/** Tear down listeners and subscriptions. */
export function dispose(): void {
  if (!initialized) return;
  initialized = false;

  window.removeEventListener('focus', handleFocus);
  window.removeEventListener('blur', handleBlur);
  unlistenRefreshState?.();
  unlistenStale?.();
  unlistenRefreshState = null;
  unlistenStale = null;

  // Drop this client's interest so the backend recomputes the union without it.
  void disconnectPrPollClient(clientId).catch(() => {});
  selectedProjectId = null;
  pendingBranchHints.clear();

  for (const projectId of [...refreshingProjects]) {
    setProjectRefreshing(projectId, false);
  }
}

// ---------------------------------------------------------------------------
// Public API — interest hints (forwarded to the backend scheduler)
// ---------------------------------------------------------------------------

/** Set the currently selected project (polls more frequently). */
export function setSelectedProject(projectId: string | null): void {
  selectedProjectId = projectId;
  void setForegroundProject(clientId, projectId).catch((e) =>
    console.error('[PrPollingService] set_foreground_project failed:', e)
  );
}

/** Update whether a branch has pending CI checks (affects its project's poll interval). */
export function updateChecksStatus(
  branchId: string,
  projectId: string,
  hasPendingChecks: boolean
): void {
  if (hasPendingChecks) {
    pendingBranchHints.set(branchId, { branchId, projectId });
  } else {
    pendingBranchHints.delete(branchId);
  }
  void setBranchPending(clientId, branchId, projectId, hasPendingChecks).catch((e) =>
    console.error('[PrPollingService] set_branch_pending failed:', e)
  );
}

/**
 * Re-send this client's current interest after a browser WebSocket reconnect.
 * The backend drops all interest on clean WS close, then recreates an empty
 * client when the same id reconnects.
 */
export async function replayPrPollInterestHints(): Promise<void> {
  if (!initialized) return;

  await Promise.all([
    setPrPollFocus(clientId, document.hasFocus()),
    setForegroundProject(clientId, selectedProjectId),
    ...Array.from(pendingBranchHints.values(), ({ branchId, projectId }) =>
      setBranchPending(clientId, branchId, projectId, true)
    ),
  ]);
}

/** Trigger an immediate refresh for a specific project (e.g. after PR creation or push). */
export function refreshNow(projectId: string): void {
  void refreshPrStatusesNow(clientId, projectId).catch((e) =>
    console.error(`[PrPollingService] refresh_now failed for project=${projectId}:`, e)
  );
}

// ---------------------------------------------------------------------------
// Public API — UI state subscriptions (driven by backend lifecycle events)
// ---------------------------------------------------------------------------

/** Register a callback for stale-data notifications. Returns an unsubscribe function. */
export function onStale(callback: StaleCallback): () => void {
  staleCallbacks.add(callback);
  return () => staleCallbacks.delete(callback);
}

/** Register a callback for PR refresh-state notifications. Returns an unsubscribe function. */
export function onRefreshing(callback: RefreshingCallback): () => void {
  refreshingCallbacks.add(callback);
  for (const projectId of refreshingProjects) {
    callback(projectId, true);
  }
  return () => refreshingCallbacks.delete(callback);
}

export function isRefreshing(projectId: string): boolean {
  return refreshingProjects.has(projectId);
}

// ---------------------------------------------------------------------------
// PR recovery coordination (frontend-only dedup, unchanged)
// ---------------------------------------------------------------------------

/** Branch IDs for which recovery has already been attempted (or is in progress). */
const recoveryAttempted = new Set<string>();

/**
 * Guard for PR recovery: returns true if recovery should proceed for this
 * branch, false if it has already been attempted or is in progress.
 * Prevents N concurrent `gh pr view` CLI calls when many BranchCardPrButton
 * components mount simultaneously for branches without PR numbers.
 */
export function shouldAttemptRecovery(branchId: string): boolean {
  if (recoveryAttempted.has(branchId)) return false;
  recoveryAttempted.add(branchId);
  return true;
}

/**
 * Clear the recovery guard for a branch so it can be retried.
 * Call this when recovery fails (e.g. network error) so a transient
 * failure doesn't permanently prevent recovery for that branch.
 */
export function clearRecoveryAttempt(branchId: string): void {
  recoveryAttempted.delete(branchId);
}
