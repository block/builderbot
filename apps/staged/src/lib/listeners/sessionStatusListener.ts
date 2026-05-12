/**
 * Global listener for `session-status-changed` Tauri events.
 *
 * Updates three independent state stores on session completion:
 * 1. projectState  — aggregate view of all sessions in a project (project tiles)
 * 2. prState       — branch-specific PR creation workflow state (PR buttons)
 * 3. pushState     — branch-specific push workflow state (push operations)
 *
 * Session lookups are delegated to the unified sessionRegistry for consistency.
 */

import { listenToEvent, type UnlistenFn } from '../transport';
import * as commands from '../api/commands';
import {
  classifyCompletedPushSession,
  extractPrUrl,
  extractPrNumber,
} from '../features/branches/branchCardHelpers';
import { navigation } from '../features/layout/navigation.svelte';
import { projectStateStore } from '../stores/projectState.svelte';
import { prStateStore } from '../stores/prState.svelte';
import { pushStateStore } from '../stores/pushState.svelte';
import { sessionRegistry, type SessionType } from '../stores/sessionRegistry.svelte';
import type { SessionStatus, SessionStatusPayload } from '../types';

export function listenForSessionStatus(): Promise<UnlistenFn> {
  return listenToEvent<SessionStatusPayload>('session-status-changed', async (payload) => {
    const {
      sessionId,
      status,
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
      return;
    }

    if (status === 'completed' || status === 'error' || status === 'cancelled') {
      handleSessionEnd(sessionId, status);
    }
  });
}

// ---------------------------------------------------------------------------
// Completion sub-handlers
// ---------------------------------------------------------------------------

async function handleSessionEnd(sessionId: string, status: SessionStatus) {
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

  // Remove running state from projectStateStore and unregister from the registry.
  sessionRegistry.cleanupSession(sessionId);
}

async function handlePrCompletion(sessionId: string, branchId: string, status: SessionStatus) {
  if (status === 'completed') {
    try {
      // Try session messages first (AI session writes PR_URL: marker).
      const messages = await commands.getSessionMessages(sessionId);
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
      const messages = await commands.getSessionMessages(sessionId);
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
