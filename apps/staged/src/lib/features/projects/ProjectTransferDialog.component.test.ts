import { flushSync, mount, tick, unmount } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectTransferPreflight, ProjectTransferRepoPreflight } from '../../types';
import type { PendingTransfer } from './projectActions.svelte';

// The dialog is driven by projectActions.pendingTransfer; the test owns that
// state and watches the confirm/cancel calls. Dialog chrome is stubbed to
// pass children through so the body renders in jsdom.
const { actions, prefs } = vi.hoisted(() => ({
  actions: {
    pendingTransfer: null as unknown,
    confirmPendingTransfer: vi.fn(async () => {}),
    cancelPendingTransfer: vi.fn(),
  },
  prefs: { transferUncommittedChanges: true },
}));

vi.mock('$lib/components/ui/dialog', () =>
  import('../sessions/testStubs/dialogMock').then((m) => m.dialogModuleMock())
);
vi.mock('./projectActions.svelte', () => ({ projectActions: actions }));
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
  statusOverride: null,
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

function ready(pf: ProjectTransferPreflight): PendingTransfer {
  return { project, direction: pf.direction, status: 'ready', preflight: pf };
}

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
const checkbox = () => document.querySelector<HTMLButtonElement>('[role="checkbox"]');

describe('ProjectTransferDialog', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    actions.confirmPendingTransfer.mockClear();
    actions.cancelPendingTransfer.mockClear();
    prefs.transferUncommittedChanges = true;
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

  it('lists each repo with its counts and hides the checkbox when nothing is uncommitted', async () => {
    const dialog = mountDialog(
      ready(
        preflight({
          repos: [repo(), repo({ branchId: 'b2', githubRepo: 'org/beta', unpushedCommits: 0 })],
        })
      )
    );
    await settle();
    const rows = document.querySelectorAll('.repo-row');
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain('2 unpushed commits');
    expect(rows[0].textContent).toContain('0 uncommitted changes');
    expect(rows[1].textContent).toContain('0 unpushed commits');
    expect(checkbox()).toBeNull();
    expect(submitButton().disabled).toBe(false);

    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).toHaveBeenCalledWith({ includeUncommitted: true });
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
    expect(document.querySelector('.repo-row')?.textContent).toContain('3 uncommitted changes');
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
    expect(actions.confirmPendingTransfer).toHaveBeenCalledWith({ includeUncommitted: true });
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
              warnings: [{ code: 'lfs', message: 'Git LFS content is not carried.' }],
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
    expect(row?.querySelector('.issue.blocker')?.textContent).toContain(
      'Check the branch out again'
    );
    expect(row?.querySelector('.issue.warning')?.textContent).toContain('LFS');

    submitButton().click();
    await settle();
    expect(actions.confirmPendingTransfer).not.toHaveBeenCalled();
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
