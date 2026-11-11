import { describe, expect, it } from 'vitest';
import type { Branch, Tag } from '../../types';
import {
  computedStatusLabel,
  resolveComputedProjectStatus,
  resolveProjectStatus,
  resolvedStatusLabel,
} from './projectStatusDisplay';

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

const blocked: Tag = {
  id: 'blocked',
  groupId: 'status',
  name: 'Blocked',
  color: 'red',
  icon: 'octagon-minus',
  position: 6,
  createdAt: 0,
  updatedAt: 0,
};

// ── resolveProjectStatus ──

describe('resolveProjectStatus', () => {
  it('uses the PR status when the project has no icon-group tag', () => {
    const status = resolveProjectStatus(null, [branch({ prNumber: 1, prState: 'MERGED' })]);
    expect(status).toEqual({ kind: 'pr', prStatus: 'merged', hasCodeChanges: false });
  });

  it('lets the icon-group tag win over the PR status', () => {
    const status = resolveProjectStatus(blocked, [branch({ prNumber: 1, prState: 'MERGED' })]);
    expect(status).toEqual({ kind: 'tag', tag: blocked });
  });

  it('lets the tag win for a remote project', () => {
    const status = resolveProjectStatus(blocked, [
      branch({ branchType: 'remote', workspaceStatus: 'running' }),
    ]);
    expect(status).toEqual({ kind: 'tag', tag: blocked });
  });

  it('shows a tag before branches are hydrated', () => {
    expect(resolveProjectStatus(blocked, [], false).kind).toBe('tag');
  });

  it('shows a placeholder until the tags have loaded, tag or not', () => {
    const merged = [branch({ prNumber: 1, prState: 'MERGED' })];
    expect(resolveProjectStatus(null, merged, true, false)).toEqual({ kind: 'placeholder' });
    expect(resolveProjectStatus(blocked, merged, true, false)).toEqual({ kind: 'placeholder' });
  });

  it('resolves a remote project from its PR status, like a local one', () => {
    const remoteBranch = branch({ branchType: 'remote', workspaceStatus: 'running' });
    expect(resolveProjectStatus(null, [{ ...remoteBranch, prNumber: 1, prState: 'OPEN' }])).toEqual(
      { kind: 'pr', prStatus: 'open', hasCodeChanges: false }
    );
    expect(resolveProjectStatus(null, [{ ...remoteBranch, commitCount: 3 }])).toEqual({
      kind: 'pr',
      prStatus: null,
      hasCodeChanges: true,
    });
    expect(resolveProjectStatus(null, [], false)).toEqual({ kind: 'placeholder' });
  });
});

// ── labels ──

describe('computedStatusLabel', () => {
  function labelFor(branches: Branch[]): string {
    return computedStatusLabel(resolveComputedProjectStatus(branches));
  }

  it('labels each PR status', () => {
    expect(labelFor([branch({ prNumber: 1, prState: 'MERGED' })])).toBe('Merged');
    expect(labelFor([branch({ prNumber: 1, prState: 'OPEN' })])).toBe('PR open');
    expect(labelFor([branch({ prNumber: 1, prState: 'CLOSED' })])).toBe('PR closed');
    expect(labelFor([branch({ prNumber: 1, prMergeable: false })])).toBe('Merge conflict');
    expect(labelFor([branch({ prNumber: 1, prChecksStatus: 'FAILURE' })])).toBe('Checks failing');
    expect(labelFor([branch({ commitCount: 1 })])).toBe('No PR yet');
    expect(labelFor([])).toBe('No changes yet');
  });

  it('labels a project still loading', () => {
    expect(computedStatusLabel(resolveComputedProjectStatus([], false))).toBe('Loading…');
  });
});

describe('resolvedStatusLabel', () => {
  it('uses the tag name, or the computed label without one', () => {
    expect(resolvedStatusLabel({ kind: 'tag', tag: blocked })).toBe('Blocked');
    expect(resolvedStatusLabel(resolveProjectStatus(null, [branch({ commitCount: 1 })]))).toBe(
      'No PR yet'
    );
    expect(resolvedStatusLabel(resolveProjectStatus(blocked, [], true, false))).toBe('Loading…');
  });
});
