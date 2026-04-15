/**
 * Centralized PR status polling service.
 *
 * Replaces the per-component setInterval polling in BranchCardPrButton with a
 * single coordinated timer that calls `refreshAllPrStatuses` on the backend.
 * The backend already emits per-branch `pr-status-changed` events, so
 * components only need to listen for those events — no per-branch IPC calls.
 */

import { refreshAllPrStatuses } from '../commands';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface TrackedBranch {
  projectId: string;
  hasPendingChecks: boolean;
}

type StaleCallback = (branchId: string, isStale: boolean) => void;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/** Branches currently being tracked, keyed by branchId. */
const tracked = new Map<string, TrackedBranch>();

/** Consecutive failure count per projectId. */
const failures = new Map<string, number>();

/** Registered stale-data callbacks. */
const staleCallbacks = new Set<StaleCallback>();

let timerId: ReturnType<typeof setTimeout> | null = null;
let refreshInFlight = false;
let windowFocused = true;

// Intervals
const PENDING_INTERVAL = 15_000;
const NORMAL_INTERVAL = 60_000;
const MAX_CONSECUTIVE_FAILURES = 3;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

function getInterval(): number {
  for (const entry of tracked.values()) {
    if (entry.hasPendingChecks) return PENDING_INTERVAL;
  }
  return NORMAL_INTERVAL;
}

/** Collect the distinct projectIds that are currently tracked. */
function trackedProjectIds(): Set<string> {
  const ids = new Set<string>();
  for (const entry of tracked.values()) {
    ids.add(entry.projectId);
  }
  return ids;
}

async function poll() {
  if (refreshInFlight || !windowFocused || tracked.size === 0) {
    console.info(
      `[PrPolling] poll skipped: refreshInFlight=${refreshInFlight}, windowFocused=${windowFocused}, tracked=${tracked.size}`
    );
    scheduleNext();
    return;
  }

  refreshInFlight = true;
  const projectIds = trackedProjectIds();
  console.info(`[PrPolling] poll start: projectIds=[${[...projectIds]}], tracked=${tracked.size}`);

  for (const projectId of projectIds) {
    try {
      await refreshAllPrStatuses(projectId);
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
  console.info(`[PrPolling] poll end`);
  scheduleNext();
}

function scheduleNext() {
  stopTimer();
  if (tracked.size === 0 || !windowFocused) {
    console.info(
      `[PrPolling] scheduleNext: not scheduling (tracked=${tracked.size}, windowFocused=${windowFocused})`
    );
    return;
  }
  const interval = getInterval();
  console.info(`[PrPolling] scheduleNext: interval=${interval}ms, tracked=${tracked.size}`);
  timerId = setTimeout(poll, interval);
}

function stopTimer() {
  if (timerId !== null) {
    clearTimeout(timerId);
    timerId = null;
  }
}

function notifyStale(projectId: string, isStale: boolean) {
  for (const [branchId, entry] of tracked) {
    if (entry.projectId === projectId) {
      for (const cb of staleCallbacks) {
        try {
          cb(branchId, isStale);
        } catch {
          // ignore callback errors
        }
      }
    }
  }
}

// ---------------------------------------------------------------------------
// Focus / blur handlers
// ---------------------------------------------------------------------------

function handleFocus() {
  console.info(`[PrPolling] window focus`);
  windowFocused = true;
  // Immediate refresh on focus, then resume schedule
  poll();
}

function handleBlur() {
  console.info(`[PrPolling] window blur`);
  windowFocused = false;
  stopTimer();
}

let listenersAttached = false;

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

/** Start tracking a branch for PR status polling. */
export function track(branchId: string, projectId: string, hasPendingChecks = false): void {
  const isFirst = tracked.size === 0;
  tracked.set(branchId, { projectId, hasPendingChecks });
  console.info(
    `[PrPolling] track: branch=${branchId}, project=${projectId}, pendingChecks=${hasPendingChecks}, isFirst=${isFirst}, totalTracked=${tracked.size}`
  );

  if (isFirst) {
    ensureWindowListeners();
    // Start polling immediately
    poll();
  } else {
    // Interval may have changed — reschedule
    scheduleNext();
  }
}

/** Stop tracking a branch. */
export function untrack(branchId: string): void {
  const existed = tracked.has(branchId);
  tracked.delete(branchId);
  console.info(
    `[PrPolling] untrack: branch=${branchId}, existed=${existed}, remaining=${tracked.size}`
  );
  if (tracked.size === 0) {
    stopTimer();
    removeWindowListeners();
    failures.clear();
  }
}

/** Update whether a tracked branch has pending checks (affects poll interval). */
export function updateChecksStatus(branchId: string, hasPendingChecks: boolean): void {
  const entry = tracked.get(branchId);
  if (entry && entry.hasPendingChecks !== hasPendingChecks) {
    entry.hasPendingChecks = hasPendingChecks;
    // Interval may have changed — reschedule
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
    console.info(`[PrPolling] refreshNow: DROPPED for project=${projectId} (refreshInFlight=true)`);
    return;
  }
  console.info(`[PrPolling] refreshNow: accepted for project=${projectId}`);
  refreshInFlight = true;
  refreshAllPrStatuses(projectId)
    .catch((e) =>
      console.error(`[PrPollingService] immediate refresh failed for project=${projectId}:`, e)
    )
    .finally(() => {
      refreshInFlight = false;
      scheduleNext();
    });
}
