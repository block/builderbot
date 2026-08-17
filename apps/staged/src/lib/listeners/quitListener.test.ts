import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ActiveSessionInfo, QuitRequestedPayload } from '../types';

const confirmQuit = vi.fn<() => Promise<void>>();
const cancelQuit = vi.fn<() => Promise<void>>();
const unlisten = vi.fn();
// Window-scoped on purpose: the backend addresses the event to one window with
// `emit_to`, and an any-target listener would raise the dialog in all of them.
const listenToWindowEvent = vi.fn();

let handlers: Array<(payload: QuitRequestedPayload) => void>;

/**
 * Load the listener and store fresh, with transport in the requested mode. The
 * store is a singleton, so each test needs its own module registry.
 */
async function load({ isTauri = true } = {}) {
  vi.resetModules();
  vi.doMock('../transport', () => ({ isTauri, listenToWindowEvent }));
  vi.doMock('../api/commands', () => ({ confirmQuit, cancelQuit }));

  const { listenForQuitRequests } = await import('./quitListener');
  const { quitPrompt } = await import('../stores/quitPrompt.svelte');
  return { listenForQuitRequests, quitPrompt };
}

function session(overrides: Partial<ActiveSessionInfo> = {}): ActiveSessionInfo {
  return {
    sessionId: 's1',
    projectId: 'p1',
    branchId: 'b1',
    sessionType: 'commit',
    status: 'running',
    ...overrides,
  };
}

describe('quitListener', () => {
  beforeEach(() => {
    // The store's runes compile away in the app build; under vitest they stay
    // plain global calls, so stub $state as identity (projectsData.test.ts
    // precedent).
    vi.stubGlobal('$state', (initial: unknown) => initial);
    handlers = [];
    confirmQuit.mockReset().mockResolvedValue(undefined);
    cancelQuit.mockReset().mockResolvedValue(undefined);
    unlisten.mockReset();
    listenToWindowEvent.mockReset().mockImplementation((_event, handler) => {
      handlers.push(handler as (payload: QuitRequestedPayload) => void);
      return unlisten;
    });
  });

  afterEach(() => {
    vi.doUnmock('../transport');
    vi.doUnmock('../api/commands');
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('opens the prompt with the payload from app:quit-requested', async () => {
    const { listenForQuitRequests, quitPrompt } = await load();
    listenForQuitRequests();

    expect(listenToWindowEvent).toHaveBeenCalledWith('app:quit-requested', expect.any(Function));
    expect(quitPrompt.open).toBe(false);

    handlers[0]({ sessions: [session()], runningActionCount: 2 });

    expect(quitPrompt.open).toBe(true);
    expect(quitPrompt.payload?.sessions).toHaveLength(1);
    expect(quitPrompt.payload?.runningActionCount).toBe(2);
    expect(quitPrompt.stopping).toBe(false);
  });

  it('registers no listener in web mode', async () => {
    const { listenForQuitRequests } = await load({ isTauri: false });

    // Callable no-op, so App.svelte's teardown needs no extra guard.
    listenForQuitRequests()();

    expect(listenToWindowEvent).not.toHaveBeenCalled();
  });

  it('confirming invokes confirm_quit and leaves the dialog stopping', async () => {
    const { listenForQuitRequests, quitPrompt } = await load();
    listenForQuitRequests();
    handlers[0]({ sessions: [session()], runningActionCount: 0 });

    await quitPrompt.confirm();

    expect(confirmQuit).toHaveBeenCalledTimes(1);
    // The backend exits the process; until it does, the dialog reports progress
    // instead of pretending the app is still usable.
    expect(quitPrompt.open).toBe(true);
    expect(quitPrompt.stopping).toBe(true);

    await quitPrompt.confirm();
    expect(confirmQuit).toHaveBeenCalledTimes(1);
  });

  it('closes the dialog when confirm_quit fails', async () => {
    const { listenForQuitRequests, quitPrompt } = await load();
    listenForQuitRequests();
    handlers[0]({ sessions: [session()], runningActionCount: 0 });
    confirmQuit.mockRejectedValueOnce(new Error('nope'));
    vi.spyOn(console, 'error').mockImplementation(() => {});

    await quitPrompt.confirm();

    expect(quitPrompt.open).toBe(false);
    expect(quitPrompt.stopping).toBe(false);
  });

  it('cancelling invokes cancel_quit and closes the dialog', async () => {
    const { listenForQuitRequests, quitPrompt } = await load();
    listenForQuitRequests();
    handlers[0]({ sessions: [session()], runningActionCount: 0 });

    quitPrompt.cancel();

    expect(cancelQuit).toHaveBeenCalledTimes(1);
    expect(quitPrompt.open).toBe(false);
  });

  it('ignores a cancel once the quit is under way', async () => {
    const { listenForQuitRequests, quitPrompt } = await load();
    listenForQuitRequests();
    handlers[0]({ sessions: [session()], runningActionCount: 0 });

    await quitPrompt.confirm();
    quitPrompt.cancel();

    expect(cancelQuit).not.toHaveBeenCalled();
    expect(quitPrompt.open).toBe(true);
  });
});
