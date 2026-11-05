import type { Branch, Project } from '../../types';

/**
 * Why a repo can't be added to `project` right now, or `null` when it can.
 *
 * A remote project's repos share one Blox workspace, and a repo joins it by
 * being cloned into it, so the workspace has to be running. A remote project
 * with no remote branch has no workspace yet — one created without a repo —
 * and the first repo added provisions it, so nothing blocks that add.
 */
export function addRepoBlocker(project: Project, branches: Branch[]): string | null {
  if (project.location !== 'remote') return null;
  const workspaceBranches = branches.filter(
    (b) => b.branchType === 'remote' && b.workspaceName?.trim()
  );
  if (workspaceBranches.length === 0) return null;
  if (workspaceBranches.some((b) => b.workspaceStatus === 'running')) return null;
  if (workspaceBranches.some((b) => b.workspaceStatus === 'starting')) {
    return 'Workspace is provisioning. Wait until it is running, then add another repo.';
  }
  return 'Workspace must be running before adding another repo.';
}
