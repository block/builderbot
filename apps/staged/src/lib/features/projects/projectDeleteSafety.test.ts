import { describe, expect, it, vi } from 'vitest';
import type { Branch } from '../../types';
import {
  canDeleteProjectWithoutConfirmation,
  computeSafeToDeleteSignature,
} from './projectDeleteSafety';

function branch(overrides: Partial<Branch> = {}): Branch {
  return {
    id: 'branch-1',
    projectId: 'project-1',
    projectRepoId: 'repo-1',
    branchName: 'feature',
    baseBranch: 'main',
    prNumber: 123,
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    setupComplete: true,
    worktreePath: '/tmp/repo',
    createdAt: 0,
    updatedAt: 0,
    prState: 'MERGED',
    prChecksStatus: 'SUCCESS',
    prReviewDecision: 'APPROVED',
    prMergeable: true,
    prDraft: false,
    prUrl: null,
    prUpdatedAt: null,
    prFetchedAt: null,
    prHeadSha: null,
    ...overrides,
  };
}

describe('canDeleteProjectWithoutConfirmation', () => {
  it('allows projects with no repos to delete immediately', async () => {
    const hasUnpushedCommits = vi.fn();

    await expect(
      canDeleteProjectWithoutConfirmation({
        branches: [],
        repoCount: 0,
        hasUnpushedCommits,
      })
    ).resolves.toBe(true);
    expect(hasUnpushedCommits).not.toHaveBeenCalled();
  });

  it('allows merged remote branches without checking unpushed commits', async () => {
    const hasUnpushedCommits = vi.fn();

    await expect(
      canDeleteProjectWithoutConfirmation({
        branches: [branch({ branchType: 'remote', id: 'remote-branch' })],
        repoCount: 1,
        hasUnpushedCommits,
      })
    ).resolves.toBe(true);
    expect(hasUnpushedCommits).not.toHaveBeenCalled();
  });

  it('allows merged local branches without unpushed commits', async () => {
    const hasUnpushedCommits = vi.fn().mockResolvedValue(false);

    await expect(
      canDeleteProjectWithoutConfirmation({
        branches: [branch({ id: 'local-branch' })],
        repoCount: 1,
        hasUnpushedCommits,
      })
    ).resolves.toBe(true);
    expect(hasUnpushedCommits).toHaveBeenCalledWith('local-branch');
  });

  it('requires confirmation for merged local branches with unpushed commits', async () => {
    const hasUnpushedCommits = vi.fn().mockResolvedValue(true);

    await expect(
      canDeleteProjectWithoutConfirmation({
        branches: [branch()],
        repoCount: 1,
        hasUnpushedCommits,
      })
    ).resolves.toBe(false);
  });

  it('requires confirmation for unmerged branches', async () => {
    const hasUnpushedCommits = vi.fn();

    await expect(
      canDeleteProjectWithoutConfirmation({
        branches: [branch({ prState: 'OPEN' })],
        repoCount: 1,
        hasUnpushedCommits,
      })
    ).resolves.toBe(false);
    expect(hasUnpushedCommits).not.toHaveBeenCalled();
  });

  it('requires confirmation when the unpushed commits check fails', async () => {
    const error = new Error('git failed');
    const hasUnpushedCommits = vi.fn().mockRejectedValue(error);
    const onCheckError = vi.fn();
    const checkedBranch = branch();

    await expect(
      canDeleteProjectWithoutConfirmation({
        branches: [checkedBranch],
        repoCount: 1,
        hasUnpushedCommits,
        onCheckError,
      })
    ).resolves.toBe(false);
    expect(onCheckError).toHaveBeenCalledWith(error, checkedBranch);
  });
});

describe('computeSafeToDeleteSignature', () => {
  const projects = [{ id: 'project-1' }];

  it('produces an identical signature for identical inputs', () => {
    const branchesA = new Map([['project-1', [branch()]]]);
    const branchesB = new Map([['project-1', [branch()]]]);
    const repoCounts = new Map([['project-1', 1]]);

    expect(computeSafeToDeleteSignature(projects, branchesA, repoCounts)).toBe(
      computeSafeToDeleteSignature(projects, branchesB, repoCounts)
    );
  });

  it('changes when a branch prHeadSha changes', () => {
    const repoCounts = new Map([['project-1', 1]]);
    const before = computeSafeToDeleteSignature(
      projects,
      new Map([['project-1', [branch({ prHeadSha: 'abc' })]]]),
      repoCounts
    );
    const after = computeSafeToDeleteSignature(
      projects,
      new Map([['project-1', [branch({ prHeadSha: 'def' })]]]),
      repoCounts
    );

    expect(before).not.toBe(after);
  });

  it('changes when the repo count changes', () => {
    const branches = new Map([['project-1', [branch()]]]);
    const before = computeSafeToDeleteSignature(projects, branches, new Map([['project-1', 1]]));
    const after = computeSafeToDeleteSignature(projects, branches, new Map([['project-1', 2]]));

    expect(before).not.toBe(after);
  });
});
