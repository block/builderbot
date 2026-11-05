import { describe, expect, it } from 'vitest';
import type {
  ProjectTransferPreflight,
  ProjectTransferRepoPreflight,
  ProjectTransferResult,
} from '../../types';
import {
  asksAboutUncommitted,
  formatBytes,
  ignoredSummary,
  lfsSummary,
  isTransferCancellable,
  isTransferCancelledError,
  issueMessage,
  plural,
  transferActionLabel,
  transferDirectionFrom,
  transferPhaseLabel,
  transferResultWarnings,
  transferringTitle,
  uncommittedBreakdown,
  uncommittedCount,
  uncommittedOnlyPossible,
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
    ignoredDirs: 0,
    lfsPresent: 0,
    lfsPresentBytes: 0,
    lfsMissing: 0,
    lfsMissingBytes: 0,
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
    expect(transferringTitle('toRemote')).toBe('Transferring to Blox…');
    expect(transferringTitle('toLocal')).toBe('Transferring to local…');
    expect(transferringTitle(undefined)).toBe('Transferring…');
  });

  it('counts every kind of uncommitted change', () => {
    expect(uncommittedCount(repo({ staged: 1, unstaged: 2, untracked: 3, conflicted: 4 }))).toBe(
      10
    );
    expect(plural(1, 'unpushed commit')).toBe('1 unpushed commit');
    expect(plural(2, 'uncommitted change')).toBe('2 uncommitted changes');
    expect(uncommittedBreakdown(repo({ staged: 1, untracked: 2 }))).toBe('1 staged, 2 untracked');
    expect(uncommittedBreakdown(repo())).toBeNull();
  });

  it('sums up what stays behind as ignored, folders included', () => {
    expect(ignoredSummary(repo())).toBeNull();
    expect(ignoredSummary(repo({ ignoredFiles: 1 }))).toBe('1 ignored file stays behind');
    expect(ignoredSummary(repo({ ignoredDirs: 1 }))).toBe('1 ignored folder stays behind');
    expect(ignoredSummary(repo({ ignoredDirs: 2 }))).toBe('2 ignored folders stay behind');
    expect(ignoredSummary(repo({ ignoredFiles: 3, ignoredDirs: 1 }))).toBe(
      '3 ignored files and 1 ignored folder stay behind'
    );
  });

  it('says how many LFS objects a repo carries, and how much they hold', () => {
    expect(lfsSummary(repo())).toBeNull();
    // Missing objects are a warning of their own, not something carried.
    expect(lfsSummary(repo({ lfsMissing: 2, lfsMissingBytes: 4096 }))).toBeNull();
    expect(lfsSummary(repo({ lfsPresent: 1, lfsPresentBytes: 7 }))).toBe(
      '1 LFS object (7 B) will be carried'
    );
    expect(lfsSummary(repo({ lfsPresent: 3, lfsPresentBytes: 12 * 1024 * 1024 }))).toBe(
      '3 LFS objects (12 MB) will be carried'
    );
  });

  it('asks about uncommitted changes when a repo has some or may have some', () => {
    expect(asksAboutUncommitted(preflight())).toBe(false);
    expect(asksAboutUncommitted(preflight({ hasUncommittedChanges: true }))).toBe(true);

    // A workspace the transfer resumes first can't be counted yet.
    const unknown = preflight({ repos: [repo(), repo({ branchId: 'b2', state: 'unknown' })] });
    expect(asksAboutUncommitted(unknown)).toBe(true);
    expect(uncommittedOnlyPossible(unknown)).toBe(true);
    expect(uncommittedOnlyPossible({ ...unknown, hasUncommittedChanges: true })).toBe(false);
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
    // The backend's sentence names the branch and its worktree or workspace,
    // which a toast has no row to supply, so the next step is appended.
    expect(
      issueMessage({ code: 'detached_head', message: 'The worktree for f has a detached HEAD.' })
    ).toBe(
      'The worktree for f has a detached HEAD. Check the branch out again before transferring.'
    );
    expect(issueMessage({ code: 'conflicts', message: 'f has unresolved conflicts.' })).toBe(
      'f has unresolved conflicts. Resolve them, or abort the merge or rebase, first.'
    );
    expect(
      issueMessage({ code: 'wrong_branch', message: 'The worktree for f has main checked out.' })
    ).toBe("The worktree for f has main checked out. Switch back to the project's branch first.");
    expect(
      issueMessage({
        code: 'blox_not_authenticated',
        message: 'Not authenticated with Blox. Run: sq login',
      })
    ).toBe("You're not signed in to Blox. Run “sq login” in a terminal, then try again.");
  });

  it('passes the rest through, including the same code for another condition', () => {
    expect(
      issueMessage({ code: 'blox_not_authenticated', message: 'Blox is unavailable: timeout' })
    ).toBe('Blox is unavailable: timeout');
    expect(issueMessage({ code: 'session_running', message: 'A session is running on f.' })).toBe(
      'A session is running on f.'
    );
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
    const lfs = {
      code: 'lfs_missing',
      message:
        '2 LFS objects are not available locally and will be fetched from origin if possible.',
    };
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
