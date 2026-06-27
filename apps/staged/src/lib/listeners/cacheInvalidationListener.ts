/**
 * Event-driven cache invalidation listener.
 *
 * Listens for backend events (pr-status-changed, branch-git-state-changed)
 * and invalidates the corresponding IndexedDB cache entries so that stale
 * data is never served after the backend pushes an update.
 */

import { listenToEvent, type UnlistenFn } from '../transport';
import { invalidateCache, invalidateCacheByArgs, invalidateCacheByCommand } from '../cache';
import { invalidateBranchTimeline } from '../commands';
import type { PrStatusChangedEvent, SessionStatusPayload } from '../types';

interface BranchGitStateChangedEvent {
  branchId: string;
}

export function listenForCacheInvalidation(): UnlistenFn {
  const unlisteners: UnlistenFn[] = [];

  // PR status changed → invalidate branch listings (they embed PR state)
  unlisteners.push(
    listenToEvent<PrStatusChangedEvent>('pr-status-changed', () => {
      invalidateCacheByCommand('list_branches_for_project');
    })
  );

  // Branch git state changed → invalidate timeline and diff caches
  unlisteners.push(
    listenToEvent<BranchGitStateChangedEvent>('branch-git-state-changed', (payload) => {
      invalidateBranchTimeline(payload.branchId);
      invalidateCacheByCommand('list_branches_for_project');
      invalidateCacheByArgs('get_diff_files', { branchId: payload.branchId });
      invalidateCacheByArgs('get_file_diff', { branchId: payload.branchId });
    })
  );

  // Session status changed → invalidate cached session messages when a session
  // completes, errors, or is cancelled (messages are now final)
  unlisteners.push(
    listenToEvent<SessionStatusPayload>('session-status-changed', (payload) => {
      if (
        payload.status === 'completed' ||
        payload.status === 'error' ||
        payload.status === 'cancelled'
      ) {
        invalidateCache('get_session_messages', { sessionId: payload.sessionId });
      }
    })
  );

  return () => {
    for (const unlisten of unlisteners) unlisten();
  };
}
