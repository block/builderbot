import { describe, expect, it } from 'vitest';
import type { Branch } from '../../types';
import { branchTimelineReadyKey } from './branchTimelineReady';

function branch(overrides: Partial<Branch> = {}): Branch {
  return {
    id: 'branch-1',
    projectId: 'project-1',
    projectRepoId: null,
    branchName: 'feature',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    setupComplete: false,
    worktreePath: null,
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

describe('branchTimelineReadyKey', () => {
  it('waits for local branches to have a worktree path', () => {
    expect(branchTimelineReadyKey(branch())).toBeNull();
    expect(branchTimelineReadyKey(branch({ worktreePath: '/tmp/repo' }))).toBe(
      'branch-1:/tmp/repo'
    );
  });

  it('waits for remote branches to be running', () => {
    expect(
      branchTimelineReadyKey(
        branch({
          branchType: 'remote',
          workspaceStatus: 'starting',
        })
      )
    ).toBeNull();

    expect(
      branchTimelineReadyKey(
        branch({
          branchType: 'remote',
          workspaceStatus: 'running',
        })
      )
    ).toBe('branch-1:<remote>');
  });
});
