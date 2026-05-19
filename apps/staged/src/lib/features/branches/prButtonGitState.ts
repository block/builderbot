import type { BranchGitState } from '../../types';

export interface ShouldShowPushChangesInput {
  prNumber: number | null;
  prState: string | null;
  prHeadSha: string | null;
  gitState: BranchGitState | null | undefined;
}

export function shouldShowPushChanges(input: ShouldShowPushChangesInput): boolean {
  const { prNumber, prState, prHeadSha, gitState } = input;
  if (!prNumber) return false;
  if (prState === 'MERGED') return false;
  if (!gitState) return false;
  // When a different branch is checked out, neither `upstream.relation`
  // (computed against HEAD's upstream) nor `headSha` reflect this PR's
  // branch, so the comparison would be meaningless.
  if (!gitState.expectedBranchMatches) return false;

  switch (gitState.upstream.relation) {
    case 'localAhead':
    case 'diverged':
      return true;
    case 'inSync':
    case 'originAhead':
      return false;
    case 'missing': {
      // Fork PRs may not have origin/<branch>. Fall back to comparing local
      // HEAD with the PR head SHA reported by GitHub.
      if (!gitState.headSha || !prHeadSha) return false;
      return gitState.headSha !== prHeadSha;
    }
    default: {
      const _exhaustive: never = gitState.upstream.relation;
      void _exhaustive;
      return false;
    }
  }
}
