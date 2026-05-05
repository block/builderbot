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
