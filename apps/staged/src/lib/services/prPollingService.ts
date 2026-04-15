/**
 * Centralized PR status polling service.
 *
 * Polls all projects app-wide. The selected project polls more frequently
 * than background projects, and projects with pending CI checks poll fastest.
 *
 * The backend's `refreshAllPrStatuses` already emits per-branch
 * `pr-status-changed` events, so components only need to listen for those.
 */

import { refreshAllPrStatuses } from '../commands';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type StaleCallback = (projectId: string, isStale: boolean) => void;

// ---------------------------------------------------------------------------
// Intervals
// ---------------------------------------------------------------------------

const PENDING_INTERVAL = 15_000; // any project with pending CI checks
const SELECTED_INTERVAL = 60_000; // selected project, no pending checks
const BACKGROUND_INTERVAL = 5 * 60_000; // non-selected, no pending checks
const MAX_CONSECUTIVE_FAILURES = 3;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/** All project IDs to poll. */
const allProjectIds = new Set<string>();

/** Currently selected (viewed) project. */
let selectedProjectId: string | null = null;

/** Branches with pending checks, keyed by branchId → projectId. */
const pendingBranches = new Map<string, string>();

/** When each project was last successfully polled. */
const lastPolledAt = new Map<string, number>();

/** Consecutive failure count per projectId. */
const failures = new Map<string, number>();

/** Registered stale-data callbacks. */
const staleCallbacks = new Set<StaleCallback>();

let timerId: ReturnType<typeof setTimeout> | null = null;
let refreshInFlight = false;
let windowFocused = true;
let listenersAttached = false;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

function projectHasPendingChecks(projectId: string): boolean {
  for (const pId of pendingBranches.values()) {
    if (pId === projectId) return true;
  }
  return false;
}

function getProjectInterval(projectId: string): number {
  if (projectHasPendingChecks(projectId)) return PENDING_INTERVAL;
  if (projectId === selectedProjectId) return SELECTED_INTERVAL;
  return BACKGROUND_INTERVAL;
}

/** Return project IDs whose polling interval has elapsed. */
function getProjectsDue(): string[] {
  const now = Date.now();
  const due: string[] = [];
  for (const projectId of allProjectIds) {
    const interval = getProjectInterval(projectId);
    const last = lastPolledAt.get(projectId) ?? 0;
    if (now - last >= interval) {
      due.push(projectId);
    }
  }
  return due;
}

async function poll() {
  if (refreshInFlight || !windowFocused || allProjectIds.size === 0) {
    scheduleNext();
    return;
  }

  refreshInFlight = true;
  const due = getProjectsDue();

  for (const projectId of due) {
    try {
      await refreshAllPrStatuses(projectId);
      lastPolledAt.set(projectId, Date.now());
      // Reset failure counter on success
      const prev = failures.get(projectId) ?? 0;
      if (prev > 0) {
        failures.set(projectId, 0);
        notifyStale(projectId, false);
      }
    } catch (e) {
      const count = (failures.get(projectId) ?? 0) + 1;
      failures.set(projectId, count);
      console.error(
        `[PrPollingService] refreshAllPrStatuses failed for project=${projectId} (attempt ${count}):`,
        e
      );
      if (count >= MAX_CONSECUTIVE_FAILURES) {
        notifyStale(projectId, true);
      }
    }
  }

  refreshInFlight = false;
  scheduleNext();
}

function scheduleNext() {
  stopTimer();
  if (allProjectIds.size === 0 || !windowFocused) return;

  const now = Date.now();
  let minDelay = Infinity;
  for (const projectId of allProjectIds) {
    const interval = getProjectInterval(projectId);
    const last = lastPolledAt.get(projectId) ?? 0;
    const remaining = Math.max(0, interval - (now - last));
    minDelay = Math.min(minDelay, remaining);
  }

  if (!Number.isFinite(minDelay)) return;
  // Floor at 1s to avoid tight loops
  timerId = setTimeout(poll, Math.max(1_000, minDelay));
}

function stopTimer() {
  if (timerId !== null) {
    clearTimeout(timerId);
    timerId = null;
  }
}

function notifyStale(projectId: string, isStale: boolean) {
  for (const cb of staleCallbacks) {
    try {
      cb(projectId, isStale);
    } catch {
      // ignore callback errors
    }
  }
}

// ---------------------------------------------------------------------------
// Focus / blur handlers
// ---------------------------------------------------------------------------

function handleFocus() {
  windowFocused = true;
  poll();
}

function handleBlur() {
  windowFocused = false;
  stopTimer();
}

function ensureWindowListeners() {
  if (listenersAttached) return;
  window.addEventListener('focus', handleFocus);
  window.addEventListener('blur', handleBlur);
  listenersAttached = true;
}

function removeWindowListeners() {
  if (!listenersAttached) return;
  window.removeEventListener('focus', handleFocus);
  window.removeEventListener('blur', handleBlur);
  listenersAttached = false;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Set the full list of project IDs to poll. Starts/stops polling as needed. */
export function setProjects(projectIds: string[]): void {
  const newIds = new Set(projectIds);

  // Remove projects no longer in the list
  for (const id of allProjectIds) {
    if (!newIds.has(id)) {
      allProjectIds.delete(id);
      lastPolledAt.delete(id);
      failures.delete(id);
    }
  }

  // Clean up pending branches for removed projects
  for (const [branchId, projectId] of pendingBranches) {
    if (!newIds.has(projectId)) {
      pendingBranches.delete(branchId);
    }
  }

  // Add new projects
  for (const id of newIds) {
    allProjectIds.add(id);
  }

  if (allProjectIds.size > 0) {
    ensureWindowListeners();
    // Trigger poll — new projects have no lastPolledAt so they'll be due
    poll();
  } else {
    stopTimer();
    removeWindowListeners();
    failures.clear();
  }
}

/** Set the currently selected project (polls more frequently). */
export function setSelectedProject(projectId: string | null): void {
  if (selectedProjectId === projectId) return;
  selectedProjectId = projectId;
  if (projectId && allProjectIds.has(projectId)) {
    // Selected project's interval just changed — trigger a poll if it's due
    poll();
  } else {
    scheduleNext();
  }
}

/** Update whether a branch has pending CI checks (affects its project's poll interval). */
export function updateChecksStatus(
  branchId: string,
  projectId: string,
  hasPendingChecks: boolean
): void {
  const hadPending = pendingBranches.has(branchId);
  if (hasPendingChecks) {
    pendingBranches.set(branchId, projectId);
  } else {
    pendingBranches.delete(branchId);
  }
  if (hadPending !== hasPendingChecks) {
    scheduleNext();
  }
}

/** Register a callback for stale-data notifications. Returns an unsubscribe function. */
export function onStale(callback: StaleCallback): () => void {
  staleCallbacks.add(callback);
  return () => staleCallbacks.delete(callback);
}

/** Trigger an immediate refresh for a specific project (e.g. after PR creation or push). */
export function refreshNow(projectId: string): void {
  if (refreshInFlight) {
    return;
  }
  refreshInFlight = true;
  refreshAllPrStatuses(projectId)
    .then(() => {
      lastPolledAt.set(projectId, Date.now());
      // Reset failure counter on success
      const prev = failures.get(projectId) ?? 0;
      if (prev > 0) {
        failures.set(projectId, 0);
        notifyStale(projectId, false);
      }
    })
    .catch((e) =>
      console.error(`[PrPollingService] immediate refresh failed for project=${projectId}:`, e)
    )
    .finally(() => {
      refreshInFlight = false;
      scheduleNext();
    });
}
