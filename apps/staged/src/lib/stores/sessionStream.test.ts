import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  normalizeDisplayRoots,
  type DisplayRootInput,
} from '../features/sessions/pathDisplayRoots';
import { deriveHint, HINT_ROW_WINDOW } from '../features/timeline/sessionHint';
import type { SessionStreamHandlers } from '../transport';
import type {
  SessionStatus,
  SessionStreamFrame,
  SessionStreamInterest,
  SessionSummary,
  SessionSummaryRow,
} from '../types';
import {
  ESTABLISH_GRACE_MS,
  GAP_RESTART_SPACING_MS,
  INTEREST_DELAY_MS,
  LINGER_MS,
  RETRY_BASE_MS,
  SESSION_STREAM_OVERRIDE_KEY,
  SessionStreamOwner,
  sessionStreamEnabled,
  type SessionStreamConsumer,
} from './sessionStream.svelte';

// The plain project has no Svelte plugin and resolves `svelte/reactivity` to
// its server build, where the owner's reactive maps are plain `Map`s. That is
// all these tests need: they are about what the owner declares and holds.
// `sessionStream.component.test.ts` covers the reactivity.

const OPENING_PROMPT: SessionSummaryRow = { id: 1, role: 'user' };

function toolCall(id: number, path: string): SessionSummaryRow {
  return {
    id,
    role: 'tool_call',
    content: JSON.stringify({ name: 'Read', arguments: { file_path: path } }),
  };
}

function assistant(id: number, content: string): SessionSummaryRow {
  return { id, role: 'assistant', content };
}

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

/** What a title-level frame carries: the status and title, nothing else. */
function titleSummary(acpTitle: string | null, status: SessionStatus = 'running'): SessionSummary {
  return { status, acpTitle, openingPromptId: null, rows: [] };
}

type Harness = ReturnType<typeof createHarness>;

function createHarness(
  resolveRoots: (roots: DisplayRootInput) => Promise<string[]> = async (roots) =>
    normalizeDisplayRoots(roots)
) {
  const interests: SessionStreamInterest[] = [];
  let handlers: SessionStreamHandlers | undefined;
  let storeReset: (() => void) | undefined;
  let visibilityChanged: (() => void) | undefined;
  const state: {
    hidden: boolean;
    closed: boolean;
    failing: boolean;
    seq: number;
    generation?: number;
  } = { hidden: false, closed: false, failing: false, seq: 0 };

  const owner = new SessionStreamOwner({
    open: (given) => {
      handlers = given;
      return {
        setInterest: async (interest) => {
          interests.push(interest);
          if (state.failing) throw new Error('No session stream is open for client');
        },
        close: () => {
          state.closed = true;
        },
      };
    },
    resolveRoots,
    onStoreReset: (listener) => {
      storeReset = listener;
      return () => {};
    },
    visibility: {
      hidden: () => state.hidden,
      listen: (listener) => {
        visibilityChanged = listener;
        return () => {};
      },
    },
  });

  const last = () => interests[interests.length - 1];

  /** Let the interest timer fire and its call settle. */
  const settle = () => vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS);

  return {
    owner,
    interests,
    state,
    last,
    settle,
    /** Start the owner and open its stream, as the transport does. */
    async open() {
      owner.start();
      handlers?.onOpen();
      await settle();
    },
    reopen: () => handlers?.onOpen(),
    close: () => handlers?.onClose(),
    resetStore: () => storeReset?.(),
    setHidden(hidden: boolean) {
      state.hidden = hidden;
      visibilityChanged?.();
    },
    /** Deliver a frame in the current generation with the next sequence number. */
    async frame(kind: 'snapshot' | 'delta', sessionId: string, given: SessionSummary) {
      const interest = last();
      if (state.seq > 0 && interest.generation !== state.generation) state.seq = 0;
      state.generation = interest.generation;
      state.seq += 1;
      handlers?.onFrame({
        generation: interest.generation,
        seq: state.seq,
        sessionId,
        kind,
        summary: given,
      });
      // Root aliases resolve in a microtask.
      await vi.advanceTimersByTimeAsync(0);
    },
    raw: (frame: SessionStreamFrame) => handlers?.onFrame(frame),
  } as {
    owner: SessionStreamOwner;
    interests: SessionStreamInterest[];
    state: typeof state;
    last: () => SessionStreamInterest;
    settle: () => Promise<unknown>;
    open: () => Promise<void>;
    reopen: () => void;
    close: () => void;
    resetStore: () => void;
    setHidden: (hidden: boolean) => void;
    frame: (kind: 'snapshot' | 'delta', sessionId: string, given: SessionSummary) => Promise<void>;
    raw: (frame: SessionStreamFrame) => void;
  };
}

function levels(interest: SessionStreamInterest): Record<string, string> {
  return Object.fromEntries(interest.sessions.map((s) => [s.sessionId, s.level]));
}

describe('SessionStreamOwner', () => {
  let h: Harness;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    h = createHarness();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  /** A consumer showing `ids` with subtitles against `/repo`. */
  async function show(ids: string[], roots: string[] = ['/repo']): Promise<SessionStreamConsumer> {
    const consumer = h.owner.consume();
    consumer.sync({ summary: ids, title: [], displayRoots: roots });
    await h.settle();
    return consumer;
  }

  describe('availability', () => {
    it('is unavailable until it is started, then connecting until an interest is accepted', async () => {
      expect(h.owner.availability).toBe('unavailable');

      h.owner.start();
      expect(h.owner.availability).toBe('connecting');
      expect(h.interests).toEqual([]);

      // Nothing is shown yet, and the empty interest is still declared: its
      // answer is what shows the stream is there.
      h.reopen();
      await h.settle();
      expect(h.interests).toEqual([
        { generation: 1, paused: false, summaryWindow: HINT_ROW_WINDOW, sessions: [] },
      ]);
      expect(h.owner.availability).toBe('established');
    });

    it('gives up on a stream that is not established within the grace period', async () => {
      const seen: string[] = [];
      h.owner.onAvailability((availability) => seen.push(availability));

      h.owner.start();
      await vi.advanceTimersByTimeAsync(ESTABLISH_GRACE_MS - 1);
      expect(h.owner.availability).toBe('connecting');
      await vi.advanceTimersByTimeAsync(1);
      expect(h.owner.availability).toBe('unavailable');

      // A stream that turns up late is still taken.
      h.reopen();
      await h.settle();
      expect(seen).toEqual(['connecting', 'unavailable', 'established']);
    });

    it('holds through a closed stream for the grace period, then gives up', async () => {
      await h.open();
      h.close();
      expect(h.owner.availability).toBe('connecting');

      await vi.advanceTimersByTimeAsync(ESTABLISH_GRACE_MS);
      expect(h.owner.availability).toBe('unavailable');
    });

    it('stays off a closed stream until it reopens', async () => {
      await h.open();
      h.close();
      const calls = h.interests.length;

      h.owner.consume().sync({ summary: ['s1'], title: [] });
      await vi.advanceTimersByTimeAsync(1_000);
      expect(h.interests).toHaveLength(calls);

      h.reopen();
      await h.settle();
      expect(levels(h.last())).toEqual({ s1: 'summary' });
      expect(h.owner.availability).toBe('established');
    });
  });

  describe('interest', () => {
    it('declares one interest for a burst of changes', async () => {
      await h.open();
      const calls = h.interests.length;

      const a = h.owner.consume();
      const b = h.owner.consume();
      a.sync({ summary: ['s1'], title: [] });
      b.sync({ summary: ['s2'], title: ['s3'] });
      a.sync({ summary: ['s1', 's4'], title: [] });
      await h.settle();

      expect(h.interests).toHaveLength(calls + 1);
      expect(levels(h.last())).toEqual({
        s1: 'summary',
        s2: 'summary',
        s3: 'title',
        s4: 'summary',
      });
      expect(h.last().summaryWindow).toBe(HINT_ROW_WINDOW);
    });

    it('declares a session once, at the highest level any consumer wants', async () => {
      await h.open();
      const section = h.owner.consume();
      const timeline = h.owner.consume();
      section.sync({ summary: [], title: ['child'] });
      timeline.sync({ summary: ['child'], title: [] });
      await h.settle();

      expect(h.last().sessions).toEqual([{ sessionId: 'child', level: 'summary' }]);
    });

    it('counts a session listed for both as its summary', async () => {
      await h.open();
      h.owner.consume().sync({ summary: ['s1'], title: ['s1'] });
      await h.settle();

      expect(h.last().sessions).toEqual([{ sessionId: 's1', level: 'summary' }]);
    });

    it('only ever asks for the title and summary levels', async () => {
      await h.open();
      h.owner.consume().sync({ summary: ['s1', 's2'], title: ['s3'] });
      await h.settle();
      h.reopen();
      await h.settle();

      for (const interest of h.interests) {
        for (const session of interest.sessions) {
          expect(['title', 'summary']).toContain(session.level);
        }
      }
    });

    it('sends nothing when a sync changes nothing', async () => {
      await h.open();
      const consumer = await show(['s1']);
      const calls = h.interests.length;

      consumer.sync({ summary: ['s1'], title: [], displayRoots: ['/repo'] });
      await h.settle();

      expect(h.interests).toHaveLength(calls);
    });

    it('keeps a session two consumers share while either still shows it', async () => {
      await h.open();
      const a = await show(['s1']);
      const b = await show(['s1']);
      const calls = h.interests.length;

      a.release();
      await vi.advanceTimersByTimeAsync(LINGER_MS + INTEREST_DELAY_MS);
      expect(h.interests).toHaveLength(calls);
      expect(levels(h.last())).toEqual({ s1: 'summary' });

      b.release();
      await vi.advanceTimersByTimeAsync(LINGER_MS + INTEREST_DELAY_MS);
      expect(h.last().sessions).toEqual([]);
    });
  });

  describe('linger', () => {
    it('keeps a released session declared, with its hint, for the linger time', async () => {
      await h.open();
      const consumer = await show(['s1']);
      await h.frame('snapshot', 's1', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')]));
      const calls = h.interests.length;

      consumer.release();
      await vi.advanceTimersByTimeAsync(LINGER_MS - 1);
      expect(h.interests).toHaveLength(calls);
      expect(h.owner.hint('s1', ['/repo'])).toBe('Reading the cache');

      // Still fed while it lingers.
      await h.frame('delta', 's1', summary([assistant(3, 'Writing the note')]));
      expect(h.owner.hint('s1', ['/repo'])).toBe('Writing the note');
    });

    it('hands a returning consumer the hint at once, with no new call', async () => {
      await h.open();
      const first = await show(['s1']);
      await h.frame('snapshot', 's1', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')]));
      const calls = h.interests.length;

      first.release();
      await vi.advanceTimersByTimeAsync(LINGER_MS / 2);
      const second = h.owner.consume();
      second.sync({ summary: ['s1'], title: [], displayRoots: ['/repo'] });

      expect(second.read('s1')).toEqual({
        status: 'running',
        title: undefined,
        hint: 'Reading the cache',
      });
      // And the first claim running out changes nothing: the second holds it.
      await vi.advanceTimersByTimeAsync(LINGER_MS);
      expect(h.interests).toHaveLength(calls);
    });

    it('drops a session, and what it held, once the linger time is up', async () => {
      await h.open();
      const consumer = await show(['s1']);
      await h.frame(
        'snapshot',
        's1',
        summary([OPENING_PROMPT, assistant(2, 'Reading the cache')], { acpTitle: 'Cache note' })
      );

      consumer.release();
      await vi.advanceTimersByTimeAsync(LINGER_MS + INTEREST_DELAY_MS);

      expect(h.last().sessions).toEqual([]);
      expect(h.owner.hint('s1', ['/repo'])).toBeUndefined();
      expect(h.owner.title('s1')).toBeUndefined();
      expect(h.owner.status('s1')).toBeUndefined();
    });

    it('keeps a summary a consumer let go of while another still shows the title', async () => {
      await h.open();
      const section = h.owner.consume();
      section.sync({ summary: [], title: ['child'] });
      const timeline = await show(['child']);
      await h.frame('snapshot', 'child', summary([OPENING_PROMPT, assistant(2, 'Running tests')]));

      // The branch card scrolls out: its summary claim lingers, so the
      // session is not lowered to its title and its rows are kept.
      timeline.release();
      await vi.advanceTimersByTimeAsync(LINGER_MS / 2);
      expect(levels(h.last())).toEqual({ child: 'summary' });
      expect(h.owner.hint('child', ['/repo'])).toBe('Running tests');

      await vi.advanceTimersByTimeAsync(LINGER_MS);
      expect(levels(h.last())).toEqual({ child: 'title' });
      expect(h.owner.hint('child', ['/repo'])).toBeUndefined();
      expect(h.owner.status('child')).toBe('running');
    });
  });

  describe('frames', () => {
    it('exposes the status and the cleaned-up title', async () => {
      await h.open();
      const consumer = h.owner.consume();
      consumer.sync({ summary: [], title: ['s1'] });
      await h.settle();
      expect(consumer.read('s1')).toBeUndefined();

      await h.frame(
        'snapshot',
        's1',
        titleSummary('  Fix   login <action>injected</action> flow ')
      );
      expect(h.owner.status('s1')).toBe('running');
      expect(h.owner.title('s1')).toBe('Fix login flow');
      expect(consumer.read('s1')).toEqual({
        status: 'running',
        title: 'Fix login flow',
        hint: undefined,
      });

      await h.frame('delta', 's1', titleSummary(null, 'completed'));
      expect(h.owner.status('s1')).toBe('completed');
      expect(h.owner.title('s1')).toBeUndefined();
    });

    it('replaces the rows on a snapshot', async () => {
      await h.open();
      await show(['s1']);
      await h.frame('snapshot', 's1', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')]));
      expect(h.owner.hint('s1', ['/repo'])).toBe('Reading the cache');

      // The row behind the hint is not in the new snapshot.
      await h.frame('snapshot', 's1', summary([OPENING_PROMPT]));
      expect(h.owner.hint('s1', ['/repo'])).toBeUndefined();
    });

    it('upserts a delta’s rows by id', async () => {
      await h.open();
      await show(['s1']);
      await h.frame('snapshot', 's1', summary([OPENING_PROMPT, assistant(2, 'Reading')]));

      // The same row, grown: replaced in place, not appended.
      await h.frame('delta', 's1', summary([assistant(2, 'Reading the cache layer')]));
      expect(h.owner.hint('s1', ['/repo'])).toBe('Reading the cache layer');

      // A newer row wins, and an update to the older one does not take over.
      await h.frame('delta', 's1', summary([toolCall(3, '/repo/src/cache.ts')]));
      expect(h.owner.hint('s1', ['/repo'])).toBe('Reading src/cache.ts…');
      await h.frame('delta', 's1', summary([assistant(2, 'Reading the cache layer again')]));
      expect(h.owner.hint('s1', ['/repo'])).toBe('Reading src/cache.ts…');

      // Applying a delta twice changes nothing.
      await h.frame('delta', 's1', summary([toolCall(3, '/repo/src/cache.ts')]));
      expect(h.owner.hint('s1', ['/repo'])).toBe('Reading src/cache.ts…');
    });

    it('trims the rows to the window', async () => {
      await h.open();
      await show(['s1']);
      // A reply, then a window's worth of rows that derive to nothing.
      await h.frame(
        'snapshot',
        's1',
        summary([OPENING_PROMPT, { id: 2, role: 'user', content: 'Also cover eviction' }])
      );
      expect(h.owner.hint('s1', ['/repo'])).toBe('Also cover eviction');

      const results: SessionSummaryRow[] = Array.from({ length: HINT_ROW_WINDOW - 1 }, (_, i) => ({
        id: 3 + i,
        role: 'tool_result',
      }));
      await h.frame('delta', 's1', summary(results));
      // The reply is the oldest row of a full window, and still read.
      expect(h.owner.hint('s1', ['/repo'])).toBe('Also cover eviction');

      await h.frame('delta', 's1', summary([{ id: 3 + HINT_ROW_WINDOW, role: 'tool_result' }]));
      expect(h.owner.hint('s1', ['/repo'])).toBeUndefined();
    });

    it('keeps a snapshot larger than the window to its newest rows', async () => {
      await h.open();
      await show(['s1']);
      const rows: SessionSummaryRow[] = [
        { id: 2, role: 'user', content: 'Also cover eviction' },
        ...Array.from({ length: HINT_ROW_WINDOW }, (_, i): SessionSummaryRow => ({
          id: 3 + i,
          role: 'tool_result',
        })),
      ];
      await h.frame('snapshot', 's1', summary(rows));

      expect(h.owner.hint('s1', ['/repo'])).toBeUndefined();
    });

    it('does not take the opening prompt for a reply, on a snapshot or after it', async () => {
      await h.open();
      await show(['s1']);
      await h.frame(
        'snapshot',
        's1',
        summary([{ id: 7, role: 'user', content: 'Summarise the cache' }], { openingPromptId: 7 })
      );
      expect(h.owner.hint('s1', ['/repo'])).toBeUndefined();

      // A delta that read no transcript carries no opening prompt id; the
      // one already known stands.
      await h.frame('delta', 's1', summary([], { openingPromptId: null }));
      expect(h.owner.hint('s1', ['/repo'])).toBeUndefined();

      await h.frame(
        'delta',
        's1',
        summary([{ id: 9, role: 'user', content: 'Also cover eviction' }], {
          openingPromptId: null,
        })
      );
      expect(h.owner.hint('s1', ['/repo'])).toBe('Also cover eviction');
    });

    it('shows the pipeline step of a session whose transcript is still empty', async () => {
      await h.open();
      await show(['s1']);
      await h.frame(
        'snapshot',
        's1',
        summary([], {
          openingPromptId: null,
          pipeline: {
            steps: [
              {
                label: 'Fetching the branch',
                stepType: 'command',
                status: 'running',
                output: null,
                error: null,
                startedAt: null,
                completedAt: null,
              },
            ],
            currentStep: 0,
            completedWithoutAi: false,
          },
        })
      );

      expect(h.owner.hint('s1', ['/repo'])).toBe('Fetching the branch…');
    });

    it('ignores a frame for a session it does not hold', async () => {
      await h.open();
      await show(['s1']);
      await h.frame('snapshot', 'other', summary([OPENING_PROMPT, assistant(2, 'Elsewhere')]));

      expect(h.owner.status('other')).toBeUndefined();
      // And it still counted, so the next frame is in sequence.
      const calls = h.interests.length;
      await h.frame('snapshot', 's1', summary([OPENING_PROMPT, assistant(2, 'Here')]));
      await h.settle();
      expect(h.interests).toHaveLength(calls);
      expect(h.owner.hint('s1', ['/repo'])).toBe('Here');
    });

    it('tells a consumer about its own sessions only', async () => {
      await h.open();
      const changed: string[] = [];
      const consumer = h.owner.consume((sessionId) => changed.push(sessionId));
      consumer.sync({ summary: ['s1'], title: ['s2'], displayRoots: ['/repo'] });
      await show(['s3']);

      await h.frame('snapshot', 's3', summary([OPENING_PROMPT, assistant(2, 'Not mine')]));
      expect(changed).toEqual([]);

      await h.frame('snapshot', 's2', titleSummary('A title'));
      expect(changed).toEqual(['s2']);

      await h.frame('snapshot', 's1', summary([OPENING_PROMPT, assistant(2, 'Mine')]));
      expect(changed.filter((id) => id === 's1').length).toBeGreaterThan(0);
      expect(consumer.read('s1')?.hint).toBe('Mine');
    });
  });

  describe('per-root hints', () => {
    it('derives a session’s hint once for each root set that shows it', async () => {
      await h.open();
      const section = await show(['s1'], ['/repo']);
      const timeline = await show(['s1'], ['/repo/src']);
      const rows = [OPENING_PROMPT, toolCall(2, '/repo/src/cache.ts')];
      await h.frame('snapshot', 's1', summary(rows, { workingDir: '/elsewhere' }));

      const underRepo = deriveHint(rows, ['/repo', '/elsewhere'], 1);
      const underSrc = deriveHint(rows, ['/repo/src', '/elsewhere'], 1);
      expect(underRepo).toBe('Reading src/cache.ts…');
      expect(underSrc).not.toBe(underRepo);

      expect(h.owner.hint('s1', ['/repo'])).toBe(underRepo);
      expect(h.owner.hint('s1', ['/repo/src'])).toBe(underSrc);
      expect(section.read('s1')?.hint).toBe(underRepo);
      expect(timeline.read('s1')?.hint).toBe(underSrc);
      // One session on the wire all the same.
      expect(h.last().sessions).toEqual([{ sessionId: 's1', level: 'summary' }]);
    });

    it('shortens against the session’s working directory as well', async () => {
      await h.open();
      await show(['s1'], []);
      await h.frame('snapshot', 's1', summary([OPENING_PROMPT, toolCall(2, '/repo/src/cache.ts')]));

      expect(h.owner.hint('s1', [])).toBe('Reading src/cache.ts…');
    });

    it('holds a hint back until its roots’ aliases are resolved', async () => {
      let resolve: ((roots: string[]) => void) | undefined;
      h = createHarness(() => new Promise((given) => (resolve = given)));
      await h.open();
      const changed: string[] = [];
      const consumer = h.owner.consume((sessionId) => changed.push(sessionId));
      consumer.sync({ summary: ['s1'], title: [], displayRoots: ['/link'] });
      await h.settle();

      await h.frame(
        'snapshot',
        's1',
        summary([OPENING_PROMPT, toolCall(2, '/real/src/cache.ts')], { workingDir: '' })
      );
      // Not shown long and then shortened: nothing until the aliases are in.
      expect(consumer.read('s1')).toBeUndefined();
      expect(h.owner.hint('s1', ['/link'])).toBeUndefined();

      changed.length = 0;
      resolve?.(['/link', '/real']);
      await vi.advanceTimersByTimeAsync(0);
      expect(h.owner.hint('s1', ['/link'])).toBe('Reading src/cache.ts…');
      expect(consumer.read('s1')?.hint).toBe('Reading src/cache.ts…');
      expect(changed).toEqual(['s1']);
    });
  });

  describe('levels', () => {
    it('drops the rows of a session lowered to its title', async () => {
      await h.open();
      const consumer = await show(['s1']);
      await h.frame(
        'snapshot',
        's1',
        summary([OPENING_PROMPT, assistant(2, 'Reading the cache')], { acpTitle: 'Cache note' })
      );

      consumer.sync({ summary: [], title: ['s1'] });
      await vi.advanceTimersByTimeAsync(LINGER_MS + INTEREST_DELAY_MS);
      expect(levels(h.last())).toEqual({ s1: 'title' });
      expect(h.owner.hint('s1', ['/repo'])).toBeUndefined();
      expect(h.owner.title('s1')).toBe('Cache note');
      expect(consumer.read('s1')).toEqual({
        status: 'running',
        title: 'Cache note',
        hint: undefined,
      });
    });

    it('waits for the snapshot of a session raised to its summary', async () => {
      await h.open();
      const consumer = h.owner.consume();
      consumer.sync({ summary: [], title: ['s1'] });
      await h.settle();
      await h.frame('snapshot', 's1', titleSummary('Cache note'));

      consumer.sync({ summary: ['s1'], title: [], displayRoots: ['/repo'] });
      await h.settle();
      expect(levels(h.last())).toEqual({ s1: 'summary' });
      // Known at the title level, not yet at the one now asked for.
      expect(consumer.read('s1')).toBeUndefined();

      // A title-level frame that was already on its way is not the summary.
      await h.frame('delta', 's1', titleSummary('Cache note, renamed'));
      expect(consumer.read('s1')).toBeUndefined();
      expect(h.owner.title('s1')).toBe('Cache note, renamed');

      await h.frame(
        'snapshot',
        's1',
        summary([OPENING_PROMPT, assistant(2, 'Reading the cache')], {
          acpTitle: 'Cache note, renamed',
        })
      );
      expect(consumer.read('s1')).toEqual({
        status: 'running',
        title: 'Cache note, renamed',
        hint: 'Reading the cache',
      });
    });
  });

  describe('starting over', () => {
    it('drops a frame from a generation it has replaced', async () => {
      await h.open();
      await show(['s1']);
      await h.frame('snapshot', 's1', summary([OPENING_PROMPT, assistant(2, 'Current')]));
      const generation = h.last().generation;

      h.resetStore();
      await h.settle();
      expect(h.last().generation).toBe(generation + 1);

      h.raw({
        generation,
        seq: 2,
        sessionId: 's1',
        kind: 'delta',
        summary: summary([assistant(3, 'From before')]),
      });
      expect(h.owner.hint('s1', ['/repo'])).toBe('Current');
    });

    it('declares its interest again under a new generation when a frame is lost', async () => {
      await h.open();
      await show(['s1']);
      await h.frame('snapshot', 's1', summary([OPENING_PROMPT, assistant(2, 'Before the gap')]));
      const before = h.last();
      const calls = h.interests.length;

      // Frame 2 never arrives.
      h.raw({
        generation: before.generation,
        seq: 3,
        sessionId: 's1',
        kind: 'delta',
        summary: summary([assistant(4, 'After the gap')]),
      });
      await h.settle();

      expect(h.interests).toHaveLength(calls + 1);
      expect(h.last()).toEqual({ ...before, generation: before.generation + 1 });
      // The frame that showed the gap is not applied, and what was held
      // stays until the snapshot replaces it.
      expect(h.owner.hint('s1', ['/repo'])).toBe('Before the gap');
      expect(h.owner.availability).toBe('established');

      await h.frame('snapshot', 's1', summary([OPENING_PROMPT, assistant(4, 'After the gap')]));
      expect(h.owner.hint('s1', ['/repo'])).toBe('After the gap');
    });

    it('spaces out restarts for lost frames', async () => {
      await h.open();
      await show(['s1']);
      const gap = () =>
        h.raw({
          generation: h.last().generation,
          seq: 5,
          sessionId: 's1',
          kind: 'delta',
          summary: summary([]),
        });

      gap();
      await h.settle();
      const calls = h.interests.length;

      gap();
      await vi.advanceTimersByTimeAsync(GAP_RESTART_SPACING_MS - INTEREST_DELAY_MS - 1);
      expect(h.interests).toHaveLength(calls);
      await vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS + 1);
      expect(h.interests).toHaveLength(calls + 1);
    });

    it('declares its whole interest again when the stream reopens', async () => {
      await h.open();
      await show(['s1']);
      h.owner.consume().sync({ summary: [], title: ['s2'] });
      await h.settle();
      const before = h.last();

      // The socket dropped and came back: the backend forgot this client.
      h.close();
      h.reopen();
      await h.settle();

      expect(h.last()).toEqual({ ...before, generation: before.generation + 1 });
      expect(h.owner.availability).toBe('established');
    });

    it('asks again when the backend’s store is replaced', async () => {
      await h.open();
      await show(['s1']);
      const before = h.last();

      h.resetStore();
      await h.settle();

      expect(h.last()).toEqual({ ...before, generation: before.generation + 1 });
    });

    it('retries a refused interest under a new generation, backing off', async () => {
      h.state.failing = true;
      h.owner.start();
      h.reopen();
      await h.settle();
      expect(h.interests.map((interest) => interest.generation)).toEqual([1]);
      expect(h.owner.availability).toBe('connecting');

      await vi.advanceTimersByTimeAsync(RETRY_BASE_MS - 1);
      expect(h.interests).toHaveLength(1);
      await vi.advanceTimersByTimeAsync(1);
      expect(h.interests.map((interest) => interest.generation)).toEqual([1, 2]);

      // Twice as long before the third.
      await vi.advanceTimersByTimeAsync(2 * RETRY_BASE_MS - 1);
      expect(h.interests).toHaveLength(2);

      h.state.failing = false;
      await vi.advanceTimersByTimeAsync(1);
      expect(h.interests.map((interest) => interest.generation)).toEqual([1, 2, 3]);
      expect(h.owner.availability).toBe('established');
    });

    it('falls back when an established stream starts refusing, and returns when it stops', async () => {
      await h.open();
      const seen: string[] = [];
      h.owner.onAvailability((availability) => seen.push(availability));

      h.state.failing = true;
      h.owner.consume().sync({ summary: ['s1'], title: [] });
      await h.settle();
      expect(seen).toEqual(['connecting']);

      await vi.advanceTimersByTimeAsync(ESTABLISH_GRACE_MS);
      expect(seen).toEqual(['connecting', 'unavailable']);

      h.state.failing = false;
      await vi.advanceTimersByTimeAsync(60_000);
      expect(seen).toEqual(['connecting', 'unavailable', 'established']);
      expect(levels(h.last())).toEqual({ s1: 'summary' });
    });
  });

  describe('a hidden window', () => {
    it('pauses its interest, and unpauses it when shown', async () => {
      await h.open();
      await show(['s1']);
      const shown = h.last();

      h.setHidden(true);
      await h.settle();
      expect(h.last()).toEqual({ ...shown, paused: true });

      // Same generation: the backend answers the unpausing with snapshots.
      h.setHidden(false);
      await h.settle();
      expect(h.last()).toEqual(shown);
    });

    it('starts paused in a window that is already hidden', async () => {
      h.state.hidden = true;
      await h.open();

      expect(h.last().paused).toBe(true);
    });
  });

  describe('stop', () => {
    it('hands back an empty interest, closes the stream and forgets what it held', async () => {
      await h.open();
      await show(['s1']);
      await h.frame('snapshot', 's1', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')]));

      h.owner.stop();
      await vi.advanceTimersByTimeAsync(0);

      expect(h.last().sessions).toEqual([]);
      expect(h.state.closed).toBe(true);
      expect(h.owner.availability).toBe('unavailable');
      expect(h.owner.hint('s1', ['/repo'])).toBeUndefined();
      expect(h.owner.status('s1')).toBeUndefined();
    });
  });
});

describe('sessionStreamEnabled', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function stubStorage(value: string | null) {
    vi.stubGlobal('localStorage', {
      getItem: (key: string) => (key === SESSION_STREAM_OVERRIDE_KEY ? value : null),
    });
  }

  it('is on by default', () => {
    stubStorage(null);
    expect(sessionStreamEnabled()).toBe(true);
  });

  it('is on with no storage to read', () => {
    expect(sessionStreamEnabled()).toBe(true);
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('denied');
      },
    });
    expect(sessionStreamEnabled()).toBe(true);
  });

  it('follows the override', () => {
    stubStorage('off');
    expect(sessionStreamEnabled()).toBe(false);
    stubStorage('on');
    expect(sessionStreamEnabled()).toBe(true);
    stubStorage('something else');
    expect(sessionStreamEnabled()).toBe(true);
  });
});
