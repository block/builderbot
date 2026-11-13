/**
 * Listener for `stagedctl project open` requests addressed to this window.
 *
 * The backend parks a request for the one window it picked, emits
 * `project-open-requested` to that window alone (`emit_to` by label, see
 * `project_open/control.rs`) and waits for the window's answer. This is the
 * window's side: it hands each request to `openProjectExternally` and reports
 * what happened through `answer_project_open`.
 *
 * **Window-scoped.** The listener has to be `listenToWindowEvent`: an
 * any-target listener also matches emits addressed to other windows, so every
 * window would navigate, and all but one would then be refused the answer.
 *
 * **Not lost before the window loads.** Registration is asynchronous, and the
 * app may have been launched for the request. Once the listener is live the
 * backend's snapshot of this window's pending request is taken as well.
 *
 * **Never answered twice.** The snapshot can hold a request the event also
 * delivered, so each request id is taken once. The backend refuses a second
 * answer in any case; taking the request once is what stops a second
 * navigation.
 *
 * **One at a time, newest wins.** A request can wait on the project list or
 * on a diff comment being saved. One that arrives meanwhile waits its turn
 * instead of navigating underneath it, so the window ends on the project asked
 * for last. The backend holds at most one request per window and has already
 * told the caller of a replaced one that it was superseded, so only the newest
 * waiting request is kept.
 */

import { listenToWindowEvent, type UnlistenFn } from '../transport';
import { answerProjectOpen, listPendingProjectOpens } from '../commands';
import {
  openProjectExternally,
  type ProjectOpenAnswer,
} from '../features/layout/navigation.svelte';
import type { ProjectOpenRequest } from '../types';

/** Mirrors `PROJECT_OPEN_REQUESTED_EVENT` in `project_open/control.rs`. */
export const PROJECT_OPEN_REQUESTED_EVENT = 'project-open-requested';

export function listenForProjectOpens(): UnlistenFn {
  /** Ids already taken from the event or the snapshot. One short string per
   *  CLI call for the life of the window, so it is never pruned. */
  const taken = new Set<string>();
  /** The newest request still waiting its turn. */
  let waiting: ProjectOpenRequest | null = null;
  let working = false;
  let stopped = false;

  function take(request: ProjectOpenRequest): void {
    if (stopped || taken.has(request.requestId)) return;
    taken.add(request.requestId);
    waiting = request;
    if (!working) void work();
  }

  async function work(): Promise<void> {
    working = true;
    try {
      while (waiting && !stopped) {
        const request = waiting;
        waiting = null;
        await act(request);
      }
    } finally {
      working = false;
    }
  }

  async function loadPending(): Promise<void> {
    let pending: ProjectOpenRequest[];
    try {
      pending = await listPendingProjectOpens();
    } catch (e) {
      console.error('[projectOpen] Failed to load the pending project-open request:', e);
      return;
    }
    for (const request of pending) take(request);
  }

  const unlisten = listenToWindowEvent<ProjectOpenRequest>(PROJECT_OPEN_REQUESTED_EVENT, take, {
    onEstablished: () => void loadPending(),
  });

  return () => {
    stopped = true;
    waiting = null;
    unlisten();
  };
}

/** Act on one request and report the result. Never rejects. */
async function act(request: ProjectOpenRequest): Promise<void> {
  let outcome: ProjectOpenAnswer;
  try {
    outcome = await openProjectExternally(request.projectId);
  } catch (e) {
    // No answer is true here, so none is sent: the caller reads a timeout.
    console.error(`[projectOpen] Failed to open project ${request.projectId}:`, e);
    return;
  }
  try {
    await answerProjectOpen(request.requestId, outcome);
  } catch (e) {
    // Superseded, or the caller stopped waiting while the window was busy.
    console.warn(`[projectOpen] The answer "${outcome}" was not recorded:`, e);
  }
}
