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

import type { SessionBackgroundHoldPayload } from '../../types';

/**
 * The hold reported for the session currently being viewed. `tasks` is absent
 * or empty when the agent only reports a count (older bridges name nothing).
 */
export type SessionBackgroundHold = Pick<SessionBackgroundHoldPayload, 'holding' | 'liveTasks'> &
  Partial<Pick<SessionBackgroundHoldPayload, 'tasks'>>;

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
