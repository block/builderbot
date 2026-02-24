import type { Branch } from '../../types';
import { projectStateStore } from '../../stores/projectState.svelte';

export type ProjectStatusKind = 'deleting' | 'running' | 'unread' | 'idle';

export interface ProjectStatus {
  kind: ProjectStatusKind;
  runningCount: number;
}

function hasProvisioningWorkspace(branches: Branch[]): boolean {
  return branches.some(
    (branch) => branch.branchType === 'remote' && branch.workspaceStatus === 'starting'
  );
}

export function getProjectStatus(
  projectId: string,
  deletingProjectNames?: Map<string, string>,
  branches: Branch[] = []
): ProjectStatus {
  if (deletingProjectNames?.has(projectId)) {
    return { kind: 'deleting', runningCount: 0 };
  }

  const hasRunningSessions = projectStateStore.hasRunningSessions(projectId);
  const hasStartingWorkspace = hasProvisioningWorkspace(branches);
  if (hasRunningSessions || hasStartingWorkspace) {
    return {
      kind: 'running',
      runningCount: projectStateStore.getRunningSessionCount(projectId),
    };
  }

  if (projectStateStore.isUnread(projectId)) {
    return { kind: 'unread', runningCount: 0 };
  }

  return { kind: 'idle', runningCount: 0 };
}
