import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ControlAuthorizationRequest } from '../../types';

// The dialog is driven by controlAuthorization.pending; the test owns that
// state and watches Approve / Deny. Dialog chrome passes children through and
// the two buttons are plain buttons, so the body renders and clicks in jsdom;
// the Root stub exposes the dismiss path (Escape / overlay) as a button.
const { controller } = vi.hoisted(() => ({
  controller: {
    pending: null as unknown,
    queuedCount: 0,
    busy: false,
    approve: vi.fn(async () => {}),
    deny: vi.fn(async () => {}),
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
vi.mock('./controlAuthorization.svelte', async () => {
  const actual = await vi.importActual<typeof import('./controlAuthorization.svelte')>(
    './controlAuthorization.svelte'
  );
  const { reactiveProps } = await import('../sessions/testStubs/reactiveProps.svelte');
  const state = reactiveProps({ pending: null as unknown, queuedCount: 0, busy: false });
  for (const key of ['pending', 'queuedCount', 'busy'] as const) {
    Object.defineProperty(controller, key, {
      get: () => state[key],
      set: (value: never) => {
        state[key] = value;
      },
    });
  }
  return {
    scopeLabel: actual.scopeLabel,
    accessLabel: actual.accessLabel,
    controlAuthorization: controller,
  };
});
import ControlAuthorizationDialog from './ControlAuthorizationDialog.svelte';

function request(
  overrides: Partial<ControlAuthorizationRequest> = {}
): ControlAuthorizationRequest {
  return {
    requestId: 'req-1',
    client: 'my-agent',
    projectId: 'p1',
    projectName: 'Widgets',
    access: 'read',
    peerPid: 4242,
    peerParent: 'zsh',
    orphaned: false,
    requestedAt: 0,
    expiresAt: Date.now() + 60_000,
    ...overrides,
  };
}

let host: HTMLElement;
let dialog: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  controller.pending = null;
  controller.queuedCount = 0;
  controller.busy = false;
  controller.approve.mockClear();
  controller.deny.mockClear();
});

afterEach(() => {
  if (dialog) unmount(dialog);
  dialog = null;
  host.remove();
});

function render() {
  dialog = mount(ControlAuthorizationDialog, { target: host });
  flushSync();
}

function buttons(): Record<string, HTMLButtonElement> {
  const out: Record<string, HTMLButtonElement> = {};
  for (const b of host.querySelectorAll('button')) out[b.textContent?.trim() ?? ''] = b;
  return out;
}

describe('ControlAuthorizationDialog', () => {
  it('names the client, access, scope and requesting process', async () => {
    controller.pending = request();
    render();
    await tick();
    expect(host.textContent).toContain('Allow “my-agent” to control Staged?');
    expect(host.querySelector('[data-testid="control-authorization-access"]')?.textContent).toBe(
      'read-only'
    );
    expect(
      host.querySelector('[data-testid="control-authorization-scope"]')?.textContent?.trim()
    ).toBe('the project “Widgets”');
    expect(host.querySelector('[data-testid="control-authorization-peer"]')?.textContent).toBe(
      'Requested by process 4242 (zsh) on this Mac.'
    );
    expect(host.querySelector('[data-testid="control-authorization-orphaned"]')).toBeNull();
  });

  it('warns when the asking process has been orphaned', async () => {
    controller.pending = request({ peerParent: null, orphaned: true });
    render();
    await tick();
    expect(host.querySelector('[data-testid="control-authorization-peer"]')?.textContent).toBe(
      'Requested by process 4242 on this Mac.'
    );
    const notice = host.querySelector('[data-testid="control-authorization-orphaned"]');
    expect(notice?.textContent?.replace(/\s+/g, ' ').trim()).toBe(
      "The asking process's parent has already exited, so where it was started from cannot be shown. It may have been backgrounded from an agent session; approve only if you ran this command yourself."
    );
  });

  it('omits the source line when the peer is unknown and counts the queue', async () => {
    controller.pending = request({ peerPid: null, peerParent: null, projectId: null });
    controller.queuedCount = 2;
    render();
    await tick();
    expect(host.querySelector('[data-testid="control-authorization-peer"]')).toBeNull();
    expect(
      host.querySelector('[data-testid="control-authorization-scope"]')?.textContent?.trim()
    ).toBe('every project');
    expect(host.textContent).toContain('2 more requests waiting.');
  });

  it('Approve and Deny call the controller', async () => {
    controller.pending = request();
    render();
    await tick();
    buttons()['Approve'].click();
    expect(controller.approve).toHaveBeenCalledTimes(1);
    buttons()['Deny'].click();
    expect(controller.deny).toHaveBeenCalledTimes(1);
  });

  it('closing the dialog any other way denies the request', async () => {
    controller.pending = request();
    render();
    await tick();
    buttons()['dismiss'].click();
    expect(controller.deny).toHaveBeenCalledTimes(1);
    expect(controller.approve).not.toHaveBeenCalled();
  });

  it('a dismiss while an answer is in flight is not a second answer', async () => {
    controller.pending = request();
    controller.busy = true;
    render();
    await tick();
    buttons()['dismiss'].click();
    expect(controller.deny).not.toHaveBeenCalled();
  });

  it('renders nothing while no request is on screen', () => {
    render();
    expect(host.querySelector('[data-testid="control-authorization-details"]')).toBeNull();
    expect(Object.keys(buttons())).toEqual([]);
  });
});
