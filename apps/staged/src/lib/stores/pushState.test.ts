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

describe('pushStateStore.trackPushIfIdle', () => {
  it('tracks the queued push when nothing is in flight', async () => {
    const store = await importStore();

    store.trackPushIfIdle('branch', 'gated-push', 'queued', true);

    expect(store.getPushState('branch')).toMatchObject({
      state: 'queued',
      sessionId: 'gated-push',
      force: true,
    });
  });

  it('replaces a finished or failed push', async () => {
    const store = await importStore();
    store.setPushDone('branch');
    store.trackPushIfIdle('branch', 'after-done', 'queued', true);
    expect(store.getSessionId('branch')).toBe('after-done');

    store.setPushError('branch', 'rejected');
    store.trackPushIfIdle('branch', 'after-error', 'queued', true);
    expect(store.getPushState('branch')).toMatchObject({
      state: 'queued',
      sessionId: 'after-error',
    });
  });

  it('adopts a running push with the force flag it is given', async () => {
    const store = await importStore();

    store.trackPushIfIdle('branch', 'snapshot-push', 'pushing', false);

    expect(store.getPushState('branch')).toMatchObject({
      state: 'pushing',
      sessionId: 'snapshot-push',
      force: false,
    });
  });

  it('keeps a push whose launch has not resolved yet', async () => {
    const store = await importStore();
    store.setPushing('branch', '__pending__', false);

    store.trackPushIfIdle('branch', 'snapshot-push', 'queued', true);

    expect(store.getPushState('branch')).toMatchObject({
      state: 'pushing',
      sessionId: '__pending__',
      force: false,
    });
  });

  it('keeps a plain push that is already queued or running', async () => {
    const store = await importStore();

    store.setPushQueued('branch', 'plain-queued', false);
    store.trackPushIfIdle('branch', 'gated-push', 'queued', true);
    expect(store.getPushState('branch')).toMatchObject({
      state: 'queued',
      sessionId: 'plain-queued',
    });

    store.setPushing('branch', 'plain-running', false);
    store.trackPushIfIdle('branch', 'gated-push', 'queued', true);
    expect(store.getPushState('branch')).toMatchObject({
      state: 'pushing',
      sessionId: 'plain-running',
    });
  });

  it('does not reset the push it already follows once it is running', async () => {
    const store = await importStore();
    store.setPushQueued('branch', 'gated-push', true);
    store.markQueuedPushStarted('branch', 'gated-push');

    // A repeat "Rebase and force push" dedupes onto the same queued row.
    store.trackPushIfIdle('branch', 'gated-push', 'queued', true);

    expect(store.getPushState('branch')).toMatchObject({
      state: 'pushing',
      sessionId: 'gated-push',
    });
  });
});

describe('pushStateStore force flag', () => {
  it('keeps a force push forced from launch through queue drain to failure', async () => {
    const store = await importStore();

    store.setPushing('branch', '__pending__', true);
    store.setPushLaunch('branch', { sessionId: 'force-push', sessionStatus: 'queued' }, true);
    store.markQueuedPushStarted('branch', 'force-push');
    expect(store.getPushState('branch')).toMatchObject({ state: 'pushing', force: true });

    store.setPushError('branch', 'Push session failed.');

    // "Push Failed → Retry" reads this to re-run the same operation.
    expect(store.getPushState('branch')).toMatchObject({ state: 'error', force: true });
  });

  it('keeps the gated push of "Rebase and force push" forced when it fails', async () => {
    const store = await importStore();
    store.trackPushIfIdle('branch', 'gated-push', 'queued', true);

    store.setPushError('branch', 'Push session was cancelled.');

    expect(store.getPushState('branch')?.force).toBe(true);
  });

  it('leaves a plain push and a launch failure with no prior entry unforced', async () => {
    const store = await importStore();
    store.setPushing('branch', '__pending__', false);
    store.setPushError('branch', 'Push session failed.');
    expect(store.getPushState('branch')?.force).toBe(false);

    store.setPushError('other', 'no entry before this');
    expect(store.getPushState('other')?.force).toBe(false);
  });
});

describe('pushStateStore noVerify flag', () => {
  it('keeps a no-verify push from launch through queue drain to failure', async () => {
    const store = await importStore();

    store.setPushing('branch', '__pending__', false, true);
    store.setPushLaunch('branch', { sessionId: 'push', sessionStatus: 'queued' }, false, true);
    expect(store.getPushState('branch')).toMatchObject({ state: 'queued', noVerify: true });

    store.markQueuedPushStarted('branch', 'push');
    expect(store.getPushState('branch')).toMatchObject({ state: 'pushing', noVerify: true });

    // Retry and the rejection dialog read this to skip the hook again.
    store.setPushError('branch', '', true);
    expect(store.getPushState('branch')).toMatchObject({
      state: 'error',
      rejectedNonFastForward: true,
      force: false,
      noVerify: true,
    });
  });

  it('records it on a running launch and an adopted push', async () => {
    const store = await importStore();

    store.setPushLaunch('branch', { sessionId: 'push', sessionStatus: 'running' }, true, true);
    expect(store.getPushState('branch')).toMatchObject({
      state: 'pushing',
      force: true,
      noVerify: true,
    });

    store.trackPushIfIdle('other', 'adopted', 'queued', false, true);
    expect(store.getPushState('other')).toMatchObject({ state: 'queued', noVerify: true });
  });

  it('defaults to verifying, and a finished push forgets the flag', async () => {
    const store = await importStore();

    store.setPushing('branch', '__pending__', true);
    expect(store.getPushState('branch')?.noVerify).toBe(false);
    store.trackPushIfIdle('adopted', 'gated-push', 'queued', true);
    expect(store.getPushState('adopted')?.noVerify).toBe(false);

    store.setPushing('done', 'push', false, true);
    store.setPushDone('done');
    expect(store.getPushState('done')?.noVerify).toBe(false);
  });
});
