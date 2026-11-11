import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ControlAuthorizationRequest } from '../../types';

function request(
  overrides: Partial<ControlAuthorizationRequest> = {}
): ControlAuthorizationRequest {
  return {
    requestId: 'req-1',
    client: 'my-agent',
    projectId: null,
    projectName: null,
    access: 'write',
    peerPid: 4242,
    peerParent: 'zsh',
    requestedAt: 1_000,
    expiresAt: Date.now() + 5 * 60_000,
    ...overrides,
  };
}

let listPendingControlAuthorizations: ReturnType<typeof vi.fn>;
let answerControlAuthorization: ReturnType<typeof vi.fn>;
let toastError: ReturnType<typeof vi.fn>;

async function importController() {
  const mod = await import('./controlAuthorization.svelte');
  return mod;
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (resumeOnLaunch.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);
  listPendingControlAuthorizations = vi.fn().mockResolvedValue([]);
  answerControlAuthorization = vi.fn().mockResolvedValue(undefined);
  toastError = vi.fn();
  vi.doMock('../../api/commands', () => ({
    listPendingControlAuthorizations,
    answerControlAuthorization,
  }));
  vi.doMock('svelte-sonner', () => ({ toast: { error: toastError } }));
});

afterEach(() => {
  vi.useRealTimers();
});

describe('controlAuthorization queue', () => {
  it('offers requests one at a time, oldest first, deduping the snapshot overlap', async () => {
    const { controlAuthorization } = await importController();
    controlAuthorization.offer(request({ requestId: 'a' }));
    controlAuthorization.offer(request({ requestId: 'b', client: 'other' }));
    controlAuthorization.offer(request({ requestId: 'a' }));
    expect(controlAuthorization.pending?.requestId).toBe('a');
    expect(controlAuthorization.queuedCount).toBe(1);

    await controlAuthorization.deny();
    expect(answerControlAuthorization).toHaveBeenCalledWith('a', false);
    expect(controlAuthorization.pending?.requestId).toBe('b');
    expect(controlAuthorization.queuedCount).toBe(0);
  });

  it('takes the backend snapshot and ignores requests that already lapsed', async () => {
    listPendingControlAuthorizations.mockResolvedValueOnce([
      request({ requestId: 'stale', expiresAt: Date.now() - 1 }),
      request({ requestId: 'fresh' }),
    ]);
    const { controlAuthorization } = await importController();
    await controlAuthorization.loadPending();
    expect(controlAuthorization.pending?.requestId).toBe('fresh');
    expect(controlAuthorization.queuedCount).toBe(0);
  });

  it('drops a request on its own when it expires unanswered', async () => {
    vi.useFakeTimers();
    const { controlAuthorization } = await importController();
    controlAuthorization.offer(request({ requestId: 'a', expiresAt: Date.now() + 1_000 }));
    expect(controlAuthorization.pending?.requestId).toBe('a');
    vi.advanceTimersByTime(1_001);
    expect(controlAuthorization.pending).toBeNull();
    expect(answerControlAuthorization).not.toHaveBeenCalled();
  });

  it('never blocks the launch on a failed snapshot', async () => {
    listPendingControlAuthorizations.mockRejectedValueOnce(new Error('no store'));
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { controlAuthorization } = await importController();
    await controlAuthorization.loadPending();
    expect(controlAuthorization.pending).toBeNull();
    errorSpy.mockRestore();
  });
});

describe('controlAuthorization answers', () => {
  it('approve and deny answer the request on screen and close it', async () => {
    const { controlAuthorization } = await importController();
    controlAuthorization.offer(request({ requestId: 'a' }));
    await controlAuthorization.approve();
    expect(answerControlAuthorization).toHaveBeenCalledWith('a', true);
    expect(controlAuthorization.pending).toBeNull();
    expect(controlAuthorization.busy).toBe(false);

    controlAuthorization.offer(request({ requestId: 'b' }));
    await controlAuthorization.deny();
    expect(answerControlAuthorization).toHaveBeenLastCalledWith('b', false);
    expect(controlAuthorization.pending).toBeNull();
  });

  it('does nothing with no request on screen', async () => {
    const { controlAuthorization } = await importController();
    await controlAuthorization.approve();
    await controlAuthorization.deny();
    expect(answerControlAuthorization).not.toHaveBeenCalled();
  });

  it('reports a failed answer and still closes the request', async () => {
    answerControlAuthorization.mockRejectedValueOnce(new Error('request expired'));
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const { controlAuthorization } = await importController();
    controlAuthorization.offer(request({ requestId: 'a' }));
    await controlAuthorization.approve();
    expect(toastError).toHaveBeenCalledWith('Could not approve my-agent: request expired');
    expect(controlAuthorization.pending).toBeNull();
    errorSpy.mockRestore();
  });
});

describe('labels', () => {
  it('spell scope and access for the dialog and the panel', async () => {
    const { scopeLabel, accessLabel } = await importController();
    expect(scopeLabel(null, null)).toBe('every project');
    expect(scopeLabel('p1', 'Widgets')).toBe('the project “Widgets”');
    expect(scopeLabel('p1', null)).toBe('one project (p1)');
    expect(accessLabel('read')).toBe('read-only');
    expect(accessLabel('write')).toBe('read and write');
  });
});
