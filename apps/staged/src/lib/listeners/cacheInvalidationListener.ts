/**
 * Event-driven cache invalidation listener.
 *
 * Listens for backend events (pr-status-changed and the store change feed's
 * project-changed / branch-changed / notes-changed / review-changed) and
 * invalidates the corresponding caches so that stale data is never served
 * after the backend pushes an update. The store change feed publishes from
 * every mutating store method, so a write in any window (or the backend
 * itself) invalidates every window.
 *
 * This is the cache leg only. The in-memory view stores subscribe to the
 * same feed themselves: projectsDataStore consumes project-changed,
 * branch-changed, and repos-changed for the project/branch/repo lists. Its
 * event-driven refetches bypass cache reads, but this listener still protects
 * shared IDB data for the next non-forced read in this tab or another tab.
 *
 * When the feed falls behind it emits every event with all ids null, which
 * these handlers read as "refetch the whole surface" — one broad
 * invalidation instead of silently stale windows.
 */

import { listenToEvent, type UnlistenFn } from '../transport';
import { invalidateCache, invalidateCacheByArgs, invalidateCacheByCommand } from '../cache';
import { invalidateAllBranchTimelines, invalidateBranchTimeline } from '../commands';
import type {
  BranchChangedEvent,
  NotesChangedEvent,
  PrStatusChangedEvent,
  ProjectChangedEvent,
  ReviewChangedEvent,
  SessionStatusPayload,
} from '../types';

export function listenForCacheInvalidation(): UnlistenFn {
  const unlisteners: UnlistenFn[] = [];

  // PR status changed → invalidate branch listings (they embed PR state)
  unlisteners.push(
    listenToEvent<PrStatusChangedEvent>('pr-status-changed', () => {
      invalidateCacheByCommand('list_branches_for_project');
    })
  );

  // Project changed (create / rename / delete / repo attach) → drop the
  // project-list caches for future non-forced reads in this tab and any other
  // tab sharing the same IDB store. list_projects takes no args so the drop is
  // command-wide; the repo lists scope to the named project when the payload
  // names one, widening only on the feed's lag recovery.
  unlisteners.push(
    listenToEvent<ProjectChangedEvent>('project-changed', (payload) => {
      invalidateCacheByCommand('list_projects');
      if (payload.projectId === null) {
        invalidateCacheByCommand('list_project_repos');
      } else {
        invalidateCacheByArgs('list_project_repos', { projectId: payload.projectId });
      }
    })
  );

  // Branch changed (any store write touching the branch or its timeline)
  // → invalidate that branch's list, timeline and diff caches. Three tiers,
  // widening as the payload names less:
  //   - a named project scopes the branch-list drop to that one project, so
  //     every other project keeps its instant cached paint. A mutation
  //     touching two projects' lists (a move) publishes once per project,
  //     so scoping loses nothing;
  //   - an unresolved project widens to every project's list, matching the
  //     store's own scan-all-known-lists fallback;
  //   - a null branchId is the feed's lag recovery — it dropped changes it
  //     can't name, so widen to every branch as well.
  unlisteners.push(
    listenToEvent<BranchChangedEvent>('branch-changed', (payload) => {
      if (payload.branchId === null) {
        invalidateCacheByCommand('list_branches_for_project');
        invalidateAllBranchTimelines();
        invalidateCacheByCommand('get_diff_files');
        invalidateCacheByCommand('get_file_diff');
        return;
      }
      if (payload.projectId === null) {
        invalidateCacheByCommand('list_branches_for_project');
      } else {
        invalidateCacheByArgs('list_branches_for_project', { projectId: payload.projectId });
      }
      invalidateBranchTimeline(payload.branchId);
      invalidateCacheByArgs('get_diff_files', { branchId: payload.branchId });
      invalidateCacheByArgs('get_file_diff', { branchId: payload.branchId });
    })
  );

  // Notes changed → branch notes are timeline items, so refresh that branch's
  // timeline; project-note surfaces (ProjectSection's list, BranchCard's
  // hashtag items) refetch through the existing window event.
  unlisteners.push(
    listenToEvent<NotesChangedEvent>('notes-changed', (payload) => {
      if (payload.branchId) {
        invalidateBranchTimeline(payload.branchId);
      } else {
        window.dispatchEvent(new CustomEvent('project-notes-invalidated'));
      }
    })
  );

  // Review changed → reviews and their comment counts render as timeline
  // items. An open diff viewer keeps its own optimistic review state and is
  // deliberately not reloaded here: the echo of a window's own edit would
  // clobber in-flight comment drafts.
  unlisteners.push(
    listenToEvent<ReviewChangedEvent>('review-changed', (payload) => {
      if (payload.branchId) {
        invalidateBranchTimeline(payload.branchId);
      }
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
