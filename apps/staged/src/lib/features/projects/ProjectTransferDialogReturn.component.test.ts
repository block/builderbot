import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectTransferPreflight, ProjectTransferRepoPreflight } from '../../types';
import type { PendingTransfer } from './projectActions.svelte';

// The transfer dialog's Return key, with the real dialog and alert dialog
// chrome: ProjectTransferDialog.component.test.ts stubs both, and the chrome
// is what decides where focus lands and which keys reach the form. The
// dialog has no text field, so nothing native answers Return in it.
const { actions, prefs } = vi.hoisted(() => ({
  actions: {
    pendingTransfer: null as unknown,
    confirmPendingTransfer: vi.fn(async () => {}),
    cancelPendingTransfer: vi.fn(),
  },
  prefs: { transferUncommittedChanges: true, transferIgnoredFiles: false },
}));

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

function ready(pf: ProjectTransferPreflight): PendingTransfer {
  return { project, direction: pf.direction, status: 'ready', preflight: pf };
}

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;
let pageButton: HTMLButtonElement;

/** The dialogs take focus a frame or a 0 ms timer after opening; let them, then render. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 30));
  await tick();
  flushSync();
}

/** Open the dialog from a focused button on the page behind it, as a menu item does. */
async function openDialog(pending: PendingTransfer) {
  actions.pendingTransfer = null;
  target = document.createElement('div');
  document.body.appendChild(target);
  pageButton = document.createElement('button');
  target.appendChild(pageButton);
  component = mount(ProjectTransferDialog, { target });
  flushSync();
  pageButton.focus();
  actions.pendingTransfer = pending;
  flushSync();
  await settle();
}

function dialogContent(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-slot="dialog-content"]');
  if (!el) throw new Error('transfer dialog not rendered');
  return el;
}

function buttonNamed(name: string): HTMLButtonElement {
  const found = [...document.querySelectorAll('button')].find(
    (b) => b.textContent?.trim() === name
  );
  if (!found) throw new Error(`no "${name}" button`);
  return found;
}

function pressReturn(el: Element, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent('keydown', {
    key: 'Enter',
    bubbles: true,
    cancelable: true,
    ...init,
  });
  el.dispatchEvent(event);
  flushSync();
  return event;
}

beforeEach(() => {
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
      unobserve() {}
    }
  );
  actions.confirmPendingTransfer.mockClear();
  actions.cancelPendingTransfer.mockClear();
  prefs.transferUncommittedChanges = true;
  prefs.transferIgnoredFiles = false;
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  target.remove();
  vi.unstubAllGlobals();
});

describe('ProjectTransferDialog Return key', () => {
  it('opens with focus on the dialog, not on the page behind or a button', async () => {
    await openDialog(ready(preflight()));

    expect(document.activeElement).toBe(dialogContent());
  });

  it('transfers on Return as it opens', async () => {
    await openDialog(ready(preflight()));

    const event = pressReturn(document.activeElement!);

    expect(actions.confirmPendingTransfer).toHaveBeenCalledExactlyOnceWith({
      includeUncommitted: true,
      includeIgnored: false,
    });
    expect(actions.cancelPendingTransfer).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(true);
  });

  it('transfers with the answer a click on the checkbox just gave', async () => {
    await openDialog(
      ready(
        preflight({
          repos: [repo({ unstaged: 1, hasUncommittedChanges: true })],
          hasUncommittedChanges: true,
        })
      )
    );
    document
      .querySelector<HTMLElement>('[role="checkbox"][id^="project-transfer-uncommitted"]')!
      .click();
    flushSync();
    // WebKit leaves a clicked checkbox unfocused and focuses the dialog.
    dialogContent().focus();

    pressReturn(document.activeElement!);

    expect(actions.confirmPendingTransfer).toHaveBeenCalledExactlyOnceWith({
      includeUncommitted: false,
      includeIgnored: false,
    });
  });

  it('does not transfer on Return while the preflight is loading', async () => {
    await openDialog({ project, direction: 'toRemote', status: 'loading' });
    expect(document.activeElement).toBe(dialogContent());

    const event = pressReturn(document.activeElement!);

    expect(actions.confirmPendingTransfer).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it('does not transfer on Return while something blocks the transfer', async () => {
    await openDialog(
      ready(
        preflight({
          canTransfer: false,
          blockers: [{ code: 'session_running', message: 'A session is still running.' }],
        })
      )
    );

    pressReturn(document.activeElement!);

    expect(actions.confirmPendingTransfer).not.toHaveBeenCalled();
  });

  it('does not transfer on Return on a focused Cancel', async () => {
    await openDialog(ready(preflight()));
    buttonNamed('Cancel').focus();

    // The browser turns this keydown into Cancel's own click.
    const event = pressReturn(document.activeElement!);

    expect(actions.confirmPendingTransfer).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it('does not transfer on a held Return repeating, such as the press that opened it', async () => {
    await openDialog(ready(preflight()));

    pressReturn(document.activeElement!, { repeat: true });

    expect(actions.confirmPendingTransfer).not.toHaveBeenCalled();
  });

  it('asks first on Return mid-scan, and a second Return transfers anyway', async () => {
    await openDialog(ready(preflight({ repos: [repo({ scanning: true })], scanning: true })));

    pressReturn(document.activeElement!);
    await settle();

    expect(actions.confirmPendingTransfer).not.toHaveBeenCalled();
    expect(document.activeElement?.getAttribute('role')).toBe('alertdialog');
    expect(document.activeElement?.textContent).toContain('Transfer anyway');

    pressReturn(document.activeElement!);
    await settle();

    expect(actions.confirmPendingTransfer).toHaveBeenCalledTimes(1);
    expect(document.querySelector('[role="alertdialog"]')).toBeNull();
  });
});
