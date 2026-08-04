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
import type { SessionStatus, SessionStatusPayload } from '../types';

export function listenForSessionStatus(): UnlistenFn {
  return listenToEvent<SessionStatusPayload>('session-status-changed', async (payload) => {
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
  });
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
