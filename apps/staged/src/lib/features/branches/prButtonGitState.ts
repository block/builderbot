import type { BranchGitState } from '../../types';

export interface ShouldShowPushChangesInput {
  prNumber: number | null;
  prState: string | null;
  prHeadSha: string | null;
  gitState: BranchGitState | null | undefined;
}

export type PrPushAction = 'none' | 'push' | 'forcePush';

export function derivePrPushAction(input: ShouldShowPushChangesInput): PrPushAction {
  const { prNumber, prState, prHeadSha, gitState } = input;
  if (!prNumber) return 'none';
  if (prState === 'MERGED') return 'none';
  if (!gitState) return 'none';
  // When a different branch is checked out, neither `upstream.relation`
  // (computed against HEAD's upstream) nor `headSha` reflect this PR's
  // branch, so the comparison would be meaningless.
  if (!gitState.expectedBranchMatches) return 'none';

  switch (gitState.upstream.relation) {
    case 'localAhead':
      return 'push';
    case 'diverged':
      return 'forcePush';
    case 'inSync':
    case 'originAhead':
      return 'none';
    case 'missing': {
      // Fork PRs may not have origin/<branch>. Fall back to comparing local
      // HEAD with the PR head SHA reported by GitHub. We can't tell from
      // this data alone whether the remote diverged, so classify as 'push'
      // and rely on the backend's non-fast-forward rejection to surface
      // the force-push dialog if needed.
      if (!gitState.headSha || !prHeadSha) return 'none';
      return gitState.headSha !== prHeadSha ? 'push' : 'none';
    }
    default: {
      const _exhaustive: never = gitState.upstream.relation;
      void _exhaustive;
      return 'none';
    }
  }
}

export function shouldShowPushChanges(input: ShouldShowPushChangesInput): boolean {
  return derivePrPushAction(input) !== 'none';
}
