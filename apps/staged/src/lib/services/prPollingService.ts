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
type RefreshingCallback = (projectId: string, isRefreshing: boolean) => void;

// ---------------------------------------------------------------------------
// Intervals
// ---------------------------------------------------------------------------

const PENDING_INTERVAL = 15_000; // any project with pending CI checks
const SELECTED_INTERVAL = 60_000; // selected project, no pending checks
const BACKGROUND_INTERVAL = 5 * 60_000; // non-selected, no pending checks
const MAX_CONSECUTIVE_FAILURES = 3;
// After a project switch, hold background-tier refreshes for a beat so the
// switch's reactive work isn't competing with a background poll cycle for the
// main thread. Selected/pending tiers still poll during the cooldown.
const SWITCH_COOLDOWN = 1_500;

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

/** Registered refresh-state callbacks. */
const refreshingCallbacks = new Set<RefreshingCallback>();

/** Projects currently being refreshed. */
const refreshingProjects = new Set<string>();

let timerId: ReturnType<typeof setTimeout> | null = null;
let refreshInFlight = false;
let windowFocused = true;
let listenersAttached = false;

/** Background-tier polling is deprioritized until this timestamp (see SWITCH_COOLDOWN). */
let switchCooldownUntil = 0;

/** Project IDs queued for immediate refresh while another refresh is in-flight. */
const pendingRefreshProjectIds = new Set<string>();

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

/** Yield to the macrotask queue so foreground work can interleave between projects. */
function yieldToEventLoop(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
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
    // Don't reschedule here — the in-flight operation's `finally` block
    // already calls scheduleNext(), and the other two cases (unfocused /
    // empty) intentionally have no timer running.
    return;
  }

  refreshInFlight = true;
  const due = getProjectsDue();
  // Right after a switch, hold background-tier projects so the switch's
  // reactive work isn't competing with a background poll cycle. They stay due
  // and poll on the next cycle once the cooldown elapses.
  const inSwitchCooldown = Date.now() < switchCooldownUntil;

  for (const projectId of due) {
    if (inSwitchCooldown && getProjectInterval(projectId) === BACKGROUND_INTERVAL) {
      continue;
    }
    setProjectRefreshing(projectId, true);
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
      if (count === MAX_CONSECUTIVE_FAILURES) {
        notifyStale(projectId, true);
      }
    } finally {
      setProjectRefreshing(projectId, false);
    }
    // Yield between projects so a project switch's reactive flush can interleave
    // instead of waiting out the whole serial chain of IPC round-trips.
    await yieldToEventLoop();
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

  // Short-circuit if the set of project IDs hasn't changed
  if (newIds.size === allProjectIds.size && projectIds.every((id) => allProjectIds.has(id))) {
    return;
  }

  // Remove projects no longer in the list
  for (const id of allProjectIds) {
    if (!newIds.has(id)) {
      allProjectIds.delete(id);
      lastPolledAt.delete(id);
      failures.delete(id);
      setProjectRefreshing(id, false);
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
    for (const projectId of [...refreshingProjects]) {
      setProjectRefreshing(projectId, false);
    }
  }
}

/** Set the currently selected project (polls more frequently). */
export function setSelectedProject(projectId: string | null): void {
  if (selectedProjectId === projectId) return;
  selectedProjectId = projectId;
  // Give the switch's reactive work room to flush before background polling
  // resumes competing for the main thread.
  switchCooldownUntil = Date.now() + SWITCH_COOLDOWN;
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
// PR recovery coordination
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

/** Trigger an immediate refresh for a specific project (e.g. after PR creation or push). */
export function refreshNow(projectId: string): void {
  if (refreshInFlight) {
    // Queue so the project is refreshed as soon as the current operation finishes.
    pendingRefreshProjectIds.add(projectId);
    return;
  }
  refreshInFlight = true;
  setProjectRefreshing(projectId, true);
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
      setProjectRefreshing(projectId, false);
      refreshInFlight = false;
      // Drain queued immediate-refresh requests one at a time.
      if (pendingRefreshProjectIds.size > 0) {
        const queued = [...pendingRefreshProjectIds];
        pendingRefreshProjectIds.clear();
        // Re-queue all but the first; they'll drain on the next finally cycle.
        for (let i = 1; i < queued.length; i++) {
          pendingRefreshProjectIds.add(queued[i]);
        }
        refreshNow(queued[0]);
      } else {
        scheduleNext();
      }
    });
}
