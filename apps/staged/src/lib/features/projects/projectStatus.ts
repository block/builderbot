import type { Branch } from '../../types';
import { projectStateStore } from '../../stores/projectState.svelte';
import { projectRunActionsStore, type RunActionPhase } from '../../stores/projectRunActions.svelte';

export type ProjectStatusKind =
  'deleting' | 'runAction' | 'running' | 'backgroundHold' | 'unread' | 'idle';

export interface ProjectStatus {
  kind: ProjectStatusKind;
  runningCount: number;
  runActionPhase: RunActionPhase;
}

function hasProvisioningWorkspace(
  branches: Branch[],
  worktreeErrors?: ReadonlyMap<string, string>
): boolean {
  return branches.some((branch) => {
    if (branch.branchType === 'remote') {
      return branch.workspaceStatus === 'starting';
    }

    // Local branches are considered provisioning until their worktree path is
    // attached — unless setup already failed: the branch card shows that error
    // and nothing is in progress until a retry.
    return !branch.worktreePath && !worktreeErrors?.has(branch.id);
  });
}

/**
 * `worktreeErrors` is `workspaceLifecycle.getWorktreeErrors()`: branch id →
 * message for local branches whose `setup_worktree` failed.
 */
export function getProjectStatus(
  projectId: string,
  deletingProjectNames?: Map<string, string>,
  branches: Branch[] = [],
  worktreeErrors?: ReadonlyMap<string, string>
): ProjectStatus {
  if (deletingProjectNames?.has(projectId)) {
    return { kind: 'deleting', runningCount: 0, runActionPhase: null };
  }

  const runActionPhase = projectRunActionsStore.getRunActionPhase(projectId);
  const hasRunningSessions = projectStateStore.hasRunningSessions(projectId);
  const hasStartingWorkspace = hasProvisioningWorkspace(branches, worktreeErrors);

  // Sessions or provisioning workspaces. When every running session is parked
  // on background work and nothing is provisioning, the project is busy but
  // nothing visible is happening — report that rather than a generic spin.
  if (hasRunningSessions || hasStartingWorkspace) {
    const waitingOnBackground =
      !hasStartingWorkspace && projectStateStore.isWaitingOnBackground(projectId);
    return {
      kind: waitingOnBackground ? 'backgroundHold' : 'running',
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
