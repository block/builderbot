/**
 * Pure helpers for grouping a scope's configured actions and splitting its
 * running executions into the ones that already have a header button vs
 * everything else. Shared by the action-runner state machine and the Actions
 * submenu builder.
 */

import type { ProjectAction } from '../../api/commands';

export function groupActionsByType(actions: ProjectAction[]): Record<string, ProjectAction[]> {
  const groups: Record<string, ProjectAction[]> = {
    prerun: [],
    run: [],
    build: [],
    format: [],
    check: [],
    test: [],
    cleanUp: [],
  };

  for (const action of actions) {
    if (groups[action.actionType]) {
      groups[action.actionType].push(action);
    }
  }

  return groups;
}

/**
 * The actions a card header renders as their own button, in the order the
 * settings list shows them.
 */
export function getPinnedActions(actions: ProjectAction[]): ProjectAction[] {
  return actions.filter((a) => a.pinned).sort((a, b) => a.sortOrder - b.sortOrder);
}

/**
 * Whether the editor should pre-check "Show in card header" for an action the
 * user is adding by hand.
 *
 * Every other way an action reaches a context pins on the user's behalf:
 * detection pins its run suggestion, and migration 0025 pinned the implicit
 * main action of every context that predates pinning. A context whose actions
 * are all typed in here goes through neither, so without a default its first
 * action leaves the card header empty — where before pinning existed, adding a
 * run action grew a play button on its own. Only the first one gets the
 * default: once anything is pinned the box starts clear, and it's visible and
 * toggleable either way.
 */
export function shouldPinNewAction(actions: ProjectAction[]): boolean {
  return !actions.some((a) => a.pinned);
}

/**
 * The running executions that need a pill: every one whose action isn't
 * already showing its own header button.
 */
export function getSecondaryRunningActions<T extends { actionId: string }>(
  runningActions: T[],
  pinnedActionIds: Set<string>
): T[] {
  if (pinnedActionIds.size === 0) return runningActions;
  return runningActions.filter((a) => !pinnedActionIds.has(a.actionId));
}
