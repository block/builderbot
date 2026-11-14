import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { StagedctlLinkOutcome, StagedctlShownLink } from '../../types';

// The setting calls one command and shows what it answered. The command is
// mocked, so nothing here touches a PATH directory; the button and spinner
// are stubbed to plain elements the test can click.
const { commands } = vi.hoisted(() => ({
  commands: {
    linkStagedctlOnPath: vi.fn<(replace?: StagedctlShownLink) => Promise<StagedctlLinkOutcome>>(),
  },
}));

vi.mock('../../api/commands', () => commands);
vi.mock('$lib/components/ui/button', async () => {
  const { default: ButtonStub } = await import('../sessions/testStubs/ButtonStub.svelte');
  return { Button: ButtonStub };
});
vi.mock('../../shared/Spinner.svelte', async () => {
  const { default: Empty } = await import('../sessions/testStubs/EmptyStub.svelte');
  return { default: Empty };
});

import StagedctlPathSetting from './StagedctlPathSetting.svelte';

const SYSTEM_LINK = '/usr/local/bin/stagedctl';
const USER_LINK = '/Users/me/.local/bin/stagedctl';
const OTHER_BUILD = '/Users/me/old/Staged.app/Contents/MacOS/stagedctl';

let host: HTMLElement;
let setting: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  commands.linkStagedctlOnPath.mockReset();
  setting = mount(StagedctlPathSetting, { target: host });
  flushSync();
});

afterEach(() => {
  if (setting) unmount(setting);
  setting = null;
  host.remove();
});

async function flushPromises() {
  for (let i = 0; i < 3; i++) await new Promise((r) => setTimeout(r, 0));
  flushSync();
}

function button(label: string): HTMLButtonElement {
  const found = Array.from(host.querySelectorAll('button')).filter(
    (b) => b.textContent?.trim() === label
  );
  expect(found, `one "${label}" button`).toHaveLength(1);
  return found[0];
}

async function press(label: string) {
  button(label).click();
  await flushPromises();
}

function text(testId: string): string | null {
  const element = host.querySelector(`[data-testid="${testId}"]`);
  return element ? (element.textContent ?? '').replace(/\s+/g, ' ').trim() : null;
}

describe('StagedctlPathSetting', () => {
  it('does nothing until the button is pressed', () => {
    expect(commands.linkStagedctlOnPath).not.toHaveBeenCalled();
    expect(host.querySelector('[role="status"]')?.textContent?.trim()).toBe('');
    expect(host.querySelector('[role="alert"]')).toBeNull();
  });

  it('asks for the link with nothing to replace, and says where it went', async () => {
    commands.linkStagedctlOnPath.mockResolvedValue({
      outcome: 'linked',
      link: SYSTEM_LINK,
      onPath: true,
    });

    await press('Add to PATH');

    expect(commands.linkStagedctlOnPath).toHaveBeenCalledExactlyOnceWith(undefined);
    expect(text('stagedctl-linked')).toBe(`Linked ${SYSTEM_LINK} to this copy of Staged.`);
    expect(text('stagedctl-path-hint')).toBeNull();
    // The result is announced through the live region.
    expect(host.querySelector('[role="status"] [data-testid="stagedctl-linked"]')).not.toBeNull();
  });

  it('says so when the link was already there', async () => {
    commands.linkStagedctlOnPath.mockResolvedValue({
      outcome: 'alreadyLinked',
      link: SYSTEM_LINK,
      onPath: true,
    });

    await press('Add to PATH');

    expect(text('stagedctl-linked')).toBe(`${SYSTEM_LINK} already points to this copy of Staged.`);
  });

  it('names the directory to add when the link is not on PATH', async () => {
    commands.linkStagedctlOnPath.mockResolvedValue({
      outcome: 'linked',
      link: USER_LINK,
      onPath: false,
    });

    await press('Add to PATH');

    expect(text('stagedctl-linked')).toBe(`Linked ${USER_LINK} to this copy of Staged.`);
    expect(text('stagedctl-path-hint')).toBe(
      '/Users/me/.local/bin is not on your PATH. Add it in your shell profile, then open a new terminal.'
    );
  });

  it('does not claim the directory is missing from a PATH it could not read', async () => {
    commands.linkStagedctlOnPath.mockResolvedValue({
      outcome: 'alreadyLinked',
      link: USER_LINK,
      onPath: null,
    });

    await press('Add to PATH');

    expect(text('stagedctl-path-hint')).toBe(
      'Staged could not read your PATH. If a new terminal cannot find stagedctl, add /Users/me/.local/bin to your PATH.'
    );
  });

  it('shows a link that points elsewhere and replaces it only when asked', async () => {
    commands.linkStagedctlOnPath.mockResolvedValueOnce({
      outcome: 'conflict',
      link: SYSTEM_LINK,
      existing: { kind: 'link', target: OTHER_BUILD },
    });

    await press('Add to PATH');

    // What is there is on screen before anything can replace it.
    expect(text('stagedctl-conflict')).toBe(
      `${SYSTEM_LINK} already exists and points to ${OTHER_BUILD}. Nothing was changed. ` +
        'Replace it to make stagedctl run this copy of Staged.'
    );
    expect(commands.linkStagedctlOnPath).toHaveBeenCalledTimes(1);

    commands.linkStagedctlOnPath.mockResolvedValueOnce({
      outcome: 'linked',
      link: SYSTEM_LINK,
      onPath: true,
    });
    await press('Replace link');

    // The confirmation is exactly the link and target that were shown.
    expect(commands.linkStagedctlOnPath).toHaveBeenLastCalledWith({
      link: SYSTEM_LINK,
      target: OTHER_BUILD,
    });
    expect(text('stagedctl-conflict')).toBeNull();
    expect(text('stagedctl-linked')).toBe(`Linked ${SYSTEM_LINK} to this copy of Staged.`);
    expect(host.textContent).not.toContain('Replace link');
  });

  it('shows the new target when the link changed before it was replaced', async () => {
    commands.linkStagedctlOnPath
      .mockResolvedValueOnce({
        outcome: 'conflict',
        link: SYSTEM_LINK,
        existing: { kind: 'link', target: OTHER_BUILD },
      })
      .mockResolvedValueOnce({
        outcome: 'conflict',
        link: SYSTEM_LINK,
        existing: { kind: 'link', target: '/somewhere/else' },
      })
      .mockResolvedValueOnce({ outcome: 'linked', link: SYSTEM_LINK, onPath: true });

    await press('Add to PATH');
    await press('Replace link');

    expect(text('stagedctl-conflict')).toContain('points to /somewhere/else.');

    // A second press confirms what is on screen now, not the stale target.
    await press('Replace link');
    expect(commands.linkStagedctlOnPath).toHaveBeenLastCalledWith({
      link: SYSTEM_LINK,
      target: '/somewhere/else',
    });
  });

  it('offers no replace for a file in the way', async () => {
    commands.linkStagedctlOnPath.mockResolvedValue({
      outcome: 'conflict',
      link: SYSTEM_LINK,
      existing: { kind: 'other' },
    });

    await press('Add to PATH');

    expect(text('stagedctl-conflict')).toBe(
      `${SYSTEM_LINK} already exists and is not a link, so Staged left it alone. ` +
        'Move or delete it, then try again.'
    );
    expect(host.textContent).not.toContain('Replace link');
  });

  it("shows the backend's reason when nothing was linked", async () => {
    const refusal =
      'macOS is running Staged from a temporary, read-only copy, so a link to it would stop ' +
      'working. Move Staged to your Applications folder, open it from there, and try again.';
    // Tauri rejects with the command's `Err` string, not an Error.
    commands.linkStagedctlOnPath.mockRejectedValue(refusal);

    await press('Add to PATH');

    expect(host.querySelector('[role="alert"]')?.textContent).toBe(refusal);
    expect(text('stagedctl-linked')).toBeNull();
  });

  it('clears an earlier result when a later press fails, and the error when one works', async () => {
    commands.linkStagedctlOnPath
      .mockResolvedValueOnce({ outcome: 'alreadyLinked', link: SYSTEM_LINK, onPath: true })
      .mockRejectedValueOnce(new Error('Staged could not read /usr/local/bin/stagedctl.'))
      .mockResolvedValueOnce({ outcome: 'alreadyLinked', link: SYSTEM_LINK, onPath: true });

    await press('Add to PATH');
    await press('Add to PATH');

    expect(host.querySelector('[role="alert"]')?.textContent).toBe(
      'Staged could not read /usr/local/bin/stagedctl.'
    );
    expect(text('stagedctl-linked')).toBeNull();

    await press('Add to PATH');

    expect(host.querySelector('[role="alert"]')).toBeNull();
    expect(text('stagedctl-linked')).not.toBeNull();
  });

  it('ignores presses while a request is in flight', async () => {
    let finish!: (outcome: StagedctlLinkOutcome) => void;
    commands.linkStagedctlOnPath.mockReturnValue(
      new Promise<StagedctlLinkOutcome>((resolve) => (finish = resolve))
    );

    button('Add to PATH').click();
    button('Add to PATH').click();
    flushSync();

    expect(commands.linkStagedctlOnPath).toHaveBeenCalledTimes(1);

    finish({ outcome: 'linked', link: SYSTEM_LINK, onPath: true });
    await flushPromises();
    expect(text('stagedctl-linked')).not.toBeNull();
  });
});
