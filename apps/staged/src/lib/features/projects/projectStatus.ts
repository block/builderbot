import type { Branch } from '../../types';
import { projectStateStore } from '../../stores/projectState.svelte';
import { projectRunActionsStore, type RunActionPhase } from '../../stores/projectRunActions.svelte';

export type ProjectStatusKind = 'deleting' | 'runAction' | 'running' | 'unread' | 'idle';

export interface ProjectStatus {
  kind: ProjectStatusKind;
  runningCount: number;
  runActionPhase: RunActionPhase;
}

function hasProvisioningWorkspace(branches: Branch[]): boolean {
  return branches.some((branch) => {
    if (branch.branchType === 'remote') {
      return branch.workspaceStatus === 'starting';
    }

    // Local branches are considered provisioning until their worktree path is attached.
    return !branch.worktreePath;
  });
}

export function getProjectStatus(
  projectId: string,
  deletingProjectNames?: Map<string, string>,
  branches: Branch[] = []
): ProjectStatus {
  if (deletingProjectNames?.has(projectId)) {
    return { kind: 'deleting', runningCount: 0, runActionPhase: null };
  }

  const runActionPhase = projectRunActionsStore.getRunActionPhase(projectId);
  const hasRunningSessions = projectStateStore.hasRunningSessions(projectId);
  const hasStartingWorkspace = hasProvisioningWorkspace(branches);

  // Sessions or provisioning workspaces
  if (hasRunningSessions || hasStartingWorkspace) {
    return {
      kind: 'running',
      runningCount: projectStateStore.getRunningSessionCount(projectId),
      runActionPhase,
    };
  }

  // A run-action in "building" phase
  if (runActionPhase === 'building') {
    return {
      kind: 'runAction',
      runningCount: 0,
      runActionPhase,
    };
  }

  if (projectStateStore.isUnread(projectId)) {
    return { kind: 'unread', runningCount: 0, runActionPhase };
  }

  return { kind: 'idle', runningCount: 0, runActionPhase };
}
