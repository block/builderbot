/**
 * Presentation of the background hold — the sub-state of a *running* session
 * whose agent is held open past turn end so background work (a
 * `run_in_background` shell and the out-of-turn continuation it triggers) can
 * finish.
 *
 * Holding is deliberately not a `SessionStatus`: the session stays `running`,
 * and the backend reports the wait separately over `session-background-hold`.
 * These helpers turn that report into what the live activity row shows —
 * "Waiting on background task (N)" in place of "Thinking…", with the same Stop
 * button (wired to `cancel_session`) relabeled for what it stops — plus, when
 * the agent names its tasks, one row per task with a stop of its own (wired to
 * `stop_session_async_task`, which leaves the session waiting on the rest).
 */

import type { SessionBackgroundHoldPayload, SessionStatus } from '../../types';

/**
 * The hold reported for the session currently being viewed. `tasks` is absent
 * or empty when the agent only reports a count (older bridges name nothing).
 */
export type SessionBackgroundHold = Pick<SessionBackgroundHoldPayload, 'holding' | 'liveTasks'> &
  Partial<Pick<SessionBackgroundHoldPayload, 'tasks'>>;

/** Statuses a session never holds in: its agent has already been torn down. */
const TERMINAL_STATUSES: ReadonlySet<SessionStatus> = new Set<SessionStatus>([
  'completed',
  'error',
  'cancelled',
]);

/**
 * The hold to hold onto, given what was just reported (by event or by the
 * mount-time snapshot) and the status the session is in.
 *
 * A hold reported for a session that has already reached a terminal status is
 * stale — an event still in flight when the agent tore down, or a snapshot
 * answered for a session that finished during the request. Dropping it is what
 * stops a later flip back to `running` (a resume, a queued send) from
 * rendering the previous turn's "Waiting on background task (N)" in place of
 * "Thinking…".
 *
 * A status that isn't known yet keeps the hold: on mount the snapshot request
 * and the session load are in flight together, and the wait is rendered behind
 * its own `running` check anyway — so discarding a hold for a session still
 * loading would lose the very report the mounting pane asked for.
 */
export function nextBackgroundHold(
  report: SessionBackgroundHold | null | undefined,
  sessionStatus: SessionStatus | null | undefined
): SessionBackgroundHold | null {
  if (!report?.holding) return null;
  if (sessionStatus && TERMINAL_STATUSES.has(sessionStatus)) return null;
  return { holding: true, liveTasks: report.liveTasks, tasks: report.tasks };
}

/** What the live activity row of a running session renders. */
export interface LiveActivityRow {
  /** Label beside the spinner. */
  label: string;
  /** Accessible name and tooltip of the row's Stop button. */
  stopLabel: string;
  /** True while the session is waiting on background work rather than working. */
  waitingOnBackground: boolean;
}

const THINKING: LiveActivityRow = {
  label: 'Thinking…',
  stopLabel: 'Stop session',
  waitingOnBackground: false,
};

/**
 * Whether a reported hold is currently showing. A cleared report (`holding:
 * false`) means a new turn took over or the session is tearing down.
 */
export function isBackgroundHolding(hold: SessionBackgroundHold | null | undefined): boolean {
  return !!hold?.holding;
}

/**
 * The activity row for a running session, given the hold most recently
 * reported for it (`null` before any report, or once one is withdrawn).
 *
 * The task count is dropped when the agent reports none: the wait is real
 * either way — with no raw-SDK stream to confirm the background state the hold
 * runs to its cap — but "(0)" would read as though nothing were pending.
 */
export function liveActivityRow(hold: SessionBackgroundHold | null | undefined): LiveActivityRow {
  if (!isBackgroundHolding(hold)) return THINKING;
  const count = hold?.liveTasks ?? 0;
  const noun = count === 1 ? 'task' : 'tasks';
  return {
    label: count > 0 ? `Waiting on background ${noun} (${count})` : 'Waiting on background work',
    stopLabel: 'Stop waiting and end session',
    waitingOnBackground: true,
  };
}

/** One named background task, rendered as its own wait row with its own stop. */
export interface BackgroundHoldTaskRow {
  /** Keys the per-task stop (`stopSessionAsyncTask`). */
  id: string;
  /** Row label: the task's announced name, or the id when it announced none. */
  label: string;
  /** Accessible name and tooltip of the row's own stop button. */
  stopLabel: string;
  /** Tooltip text, when the spawn described the task. */
  description: string | null;
}

/**
 * The per-task rows shown under the wait, one per named live task, each with
 * its own stop — stopping one task leaves the session waiting on the rest,
 * unlike the wait row's session-level Stop. Empty when the agent names
 * nothing (older bridges), which keeps the wait a bare-count row.
 */
export function backgroundHoldTaskRows(
  hold: SessionBackgroundHold | null | undefined
): BackgroundHoldTaskRow[] {
  if (!isBackgroundHolding(hold)) return [];
  return (hold?.tasks ?? []).map((task) => {
    const label = task.name?.trim() || `Background task ${task.id}`;
    return {
      id: task.id,
      label,
      stopLabel: `Stop "${label}"`,
      description: task.description ?? null,
    };
  });
}

/**
 * The per-task stops still worth showing as in-flight, after a hold report.
 *
 * A stop stays marked until its row actually leaves the live set — the agent
 * publishing the task's terminal state is what proves the stop took. Clearing
 * the mark when the request merely *resolves* re-enables the button for the
 * gap before that report lands, which reads as the stop not having taken and
 * invites a second click.
 *
 * Anything not in the reported set is done (or was never in it), so it is
 * dropped rather than left spinning forever.
 */
export function pruneStoppingTaskIds(
  stopping: ReadonlySet<string>,
  hold: SessionBackgroundHold | null | undefined
): Set<string> {
  if (!isBackgroundHolding(hold)) return new Set();
  const live = new Set((hold?.tasks ?? []).map((task) => task.id));
  return new Set([...stopping].filter((id) => live.has(id)));
}
