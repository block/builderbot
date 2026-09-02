/**
 * The tooltip and accessible name for a button that represents one action's
 * live state — shared by the pinned-action buttons and the running-action
 * pills so the three copies of this ladder can't drift apart.
 */

import type { ActionStatus } from './actions';

export interface ActionStatusLabelInput {
  /** The action's name, as it appears in every rung of the ladder. */
  actionName: string;
  /** A stop has been requested and the process hasn't exited yet. */
  stopping: boolean;
  /** The button is currently offering to stop the action (alt held). */
  showStop: boolean;
  /** The execution is live. */
  running: boolean;
  /** The execution's status, when there is an execution at all. */
  status?: ActionStatus;
}

export interface ActionStatusLabels {
  title: string;
  ariaLabel: string;
}

/**
 * Six rungs, most specific first. `title` and `ariaLabel` differ only while
 * stopping, where the tooltip carries an ellipsis the screen reader shouldn't.
 */
export function actionStatusLabels({
  actionName,
  stopping,
  showStop,
  running,
  status,
}: ActionStatusLabelInput): ActionStatusLabels {
  if (stopping) return { title: 'Stopping…', ariaLabel: 'Stopping' };
  if (showStop) return both(`Stop ${actionName}`);
  if (running) return both(`View output for ${actionName}`);
  if (status === 'completed') return both(`${actionName} completed`);
  if (status === 'failed') return both(`${actionName} failed`);
  return both(actionName);
}

function both(label: string): ActionStatusLabels {
  return { title: label, ariaLabel: label };
}
