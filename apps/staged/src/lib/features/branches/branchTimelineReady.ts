import type { Branch } from '../../types';

type TimelineReadyBranch = Pick<Branch, 'id' | 'branchType' | 'worktreePath' | 'workspaceStatus'>;

/**
 * Returns the stable timeline readiness key for a branch, or null while the
 * branch cannot load a timeline yet.
 */
export function branchTimelineReadyKey(branch: TimelineReadyBranch): string | null {
  if (branch.branchType === 'local') {
    return branch.worktreePath ? `${branch.id}:${branch.worktreePath}` : null;
  }

  if (branch.branchType === 'remote') {
    return branch.workspaceStatus === 'running' ? `${branch.id}:<remote>` : null;
  }

  return null;
}
