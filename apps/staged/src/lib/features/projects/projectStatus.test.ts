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
    prTitle: null,
    deferredPush: null,
    deferredPushRequestedAt: null,
    ...overrides,
  };
}

// ── Mock plumbing ──
//
// The helper is pure over the rune stores it reads; those can't be imported
// under plain vitest, so they are replaced with spies, rebuilt per test so a
// return value set in one test can't leak into the next.

let projectStateStore: {
  hasRunningSessions: ReturnType<typeof vi.fn>;
  getRunningSessionCount: ReturnType<typeof vi.fn>;
  isWaitingOnBackground: ReturnType<typeof vi.fn>;
  isUnread: ReturnType<typeof vi.fn>;
};
let disconnectedSessionsStore: { hasDisconnectedSessions: ReturnType<typeof vi.fn> };
let runActionPhase: string | null;

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
  disconnectedSessionsStore = { hasDisconnectedSessions: vi.fn().mockReturnValue(false) };
  runActionPhase = null;
  vi.doMock('../../stores/projectState.svelte', () => ({ projectStateStore }));
  vi.doMock('../../stores/disconnectedSessions.svelte', () => ({ disconnectedSessionsStore }));
  vi.doMock('../../stores/projectRunActions.svelte', () => ({
    projectRunActionsStore: { getRunActionPhase: () => runActionPhase },
  }));
});

afterEach(() => {
  vi.doUnmock('../../stores/projectState.svelte');
  vi.doUnmock('../../stores/disconnectedSessions.svelte');
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
    disconnectedSessionsStore.hasDisconnectedSessions.mockReturnValue(true);
    const { getProjectStatus } = await importStatus();
    expect(getProjectStatus('p1', new Map([['p1', 'Alpha']]), [branch()]).kind).toBe('deleting');
    expect(
      getProjectStatus(
        'p1',
        new Map([['p1', 'Alpha']]),
        [branch()],
        undefined,
        new Map([['p1', 'toRemote']])
      ).kind
    ).toBe('deleting');
  });

  it('reports a transfer ahead of sessions, provisioning and unread', async () => {
    projectStateStore.hasRunningSessions.mockReturnValue(true);
    projectStateStore.isUnread.mockReturnValue(true);
    const { getProjectStatus } = await importStatus();
    const transferring = new Map([['p1', 'toLocal']]);
    expect(
      getProjectStatus('p1', undefined, [branch({ worktreePath: null })], undefined, transferring)
    ).toEqual({ kind: 'transferring', runningCount: 0, runActionPhase: null });
    // Another project's transfer leaves this one alone.
    expect(getProjectStatus('p2', undefined, [branch()], undefined, transferring).kind).toBe(
      'running'
    );
  });

  it('reports disconnected for a project whose only session lost its connection', async () => {
    disconnectedSessionsStore.hasDisconnectedSessions.mockImplementation((id) => id === 'p1');
    const { getProjectStatus } = await importStatus();
    expect(getProjectStatus('p1', undefined, [branch()])).toEqual({
      kind: 'disconnected',
      runningCount: 0,
      runActionPhase: null,
    });
    expect(getProjectStatus('p2', undefined, [branch({ projectId: 'p2' })]).kind).toBe('idle');
  });

  it('lets live work outrank a disconnected session', async () => {
    disconnectedSessionsStore.hasDisconnectedSessions.mockReturnValue(true);
    projectStateStore.hasRunningSessions.mockReturnValue(true);
    projectStateStore.getRunningSessionCount.mockReturnValue(1);
    const { getProjectStatus } = await importStatus();
    expect(getProjectStatus('p1', undefined, [branch()]).kind).toBe('running');
  });

  it('puts a disconnected session ahead of a build and of unread', async () => {
    disconnectedSessionsStore.hasDisconnectedSessions.mockReturnValue(true);
    projectStateStore.isUnread.mockReturnValue(true);
    runActionPhase = 'building';
    const { getProjectStatus } = await importStatus();
    expect(getProjectStatus('p1', undefined, [branch()])).toEqual({
      kind: 'disconnected',
      runningCount: 0,
      runActionPhase: 'building',
    });
  });
});
