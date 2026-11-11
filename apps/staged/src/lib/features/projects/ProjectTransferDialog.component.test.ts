import { flushSync, mount, tick, unmount } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  Branch,
  Project,
  ProjectRepo,
  ProjectTransferPreflight,
  ProjectTransferRepoPreflight,
} from '../../types';
import type { PendingTransfer } from './projectActions.svelte';
import { placeholderPreflight } from './projectTransfer';

// The dialog is driven by projectActions.pendingTransfer; the test owns that
// state and watches the confirm/cancel calls. Dialog chrome is stubbed to
// pass children through so the body renders in jsdom.
const { actions, prefs } = vi.hoisted(() => ({
  actions: {
    pendingTransfer: null as unknown,
    confirmPendingTransfer: vi.fn(async () => {}),
    cancelPendingTransfer: vi.fn(),
  },
  prefs: { transferUncommittedChanges: true, transferIgnoredFiles: false },
}));

vi.mock('$lib/components/ui/dialog', () =>
  import('../sessions/testStubs/dialogMock').then((m) => m.dialogModuleMock())
);
// The mid-scan confirmation is an AlertDialog; its chrome passes children
// through and its two buttons are plain buttons, so the test can click them.
vi.mock('$lib/components/ui/alert-dialog', async () => {
  const { default: Passthrough } = await import('../sessions/testStubs/PassthroughStub.svelte');
  const { default: ButtonStub } = await import('../sessions/testStubs/ButtonStub.svelte');
  return {
    Root: Passthrough,
    Content: Passthrough,
    Header: Passthrough,
    Footer: Passthrough,
    Title: Passthrough,
    Description: Passthrough,
    Action: ButtonStub,
    Cancel: ButtonStub,
  };
});
// `pendingTransfer` is $state-backed, as the real controller's is, so a test
// can move the mounted dialog through loading → ready, a close and a reopen.
vi.mock('./projectActions.svelte', async () => {
  const { reactiveProps } = await import('../sessions/testStubs/reactiveProps.svelte');
  const state = reactiveProps({ pending: null as unknown });
  Object.defineProperty(actions, 'pendingTransfer', {
    get: () => state.pending,
    set: (value: unknown) => {
      state.pending = value;
    },
  });
  return { projectActions: actions };
});
vi.mock('../settings/preferences.svelte', () => ({ preferences: prefs }));

import ProjectTransferDialog from './ProjectTransferDialog.svelte';

const project: Project = {
  id: 'p1',
  name: 'Alpha',
  githubRepo: 'org/alpha',
  location: 'local',
  subpath: null,
  createdAt: 0,
  updatedAt: 0,
};

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
    upstream: 'origin/feature',
    unpushedCommits: 2,
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

function ready(pf: ProjectTransferPreflight, forProject: Project = project): PendingTransfer {
  return { project: forProject, direction: pf.direction, status: 'ready', preflight: pf };
}

/** A branch as the projects store knows it, for the dialog's placeholder. */
function knownBranch(overrides: Partial<Branch> = {}): Branch {
  return {
    id: 'b1',
    projectId: 'p1',
    projectRepoId: 'r1',
    branchName: 'feature',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    setupComplete: true,
    worktreePath: '/wt/b1',
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

const knownRepo: ProjectRepo = {
  id: 'r1',
  projectId: 'p1',
  githubRepo: 'org/alpha',
  branchName: 'feature',
  subpath: null,
  isPrimary: true,
  reason: null,
  headRepo: null,
  createdAt: 0,
  updatedAt: 0,
};

const dirty = () =>
  preflight({
    repos: [repo({ unstaged: 1, hasUncommittedChanges: true })],
    hasUncommittedChanges: true,
  });

function mountDialog(pending: PendingTransfer | null) {
  actions.pendingTransfer = pending;
  const target = document.createElement('div');
  document.body.appendChild(target);
  const dialog = mount(ProjectTransferDialog, { target });
  flushSync();
  return dialog;
}

async function settle() {
  for (let i = 0; i < 4; i++) await tick();
  flushSync();
}

const submitButton = () =>
  document.querySelector<HTMLButtonElement>('button[type="submit"]') ??
  (() => {
    throw new Error('no submit button');
  })();
const checkbox = () =>
  document.querySelector<HTMLButtonElement>(
    '[role="checkbox"][id^="project-transfer-uncommitted"]'
  );
const ignoredCheckbox = () =>
  document.querySelector<HTMLButtonElement>('[role="checkbox"][id^="project-transfer-ignored"]');
const rowTexts = () =>
  [...document.querySelectorAll('.repo-row')].map((r) => r.textContent?.replace(/\s+/g, ' '));
const tooLarge = {
  code: 'ignored_too_large',
  message:
    "feature's ignored files come to 300 MB, more than the 256 MB a transfer can carry or back up, so they are deleted with the worktree.",
};

describe('ProjectTransferDialog', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    actions.confirmPendingTransfer.mockClear();
    actions.cancelPendingTransfer.mockClear();
    prefs.transferUncommittedChanges = true;
    prefs.transferIgnoredFiles = false;
  });

  it('shows the preflight loading with Transfer disabled', async () => {
    const dialog = mountDialog({ project, direction: 'toRemote', status: 'loading' });
    await settle();
    expect(document.body.textContent).toContain('Checking what would move');
    expect(submitButton().disabled).toBe(true);
    expect(submitButton().textContent).toContain('Transfer to Blox');
    expect(checkbox()).toBeNull();
    unmount(dialog);
  });

  it('shows the branches it knows as Checking… with Transfer live before the scan has said anything', async () => {
    // What projectActions opens the dialog with, from the projects store
    // alone: no preflight event has arrived, or even been asked for.
    const remote: Project = { ...project, location: 'remote' };
    const placeholder = placeholderPreflight(
      remote,
      [
        knownBranch({ branchType: 'remote', workspaceName: 'stg-feature', worktreePath: null }),
        knownBranch({
          id: 'b2',
          projectRepoId: null,
          branchName: 'gizmo',
          branchType: 'remote',
          workspaceName: 'stg-feature',
          worktreePath: null,
        }),
      ],
      [knownRepo]
    );
    expect(placeholder).not.toBeNull();
    const dialog = mountDialog(ready(placeholder!, remote));
    await settle();
    expect(document.body.textContent).not.toContain('Checking what would move');
    const rows = rowTexts();
    expect(rows).toHaveLength(2);
    expect(rows[0]).toContain('feature');
    expect(rows[0]).toContain('Checking…');
    expect(rows[1]).toContain('gizmo');
    expect(rows[1]).toContain('Checking…');
    expect(document.body.textContent).toContain('Still checking what would be left behind');
    expect(document.body.textContent).toContain('Transfer uncommitted changes, if there are any');
    expect(submitButton().textContent).toContain('Transfer to local');
    expect(submitButton().disabled).toBe(false);

    // Transfer now goes through the mid-scan confirmation, like any confirm
    // before the scan is done.
    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain(
      "The scan of what will be left behind hasn't finished; files not yet listed may be lost on the remote. Transfer anyway?"
    );
    [...document.querySelectorAll('button')]
      .find((b) => b.textContent?.trim() === 'Transfer anyway')!
      .click();
    await settle();
    expect(actions.confirmPendingTransfer).toHaveBeenCalledWith({
      includeUncommitted: true,
      includeIgnored: false,
    });
    unmount(dialog);
  });

  it('lists each repo with its upstream and counts and hides the checkbox when nothing is uncommitted', async () => {
    const dialog = mountDialog(
      ready(
        preflight({
          repos: [
            repo(),
            repo({ branchId: 'b2', githubRepo: 'org/beta', upstream: null, unpushedCommits: 0 }),
          ],
        })
      )
    );
    await settle();
    const rows = document.querySelectorAll('.repo-row');
    expect(rows).toHaveLength(2);
    expect(rows[0].querySelector('.upstream')?.textContent?.trim()).toBe('tracks origin/feature');
    expect(rows[0].textContent).toContain('2 unpushed commits');
    expect(rows[0].textContent).toContain('0 uncommitted changes');
    expect(rows[1].querySelector('.upstream')?.textContent?.trim()).toBe('no upstream');
    expect(rows[1].textContent).toContain('0 unpushed commits');
    expect(checkbox()).toBeNull();
    expect(ignoredCheckbox()).toBeNull();
    expect(document.body.textContent).not.toContain('Also transfer ignored files');
    expect(document.body.textContent).not.toContain('backed up');
    expect(submitButton().disabled).toBe(false);

    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).toHaveBeenCalledWith({
      includeUncommitted: true,
      includeIgnored: false,
    });
    unmount(dialog);
  });

  it('shows the checkbox from the remembered answer and reports it on confirm', async () => {
    prefs.transferUncommittedChanges = false;
    const dialog = mountDialog(
      ready(
        preflight({
          repos: [repo({ staged: 1, untracked: 2, hasUncommittedChanges: true })],
          hasUncommittedChanges: true,
        })
      )
    );
    await settle();
    // The kinds are spelled out, not left to a tooltip.
    expect(document.querySelector('.repo-row')?.textContent).toContain(
      '3 uncommitted changes\u00a0(1 staged, 2 untracked)'
    );
    expect(document.body.textContent).toContain('Transfer uncommitted changes');
    expect(document.body.textContent).not.toContain('if there are any');
    const box = checkbox();
    expect(box).not.toBeNull();
    expect(box!.getAttribute('aria-checked')).toBe('false');
    // Off means left behind, and the dialog says so.
    expect(document.body.textContent).toContain('left behind');

    box!.click();
    await settle();
    expect(box!.getAttribute('aria-checked')).toBe('true');
    expect(document.body.textContent).not.toContain('left behind');

    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).toHaveBeenCalledWith({
      includeUncommitted: true,
      includeIgnored: false,
    });
    unmount(dialog);
  });

  it('disables Transfer on blockers and says why, in friendlier words where needed', async () => {
    const dialog = mountDialog(
      ready(
        preflight({
          canTransfer: false,
          blockers: [{ code: 'sq_unavailable', message: 'The sq CLI is not installed.' }],
          repos: [
            repo({
              blockers: [
                { code: 'detached_head', message: 'The worktree for feature has a detached HEAD.' },
              ],
              warnings: [
                {
                  code: 'lfs_missing',
                  message:
                    '2 LFS objects are not available locally and will be fetched from origin if possible.',
                },
              ],
            }),
          ],
        })
      )
    );
    await settle();
    expect(submitButton().disabled).toBe(true);
    const alert = document.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("Can't transfer yet");
    expect(alert?.textContent).toContain("The sq CLI isn't installed");
    expect(alert?.textContent).toContain('Resolve the issues marked on the branches above');
    const row = document.querySelector('.repo-row');
    expect(row?.classList.contains('blocked')).toBe(true);
    // Told apart by more than colour: a prefix for screen readers.
    const blocker = row?.querySelector('.issue.blocker');
    expect(blocker?.querySelector('.sr-only')?.textContent).toBe('Blocks transfer:');
    expect(blocker?.textContent).toContain('Check the branch out again');
    const warning = row?.querySelector('.issue.warning');
    expect(warning?.querySelector('.sr-only')?.textContent).toBe('Warning:');
    expect(warning?.textContent).toContain('2 LFS objects are not available locally');

    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).not.toHaveBeenCalled();
    unmount(dialog);
  });

  it('asks about uncommitted changes a resumed workspace may have', async () => {
    prefs.transferUncommittedChanges = false;
    const dialog = mountDialog(
      ready(
        preflight({
          direction: 'toLocal',
          repos: [repo({ state: 'unknown', unpushedCommits: 0 })],
        })
      )
    );
    await settle();
    const row = document.querySelector('.repo-row');
    expect(row?.textContent).toContain('Checked once the workspace resumes');
    // Not read yet, so neither "tracks …" nor "no upstream".
    expect(row?.querySelector('.upstream')).toBeNull();
    expect(document.body.textContent).toContain('Transfer uncommitted changes, if there are any');
    expect(checkbox()!.getAttribute('aria-checked')).toBe('false');

    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).toHaveBeenCalledWith({
      includeUncommitted: false,
      includeIgnored: false,
    });
    unmount(dialog);
  });

  it('says how many ignored files and folders stay behind', async () => {
    const dialog = mountDialog(
      ready(
        preflight({
          repos: [
            repo({ ignoredFiles: 3 }),
            repo({ branchId: 'b2', ignoredFiles: 1 }),
            repo({ branchId: 'b3' }),
            repo({ branchId: 'b4', ignoredFiles: 1, ignoredDirs: 2 }),
            repo({ branchId: 'b5', ignoredDirs: 1 }),
          ],
        })
      )
    );
    await settle();
    const rows = rowTexts();
    expect(rows[0]).toContain('3 ignored files stay behind (kept in the backup)');
    expect(rows[1]).toContain('1 ignored file stays behind (kept in the backup)');
    expect(rows[2]).not.toContain('ignored');
    expect(rows[3]).toContain('1 ignored file stays behind (kept in the backup)');
    expect(rows[3]).toContain('2 ignored folders stay behind');
    expect(rows[4]).toContain('1 ignored folder stays behind');
    expect(rows[4]).not.toContain('backup');
    unmount(dialog);
  });

  it('asks about ignored files only when some repo has them, from the remembered answer', async () => {
    // Ignored folders alone don't ask: they never move.
    let dialog = mountDialog(ready(preflight({ repos: [repo({ ignoredDirs: 2 })] })));
    await settle();
    expect(ignoredCheckbox()).toBeNull();
    unmount(dialog);

    document.body.innerHTML = '';
    const pf = preflight({
      repos: [repo({ ignoredFiles: 2, ignoredDirs: 1 }), repo({ branchId: 'b2' })],
    });
    dialog = mountDialog(ready(pf));
    await settle();
    const box = ignoredCheckbox();
    expect(box).not.toBeNull();
    expect(box!.disabled).toBe(false);
    expect(box!.getAttribute('aria-checked')).toBe('false');
    const label = document.querySelector(`label[for="${box!.id}"]`);
    expect(label?.textContent?.replace(/\s+/g, ' ').trim()).toBe(
      'Also transfer ignored files (such as .env)'
    );
    expect(document.getElementById(box!.getAttribute('aria-describedby')!)?.textContent).toContain(
      'Ignored folders such as node_modules are never transferred.'
    );
    expect(document.body.textContent?.replace(/\s+/g, ' ')).toContain(
      'after their ignored files are backed up on this machine'
    );
    expect(rowTexts()[0]).toContain('2 ignored files stay behind (kept in the backup)');

    box!.click();
    await settle();
    expect(box!.getAttribute('aria-checked')).toBe('true');
    expect(rowTexts()[0]).toContain('2 ignored files will be transferred');
    expect(rowTexts()[0]).toContain('1 ignored folder stays behind');
    expect(document.querySelector('.repo-row')?.textContent).not.toContain('kept in the backup');

    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).toHaveBeenCalledWith({
      includeUncommitted: true,
      includeIgnored: true,
    });
    unmount(dialog);

    // Each open starts from the preference.
    document.body.innerHTML = '';
    prefs.transferIgnoredFiles = true;
    dialog = mountDialog(ready(pf, { ...project, id: 'p2' }));
    await settle();
    expect(ignoredCheckbox()!.getAttribute('aria-checked')).toBe('true');
    expect(rowTexts()[0]).toContain('2 ignored files will be transferred');
    unmount(dialog);
  });

  it('keeps the two checkboxes independent', async () => {
    const dialog = mountDialog(
      ready(
        preflight({
          repos: [repo({ unstaged: 1, hasUncommittedChanges: true, ignoredFiles: 1 })],
          hasUncommittedChanges: true,
        })
      )
    );
    await settle();
    const uncommitted = checkbox()!;
    const ignored = ignoredCheckbox()!;
    expect(uncommitted.id).not.toBe(ignored.id);
    expect([uncommitted, ignored].map((b) => b.getAttribute('aria-checked'))).toEqual([
      'true',
      'false',
    ]);

    ignored.click();
    await settle();
    expect([uncommitted, ignored].map((b) => b.getAttribute('aria-checked'))).toEqual([
      'true',
      'true',
    ]);
    uncommitted.click();
    await settle();
    expect([uncommitted, ignored].map((b) => b.getAttribute('aria-checked'))).toEqual([
      'false',
      'true',
    ]);
    // Leaving uncommitted changes behind doesn't hold the ignored files back.
    expect(rowTexts()[0]).toContain('1 ignored file will be transferred');

    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).toHaveBeenCalledWith({
      includeUncommitted: false,
      includeIgnored: true,
    });
    unmount(dialog);
  });

  it('blocks the ignored-files checkbox when they are over the cap and says why', async () => {
    prefs.transferIgnoredFiles = true;
    const dialog = mountDialog(
      ready(
        preflight({
          repos: [
            repo({ ignoredFiles: 4, ignoredBytes: 300 * 1024 * 1024, warnings: [tooLarge] }),
            repo({ branchId: 'b2', branchName: 'other', ignoredFiles: 1 }),
          ],
        })
      )
    );
    await settle();
    const box = ignoredCheckbox()!;
    expect(box.disabled).toBe(true);
    expect(box.getAttribute('aria-checked')).toBe('false');
    expect(document.getElementById(box.getAttribute('aria-describedby')!)?.textContent).toContain(
      "feature's ignored files are too large to transfer"
    );
    // Neither repo's move, and the big ones aren't in the backup either.
    const rows = rowTexts();
    expect(rows[0]).toContain('4 ignored files stay behind');
    expect(rows[0]).not.toContain('kept in the backup');
    expect(rows[0]).toContain('more than the 256 MB a transfer can carry');
    expect(rows[1]).toContain('1 ignored file stays behind (kept in the backup)');
    // The footnote promises a backup only of the ones under the limit.
    const text = document.body.textContent?.replace(/\s+/g, ' ');
    expect(text).toContain(
      'after ignored files under the size limit are backed up on this machine'
    );
    expect(text).not.toContain('after their ignored files are backed up');
    // A warning, not a blocker: the transfer can go without them.
    expect(submitButton().disabled).toBe(false);

    box.click();
    await settle();
    expect(box.getAttribute('aria-checked')).toBe('false');
    unmount(dialog);
  });

  it('says how many LFS objects each repo carries', async () => {
    const dialog = mountDialog(
      ready(
        preflight({
          repos: [
            repo({ lfsPresent: 3, lfsPresentBytes: 5 * 1024 * 1024 }),
            repo({ branchId: 'b2' }),
          ],
        })
      )
    );
    await settle();
    const rows = [...document.querySelectorAll('.repo-row')].map((r) =>
      r.textContent?.replace(/\s+/g, ' ')
    );
    expect(rows[0]).toContain('3 LFS objects (5.0 MB) will be carried');
    expect(rows[1]).not.toContain('LFS');
    unmount(dialog);
  });

  // Each open starts from the remembered answer, and nothing but a new open
  // resets it: not the preflight landing, nor the preference changing in
  // another window meanwhile.
  it('keeps the answer through loading → ready and resets it for each new open', async () => {
    const dialog = mountDialog({ project, direction: 'toRemote', status: 'loading' });
    await settle();
    prefs.transferUncommittedChanges = false;

    actions.pendingTransfer = ready(dirty());
    await settle();
    expect(checkbox()!.getAttribute('aria-checked')).toBe('true');
    checkbox()!.click();
    await settle();
    expect(checkbox()!.getAttribute('aria-checked')).toBe('false');

    // Reopened straight onto another project: from the preference again.
    prefs.transferUncommittedChanges = true;
    actions.pendingTransfer = ready(dirty(), { ...project, id: 'p2', name: 'Beta' });
    await settle();
    expect(checkbox()!.getAttribute('aria-checked')).toBe('true');
    checkbox()!.click();
    await settle();
    expect(checkbox()!.getAttribute('aria-checked')).toBe('false');

    // Closed, then reopened for the same project: from the preference too.
    actions.pendingTransfer = null;
    await settle();
    expect(checkbox()).toBeNull();
    actions.pendingTransfer = ready(dirty());
    await settle();
    expect(checkbox()!.getAttribute('aria-checked')).toBe('true');

    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).toHaveBeenCalledWith({
      includeUncommitted: true,
      includeIgnored: false,
    });
    unmount(dialog);
  });

  it('shows the rows as the scan reads them and lets Transfer go once confirmed', async () => {
    const scanning = preflight({
      repos: [repo({ scanning: true }), repo({ branchId: 'b2', githubRepo: 'org/beta' })],
      scanning: true,
    });
    const dialog = mountDialog(ready(scanning));
    await settle();
    const rows = rowTexts();
    expect(rows[0]).toContain('Checking…');
    expect(rows[0]).not.toContain('unpushed');
    expect(rows[1]).toContain('2 unpushed commits');
    expect(document.body.textContent).toContain('Still checking what would be left behind');
    // A row still being read may have uncommitted changes nobody has counted.
    expect(document.body.textContent).toContain('Transfer uncommitted changes, if there are any');
    expect(submitButton().disabled).toBe(false);
    expect(document.body.textContent).not.toContain('Transfer anyway');

    // Transfer mid-scan asks first, in these exact words.
    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain(
      "The scan of what will be left behind hasn't finished; files not yet listed may be lost on the remote. Transfer anyway?"
    );
    const button = (label: string) =>
      [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === label)!;

    // Backing out keeps the dialog, and the scan, going.
    button('Keep checking').click();
    await settle();
    expect(document.body.textContent).not.toContain('Transfer anyway');
    expect(actions.confirmPendingTransfer).not.toHaveBeenCalled();
    expect(actions.cancelPendingTransfer).not.toHaveBeenCalled();

    submitButton().click();
    await settle();
    button('Transfer anyway').click();
    await settle();
    expect(actions.confirmPendingTransfer).toHaveBeenCalledWith({
      includeUncommitted: true,
      includeIgnored: false,
    });
    expect(document.body.textContent).not.toContain('Transfer anyway');

    // Once the scan lands, Transfer goes straight through.
    actions.confirmPendingTransfer.mockClear();
    actions.pendingTransfer = ready(
      preflight({ repos: scanning.repos.map((r) => ({ ...r, scanning: false })) })
    );
    await settle();
    expect(document.body.textContent).not.toContain('Still checking');
    expect(rowTexts()[0]).toContain('2 unpushed commits');
    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).toHaveBeenCalledTimes(1);
    expect(document.body.textContent).not.toContain('Transfer anyway');
    unmount(dialog);
  });

  it('keeps Transfer disabled while the scan runs if something blocks it', async () => {
    const dialog = mountDialog(
      ready(
        preflight({
          canTransfer: false,
          scanning: true,
          repos: [
            repo({
              scanning: true,
              blockers: [{ code: 'session_running', message: 'A session is running on feature.' }],
            }),
          ],
        })
      )
    );
    await settle();
    expect(submitButton().disabled).toBe(true);
    submitButton().click();
    await settle();
    expect(document.body.textContent).not.toContain('Transfer anyway');
    unmount(dialog);
  });

  it('shows a failed preflight and lets Cancel dismiss it', async () => {
    const dialog = mountDialog({
      project,
      direction: 'toLocal',
      status: 'error',
      error: 'sq down',
    });
    await settle();
    expect(document.querySelector('[role="alert"]')?.textContent).toContain('sq down');
    expect(submitButton().disabled).toBe(true);
    const cancel = [...document.querySelectorAll('button')].find(
      (b) => b.textContent?.trim() === 'Cancel'
    );
    cancel!.click();
    expect(actions.cancelPendingTransfer).toHaveBeenCalled();
    unmount(dialog);
  });
});
