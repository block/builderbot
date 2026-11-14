import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ControlGrantSummary } from '../../types';

// The panel loads the grant list on mount, follows `control-grants-changed`,
// and revokes behind an AlertDialog confirm. Commands and the transport are
// mocked; dialog chrome passes children through so the confirm's buttons are
// plain buttons the test can click.
const { commands, transport } = vi.hoisted(() => ({
  commands: {
    listControlGrants: vi.fn<() => Promise<ControlGrantSummary[]>>(),
    revokeControlGrant: vi.fn<(id: string) => Promise<void>>(),
    // The "Command line" setting above the list; it calls nothing on mount.
    linkStagedctlOnPath: vi.fn(),
  },
  transport: {
    listeners: new Map<string, (payload: unknown) => void>(),
    unlisten: vi.fn(),
  },
}));

vi.mock('../../api/commands', () => commands);
vi.mock('../../transport', () => ({
  isTauri: true,
  listenToEvent: (event: string, cb: (payload: unknown) => void) => {
    transport.listeners.set(event, cb);
    return transport.unlisten;
  },
}));
vi.mock('$lib/components/ui/alert-dialog', async () => {
  const { default: Passthrough } = await import('../sessions/testStubs/PassthroughStub.svelte');
  const { default: ButtonStub } = await import('../sessions/testStubs/ButtonStub.svelte');
  const { default: Root } = await import('../sessions/testStubs/AlertDialogRootStub.svelte');
  return {
    Root,
    Content: Passthrough,
    Header: Passthrough,
    Footer: Passthrough,
    Title: Passthrough,
    Description: Passthrough,
    Action: ButtonStub,
    Cancel: ButtonStub,
  };
});
vi.mock('$lib/components/ui/button', async () => {
  const { default: ButtonStub } = await import('../sessions/testStubs/ButtonStub.svelte');
  return { Button: ButtonStub };
});
vi.mock('../../shared/Spinner.svelte', async () => {
  const { default: Empty } = await import('../sessions/testStubs/EmptyStub.svelte');
  return { default: Empty };
});

import ConnectedAppsPanel from './ConnectedAppsPanel.svelte';

const DAY = 86_400_000;

function grant(overrides: Partial<ControlGrantSummary> = {}): ControlGrantSummary {
  return {
    id: 'g-1',
    client: 'my-agent',
    projectId: null,
    projectName: null,
    access: 'write',
    createdAt: Date.now() - DAY,
    expiresAt: Date.now() + 29 * DAY,
    lastUsedAt: null,
    live: true,
    ...overrides,
  };
}

let host: HTMLElement;
let panel: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  commands.listControlGrants.mockReset();
  commands.revokeControlGrant.mockReset().mockResolvedValue(undefined);
  transport.listeners.clear();
  transport.unlisten.mockClear();
});

afterEach(() => {
  if (panel) unmount(panel);
  panel = null;
  host.remove();
});

async function render() {
  panel = mount(ConnectedAppsPanel, { target: host });
  flushSync();
  await vi.waitFor(() => expect(commands.listControlGrants).toHaveBeenCalled());
  await flushPromises();
}

async function flushPromises() {
  for (let i = 0; i < 3; i++) await new Promise((r) => setTimeout(r, 0));
  flushSync();
}

function rows(): HTMLElement[] {
  return Array.from(host.querySelectorAll<HTMLElement>('[data-testid="connected-app"]'));
}

function buttonsNamed(label: string): HTMLButtonElement[] {
  return Array.from(host.querySelectorAll('button')).filter((b) => b.textContent?.trim() === label);
}

describe('ConnectedAppsPanel', () => {
  it('lists each grant with client, access, scope, last use and expiry', async () => {
    commands.listControlGrants.mockResolvedValue([
      grant(),
      grant({
        id: 'g-2',
        client: 'reporter',
        projectId: 'p1',
        projectName: 'Widgets',
        access: 'read',
        lastUsedAt: Date.now() - 2 * 60_000,
        live: false,
      }),
    ]);
    await render();
    const [first, second] = rows();
    expect(rows()).toHaveLength(2);
    expect(first.textContent).toContain('my-agent');
    expect(first.textContent).toContain('connected now');
    expect(first.textContent).toContain('read and write');
    expect(first.textContent).toContain('every project');
    expect(first.textContent).toContain('never used');
    expect(first.textContent).toContain('expires in 29 days');
    expect(second.textContent).toContain('reporter');
    expect(second.textContent).not.toContain('connected now');
    expect(second.textContent).toContain('read-only');
    expect(second.textContent).toContain('the project “Widgets”');
    expect(second.textContent).toContain('last used');
  });

  it('shows the empty state and reloads when the backend reports a change', async () => {
    commands.listControlGrants.mockResolvedValueOnce([]).mockResolvedValueOnce([grant()]);
    await render();
    expect(host.querySelector('[data-testid="connected-apps-empty"]')).not.toBeNull();
    expect(rows()).toHaveLength(0);

    transport.listeners.get('control-grants-changed')?.(undefined);
    await flushPromises();
    expect(commands.listControlGrants).toHaveBeenCalledTimes(2);
    expect(rows()).toHaveLength(1);
  });

  it('revokes behind a confirm and drops the row', async () => {
    commands.listControlGrants.mockResolvedValue([grant(), grant({ id: 'g-2', client: 'other' })]);
    await render();
    expect(buttonsNamed('Revoke')).toHaveLength(2);

    buttonsNamed('Revoke')[0].click();
    flushSync();
    expect(host.textContent).toContain('Revoke access for “my-agent”?');
    // The confirm's own Revoke is the last one rendered.
    const confirm = buttonsNamed('Revoke');
    confirm[confirm.length - 1].click();
    await flushPromises();
    expect(commands.revokeControlGrant).toHaveBeenCalledWith('g-1');
    expect(rows().map((r) => r.dataset.grantId)).toEqual(['g-2']);
    expect(host.textContent).not.toContain('Revoke access for');
  });

  it('cancelling or dismissing the confirm revokes nothing', async () => {
    commands.listControlGrants.mockResolvedValue([grant()]);
    await render();
    buttonsNamed('Revoke')[0].click();
    flushSync();
    buttonsNamed('Cancel')[0].click();
    flushSync();
    expect(host.textContent).not.toContain('Revoke access for');

    buttonsNamed('Revoke')[0].click();
    flushSync();
    buttonsNamed('dismiss')[0].click();
    flushSync();
    expect(host.textContent).not.toContain('Revoke access for');
    expect(commands.revokeControlGrant).not.toHaveBeenCalled();
    expect(rows()).toHaveLength(1);
  });

  it('offers the command line setting above the list, and runs nothing for it on load', async () => {
    commands.listControlGrants.mockResolvedValue([]);
    await render();
    expect(buttonsNamed('Add to PATH')).toHaveLength(1);
    const setting = host.querySelector('section[aria-labelledby="stagedctl-path-heading"]');
    const empty = host.querySelector('[data-testid="connected-apps-empty"]');
    expect(setting).not.toBeNull();
    expect(
      setting!.compareDocumentPosition(empty!) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy();
    expect(commands.linkStagedctlOnPath).not.toHaveBeenCalled();
  });

  it('surfaces a failed load and tears down its listener on unmount', async () => {
    commands.listControlGrants.mockRejectedValue(new Error('Database not initialized'));
    await render();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('Database not initialized');
    unmount(panel!);
    panel = null;
    expect(transport.unlisten).toHaveBeenCalledTimes(1);
  });
});
