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

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import * as commands from '../api/commands';
import {
  extractPrUrl,
  extractPrNumber,
  isPushRejectedNonFastForward,
} from '../features/branches/branchCardHelpers';
import { navigation } from '../features/layout/navigation.svelte';
import { projectStateStore } from '../stores/projectState.svelte';
import { prStateStore } from '../stores/prState.svelte';
import { pushStateStore } from '../stores/pushState.svelte';
import { sessionRegistry, type SessionType } from '../stores/sessionRegistry.svelte';
import type { SessionStatusPayload } from '../types';

export function listenForSessionStatus(): Promise<UnlistenFn> {
  return listen<SessionStatusPayload>('session-status-changed', async (event) => {
    const {
      sessionId,
      status,
      branchId: eventBranchId,
      projectId: eventProjectId,
      sessionType,
      isAutoReview,
    } = event.payload;

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

async function handleSessionEnd(sessionId: string, status: string) {
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

  // Always remove the running session from its project.
  if (sessionProjectId) {
    projectStateStore.removeRunningSession(sessionProjectId, sessionId);
  }

  if (sessionType === 'pr' && branchId) {
    await handlePrCompletion(sessionId, branchId, status);
    prStateStore.clearSessionTracking(branchId);
  }

  if (sessionType === 'push' && branchId) {
    await handlePushCompletion(sessionId, branchId, status);
    pushStateStore.clearSessionTracking(branchId);
  }

  // Clean up the session from the unified registry (single point of cleanup).
  sessionRegistry.unregister(sessionId);
}

async function handlePrCompletion(sessionId: string, branchId: string, status: string) {
  if (status === 'completed') {
    try {
      const messages = await commands.getSessionMessages(sessionId);
      const foundUrl = extractPrUrl(messages);

      if (foundUrl) {
        const prNumber = extractPrNumber(foundUrl);
        if (prNumber) {
          try {
            await commands.updateBranchPr(branchId, prNumber);
          } catch (storageError) {
            console.error('Failed to persist PR number to storage:', storageError);
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

async function handlePushCompletion(sessionId: string, branchId: string, status: string) {
  if (status === 'completed') {
    try {
      const messages = await commands.getSessionMessages(sessionId);
      if (isPushRejectedNonFastForward(messages)) {
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
