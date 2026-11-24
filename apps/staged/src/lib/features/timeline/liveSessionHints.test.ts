import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { normalizeDisplayRoots } from '../sessions/pathDisplayRoots';
import type { SessionStreamHandlers } from '../../transport';
import type {
  Session,
  SessionStreamInterest,
  SessionSummary,
  SessionSummaryRow,
} from '../../types';

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

describe('createLiveSessionHints title-only sessions', () => {
  let getSession: ReturnType<typeof vi.fn>;
  let getSessionMessages: ReturnType<typeof vi.fn>;
  let getSessionMessagesSince: ReturnType<typeof vi.fn>;
  let createLiveSessionHints: LiveSessionHintsModule['createLiveSessionHints'];
  let poller: LiveSessionHintPoller | null;
  let latestTitles: Record<string, string>;
  let hintsCallbackIds: Set<string>;

  beforeEach(async () => {
    vi.resetModules();
    getSession = vi
      .fn()
      .mockImplementation((sessionId: string) =>
        Promise.resolve(session({ id: sessionId, acpTitle: `Title ${sessionId}` }))
      );
    getSessionMessages = vi.fn().mockResolvedValue({
      data: [{ id: 1, role: 'tool_call', content: 'Read the config', createdAt: 1 }],
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
    latestTitles = {};
    hintsCallbackIds = new Set();
  });

  afterEach(() => {
    poller?.destroy();
    vi.doUnmock('../../api/commands');
  });

  function createPoller(): LiveSessionHintPoller {
    poller = createLiveSessionHints(
      (hints) => {
        for (const id of Object.keys(hints)) hintsCallbackIds.add(id);
      },
      undefined,
      (titles) => {
        latestTitles = titles;
      }
    );
    return poller;
  }

  function messageCallIds(): string[] {
    return [...getSessionMessages.mock.calls, ...getSessionMessagesSince.mock.calls].map(
      (call) => call[0] as string
    );
  }

  it('surfaces the ACP title of a title-only session without fetching its transcript', async () => {
    const p = createPoller();
    p.syncRunningSessionIds([], ['child']);
    await flushAsync();
    p.syncRunningSessionIds([], ['child']);
    await flushAsync();

    expect(getSession).toHaveBeenCalledWith('child');
    expect(latestTitles).toEqual({ child: 'Title child' });
    expect(messageCallIds()).not.toContain('child');
    expect(hintsCallbackIds.has('child')).toBe(false);
  });

  it('keeps fetching messages for a full session synced alongside a title-only one', async () => {
    const p = createPoller();
    p.syncRunningSessionIds(['parent'], ['child']);
    await flushAsync();

    expect(getSessionMessages).toHaveBeenCalledWith('parent');
    expect(messageCallIds()).not.toContain('child');
    expect(latestTitles).toEqual({ parent: 'Title parent', child: 'Title child' });
    expect(hintsCallbackIds.has('parent')).toBe(true);
    expect(hintsCallbackIds.has('child')).toBe(false);
  });

  it('fetches the whole transcript once when a title-only session is promoted to full', async () => {
    const p = createPoller();
    p.syncRunningSessionIds([], ['s1']);
    await flushAsync();
    expect(messageCallIds()).toEqual([]);

    p.syncRunningSessionIds(['s1']);
    await flushAsync();

    expect(getSessionMessages).toHaveBeenCalledTimes(1);
    expect(getSessionMessages).toHaveBeenCalledWith('s1');
    expect(hintsCallbackIds.has('s1')).toBe(true);
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

// What a given transcript derives to is `deriveHint`'s, tested in
// `sessionHint.test.ts`. These cover what the poller adds: finding the opening
// prompt in the transcript it loads, and re-deriving as increments arrive.
describe('createLiveSessionHints user replies', () => {
  let getSessionMessages: ReturnType<typeof vi.fn>;
  let getSessionMessagesSince: ReturnType<typeof vi.fn>;
  let createLiveSessionHints: LiveSessionHintsModule['createLiveSessionHints'];
  let poller: LiveSessionHintPoller | null;
  let latestHints: Record<string, string>;

  type Row = { id: number; role: string; content: string; createdAt: number };
  const row = (id: number, role: string, content: string): Row => ({
    id,
    role,
    content,
    createdAt: id,
  });

  const OPENING_PROMPT = row(
    1,
    'user',
    '<action>\nThe user is requesting a note.\n</action>\n\nSummarise the cache layer'
  );
  const TURN_END = row(2, 'assistant', 'Done');

  beforeEach(async () => {
    vi.useFakeTimers();
    vi.resetModules();
    getSessionMessages = vi.fn().mockResolvedValue({ data: [] });
    getSessionMessagesSince = vi.fn().mockResolvedValue([]);
    vi.doMock('../../api/commands', () => ({
      getSession: vi.fn().mockResolvedValue(session()),
      getSessionMessages,
      getSessionMessagesSince,
      resolvePathAliases: vi.fn().mockResolvedValue([]),
    }));
    ({ createLiveSessionHints } = await import('./liveSessionHints'));
    poller = null;
    latestHints = {};
  });

  afterEach(() => {
    poller?.destroy();
    vi.doUnmock('../../api/commands');
    vi.useRealTimers();
  });

  /** Start tracking `s1` from `transcript` and run the first poll. */
  async function track(transcript: Row[]): Promise<void> {
    getSessionMessages.mockResolvedValue({ data: transcript });
    poller = createLiveSessionHints((hints) => {
      latestHints = hints;
    });
    poller.syncRunningSessionIds(['s1']);
    await vi.advanceTimersByTimeAsync(0);
  }

  /** Run the next poll tick, which reads `rows` as the transcript's new tail. */
  async function tick(rows: Row[]): Promise<void> {
    getSessionMessagesSince.mockResolvedValueOnce(rows);
    await vi.advanceTimersByTimeAsync(750);
  }

  it('shows a reply sent after a completed turn in place of that turn’s last message', async () => {
    // What a reload reads: the reply is already the transcript's newest row.
    await track([OPENING_PROMPT, TURN_END, row(3, 'user', 'Also cover the eviction policy')]);

    expect(latestHints).toEqual({ s1: 'Also cover the eviction policy' });
  });

  it('switches to the reply when it arrives while the session is tracked', async () => {
    await track([OPENING_PROMPT, TURN_END]);
    expect(latestHints).toEqual({ s1: 'Done' });

    await tick([TURN_END, row(3, 'user', 'Also cover the eviction policy')]);

    expect(getSessionMessagesSince).toHaveBeenLastCalledWith('s1', 2);
    expect(latestHints).toEqual({ s1: 'Also cover the eviction policy' });
  });

  it('lets newer agent content replace the reply', async () => {
    const reply = row(3, 'user', 'Also cover the eviction policy');
    await track([OPENING_PROMPT, TURN_END, reply]);
    expect(latestHints).toEqual({ s1: 'Also cover the eviction policy' });

    const toolCall = row(4, 'tool_call', 'Read');
    await tick([reply, toolCall]);
    expect(latestHints).toEqual({ s1: 'Reading…' });

    await tick([toolCall, row(5, 'assistant', 'Looking at the eviction code')]);
    expect(latestHints).toEqual({ s1: 'Looking at the eviction code' });
  });

  it('keeps the reply while the agent has produced nothing newer', async () => {
    const reply = row(3, 'user', 'Also cover the eviction policy');
    await track([OPENING_PROMPT, TURN_END, reply]);

    await tick([reply]);
    await tick([reply]);

    expect(latestHints).toEqual({ s1: 'Also cover the eviction policy' });
  });

  it('shows no hint for a session that only has its opening prompt', async () => {
    await track([OPENING_PROMPT]);
    expect(latestHints).toEqual({});

    // Still nothing once the tracker is polling increments.
    await tick([OPENING_PROMPT]);
    expect(latestHints).toEqual({});
  });

  it('follows agent content as before in a session with no reply', async () => {
    await track([OPENING_PROMPT, row(2, 'tool_call', 'Read')]);
    expect(latestHints).toEqual({ s1: 'Reading…' });

    const text = row(3, 'assistant', 'Reading the cache module');
    await tick([row(2, 'tool_call', 'Read'), text]);
    expect(latestHints).toEqual({ s1: 'Reading the cache module' });

    await tick([text, row(4, 'tool_call', 'Read')]);
    expect(latestHints).toEqual({ s1: 'Reading…' });
  });
});

describe('createLiveSessionHints on the session stream', () => {
  type StreamModule = typeof import('../../stores/sessionStream.svelte');
  type StreamOwner = InstanceType<StreamModule['SessionStreamOwner']>;

  let getSession: ReturnType<typeof vi.fn>;
  let getSessionMessages: ReturnType<typeof vi.fn>;
  let getSessionMessagesSince: ReturnType<typeof vi.fn>;
  let createLiveSessionHints: LiveSessionHintsModule['createLiveSessionHints'];
  let owner: StreamOwner;
  let handlers: SessionStreamHandlers;
  let interests: SessionStreamInterest[];
  let seq: { generation: number; next: number };
  let pollers: LiveSessionHintPoller[];
  let latestHints: Record<string, string>;
  let latestTitles: Record<string, string>;

  const OPENING: SessionSummaryRow = { id: 1, role: 'user' };
  const reading = (id: number, path: string): SessionSummaryRow => ({
    id,
    role: 'tool_call',
    content: JSON.stringify({ name: 'Read', arguments: { file_path: path } }),
  });
  const says = (id: number, content: string): SessionSummaryRow => ({
    id,
    role: 'assistant',
    content,
  });

  function summary(
    rows: SessionSummaryRow[],
    overrides: Partial<SessionSummary> = {}
  ): SessionSummary {
    return {
      status: 'running',
      acpTitle: null,
      workingDir: '/repo',
      openingPromptId: 1,
      rows,
      ...overrides,
    };
  }

  beforeEach(async () => {
    vi.useFakeTimers();
    vi.resetModules();
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    getSession = vi.fn().mockResolvedValue(session({ acpTitle: 'Polled title' }));
    getSessionMessages = vi.fn().mockResolvedValue({
      data: [
        { id: 1, role: 'user', content: 'Opening prompt', createdAt: 1 },
        { id: 2, role: 'assistant', content: 'Polled text', createdAt: 2 },
      ],
    });
    getSessionMessagesSince = vi.fn().mockResolvedValue([]);
    vi.doMock('../../api/commands', () => ({
      getSession,
      getSessionMessages,
      getSessionMessagesSince,
      resolvePathAliases: vi.fn().mockResolvedValue([]),
    }));
    interests = [];
    seq = { generation: 0, next: 1 };
    // The real owner over a stream these tests drive, in place of the app's.
    vi.doMock('../../stores/sessionStream.svelte', async (importOriginal) => {
      const original = await importOriginal<StreamModule>();
      owner = new original.SessionStreamOwner({
        open: (given) => {
          handlers = given;
          return {
            setInterest: async (interest) => {
              interests.push(interest);
            },
            check: async () => true,
            close: () => {},
          };
        },
        resolveRoots: async (roots) => normalizeDisplayRoots(roots),
        onStoreReset: () => () => {},
        onHealthTrigger: () => () => {},
        visibility: { hidden: () => false, listen: () => () => {} },
      });
      return { ...original, sessionStream: owner };
    });
    ({ createLiveSessionHints } = await import('./liveSessionHints'));
    pollers = [];
    latestHints = {};
    latestTitles = {};
  });

  afterEach(() => {
    for (const poller of pollers) poller.destroy();
    owner.stop();
    vi.doUnmock('../../api/commands');
    vi.doUnmock('../../stores/sessionStream.svelte');
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  function create(roots: string[] = ['/repo']): LiveSessionHintPoller {
    const poller = createLiveSessionHints(
      (hints) => {
        latestHints = hints;
      },
      () => roots,
      (titles) => {
        latestTitles = titles;
      }
    );
    pollers.push(poller);
    return poller;
  }

  /** Let a declared interest go out and be accepted. */
  const settle = () => vi.advanceTimersByTimeAsync(10);

  /** Start the app's stream and have the backend accept its first interest. */
  async function establish(): Promise<void> {
    owner.start();
    handlers.onOpen();
    await settle();
    expect(owner.availability).toBe('established');
  }

  /** Deliver a frame in the latest generation with its next sequence number. */
  async function frame(
    kind: 'snapshot' | 'delta',
    sessionId: string,
    given: SessionSummary
  ): Promise<void> {
    const { generation } = interests[interests.length - 1];
    if (seq.generation !== generation) seq = { generation, next: 1 };
    handlers.onFrame({ generation, seq: seq.next++, sessionId, kind, summary: given });
    await vi.advanceTimersByTimeAsync(0);
  }

  function levels(): Record<string, string> {
    const last = interests[interests.length - 1];
    return Object.fromEntries(last.sessions.map((s) => [s.sessionId, s.level]));
  }

  function fetches(): number {
    return (
      getSession.mock.calls.length +
      getSessionMessages.mock.calls.length +
      getSessionMessagesSince.mock.calls.length
    );
  }

  it('takes hints and titles from the stream and fetches nothing', async () => {
    await establish();
    create().syncRunningSessionIds(['s1']);
    await settle();
    expect(levels()).toEqual({ s1: 'summary' });

    await frame('snapshot', 's1', summary([OPENING, reading(2, '/repo/src/a.ts')]));
    expect(latestHints).toEqual({ s1: 'Reading src/a.ts…' });
    expect(latestTitles).toEqual({});

    await frame('delta', 's1', summary([says(3, 'Looking at the cache')], { acpTitle: 'Caching' }));
    expect(latestHints).toEqual({ s1: 'Looking at the cache' });
    expect(latestTitles).toEqual({ s1: 'Caching' });

    // Well past several poll intervals.
    await vi.advanceTimersByTimeAsync(5000);
    expect(fetches()).toBe(0);
  });

  it('shows a reply as soon as its row arrives', async () => {
    await establish();
    create().syncRunningSessionIds(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Done')]));
    expect(latestHints).toEqual({ s1: 'Done' });

    await frame('delta', 's1', summary([{ id: 3, role: 'user', content: 'Also cover eviction' }]));

    expect(latestHints).toEqual({ s1: 'Also cover eviction' });
  });

  it('asks for the title alone for a title-only session and shows no hint for it', async () => {
    await establish();
    create().syncRunningSessionIds(['s1'], ['child', 's1']);
    await settle();
    // An id in both lists is tracked in full.
    expect(levels()).toEqual({ s1: 'summary', child: 'title' });

    await frame('snapshot', 'child', {
      status: 'running',
      acpTitle: '  Child <action>x</action>  title ',
      openingPromptId: null,
      rows: [],
    });

    expect(latestTitles).toEqual({ child: 'Child title' });
    expect(latestHints).toEqual({});
  });

  it('clears the hint and title of a session that stops running', async () => {
    await establish();
    create().syncRunningSessionIds(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')], { acpTitle: 'Title' }));
    expect(latestHints).toEqual({ s1: 'Working' });

    await frame('delta', 's1', summary([], { status: 'completed', acpTitle: 'Title' }));

    expect(latestHints).toEqual({});
    expect(latestTitles).toEqual({});
  });

  it('clears the hint and title of a session the view no longer lists', async () => {
    await establish();
    const poller = create();
    poller.syncRunningSessionIds(['s1', 's2']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'One')], { acpTitle: 'First' }));
    await frame('snapshot', 's2', summary([OPENING, says(2, 'Two')], { acpTitle: 'Second' }));

    poller.syncRunningSessionIds(['s2']);

    expect(latestHints).toEqual({ s2: 'Two' });
    expect(latestTitles).toEqual({ s2: 'Second' });
    // Later frames for it are not shown either, while the owner still holds it.
    await frame('delta', 's1', summary([says(3, 'More')]));
    expect(latestHints).toEqual({ s2: 'Two' });
  });

  it('formats tool paths against the roots the view gives', async () => {
    await establish();
    create(['/repo/apps/staged']).syncRunningSessionIds(['s1']);
    await settle();

    await frame('snapshot', 's1', summary([OPENING, reading(2, '/repo/apps/staged/src/a.ts')]));

    expect(latestHints).toEqual({ s1: 'Reading src/a.ts…' });
  });

  it('has the hint at once for a view that comes back to a session', async () => {
    await establish();
    const first = create();
    first.syncRunningSessionIds(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')], { acpTitle: 'Title' }));
    first.destroy();
    expect(latestHints).toEqual({});

    // The card scrolled out and back: a new view, the same session.
    create().syncRunningSessionIds(['s1']);

    expect(latestHints).toEqual({ s1: 'Working' });
    expect(latestTitles).toEqual({ s1: 'Title' });
    expect(fetches()).toBe(0);
  });

  it('keeps what it shows while paused and catches up on unpause', async () => {
    await establish();
    const poller = create();
    poller.syncRunningSessionIds(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Before')]));

    poller.setPaused(true);
    await settle();
    await frame('delta', 's1', summary([says(3, 'While covered')]));
    expect(latestHints).toEqual({ s1: 'Before' });

    poller.setPaused(false);

    expect(latestHints).toEqual({ s1: 'While covered' });
    expect(fetches()).toBe(0);
  });

  it('lets go of its sessions while paused, which the owner outlasts for a while', async () => {
    await establish();
    const poller = create();
    poller.syncRunningSessionIds(['s1']);
    await settle();
    expect(levels()).toEqual({ s1: 'summary' });

    poller.setPaused(true);
    await vi.advanceTimersByTimeAsync(31_000);

    expect(levels()).toEqual({});
    expect(fetches()).toBe(0);
  });

  it('neither polls nor blanks while the stream is being established', async () => {
    owner.start();
    create().syncRunningSessionIds(['s1']);
    await vi.advanceTimersByTimeAsync(3000);

    expect(owner.availability).toBe('connecting');
    expect(fetches()).toBe(0);
    expect(latestHints).toEqual({});
  });

  it('polls when the stream cannot be established, and stops when it can', async () => {
    owner.start();
    create().syncRunningSessionIds(['s1']);
    await vi.advanceTimersByTimeAsync(5000);

    expect(owner.availability).toBe('unavailable');
    expect(getSession).toHaveBeenCalledWith('s1');
    expect(getSessionMessages).toHaveBeenCalledWith('s1');
    expect(latestHints).toEqual({ s1: 'Polled text' });
    expect(latestTitles).toEqual({ s1: 'Polled title' });

    handlers.onOpen();
    await settle();
    expect(owner.availability).toBe('established');
    expect(levels()).toEqual({ s1: 'summary' });
    const polled = fetches();

    // What the poller found stays until the snapshot replaces it.
    expect(latestHints).toEqual({ s1: 'Polled text' });
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Streamed text')]));
    expect(latestHints).toEqual({ s1: 'Streamed text' });
    expect(latestTitles).toEqual({});

    await vi.advanceTimersByTimeAsync(5000);
    expect(fetches()).toBe(polled);
  });

  it('drops a poll that was in flight when the stream took over', async () => {
    owner.start();
    create().syncRunningSessionIds(['s1']);
    let answer: (value: unknown) => void = () => {};
    getSession.mockReturnValue(new Promise((resolve) => (answer = resolve)));
    await vi.advanceTimersByTimeAsync(5000);
    expect(getSession).toHaveBeenCalled();

    handlers.onOpen();
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Streamed text')]));
    answer(session({ status: 'completed' }));
    await vi.advanceTimersByTimeAsync(0);

    expect(latestHints).toEqual({ s1: 'Streamed text' });
    expect(getSessionMessages).not.toHaveBeenCalled();
  });

  it('holds what it shows when the stream is lost, then polls if it stays lost', async () => {
    await establish();
    create().syncRunningSessionIds(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Streamed text')]));

    handlers.onClose();
    await vi.advanceTimersByTimeAsync(3000);
    expect(owner.availability).toBe('connecting');
    expect(latestHints).toEqual({ s1: 'Streamed text' });
    expect(fetches()).toBe(0);

    await vi.advanceTimersByTimeAsync(2000);
    expect(owner.availability).toBe('unavailable');
    expect(getSession).toHaveBeenCalledWith('s1');
    expect(latestHints).toEqual({ s1: 'Polled text' });
  });

  it('polls as it always has when the stream is switched off', async () => {
    // Never started: what the kill switch leaves.
    create().syncRunningSessionIds(['s1'], ['child']);
    await vi.advanceTimersByTimeAsync(0);

    expect(getSession).toHaveBeenCalledWith('s1');
    expect(getSession).toHaveBeenCalledWith('child');
    expect(latestHints).toEqual({ s1: 'Polled text' });
    expect(interests).toEqual([]);
  });

  it('releases its sessions and clears what it showed when destroyed', async () => {
    await establish();
    const poller = create();
    poller.syncRunningSessionIds(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')], { acpTitle: 'Title' }));

    poller.destroy();
    expect(latestHints).toEqual({});
    expect(latestTitles).toEqual({});

    await frame('delta', 's1', summary([says(3, 'More')]));
    expect(latestHints).toEqual({});
    await vi.advanceTimersByTimeAsync(31_000);
    expect(levels()).toEqual({});
  });
});
