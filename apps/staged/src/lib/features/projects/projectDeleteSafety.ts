import type { Branch } from '../../types';

export interface ProjectDeleteSafetyOptions {
  branches: Branch[];
  repoCount: number;
  hasUnpushedCommits: (branchId: string) => Promise<boolean>;
  onCheckError?: (error: unknown, branch: Branch) => void;
}

export async function canDeleteProjectWithoutConfirmation({
  branches,
  repoCount,
  hasUnpushedCommits,
  onCheckError,
}: ProjectDeleteSafetyOptions): Promise<boolean> {
  if (repoCount === 0) return true;
  if (branches.length === 0) return false;

  const branchSafety = await Promise.all(
    branches.map(async (branch) => {
      if (branch.prState !== 'MERGED') return false;
      if (branch.branchType === 'remote') return true;

      try {
        return !(await hasUnpushedCommits(branch.id));
      } catch (error) {
        onCheckError?.(error, branch);
        return false;
      }
    })
  );

  return branchSafety.every(Boolean);
}

/**
 * Compute a cheap signature of only the inputs that affect the safe-to-delete
 * result, so the eager `$effect` in ProjectHome can skip recomputation (and the
 * expensive per-branch git work it spawns) when background hydration reassigns
 * the source Maps without actually changing the relevant fields.
 *
 * Keys on exactly the fields `canDeleteProjectWithoutConfirmation` branches on
 * (`prState`, `branchType`, `repoCount`) plus `prHeadSha`, which invalidates the
 * cache when a PR head moves — the main case where unpushed-commit state
 * changes in practice. This trades a small staleness window (a local commit not
 * reflected in the cosmetic styling until the next genuine input change) for
 * eliminating the per-switch freeze.
 */
export function computeSafeToDeleteSignature(
  projects: Array<{ id: string }>,
  branchesByProject: Map<string, Branch[]>,
  repoCountsByProject: Map<string, number>
): string {
  return projects
    .map((p) => {
      const repoCount = repoCountsByProject.get(p.id) || 0;
      const branches = (branchesByProject.get(p.id) || [])
        .map((b) => `${b.id}:${b.prState}:${b.branchType}:${b.prHeadSha ?? ''}`)
        .join(',');
      return `${p.id}|${repoCount}|${branches}`;
    })
    .join(';');
}
