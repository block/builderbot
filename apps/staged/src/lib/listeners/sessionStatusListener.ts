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
 * applies `session-status-changed` deltas on top.
 */

import { toast } from 'svelte-sonner';
import { listenToEvent, type UnlistenFn } from '../transport';
import { invalidateBranchTimeline } from '../commands';
import * as commands from '../api/commands';
import {
  classifyCompletedPushSession,
  extractPrUrl,
  extractPrNumber,
} from '../features/branches/branchCardHelpers';
import { navigation } from '../features/layout/navigation.svelte';
import { projectStateStore } from '../stores/projectState.svelte';
import { prStateStore } from '../stores/prState.svelte';
import { pullStateStore } from '../stores/pullState.svelte';
import { pushStateStore } from '../stores/pushState.svelte';
import { sessionRegistry, type SessionType } from '../stores/sessionRegistry.svelte';
import type { ActiveSessionInfo, SessionStatus, SessionStatusPayload } from '../types';

export function listenForSessionStatus(): UnlistenFn {
  const unlistenEvents = listenToEvent<SessionStatusPayload>(
    'session-status-changed',
    handleSessionStatusChanged
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
 */
export async function hydrateActiveSessions(): Promise<void> {
  // Entries registered while the fetch is in flight (an optimistic launch-site
  // registration) are newer than the snapshot — the sweep below must not
  // treat them as stale.
  const fetchStartedAt = Date.now();
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
      sessionRegistry.cleanupSession(sessionId);
    }
  }

  for (const session of active) {
    if (session.status !== 'running') continue;
    if (!session.projectId || session.isAutoReview) continue;
    if (sessionRegistry.getMetadata(session.sessionId)) continue;
    sessionRegistry.register(
      session.sessionId,
      session.projectId,
      (session.sessionType as SessionType) ?? 'other',
      session.branchId ?? undefined
    );
    projectStateStore.addRunningSession(session.projectId, session.sessionId);
  }
}

// ---------------------------------------------------------------------------
// Completion sub-handlers
// ---------------------------------------------------------------------------

async function handleSessionEnd(
  sessionId: string,
  status: SessionStatus,
  errorMessage?: string | null
) {
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
    await handlePrCompletion(sessionId, branchId, status);
    prStateStore.clearSessionTracking(branchId);
  }

  if (sessionType === 'push' && branchId) {
    await handlePushCompletion(sessionId, branchId, status);
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

async function handlePrCompletion(sessionId: string, branchId: string, status: SessionStatus) {
  if (status === 'completed') {
    try {
      // Try session messages first (AI session writes PR_URL: marker).
      const messages = await commands.getFreshSessionMessages(sessionId);
      let foundUrl = extractPrUrl(messages);

      // Also check pipeline step outputs for older or partially migrated PR sessions.
      if (!foundUrl) {
        const session = await commands.getSession(sessionId);
        if (session?.pipeline) {
          for (const step of session.pipeline.steps) {
            if (step.output) {
              const match = step.output.match(/https:\/\/github\.com\/[^\s/]+\/[^\s/]+\/pull\/\d+/);
              if (match) {
                foundUrl = match[0];
                break;
              }
            }
          }
        }
      }

      if (foundUrl) {
        const prNumber = extractPrNumber(foundUrl);
        if (prNumber) {
          try {
            await commands.updateBranchPr(branchId, prNumber);
          } catch (storageError) {
            console.error('Failed to persist PR state after creation:', storageError);
            prStateStore.setPrError(branchId, 'Failed to save PR details after creation.');
            return;
          }

          try {
            await commands.refreshPrStatus(branchId);
          } catch (refreshError) {
            console.error('Failed to refresh PR state after creation:', refreshError);
          }
        }
        prStateStore.setPrCreated(branchId, foundUrl);
      } else {
        prStateStore.setPrError(
          branchId,
          'PR session completed but no PR URL was found in the output.'
        );
      }
    } catch (e) {
      prStateStore.setPrError(branchId, e instanceof Error ? e.message : String(e));
    }
  } else {
    prStateStore.setPrError(
      branchId,
      `PR creation session ${status === 'error' ? 'failed' : 'was cancelled'}.`
    );
  }
}

async function handlePushCompletion(sessionId: string, branchId: string, status: SessionStatus) {
  if (status === 'completed') {
    try {
      const session = await commands.getSession(sessionId);
      const pipeline = session?.pipeline;
      const messages = await commands.getFreshSessionMessages(sessionId);
      const outcome = classifyCompletedPushSession(pipeline, messages);

      if (outcome === 'rejected_non_fast_forward') {
        pushStateStore.setPushError(branchId, '', true);
      } else {
        try {
          await commands.clearBranchPrStatus(branchId);
        } catch (e) {
          console.warn('[Staged] Failed to clear PR status after push:', e);
        }
        pushStateStore.setPushDone(branchId);
        setTimeout(() => {
          pushStateStore.clearPushState(branchId);
        }, 1_500);
      }
    } catch (e) {
      pushStateStore.setPushError(branchId, e instanceof Error ? e.message : String(e));
    }
  } else {
    pushStateStore.setPushError(
      branchId,
      `Push session ${status === 'error' ? 'failed' : 'was cancelled'}.`
    );
  }
}
