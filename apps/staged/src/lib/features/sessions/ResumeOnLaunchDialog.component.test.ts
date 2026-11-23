import { flushSync, mount, tick, unmount } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { InterruptedSession, LaunchRecovery } from '../../types';

// The real controller drives the dialog; the test owns the backend calls.
// Dialog chrome passes children through and the two buttons are plain
// buttons that report `disabled` as `data-disabled`.
const { commands } = vi.hoisted(() => ({
  commands: {
    getInterruptedSessions: vi.fn(),
    resumeInterruptedSessions: vi.fn(async () => {}),
  },
}));

vi.mock('../../api/commands', () => commands);
vi.mock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));
vi.mock('$lib/components/ui/alert-dialog', async () => {
  const { default: Passthrough } = await import('./testStubs/PassthroughStub.svelte');
  const { default: ButtonStub } = await import('./testStubs/ButtonStub.svelte');
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

import ResumeOnLaunchDialog from './ResumeOnLaunchDialog.svelte';
import { resumeOnLaunch } from './resumeOnLaunch.svelte';

function session(overrides: Partial<InterruptedSession>): InterruptedSession {
  return {
    sessionId: 's',
    projectId: 'p1',
    branchId: 'b1',
    sessionType: 'commit',
    label: 'commit on fix-login',
    branchName: 'fix-login',
    projectName: 'Widgets',
    disposition: 'resume',
    reason: null,
    interruptedAt: 0,
    ...overrides,
  };
}

const offer: LaunchRecovery = {
  repoSessions: [
    session({ sessionId: 'a', label: 'rebase on drafts-out-of-sync' }),
    session({ sessionId: 'b', projectId: 'p2', projectName: 'Gadgets', label: 'commit on main' }),
    session({
      sessionId: 'c',
      projectId: 'p3',
      projectName: 'Gizmos',
      label: 'push on docs',
      disposition: 'manual',
      reason: 'it never reached its agent',
    }),
  ],
  projectSessions: [],
};

async function settle() {
  await tick();
  flushSync();
}

async function open() {
  commands.getInterruptedSessions.mockResolvedValueOnce(offer);
  const dialog = mount(ResumeOnLaunchDialog, { target: document.body });
  await resumeOnLaunch.load();
  await settle();
  return dialog;
}

const rows = () => [...document.querySelectorAll<HTMLLabelElement>('label[data-row-key]')];
const row = (name: string) =>
  rows().find((r) => r.querySelector('.name')?.textContent === name) ??
  (() => {
    throw new Error(`no row ${name}`);
  })();
const rowCheckbox = (name: string) =>
  row(name).querySelector<HTMLButtonElement>('[role="checkbox"]')!;
const button = (label: string) =>
  [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === label)!;

describe('ResumeOnLaunchDialog', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    commands.resumeInterruptedSessions.mockClear();
  });

  it('shows one row per project with its sessions, resumable ones checked', async () => {
    const dialog = await open();
    expect(rows().map((r) => r.querySelector('.name')?.textContent)).toEqual([
      'Widgets',
      'Gadgets',
      'Gizmos',
    ]);
    expect(row('Widgets').textContent).toContain('rebase on drafts-out-of-sync');
    expect(rowCheckbox('Widgets').getAttribute('aria-checked')).toBe('true');
    expect(rowCheckbox('Gadgets').getAttribute('aria-checked')).toBe('true');
    // Nothing in Gizmos can be resumed: shown with its reason, disabled, unchecked.
    expect(row('Gizmos').textContent).toContain('not restarted automatically: it never reached');
    expect(rowCheckbox('Gizmos').getAttribute('aria-checked')).toBe('false');
    expect(rowCheckbox('Gizmos').disabled).toBe(true);
    expect(button('Resume').hasAttribute('data-disabled')).toBe(false);
    expect(document.body.textContent).toContain('2 of 2 selected');
    await resumeOnLaunch.dismiss();
    unmount(dialog);
  });

  it('toggles a project from anywhere on its row and resumes only the checked ones', async () => {
    const dialog = await open();
    row('Gadgets').querySelector<HTMLElement>('.sessions')!.click();
    await settle();
    expect(rowCheckbox('Gadgets').getAttribute('aria-checked')).toBe('false');
    expect(document.body.textContent).toContain('1 of 2 selected');
    button('Resume').click();
    await settle();
    expect(commands.resumeInterruptedSessions).toHaveBeenCalledWith(['a']);
    unmount(dialog);
  });

  it('disables Resume once nothing is checked', async () => {
    const dialog = await open();
    document.querySelector<HTMLLabelElement>('label.select-all')!.click();
    await settle();
    expect(rowCheckbox('Widgets').getAttribute('aria-checked')).toBe('false');
    expect(button('Resume').hasAttribute('data-disabled')).toBe(true);
    button('Dismiss').click();
    await settle();
    expect(commands.resumeInterruptedSessions).toHaveBeenCalledWith([]);
    unmount(dialog);
  });
});
