import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Session } from '../../types';
import { STAGED_RESUME_PROMPTS } from './stagedResumePrompts';

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

describe('createLiveSessionHints user replies', () => {
  let getSessionMessages: ReturnType<typeof vi.fn>;
  let getSessionMessagesSince: ReturnType<typeof vi.fn>;
  let createLiveSessionHints: LiveSessionHintsModule['createLiveSessionHints'];
  let poller: LiveSessionHintPoller | null;
  let latestHints: Record<string, string>;

  type Row = { id: number; role: string; content: string; createdAt: number; imageIds?: string[] };
  const row = (id: number, role: string, content: string, imageIds?: string[]): Row => ({
    id,
    role,
    content,
    createdAt: id,
    ...(imageIds ? { imageIds } : {}),
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

  it('shows the latest of several replies', async () => {
    await track([
      OPENING_PROMPT,
      TURN_END,
      row(3, 'user', 'Also cover the eviction policy'),
      row(4, 'user', 'And keep it short'),
    ]);

    expect(latestHints).toEqual({ s1: 'And keep it short' });
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

  it('shows the text the user typed, not the blocks Staged wrapped around it', async () => {
    await track([
      OPENING_PROMPT,
      TURN_END,
      row(
        3,
        'user',
        '<action>\nThe user is asking you to update the linked note.\n</action>\n\n' +
          '<branch-history>\nEarlier sessions…\n</branch-history>\n\n' +
          'Tighten   the intro\nand drop the last section'
      ),
    ]);

    expect(latestHints).toEqual({ s1: 'Tighten the intro and drop the last section' });
  });

  it('leaves the quoted passage out of a reply to a note selection', async () => {
    await track([
      OPENING_PROMPT,
      TURN_END,
      row(
        3,
        'user',
        '<!-- note-quote: Overview › Caching -->\n> Entries expire after five minutes.\n\n' +
          'Is that configurable?'
      ),
    ]);

    expect(latestHints).toEqual({ s1: 'Is that configurable?' });
  });

  it('skips a message Staged wrote on the user’s behalf', async () => {
    // A `write_note` reminder is a user row that is all action block: there is
    // nothing the user typed in it, so the agent's last message stays.
    await track([
      OPENING_PROMPT,
      TURN_END,
      row(3, 'user', '<action>\nYour turn ended without a `write_note` call.\n</action>'),
    ]);

    expect(latestHints).toEqual({ s1: 'Done' });
  });

  it('truncates a long reply', async () => {
    await track([OPENING_PROMPT, TURN_END, row(3, 'user', 'word '.repeat(40))]);

    expect(latestHints.s1).toHaveLength(72);
    expect(latestHints.s1.endsWith('…')).toBe(true);
  });

  it('drops an emoji the cut would split instead of leaving half of it', async () => {
    // An emoji outside the BMP is two UTF-16 units. Seventy letters put the
    // cut at unit 71 between the halves of the first one.
    await track([OPENING_PROMPT, TURN_END, row(3, 'user', `${'a'.repeat(70)}😀😀`)]);

    expect(latestHints).toEqual({ s1: `${'a'.repeat(70)}…` });
  });

  it('keeps an emoji that ends where the cut falls', async () => {
    await track([OPENING_PROMPT, TURN_END, row(3, 'user', `${'a'.repeat(69)}😀😀`)]);

    expect(latestHints).toEqual({ s1: `${'a'.repeat(69)}😀…` });
  });

  it('does not split an emoji when it truncates agent text either', async () => {
    await track([OPENING_PROMPT, row(2, 'assistant', `${'a'.repeat(70)}😀😀`)]);

    expect(latestHints).toEqual({ s1: `${'a'.repeat(70)}…` });
  });

  // The backend stores a resume prompt as a plain user row, with no action
  // block around it: nobody typed it, so it must not read as a reply.
  describe.each(Object.entries(STAGED_RESUME_PROMPTS))('the resume prompt %s', (_name, prompt) => {
    it('leaves the agent’s last message showing', async () => {
      await track([OPENING_PROMPT, TURN_END, row(3, 'user', prompt)]);

      expect(latestHints).toEqual({ s1: 'Done' });
    });

    it('leaves an earlier reply showing', async () => {
      const reply = row(3, 'user', 'Also cover the eviction policy');
      await track([OPENING_PROMPT, TURN_END, reply]);
      expect(latestHints).toEqual({ s1: 'Also cover the eviction policy' });

      await tick([reply, row(4, 'user', prompt)]);

      expect(latestHints).toEqual({ s1: 'Also cover the eviction policy' });
    });

    it('is skipped behind a block Staged wrapped around it', async () => {
      // Skipped only because the tagged blocks are stripped before the text
      // is matched against the resume prompts; this pins that order.
      const wrapped = `<action>\nThe user is requesting a note.\n</action>\n\n${prompt}`;
      await track([OPENING_PROMPT, TURN_END, row(3, 'user', wrapped)]);

      expect(latestHints).toEqual({ s1: 'Done' });
    });

    it('gives way to what the agent writes after the resume', async () => {
      const resume = row(3, 'user', prompt);
      await track([OPENING_PROMPT, TURN_END, resume]);

      await tick([resume, row(4, 'assistant', 'Picking up the eviction section')]);

      expect(latestHints).toEqual({ s1: 'Picking up the eviction section' });
    });
  });

  it('shows a reply that goes on from a resume prompt’s words', async () => {
    await track([
      OPENING_PROMPT,
      TURN_END,
      row(3, 'user', 'Continue where you left off. Skip the benchmarks this time'),
    ]);

    expect(latestHints).toEqual({
      s1: 'Continue where you left off. Skip the benchmarks this time',
    });
  });

  it('shows a message a project agent sent with continue_repo_session', async () => {
    // Stored as a plain user row, like a typed reply: it is an instruction to
    // the session and the transcript shows it as a user bubble.
    await track([
      OPENING_PROMPT,
      TURN_END,
      row(3, 'user', 'Rebase onto main and re-run the cache tests'),
    ]);

    expect(latestHints).toEqual({ s1: 'Rebase onto main and re-run the cache tests' });
  });

  it('shows the content of a reply that is only a fenced block', async () => {
    await track([
      OPENING_PROMPT,
      TURN_END,
      row(3, 'user', '```ts\nconst ttl = 300;\ncache.set(key, value, ttl);\n```'),
    ]);

    expect(latestHints).toEqual({ s1: 'const ttl = 300; cache.set(key, value, ttl);' });
  });

  it('keeps a fenced block’s content in line with the text around it', async () => {
    await track([
      OPENING_PROMPT,
      TURN_END,
      row(3, 'user', 'Use this instead:\n\n~~~\nttl = 300\n~~~\n\nand keep `ttl` an int'),
    ]);

    expect(latestHints).toEqual({ s1: 'Use this instead: ttl = 300 and keep `ttl` an int' });
  });

  it.each(['use Vec<String> here', 'if a < b and c > d'])(
    'keeps angle brackets in typed text: %s',
    async (typed) => {
      await track([OPENING_PROMPT, TURN_END, row(3, 'user', typed)]);

      expect(latestHints).toEqual({ s1: typed });
    }
  );

  it('shows a placeholder for a reply that is an attachment and no text', async () => {
    await track([OPENING_PROMPT, TURN_END, row(3, 'user', '', ['img-1'])]);

    expect(latestHints).toEqual({ s1: 'Attachment' });
  });

  it('counts the attachments of a reply with several and no text', async () => {
    await track([OPENING_PROMPT, TURN_END, row(3, 'user', '  \n', ['img-1', 'img-2'])]);

    expect(latestHints).toEqual({ s1: '2 attachments' });
  });

  it('shows the typed text of a reply that also has an attachment', async () => {
    await track([OPENING_PROMPT, TURN_END, row(3, 'user', 'Match this mock-up', ['img-1'])]);

    expect(latestHints).toEqual({ s1: 'Match this mock-up' });
  });

  it('skips an empty user row with no attachment', async () => {
    // The agent's echo of a delivered message is stored this way, and so is a
    // row whose attachment list came back empty.
    await track([OPENING_PROMPT, TURN_END, row(3, 'user', ''), row(4, 'user', '', [])]);

    expect(latestHints).toEqual({ s1: 'Done' });
  });
});
