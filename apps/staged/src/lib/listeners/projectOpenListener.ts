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
 *
 * **Not for a caller that was told it failed.** While a request waits, its
 * caller can time out or be replaced by a newer request, and is then told so.
 * Right before navigating, `openProjectExternally` asks the backend whether
 * this request is still the window's pending one, and leaves the window alone
 * when it is not. One gap is left: the caller can time out or be replaced
 * between that reply and the navigation it allows. The window then switches
 * and its answer is refused.
 *
 * **Ahead of the launch-resume offer.** On a cold start the request is parked
 * before the window loads, and the "Resume interrupted sessions?" dialog would
 * refuse it if it opened first. `startupHandled` settles once a request found
 * at startup has been acted on, and App.svelte loads the offer after it, so
 * the project opens and the dialog then appears over it. This covers the
 * request the window finds waiting. One that arrives once the offer is open
 * is refused like any other request with a dialog in the way.
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

/**
 * The longest `startupHandled` stays unsettled. Mirrors `ANSWER_TIMEOUT` in
 * `project_open.rs`: a request found at startup has waited about that long by
 * then, so its caller is being told it timed out whatever a dialog does.
 */
export const STARTUP_HANDLED_TIMEOUT_MS = 15_000;

export interface ProjectOpenListener {
  /** Stop listening. A request already under way still answers. */
  stop: UnlistenFn;
  /**
   * Settles once the window has dealt with any request that was parked before
   * it was listening: the listener is live, the backend's snapshot has been
   * read, and every request taken by then has been acted on and answered.
   * With nothing parked that is two round trips.
   *
   * It also settles when there is nothing to wait for (the registration or
   * the snapshot failed, or the listener was stopped) and after
   * `STARTUP_HANDLED_TIMEOUT_MS` at the latest, so whatever waits on it is
   * never held up for good. Never rejects.
   */
  startupHandled: Promise<void>;
}

export function listenForProjectOpens(): ProjectOpenListener {
  /** Ids already taken from the event or the snapshot. One short string per
   *  CLI call for the life of the window, so it is never pruned. */
  const taken = new Set<string>();
  /** The newest request still waiting its turn. */
  let waiting: ProjectOpenRequest | null = null;
  let working = false;
  /** The latest run of `work`: settled when no request is under way. */
  let idle: Promise<void> = Promise.resolve();
  let stopped = false;

  let settleStartup!: () => void;
  const startupHandled = new Promise<void>((resolve) => {
    const timer = setTimeout(resolve, STARTUP_HANDLED_TIMEOUT_MS);
    settleStartup = () => {
      clearTimeout(timer);
      resolve();
    };
  });

  function take(request: ProjectOpenRequest): void {
    if (stopped || taken.has(request.requestId)) return;
    taken.add(request.requestId);
    waiting = request;
    if (!working) idle = work();
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
    try {
      for (const request of await listPendingProjectOpens()) take(request);
    } catch (e) {
      console.error('[projectOpen] Failed to load the pending project-open request:', e);
    }
    // Whatever was parked before the window was listening has been taken by
    // now, from here or from the event. A request that arrives while one of
    // those is under way is waited for as well: `idle` is replaced only once
    // the run before it has ended.
    await idle;
    settleStartup();
  }

  const unlisten = listenToWindowEvent<ProjectOpenRequest>(PROJECT_OPEN_REQUESTED_EVENT, take, {
    onEstablished: () => void loadPending(),
    // Nothing will be heard, and no snapshot is taken without a listener.
    onRegistrationFailed: () => settleStartup(),
  });

  return {
    stop: () => {
      stopped = true;
      waiting = null;
      unlisten();
      settleStartup();
    },
    startupHandled,
  };
}

/** Whether `request` is still the one the backend holds for this window: not
 *  answered, not replaced by a newer one, and its caller still waiting. */
async function stillPending(request: ProjectOpenRequest): Promise<boolean> {
  const pending = await listPendingProjectOpens();
  return pending.some((held) => held.requestId === request.requestId);
}

/** Act on one request and report the result. Never rejects. */
async function act(request: ProjectOpenRequest): Promise<void> {
  let outcome: ProjectOpenAnswer | 'withdrawn';
  try {
    outcome = await openProjectExternally(request.projectId, () => stillPending(request));
  } catch (e) {
    // No answer is true here, so none is sent: the caller reads a timeout.
    console.error(`[projectOpen] Failed to open project ${request.projectId}:`, e);
    return;
  }
  if (outcome === 'withdrawn') {
    // Its caller timed out or was superseded while the window was busy, and
    // has been told so. Nothing was navigated and there is nobody to answer.
    console.warn(
      `[projectOpen] Project ${request.projectId} was not opened: the request is no longer pending.`
    );
    return;
  }
  try {
    await answerProjectOpen(request.requestId, outcome);
  } catch (e) {
    // Superseded, or the caller stopped waiting, after the last check.
    console.warn(`[projectOpen] The answer "${outcome}" was not recorded:`, e);
  }
}
