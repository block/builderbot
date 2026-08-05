/**
 * Global listener for `session-status-changed` Tauri events.
 *
 * Updates four independent state stores on session completion:
 * 1. projectState  — aggregate view of all sessions in a project (project tiles)
 * 2. prState       — branch-specific PR creation workflow state (PR buttons)
 * 3. pushState     — branch-specific push workflow state (push operations)
 * 4. pullState     — branch-specific queued-pull state (pull footer row)
 *
 * Session lookups are delegated to the unified sessionRegistry for consistency.
 *
 * Busy state follows a snapshot-then-deltas model: the backend is the source
 * of truth, and this module hydrates the registry from the
 * `get_active_sessions` snapshot (on startup, on WebSocket reconnect via
 * transport.ts, and on the page-lifecycle `cache-stale` resume signal), then
 * applies `session-status-changed` deltas on top. Sessions the snapshot
 * proves dead are swept, and a swept session still rendering a pr/push
 * workflow chip is reconciled against its persisted terminal status — a
 * client that was offline through the whole completion missed both the
 * domain event and the terminal event, so the chip would otherwise stay
 * stuck in "Creating PR…" / "Pushing…". Unread state is the only client-local
 * busy-state signal left untouched.
 *
 * Completion outcomes are parsed and persisted by the backend at the terminal
 * transition (PR number, cleared PR status) and arrive as `pr-created` /
 * `push-completed` domain events emitted *before* the terminal
 * `session-status-changed` event. The handlers here are idempotent renderers
 * — they perform no writes, so any number of connected clients can process
 * the same events safely.
 */

import { toast } from 'svelte-sonner';
import { listenToEvent, type UnlistenFn } from '../transport';
import { invalidateBranchTimeline } from '../commands';
import * as commands from '../api/commands';
import { navigation } from '../features/layout/navigation.svelte';
import { projectStateStore } from '../stores/projectState.svelte';
import { prStateStore } from '../stores/prState.svelte';
import { pullStateStore } from '../stores/pullState.svelte';
import { pushStateStore } from '../stores/pushState.svelte';
import { sessionRegistry, type SessionType } from '../stores/sessionRegistry.svelte';
import type {
  ActiveSessionInfo,
  PrCreatedPayload,
  PushCompletedPayload,
  SessionStatus,
  SessionStatusPayload,
} from '../types';

// Terminal deltas processed while a snapshot fetch is in flight are newer
// than that snapshot, which may still report the session as running — the
// register loop in hydrateActiveSessions must not resurrect them. Entries are
// keyed by arrival time so overlapping hydrations each compare against their
// own fetch start; the map is cleared once no fetch is in flight.
let hydrationFetchesInFlight = 0;
const terminalWhileFetching = new Map<string, number>();

export function listenForSessionStatus(): UnlistenFn {
  const unlistenEvents = listenToEvent<SessionStatusPayload>(
    'session-status-changed',
    handleSessionStatusChanged
  );
  const unlistenPrCreated = listenToEvent<PrCreatedPayload>('pr-created', handlePrCreated);
  const unlistenPushCompleted = listenToEvent<PushCompletedPayload>(
    'push-completed',
    handlePushCompleted
  );

  // Snapshot-then-deltas: hydrate now (app/page startup) and again after a
  // long hide (the page-lifecycle `cache-stale` resume signal). WebSocket
  // reconnects re-hydrate from transport.ts, next to the PR-poll interest
  // replay.
  void hydrateActiveSessions();
  const onCacheStale = () => void hydrateActiveSessions();
  window.addEventListener('cache-stale', onCacheStale);

  return () => {
    window.removeEventListener('cache-stale', onCacheStale);
    unlistenEvents();
    unlistenPrCreated();
    unlistenPushCompleted();
  };
}

async function handleSessionStatusChanged(payload: SessionStatusPayload): Promise<void> {
  const {
    sessionId,
    status,
    errorMessage,
    branchId: eventBranchId,
    projectId: eventProjectId,
    sessionType,
    isAutoReview,
  } = payload;

  // Auto review sessions are handled by BranchCard — don't register them
  // here so they don't cause the project list spinner. When the user
  // adopts an auto review, BranchCard registers the session at that point.
  if (status === 'running' && eventProjectId && !isAutoReview) {
    sessionRegistry.register(
      sessionId,
      eventProjectId,
      (sessionType as SessionType) ?? 'other',
      eventBranchId
    );
    projectStateStore.addRunningSession(eventProjectId, sessionId);
    // A push or pull that was queued behind other branch work starts running
    // when the branch queue drains it; this event is the only signal of that.
    if (sessionType === 'push' && eventBranchId) {
      pushStateStore.markQueuedPushStarted(eventBranchId, sessionId);
    }
    if (sessionType === 'pull' && eventBranchId) {
      pullStateStore.markQueuedPullStarted(eventBranchId, sessionId);
    }
    return;
  }

  if (status === 'completed' || status === 'error' || status === 'cancelled') {
    if (hydrationFetchesInFlight > 0) {
      terminalWhileFetching.set(sessionId, Date.now());
    }
    // Invalidate cached timeline for the branch affected by this session
    if (eventBranchId) {
      invalidateBranchTimeline(eventBranchId);
    }
    handleSessionEnd(sessionId, status, errorMessage);
  }
}

// ---------------------------------------------------------------------------
// Snapshot hydration
// ---------------------------------------------------------------------------

/**
 * Hydrate the busy-state stores from the backend's `get_active_sessions`
 * snapshot.
 *
 * The snapshot is authoritative for *which* sessions are active: local
 * entries the backend no longer reports are swept, which is what heals a
 * stuck spinner after a missed terminal event. Per-session metadata prefers
 * what the client already knows: launch sites register pipeline (pr/push)
 * sessions with their real branch/project, which the snapshot cannot resolve
 * (they link no artifact), and BranchCard registers adopted auto reviews.
 * Snapshot entries the client has never seen are applied with the same
 * gating as the live `running` event: running, resolved project, not an
 * auto review. Queued sessions register when their own running event
 * arrives. Unread state is per-device UX state and is left untouched.
 *
 * Swept sessions that a workflow store is still rendering as in-progress are
 * reconciled against their persisted status once the snapshot window closes
 * (see `reconcileSweptWorkflowSession`).
 */
export async function hydrateActiveSessions(): Promise<void> {
  // Anything that happens while the fetch is in flight is newer than the
  // snapshot, in both directions: entries registered mid-fetch (optimistic
  // launch-site registrations) must not be swept, and sessions whose terminal
  // delta was processed mid-fetch must not be re-registered from the
  // snapshot's stale "running" claim.
  const fetchStartedAt = Date.now();
  const sweptWorkflows: SweptWorkflowSession[] = [];
  hydrationFetchesInFlight++;
  try {
    let active: ActiveSessionInfo[];
    try {
      active = await commands.getActiveSessions();
    } catch (e) {
      console.error('Failed to fetch active-sessions snapshot:', e);
      return;
    }

    const activeIds = new Set(active.map((session) => session.sessionId));
    for (const sessionId of sessionRegistry.getAllSessionIds()) {
      const registeredAt = sessionRegistry.getMetadata(sessionId)?.timestamp ?? 0;
      if (!activeIds.has(sessionId) && registeredAt < fetchStartedAt) {
        // Collect before cleanupSession, which destroys the registry metadata
        // the store lookups resolve the branch through. Both lookups only
        // answer for their own session id in its in-progress state, so
        // settled chips are never collected.
        const prBranchId = prStateStore.getBranchIdForSession(sessionId);
        if (prBranchId) sweptWorkflows.push({ sessionId, kind: 'pr', branchId: prBranchId });
        const pushBranchId = pushStateStore.getBranchIdForSession(sessionId);
        if (pushBranchId) sweptWorkflows.push({ sessionId, kind: 'push', branchId: pushBranchId });
        sessionRegistry.cleanupSession(sessionId);
      }
    }

    for (const session of active) {
      if (session.status !== 'running') continue;
      if (!session.projectId || session.isAutoReview) continue;
      if (sessionRegistry.getMetadata(session.sessionId)) continue;
      if ((terminalWhileFetching.get(session.sessionId) ?? 0) >= fetchStartedAt) continue;
      sessionRegistry.register(
        session.sessionId,
        session.projectId,
        (session.sessionType as SessionType) ?? 'other',
        session.branchId ?? undefined
      );
      projectStateStore.addRunningSession(session.projectId, session.sessionId);
    }
  } finally {
    hydrationFetchesInFlight--;
    if (hydrationFetchesInFlight === 0) {
      terminalWhileFetching.clear();
    }
  }

  // Deliberately outside the in-flight window: the counter and the
  // `terminalWhileFetching` guard cover the snapshot fetch/apply only, and
  // these lookups must not extend it. Awaited so callers (and tests) can
  // observe the reconciliation.
  await Promise.all(sweptWorkflows.map(reconcileSweptWorkflowSession));
}

interface SweptWorkflowSession {
  sessionId: string;
  kind: 'pr' | 'push';
  branchId: string;
}

/**
 * Heal a pr/push workflow chip whose session the sweep just proved dead.
 *
 * A client offline through a pipeline session's entire completion misses both
 * the `pr-created` / `push-completed` domain event and the terminal
 * `session-status-changed` event, so its chip is still rendering
 * "Creating PR…" / "Pushing…" for a session that finished long ago. The
 * delta-path reconcilers can't be reused here: they read the ordered event
 * stream, so `handlePrCompletion` would render "no PR URL was found" for a PR
 * that actually succeeded.
 *
 * Instead, look up the session's persisted status (one `getSession` per
 * genuinely stuck chip — normally zero) and, on `completed`, drop the stale
 * workflow state rather than re-deriving an outcome: the backend persisted
 * the real one at the terminal transition, so the branch row drives the chip
 * (PR number → created, none → idle) and the push chip returns to its
 * git-state-derived affordance. `error` / `cancelled` are unambiguous and
 * render the same copy as the delta path.
 *
 * Race safety comes from re-checking the tracked session id after the await:
 * a terminal delta clears it, and a relaunch replaces it — either way this
 * reconciliation is stale and skips. Overlapping hydrations can't
 * double-reconcile, since the first sweep removes the registry entry the
 * second's collection step resolves the branch through.
 */
async function reconcileSweptWorkflowSession({
  sessionId,
  kind,
  branchId,
}: SweptWorkflowSession): Promise<void> {
  let status: SessionStatus | null = null;
  try {
    status = (await commands.getSession(sessionId))?.status ?? null;
  } catch (e) {
    console.error('Failed to look up swept workflow session:', sessionId, e);
  }

  const store = kind === 'pr' ? prStateStore : pushStateStore;
  if (store.getSessionId(branchId) !== sessionId) return;

  // The backend resumed it between the snapshot and this lookup — leave the
  // chip alone; its running event re-registers the session.
  if (status === 'running' || status === 'queued') return;

  if (kind === 'pr') {
    if (status === 'completed') {
      prStateStore.clearPrState(branchId);
    } else if (status) {
      handlePrCompletion(branchId, status);
      prStateStore.clearSessionTracking(branchId);
    } else {
      prStateStore.setPrError(branchId, 'Lost track of PR creation session.');
      prStateStore.clearSessionTracking(branchId);
    }
  } else {
    if (status === 'completed') {
      // No `done` flash: this completion may be arbitrarily old, and the
      // outcome was already classified and persisted server-side.
      pushStateStore.clearPushState(branchId);
    } else if (status) {
      pushStateStore.setPushError(
        branchId,
        `Push session ${status === 'error' ? 'failed' : 'was cancelled'}.`
      );
      pushStateStore.clearSessionTracking(branchId);
    } else {
      pushStateStore.setPushError(branchId, 'Lost track of push session.');
      pushStateStore.clearSessionTracking(branchId);
    }
  }
}

// ---------------------------------------------------------------------------
// Completion sub-handlers (idempotent renderers — the backend owns the writes)
// ---------------------------------------------------------------------------

/**
 * Render a PR produced by a completed PR session. The backend already
 * persisted the PR number and kicked off a status refresh; the fresh status
 * arrives via the existing `pr-status-changed` event.
 */
function handlePrCreated(payload: PrCreatedPayload): void {
  prStateStore.setPrCreated(payload.branchId, payload.prUrl);
}

/** Render the backend-classified outcome of a completed push session. */
function handlePushCompleted(payload: PushCompletedPayload): void {
  if (payload.outcome === 'rejectedNonFastForward') {
    pushStateStore.setPushError(payload.branchId, '', true);
  } else {
    pushStateStore.setPushDone(payload.branchId);
    setTimeout(() => {
      pushStateStore.clearPushState(payload.branchId);
    }, 1_500);
  }
}

function handleSessionEnd(sessionId: string, status: SessionStatus, errorMessage?: string | null) {
  const sessionProjectId = sessionRegistry.getProjectId(sessionId);
  const sessionType = sessionRegistry.getType(sessionId);
  const branchId = sessionRegistry.getBranchId(sessionId);
  const currentProjectId = navigation.selectedProjectId;

  if (!sessionProjectId && !sessionType && !branchId) {
    console.warn('Received completion event for unknown session ID', { sessionId, status });
  }

  // Mark project as unread if the user is currently viewing a different project.
  if (sessionProjectId && currentProjectId !== sessionProjectId) {
    projectStateStore.markAsUnread(sessionProjectId);
  }

  if (sessionType === 'pr' && branchId) {
    handlePrCompletion(branchId, status);
    prStateStore.clearSessionTracking(branchId);
  }

  if (sessionType === 'push' && branchId) {
    handlePushCompletion(branchId, status);
    pushStateStore.clearSessionTracking(branchId);
  }

  if (sessionType === 'pull' && branchId) {
    handlePullCompletion(branchId, status, errorMessage);
  }

  // Remove running state from projectStateStore and unregister from the registry.
  sessionRegistry.cleanupSession(sessionId);
}

/**
 * Release the pull row and report a failed pull.
 *
 * A queued pull is drained headless, so the status event is the only place its
 * failure surfaces: the backend ends an unpullable session in `error` with the
 * failing step's output (see `session_runner::aborted_pipeline_error`), and the
 * usual fix — rebase onto origin, or reset to origin — is the user's call. On
 * success the row simply disappears, since the branch is no longer behind.
 */
function handlePullCompletion(
  branchId: string,
  status: SessionStatus,
  errorMessage?: string | null
) {
  pullStateStore.clearPullState(branchId);
  if (status !== 'error') return;
  toast.error('Pull failed', {
    description: errorMessage ?? 'The queued pull could not fast-forward this branch.',
    duration: Infinity,
  });
}

/**
 * Reconcile PR workflow state with the terminal status event.
 *
 * The backend emits `pr-created` before the terminal event, so on a
 * successful completion the branch is already marked created by the time
 * this runs; a completed session whose branch is still not created means no
 * PR URL was found in the output.
 */
function handlePrCompletion(branchId: string, status: SessionStatus) {
  if (status === 'completed') {
    if (prStateStore.getPrState(branchId)?.state !== 'created') {
      prStateStore.setPrError(
        branchId,
        'PR session completed but no PR URL was found in the output.'
      );
    }
  } else {
    prStateStore.setPrError(
      branchId,
      `PR creation session ${status === 'error' ? 'failed' : 'was cancelled'}.`
    );
  }
}

/**
 * Reconcile push workflow state with the terminal status event.
 *
 * `push-completed` arrives before the terminal event and renders the
 * classified outcome; a branch still marked `pushing` here means that event
 * was missed, so fall back to the success rendering (the backend's default
 * classification when no rejection markers are present).
 */
function handlePushCompletion(branchId: string, status: SessionStatus) {
  if (status === 'completed') {
    if (pushStateStore.getPushState(branchId)?.state === 'pushing') {
      pushStateStore.setPushDone(branchId);
      setTimeout(() => {
        pushStateStore.clearPushState(branchId);
      }, 1_500);
    }
  } else {
    pushStateStore.setPushError(
      branchId,
      `Push session ${status === 'error' ? 'failed' : 'was cancelled'}.`
    );
  }
}
