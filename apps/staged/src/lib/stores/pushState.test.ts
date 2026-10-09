import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

async function importStore() {
  const { pushStateStore } = await import('./pushState.svelte');
  return pushStateStore;
}

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal('$state', (initial: unknown) => initial);
  vi.doMock('./sessionRegistry.svelte', () => ({
    sessionRegistry: { getBranchId: () => null },
  }));
});

afterEach(() => {
  vi.doUnmock('./sessionRegistry.svelte');
  vi.unstubAllGlobals();
});

describe('pushStateStore.trackQueuedPushIfIdle', () => {
  it('tracks the queued push when nothing is in flight', async () => {
    const store = await importStore();

    store.trackQueuedPushIfIdle('branch', 'gated-push');

    expect(store.getPushState('branch')).toMatchObject({
      state: 'queued',
      sessionId: 'gated-push',
    });
  });

  it('replaces a finished or failed push', async () => {
    const store = await importStore();
    store.setPushDone('branch');
    store.trackQueuedPushIfIdle('branch', 'after-done');
    expect(store.getSessionId('branch')).toBe('after-done');

    store.setPushError('branch', 'rejected');
    store.trackQueuedPushIfIdle('branch', 'after-error');
    expect(store.getPushState('branch')).toMatchObject({
      state: 'queued',
      sessionId: 'after-error',
    });
  });

  it('keeps a plain push that is already queued or running', async () => {
    const store = await importStore();

    store.setPushQueued('branch', 'plain-queued');
    store.trackQueuedPushIfIdle('branch', 'gated-push');
    expect(store.getPushState('branch')).toMatchObject({
      state: 'queued',
      sessionId: 'plain-queued',
    });

    store.setPushing('branch', 'plain-running');
    store.trackQueuedPushIfIdle('branch', 'gated-push');
    expect(store.getPushState('branch')).toMatchObject({
      state: 'pushing',
      sessionId: 'plain-running',
    });
  });

  it('does not reset the push it already follows once it is running', async () => {
    const store = await importStore();
    store.setPushQueued('branch', 'gated-push');
    store.markQueuedPushStarted('branch', 'gated-push');

    // A repeat "Rebase and force push" dedupes onto the same queued row.
    store.trackQueuedPushIfIdle('branch', 'gated-push');

    expect(store.getPushState('branch')).toMatchObject({
      state: 'pushing',
      sessionId: 'gated-push',
    });
  });
});
