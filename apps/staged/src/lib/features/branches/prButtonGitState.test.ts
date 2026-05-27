import { describe, expect, it } from 'vitest';
import { derivePrPushAction, shouldShowPushChanges } from './prButtonGitState';
import type { BranchGitState, UpstreamRelation } from '../../types';

function makeGitState(
  relation: UpstreamRelation,
  overrides: Partial<BranchGitState> = {}
): BranchGitState {
  return {
    headSha: 'localsha0000000000000000000000000000000a',
    currentBranch: 'feature',
    detachedHead: false,
    expectedBranchMatches: true,
    upstream: {
      ref: 'origin/feature',
      exists: relation !== 'missing',
      sha: relation === 'missing' ? null : 'upstreamsha000000000000000000000000000b',
      relation,
      ahead: relation === 'localAhead' || relation === 'diverged' ? 1 : 0,
      behind: relation === 'originAhead' || relation === 'diverged' ? 1 : 0,
      mergeBaseSha: null,
    },
    base: { ref: 'main', sha: null, commitsSinceFork: 0 },
    worktree: {
      dirty: false,
      modified: 0,
      added: 0,
      deleted: 0,
      untracked: 0,
      conflicted: 0,
    },
    fetch: { status: 'fresh', fetchedAt: null, error: null },
    ...overrides,
  };
}

describe('shouldShowPushChanges', () => {
  it('returns true when local is ahead of upstream', () => {
    expect(
      shouldShowPushChanges({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('localAhead'),
      })
    ).toBe(true);
  });

  it('returns true when local has diverged from upstream', () => {
    expect(
      shouldShowPushChanges({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('diverged'),
      })
    ).toBe(true);
  });

  it('returns false when origin is ahead of local (nothing to push)', () => {
    expect(
      shouldShowPushChanges({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('originAhead'),
      })
    ).toBe(false);
  });

  it('returns false when local is in sync with upstream', () => {
    expect(
      shouldShowPushChanges({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('inSync'),
      })
    ).toBe(false);
  });

  it('falls back to head SHA comparison when upstream is missing and SHAs differ', () => {
    expect(
      shouldShowPushChanges({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('missing', { headSha: 'localsha' }),
      })
    ).toBe(true);
  });

  it('falls back to head SHA comparison when upstream is missing and SHAs match', () => {
    expect(
      shouldShowPushChanges({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'samesha',
        gitState: makeGitState('missing', { headSha: 'samesha' }),
      })
    ).toBe(false);
  });

  it('returns false when upstream is missing and there is no PR head SHA yet', () => {
    expect(
      shouldShowPushChanges({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: null,
        gitState: makeGitState('missing'),
      })
    ).toBe(false);
  });

  it('returns false for merged PRs even when local is ahead', () => {
    expect(
      shouldShowPushChanges({
        prNumber: 42,
        prState: 'MERGED',
        prHeadSha: 'prsha',
        gitState: makeGitState('localAhead'),
      })
    ).toBe(false);
  });

  it('returns false when the branch has no PR yet', () => {
    expect(
      shouldShowPushChanges({
        prNumber: null,
        prState: null,
        prHeadSha: null,
        gitState: makeGitState('localAhead'),
      })
    ).toBe(false);
  });

  it('returns false when git state has not loaded yet', () => {
    expect(
      shouldShowPushChanges({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: null,
      })
    ).toBe(false);
  });

  it('returns false when a different branch is checked out (localAhead would otherwise apply)', () => {
    // User has switched to `main`; `upstream.relation` reflects main vs
    // origin/main, not the PR's branch, so the helper must bail out.
    expect(
      shouldShowPushChanges({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('localAhead', {
          expectedBranchMatches: false,
          currentBranch: 'main',
        }),
      })
    ).toBe(false);
  });

  it('returns false when a different branch is checked out and upstream is missing', () => {
    // Without the expected-branch guard, the SHA fallback would compare
    // the wrong-branch HEAD against the PR head SHA and almost certainly
    // surface a spurious "needs push".
    expect(
      shouldShowPushChanges({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('missing', {
          expectedBranchMatches: false,
          currentBranch: 'main',
          headSha: 'mainsha0000000000000000000000000000000c',
        }),
      })
    ).toBe(false);
  });
});

describe('derivePrPushAction', () => {
  it("returns 'push' when local is ahead of upstream", () => {
    expect(
      derivePrPushAction({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('localAhead'),
      })
    ).toBe('push');
  });

  it("returns 'forcePush' when local has diverged from upstream", () => {
    expect(
      derivePrPushAction({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('diverged'),
      })
    ).toBe('forcePush');
  });

  it("returns 'none' when local is in sync with upstream", () => {
    expect(
      derivePrPushAction({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('inSync'),
      })
    ).toBe('none');
  });

  it("returns 'none' when origin is ahead of local", () => {
    expect(
      derivePrPushAction({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('originAhead'),
      })
    ).toBe('none');
  });

  it("returns 'push' for missing upstream with differing SHAs (fork PR fallback)", () => {
    // We deliberately don't classify fork PRs as 'forcePush' up-front —
    // the backend's non-FF rejection still drives that path through
    // showForcePushDialog.
    expect(
      derivePrPushAction({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('missing', { headSha: 'localsha' }),
      })
    ).toBe('push');
  });

  it("returns 'none' for missing upstream when SHAs match", () => {
    expect(
      derivePrPushAction({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'samesha',
        gitState: makeGitState('missing', { headSha: 'samesha' }),
      })
    ).toBe('none');
  });

  it("returns 'none' for missing upstream when prHeadSha is null", () => {
    expect(
      derivePrPushAction({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: null,
        gitState: makeGitState('missing'),
      })
    ).toBe('none');
  });

  it("returns 'none' for merged PRs even when local has diverged", () => {
    expect(
      derivePrPushAction({
        prNumber: 42,
        prState: 'MERGED',
        prHeadSha: 'prsha',
        gitState: makeGitState('diverged'),
      })
    ).toBe('none');
  });

  it("returns 'none' when the branch has no PR yet", () => {
    expect(
      derivePrPushAction({
        prNumber: null,
        prState: null,
        prHeadSha: null,
        gitState: makeGitState('diverged'),
      })
    ).toBe('none');
  });

  it("returns 'none' when git state has not loaded yet", () => {
    expect(
      derivePrPushAction({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: null,
      })
    ).toBe('none');
  });

  it("returns 'none' when a different branch is checked out (diverged would otherwise apply)", () => {
    expect(
      derivePrPushAction({
        prNumber: 42,
        prState: 'OPEN',
        prHeadSha: 'prsha',
        gitState: makeGitState('diverged', {
          expectedBranchMatches: false,
          currentBranch: 'main',
        }),
      })
    ).toBe('none');
  });
});
