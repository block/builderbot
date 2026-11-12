import { describe, expect, it } from 'vitest';
import type {
  Branch,
  Project,
  ProjectRepo,
  ProjectTransferPreflight,
  ProjectTransferRepoPreflight,
  ProjectTransferResult,
} from '../../types';
import {
  asksAboutIgnored,
  asksAboutUncommitted,
  carriesIgnoredFiles,
  formatBytes,
  ignoredFilesSummary,
  ignoredFoldersSummary,
  ignoredTransferBlocked,
  lfsSummary,
  placeholderPreflight,
  isTransferCancellable,
  isTransferCancelledError,
  isTransferStaleError,
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
    ignoredBytes: 0,
    ignoredDirs: 0,
    lfsPresent: 0,
    lfsPresentBytes: 0,
    lfsMissing: 0,
    lfsMissingBytes: 0,
    hasUncommittedChanges: false,
    scanning: false,
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
    scanning: false,
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

  it('says whether ignored files move or stay in the backup, and that folders stay', () => {
    expect(ignoredFilesSummary(repo(), true)).toBeNull();
    expect(ignoredFilesSummary(repo({ ignoredDirs: 2 }), true)).toBeNull();
    expect(ignoredFilesSummary(repo({ ignoredFiles: 1 }), false)).toBe(
      '1 ignored file stays behind (kept in the backup)'
    );
    expect(ignoredFilesSummary(repo({ ignoredFiles: 3 }), false)).toBe(
      '3 ignored files stay behind (kept in the backup)'
    );
    expect(ignoredFilesSummary(repo({ ignoredFiles: 1 }), true)).toBe(
      '1 ignored file will be transferred'
    );
    expect(ignoredFilesSummary(repo({ ignoredFiles: 3 }), true)).toBe(
      '3 ignored files will be transferred'
    );
    // Over the cap they neither move nor go in the backup (the warning says so).
    const tooLarge = repo({
      ignoredFiles: 3,
      warnings: [{ code: 'ignored_too_large', message: 'too big' }],
    });
    expect(ignoredFilesSummary(tooLarge, true)).toBe('3 ignored files stay behind');
    expect(ignoredFilesSummary(tooLarge, false)).toBe('3 ignored files stay behind');
    expect(carriesIgnoredFiles(tooLarge, true)).toBe(false);
    expect(carriesIgnoredFiles(repo({ ignoredFiles: 3 }), true)).toBe(true);
    expect(carriesIgnoredFiles(repo({ ignoredFiles: 3 }), false)).toBe(false);
    expect(carriesIgnoredFiles(repo(), true)).toBe(false);

    expect(ignoredFoldersSummary(repo({ ignoredFiles: 1 }))).toBeNull();
    expect(ignoredFoldersSummary(repo({ ignoredDirs: 1 }))).toBe('1 ignored folder stays behind');
    expect(ignoredFoldersSummary(repo({ ignoredDirs: 2 }))).toBe('2 ignored folders stay behind');
  });

  it('asks about ignored files only when a repo has some, and blocks them over the cap', () => {
    expect(asksAboutIgnored(preflight())).toBe(false);
    // Folders never move, and a workspace that isn't resumed yet isn't counted.
    expect(asksAboutIgnored(preflight({ repos: [repo({ ignoredDirs: 4 })] }))).toBe(false);
    expect(asksAboutIgnored(preflight({ repos: [repo({ state: 'unknown' })] }))).toBe(false);
    const some = preflight({ repos: [repo(), repo({ branchId: 'b2', ignoredFiles: 1 })] });
    expect(asksAboutIgnored(some)).toBe(true);
    expect(ignoredTransferBlocked(some)).toBeNull();

    const tooLarge = { code: 'ignored_too_large', message: 'too big' };
    const one = preflight({
      repos: [repo({ ignoredFiles: 9, warnings: [tooLarge] }), repo({ branchId: 'b2' })],
    });
    expect(ignoredTransferBlocked(one)).toBe(
      "feature's ignored files are too large to transfer, so they can't move this time."
    );
    const two = preflight({
      repos: [
        repo({ ignoredFiles: 9, warnings: [tooLarge] }),
        repo({ branchId: 'b2', branchName: 'other', ignoredFiles: 9, warnings: [tooLarge] }),
      ],
    });
    expect(ignoredTransferBlocked(two)).toBe(
      "Some branches' ignored files are too large to transfer, so they can't move this time."
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

    // Nor can a checkout the scan is still reading.
    const scanning = preflight({
      repos: [repo(), repo({ branchId: 'b2', scanning: true })],
      scanning: true,
    });
    expect(asksAboutUncommitted(scanning)).toBe(true);
    expect(uncommittedOnlyPossible(scanning)).toBe(true);
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

  it('recognises the backend refusing a direction the project no longer goes', () => {
    // Word for word what check_direction says (orchestrator.rs), both ways.
    expect(
      isTransferStaleError(
        'The project is already on Blox; it was transferred after this dialog opened, so nothing changed.'
      )
    ).toBe(true);
    expect(
      isTransferStaleError(
        'The project is already local; it was transferred after this dialog opened, so nothing changed.'
      )
    ).toBe(true);
    expect(isTransferStaleError('This project is already being transferred.')).toBe(false);
    expect(isTransferStaleError('the transfer was cancelled')).toBe(false);
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

describe('placeholderPreflight', () => {
  const project: Project = {
    id: 'p1',
    name: 'Alpha',
    githubRepo: 'org/alpha',
    location: 'remote',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
  };
  const branch = (overrides: Partial<Branch> = {}): Branch => ({
    id: 'b1',
    projectId: 'p1',
    projectRepoId: 'r1',
    branchName: 'feature',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'remote',
    workspaceName: 'stg-feature',
    workstationId: null,
    workspaceStatus: 'running',
    setupComplete: true,
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
    prTitle: null,
    deferredPush: null,
    deferredPushRequestedAt: null,
    ...overrides,
  });
  const projectRepo: ProjectRepo = {
    id: 'r1',
    projectId: 'p1',
    githubRepo: 'org/beta',
    branchName: 'feature',
    subpath: 'apps/beta',
    isPrimary: false,
    reason: null,
    headRepo: null,
    createdAt: 0,
    updatedAt: 0,
  };

  it('is nothing without branches to show', () => {
    expect(placeholderPreflight(project, [], [projectRepo])).toBeNull();
  });

  it("opens a remote project's dialog with every branch being checked and Transfer live", () => {
    const pf = placeholderPreflight(
      project,
      [branch(), branch({ id: 'b2', projectRepoId: null, branchName: 'gizmo' })],
      [projectRepo]
    );
    expect(pf).toMatchObject({
      projectId: 'p1',
      from: 'remote',
      to: 'local',
      direction: 'toLocal',
      workspaceName: 'stg-feature',
      blockers: [],
      warnings: [],
      canTransfer: true,
      hasUncommittedChanges: false,
      scanning: true,
    });
    expect(pf!.repos.map((r) => [r.branchId, r.githubRepo, r.subpath, r.branchName])).toEqual([
      // The branch's project repo, or — with none — the project's own.
      ['b1', 'org/beta', 'apps/beta', 'feature'],
      ['b2', 'org/alpha', null, 'gizmo'],
    ]);
    for (const row of pf!.repos) {
      expect(row).toMatchObject({
        destBranchName: row.branchName,
        state: 'nothingToCarry',
        scanning: true,
        hasUncommittedChanges: false,
        blockers: [],
        warnings: [],
      });
      expect(uncommittedCount(row)).toBe(0);
    }
    // Rows still being checked may have uncommitted changes nobody has
    // counted yet, so the dialog asks about them.
    expect(asksAboutUncommitted(pf!)).toBe(true);
    expect(uncommittedOnlyPossible(pf!)).toBe(true);
    expect(asksAboutIgnored(pf!)).toBe(false);
  });

  it('goes the other way for a local project, naming no workspace', () => {
    const pf = placeholderPreflight(
      { ...project, location: 'local' },
      [branch({ branchType: 'local', workspaceName: null, worktreePath: '/wt/b1' })],
      [projectRepo]
    );
    expect(pf).toMatchObject({
      from: 'local',
      to: 'remote',
      direction: 'toRemote',
      workspaceName: null,
    });
  });
});
