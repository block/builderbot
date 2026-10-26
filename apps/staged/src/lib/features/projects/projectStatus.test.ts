import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch } from '../../types';

// ── Fixtures ──

function branch(overrides: Partial<Branch> = {}): Branch {
  return {
    id: 'b1',
    projectId: 'p1',
    projectRepoId: null,
    branchName: 'feature',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    setupComplete: true,
    worktreePath: '/tmp/wt',
    createdAt: 0,
    updatedAt: 0,
    prState: null,
    prChecksStatus: null,
    prReviewDecision: null,
    prMergeable: null,
    prDraft: null,
    prUrl: null,
    prUpdatedAt: null,
    prFetchedAt: null,
    prHeadSha: null,
    ...overrides,
  };
}

// ── Mock plumbing ──
//
// The helper is pure over the two rune stores it reads; those can't be
// imported under plain vitest, so they are replaced with spies, rebuilt per
// test so a return value set in one test can't leak into the next.

let projectStateStore: {
  hasRunningSessions: ReturnType<typeof vi.fn>;
  getRunningSessionCount: ReturnType<typeof vi.fn>;
  isWaitingOnBackground: ReturnType<typeof vi.fn>;
  isUnread: ReturnType<typeof vi.fn>;
};

async function importStatus() {
  return await import('./projectStatus');
}

beforeEach(() => {
  vi.resetModules();
  projectStateStore = {
    hasRunningSessions: vi.fn().mockReturnValue(false),
    getRunningSessionCount: vi.fn().mockReturnValue(0),
    isWaitingOnBackground: vi.fn().mockReturnValue(false),
    isUnread: vi.fn().mockReturnValue(false),
  };
  vi.doMock('../../stores/projectState.svelte', () => ({ projectStateStore }));
  vi.doMock('../../stores/projectRunActions.svelte', () => ({
    projectRunActionsStore: { getRunActionPhase: () => null },
  }));
});

afterEach(() => {
  vi.doUnmock('../../stores/projectState.svelte');
  vi.doUnmock('../../stores/projectRunActions.svelte');
});

// ── Tests ──

describe('getProjectStatus', () => {
  it('is idle with no sessions and attached worktrees', async () => {
    const { getProjectStatus } = await importStatus();
    expect(getProjectStatus('p1', undefined, [branch()]).kind).toBe('idle');
  });

  it('spins for a local branch whose worktree is still being provisioned', async () => {
    const { getProjectStatus } = await importStatus();
    const branches = [branch({ worktreePath: null })];
    expect(getProjectStatus('p1', undefined, branches).kind).toBe('running');
    expect(getProjectStatus('p1', undefined, branches, new Map()).kind).toBe('running');
  });

  it('stops spinning once that branch has a recorded worktree error', async () => {
    const { getProjectStatus } = await importStatus();
    const branches = [branch({ worktreePath: null })];
    const errors = new Map([['b1', 'clone failed']]);
    expect(getProjectStatus('p1', undefined, branches, errors).kind).toBe('idle');
    // An error on some other branch says nothing about this one.
    expect(getProjectStatus('p1', undefined, branches, new Map([['b2', 'x']])).kind).toBe(
      'running'
    );
  });

  it('ignores worktree errors for remote branches, which provision by workspace status', async () => {
    const { getProjectStatus } = await importStatus();
    const branches = [
      branch({ branchType: 'remote', worktreePath: null, workspaceStatus: 'starting' }),
    ];
    expect(getProjectStatus('p1', undefined, branches, new Map([['b1', 'x']])).kind).toBe(
      'running'
    );
  });

  it('reports backgroundHold when every running session is waiting on background work', async () => {
    projectStateStore.hasRunningSessions.mockReturnValue(true);
    projectStateStore.getRunningSessionCount.mockReturnValue(2);
    projectStateStore.isWaitingOnBackground.mockReturnValue(true);
    const { getProjectStatus } = await importStatus();
    expect(getProjectStatus('p1', undefined, [branch()])).toEqual({
      kind: 'backgroundHold',
      runningCount: 2,
      runActionPhase: null,
    });
  });

  it('stays running while any session is working or a workspace is provisioning', async () => {
    projectStateStore.hasRunningSessions.mockReturnValue(true);
    projectStateStore.getRunningSessionCount.mockReturnValue(1);
    const { getProjectStatus } = await importStatus();
    expect(getProjectStatus('p1', undefined, [branch()]).kind).toBe('running');

    projectStateStore.isWaitingOnBackground.mockReturnValue(true);
    expect(getProjectStatus('p1', undefined, [branch({ worktreePath: null })]).kind).toBe(
      'running'
    );
  });

  it('keeps deleting ahead of everything else', async () => {
    projectStateStore.hasRunningSessions.mockReturnValue(true);
    const { getProjectStatus } = await importStatus();
    expect(getProjectStatus('p1', new Map([['p1', 'Alpha']]), [branch()]).kind).toBe('deleting');
  });
});
