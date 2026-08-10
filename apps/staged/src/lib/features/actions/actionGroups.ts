/**
 * Pure helpers for grouping a scope's configured actions and splitting its
 * running executions into the primary run action vs everything else.
 * Shared by the action-runner state machine and the Actions submenu builder.
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

export function getPrimaryRunAction(
  groupedActions: Record<string, ProjectAction[]>
): ProjectAction | null {
  return groupedActions.run?.[0] ?? null;
}

export function getRemainingRunActions(
  groupedActions: Record<string, ProjectAction[]>
): ProjectAction[] {
  return groupedActions.run?.slice(1) ?? [];
}

export function getPrimaryActionExecution<T extends { actionId: string }>(
  runningActions: T[],
  primaryRunActionId: string | null
): T | null {
  if (!primaryRunActionId) return null;
  return runningActions.find((a) => a.actionId === primaryRunActionId) ?? null;
}

export function getSecondaryRunningActions<T extends { actionId: string }>(
  runningActions: T[],
  primaryRunActionId: string | null
): T[] {
  if (!primaryRunActionId) return runningActions;
  return runningActions.filter((a) => a.actionId !== primaryRunActionId);
}
