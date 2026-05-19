import { describe, expect, it } from 'vitest';
import { shouldShowPushChanges } from './prButtonGitState';
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
});
