import { describe, expect, it } from 'vitest';
import type {
  ProjectTransferPreflight,
  ProjectTransferRepoPreflight,
  ProjectTransferResult,
} from '../../types';
import {
  formatBytes,
  isTransferCancellable,
  isTransferCancelledError,
  issueMessage,
  plural,
  transferActionLabel,
  transferBlockers,
  transferDirectionFrom,
  transferPhaseLabel,
  transferResultWarnings,
  uncommittedCount,
} from './projectTransfer';

function repo(overrides: Partial<ProjectTransferRepoPreflight> = {}): ProjectTransferRepoPreflight {
  return {
    branchId: 'b1',
    projectRepoId: 'r1',
    githubRepo: 'org/alpha',
    subpath: null,
    branchName: 'feature',
    destBranchName: 'feature',
    state: 'carry',
    head: 'abc',
    upstream: null,
    unpushedCommits: 0,
    staged: 0,
    unstaged: 0,
    untracked: 0,
    conflicted: 0,
    untrackedBytes: 0,
    ignoredFiles: 0,
    hasUncommittedChanges: false,
    blockers: [],
    warnings: [],
    ...overrides,
  };
}

function preflight(overrides: Partial<ProjectTransferPreflight> = {}): ProjectTransferPreflight {
  return {
    projectId: 'p1',
    from: 'local',
    to: 'remote',
    direction: 'toRemote',
    workspaceName: null,
    repos: [repo()],
    blockers: [],
    warnings: [],
    canTransfer: true,
    hasUncommittedChanges: false,
    ...overrides,
  };
}

describe('labels', () => {
  it('offers the location the project is not in', () => {
    expect(transferDirectionFrom('local')).toBe('toRemote');
    expect(transferDirectionFrom('remote')).toBe('toLocal');
    expect(transferActionLabel('toRemote')).toBe('Transfer to Blox');
    expect(transferActionLabel('toLocal')).toBe('Transfer to local');
  });

  it('counts every kind of uncommitted change', () => {
    expect(uncommittedCount(repo({ staged: 1, unstaged: 2, untracked: 3, conflicted: 4 }))).toBe(
      10
    );
    expect(plural(1, 'unpushed commit')).toBe('1 unpushed commit');
    expect(plural(2, 'uncommitted change')).toBe('2 uncommitted changes');
  });

  it('formats sizes', () => {
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(70 * 1024 * 1024)).toBe('70 MB');
    expect(formatBytes(3 * 1024 ** 3)).toBe('3.0 GB');
  });

  it('names each phase, with the repo and bundle size where they apply', () => {
    expect(transferPhaseLabel('checking', 'toRemote')).toBe('Checking the project');
    expect(transferPhaseLabel('preparing_destination', 'toRemote')).toBe(
      'Preparing the Blox workspace'
    );
    expect(transferPhaseLabel('preparing_destination', 'toLocal')).toBe('Preparing local clones');
    expect(transferPhaseLabel('snapshotting', 'toRemote', 'org/alpha')).toBe(
      'Snapshotting org/alpha'
    );
    expect(transferPhaseLabel('moving', 'toRemote', 'org/alpha', 70 * 1024 * 1024)).toBe(
      'Moving org/alpha (70 MB)'
    );
    expect(transferPhaseLabel('applying', 'toLocal', null)).toBe('Applying');
    expect(transferPhaseLabel('committing', 'toLocal')).toBe('Switching the project over');
    expect(transferPhaseLabel('tearing_down', 'toRemote')).toBe('Removing the old worktrees');
    expect(transferPhaseLabel('tearing_down', 'toLocal')).toBe('Deleting the old workspace');
    expect(transferPhaseLabel('rolling_back', 'toLocal')).toBe('Undoing the transfer');
  });

  it('allows Cancel only before the database switch', () => {
    expect(isTransferCancellable('checking')).toBe(true);
    expect(isTransferCancellable('applying')).toBe(true);
    expect(isTransferCancellable('committing')).toBe(false);
    expect(isTransferCancellable('tearing_down')).toBe(false);
    expect(isTransferCancellable('rolling_back')).toBe(false);
  });

  it('recognises the backend cancellation failure', () => {
    expect(isTransferCancelledError('the transfer was cancelled')).toBe(true);
    expect(isTransferCancelledError('cancel_project_transfer: not found')).toBe(false);
  });
});

describe('issueMessage', () => {
  it('rewrites codes whose backend sentence lacks a next step', () => {
    expect(
      issueMessage({ code: 'detached_head', message: 'The worktree for f has a detached HEAD.' })
    ).toBe('The checkout has a detached HEAD. Check the branch out again before transferring.');
    expect(
      issueMessage({ code: 'wrong_branch', message: 'The worktree for f has main checked out.' })
    ).toBe("The worktree for f has main checked out. Switch back to the project's branch first.");
    expect(
      issueMessage({
        code: 'blox_not_authenticated',
        message: 'Not authenticated with Blox. Run: sq login',
      })
    ).toMatch(/sq login/);
  });

  it('passes the rest through, including the same code for another condition', () => {
    expect(
      issueMessage({ code: 'blox_not_authenticated', message: 'Blox is unavailable: timeout' })
    ).toBe('Blox is unavailable: timeout');
    expect(issueMessage({ code: 'session_running', message: 'A session is running on f.' })).toBe(
      'A session is running on f.'
    );
  });

  it('lists project-level blockers ahead of repo blockers', () => {
    const pf = preflight({
      blockers: [{ code: 'no_repos', message: 'Attach a repository first.' }],
      repos: [repo({ blockers: [{ code: 'conflicts', message: 'f has conflicts.' }] })],
    });
    expect(transferBlockers(pf).map((b) => b.code)).toEqual(['no_repos', 'conflicts']);
  });
});

describe('transferResultWarnings', () => {
  const result = (overrides: Partial<ProjectTransferResult> = {}): ProjectTransferResult => ({
    projectId: 'p1',
    transferId: 't1',
    direction: 'toRemote',
    location: 'remote',
    workspaceName: 'ws',
    repos: [
      {
        branchId: 'b1',
        projectRepoId: 'r1',
        githubRepo: 'org/alpha',
        branchName: 'feature',
        previousBranchName: 'feature',
        carried: true,
        head: 'abc',
        uncommittedRestored: true,
        uncommittedLeftBehind: false,
        retriedWithoutExclusions: false,
        backupRef: null,
        warnings: [],
      },
    ],
    warnings: [],
    backupDir: null,
    ...overrides,
  });

  it('keeps teardown warnings and repo warnings the dialog did not show', () => {
    const lfs = { code: 'lfs', message: 'LFS content is not carried.' };
    const apply = { code: 'apply', message: 'Could not fetch the upstream.' };
    const left = { code: 'worktree_left', message: 'The old worktree stayed.' };
    const r = result({ warnings: [left] });
    r.repos[0].warnings = [lfs, apply];

    expect(transferResultWarnings(r, preflight({ repos: [repo({ warnings: [lfs] })] }))).toEqual([
      left,
      apply,
    ]);
    // Without a preflight to compare against, everything is new.
    expect(transferResultWarnings(r)).toEqual([left, lfs, apply]);
  });
});
