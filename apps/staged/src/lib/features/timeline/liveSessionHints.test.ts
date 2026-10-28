import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session } from '../../types';

type LiveSessionHintsModule = typeof import('./liveSessionHints');
type LiveSessionHintPoller = ReturnType<LiveSessionHintsModule['createLiveSessionHints']>;

function session(overrides: Partial<Session> = {}): Session {
  return {
    id: 's1',
    prompt: 'do the thing',
    status: 'running',
    workingDir: '',
    provider: null,
    agentId: null,
    errorMessage: null,
    completionReason: null,
    createdAt: 1000,
    updatedAt: 2000,
    acpTitle: null,
    ...overrides,
  };
}

/** Flush the poller's promise chains (all mocked commands resolve in microtasks). */
function flushAsync(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

describe('createLiveSessionHints titles', () => {
  let getSession: ReturnType<typeof vi.fn>;
  let createLiveSessionHints: LiveSessionHintsModule['createLiveSessionHints'];
  let poller: LiveSessionHintPoller | null;
  let latestTitles: Record<string, string>;

  beforeEach(async () => {
    vi.resetModules();
    getSession = vi.fn();
    vi.doMock('../../api/commands', () => ({
      getSession,
      getSessionMessages: vi.fn().mockResolvedValue({ data: [] }),
      getSessionMessagesSince: vi.fn().mockResolvedValue([]),
      resolvePathAliases: vi.fn().mockResolvedValue([]),
    }));
    ({ createLiveSessionHints } = await import('./liveSessionHints'));
    poller = null;
    latestTitles = {};
  });

  afterEach(() => {
    poller?.destroy();
    vi.doUnmock('../../api/commands');
  });

  function createPoller(): LiveSessionHintPoller {
    poller = createLiveSessionHints(
      () => {},
      undefined,
      (titles) => {
        latestTitles = titles;
      }
    );
    return poller;
  }

  it('surfaces the ACP title of a running session', async () => {
    getSession.mockResolvedValue(session({ acpTitle: 'Fix login flow' }));

    createPoller().syncRunningSessionIds(['s1']);
    await flushAsync();

    expect(getSession).toHaveBeenCalledWith('s1');
    expect(latestTitles).toEqual({ s1: 'Fix login flow' });
  });

  it('replaces the title when the agent pushes a new one', async () => {
    getSession.mockResolvedValue(session({ acpTitle: 'First pass' }));

    const p = createPoller();
    p.syncRunningSessionIds(['s1']);
    await flushAsync();
    expect(latestTitles).toEqual({ s1: 'First pass' });

    getSession.mockResolvedValue(session({ acpTitle: 'Refined title' }));
    p.syncRunningSessionIds(['s1']);
    await flushAsync();

    expect(latestTitles).toEqual({ s1: 'Refined title' });
  });

  it('strips XML context blocks and collapses whitespace in titles', async () => {
    getSession.mockResolvedValue(
      session({ acpTitle: '  Fix   login <action>injected</action> flow  ' })
    );

    createPoller().syncRunningSessionIds(['s1']);
    await flushAsync();

    expect(latestTitles).toEqual({ s1: 'Fix login flow' });
  });

  it('does not surface blank titles', async () => {
    getSession.mockResolvedValue(session({ acpTitle: '   ' }));

    createPoller().syncRunningSessionIds(['s1']);
    await flushAsync();

    expect(latestTitles).toEqual({});
  });

  it('clears the title when the agent retracts it', async () => {
    getSession.mockResolvedValue(session({ acpTitle: 'Fix login flow' }));

    const p = createPoller();
    p.syncRunningSessionIds(['s1']);
    await flushAsync();
    expect(latestTitles).toEqual({ s1: 'Fix login flow' });

    getSession.mockResolvedValue(session({ acpTitle: null }));
    p.syncRunningSessionIds(['s1']);
    await flushAsync();

    expect(latestTitles).toEqual({});
  });

  it('clears the title when the session stops running', async () => {
    getSession.mockResolvedValue(session({ acpTitle: 'Fix login flow' }));

    const p = createPoller();
    p.syncRunningSessionIds(['s1']);
    await flushAsync();
    expect(latestTitles).toEqual({ s1: 'Fix login flow' });

    getSession.mockResolvedValue(session({ status: 'completed', acpTitle: 'Fix login flow' }));
    p.syncRunningSessionIds(['s1']);
    await flushAsync();

    expect(latestTitles).toEqual({});
  });

  it('clears the title when the session leaves the running set', async () => {
    getSession.mockResolvedValue(session({ acpTitle: 'Fix login flow' }));

    const p = createPoller();
    p.syncRunningSessionIds(['s1']);
    await flushAsync();
    expect(latestTitles).toEqual({ s1: 'Fix login flow' });

    p.syncRunningSessionIds([]);

    expect(latestTitles).toEqual({});
  });

  it('tracks titles per session', async () => {
    getSession.mockImplementation((sessionId: string) =>
      Promise.resolve(
        session({
          id: sessionId,
          acpTitle: sessionId === 's1' ? 'Fix login flow' : 'Update docs',
        })
      )
    );

    const p = createPoller();
    p.syncRunningSessionIds(['s1', 's2']);
    await flushAsync();
    expect(latestTitles).toEqual({ s1: 'Fix login flow', s2: 'Update docs' });

    p.syncRunningSessionIds(['s2']);

    expect(latestTitles).toEqual({ s2: 'Update docs' });
  });
});

describe('createLiveSessionHints setPaused', () => {
  let getSession: ReturnType<typeof vi.fn>;
  let getSessionMessages: ReturnType<typeof vi.fn>;
  let getSessionMessagesSince: ReturnType<typeof vi.fn>;
  let createLiveSessionHints: LiveSessionHintsModule['createLiveSessionHints'];
  let poller: LiveSessionHintPoller | null;
  let latestHints: Record<string, string>;
  let hintChanges: number;

  beforeEach(async () => {
    vi.useFakeTimers();
    vi.resetModules();
    getSession = vi.fn().mockResolvedValue(session());
    getSessionMessages = vi.fn().mockResolvedValue({
      data: [{ id: 1, role: 'assistant', content: 'Reading the config', createdAt: 1 }],
    });
    getSessionMessagesSince = vi.fn().mockResolvedValue([]);
    vi.doMock('../../api/commands', () => ({
      getSession,
      getSessionMessages,
      getSessionMessagesSince,
      resolvePathAliases: vi.fn().mockResolvedValue([]),
    }));
    ({ createLiveSessionHints } = await import('./liveSessionHints'));
    poller = null;
    latestHints = {};
    hintChanges = 0;
  });

  afterEach(() => {
    poller?.destroy();
    vi.doUnmock('../../api/commands');
    vi.useRealTimers();
  });

  function createPoller(): LiveSessionHintPoller {
    poller = createLiveSessionHints((hints) => {
      latestHints = hints;
      hintChanges++;
    });
    return poller;
  }

  /** Flush the mocked commands' microtasks without advancing the interval. */
  const flush = () => vi.advanceTimersByTimeAsync(0);

  it('stops the interval while paused and keeps the hints it had', async () => {
    const p = createPoller();
    p.syncRunningSessionIds(['s1']);
    await flush();
    expect(latestHints).toEqual({ s1: 'Reading the config' });
    const callsBefore = getSession.mock.calls.length;

    p.setPaused(true);
    await vi.advanceTimersByTimeAsync(5000);

    expect(getSession.mock.calls.length).toBe(callsBefore);
    expect(latestHints).toEqual({ s1: 'Reading the config' });
  });

  it('polls at once on unpause, continuing from the last message instead of refetching', async () => {
    const p = createPoller();
    p.syncRunningSessionIds(['s1']);
    await flush();
    p.setPaused(true);
    await vi.advanceTimersByTimeAsync(5000);
    const sessionCalls = getSession.mock.calls.length;
    const sinceCalls = getSessionMessagesSince.mock.calls.length;
    const changesBefore = hintChanges;

    p.setPaused(false);
    await flush();

    expect(getSession.mock.calls.length).toBe(sessionCalls + 1);
    expect(getSessionMessages).toHaveBeenCalledTimes(1);
    expect(getSessionMessagesSince.mock.calls.length).toBe(sinceCalls + 1);
    expect(getSessionMessagesSince).toHaveBeenLastCalledWith('s1', 1);
    // Nothing new arrived, so the hint stays as it was without a re-emit.
    expect(hintChanges).toBe(changesBefore);

    // And the interval is running again.
    await vi.advanceTimersByTimeAsync(750);
    expect(getSession.mock.calls.length).toBe(sessionCalls + 2);
  });

  it('does not start polling for sessions that arrive while paused', async () => {
    const p = createPoller();
    p.setPaused(true);
    p.syncRunningSessionIds(['s1']);
    await vi.advanceTimersByTimeAsync(2000);

    expect(getSession).not.toHaveBeenCalled();

    p.setPaused(false);
    await flush();
    expect(getSession).toHaveBeenCalledTimes(1);
    expect(latestHints).toEqual({ s1: 'Reading the config' });
  });

  it('is a no-op after destroy', async () => {
    const p = createPoller();
    p.syncRunningSessionIds(['s1']);
    await flush();
    p.destroy();
    const calls = getSession.mock.calls.length;

    p.setPaused(true);
    p.setPaused(false);
    await vi.advanceTimersByTimeAsync(2000);

    expect(getSession.mock.calls.length).toBe(calls);
  });
});
