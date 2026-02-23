import { projectStateStore } from '../../stores/projectState.svelte';

export type ProjectStatusKind = 'deleting' | 'running' | 'unread' | 'idle';

export interface ProjectStatus {
  kind: ProjectStatusKind;
  runningCount: number;
}

export function getProjectStatus(
  projectId: string,
  deletingProjectNames?: Map<string, string>
): ProjectStatus {
  if (deletingProjectNames?.has(projectId)) {
    return { kind: 'deleting', runningCount: 0 };
  }

  const hasRunning = projectStateStore.hasRunningSessions(projectId);
  if (hasRunning) {
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
