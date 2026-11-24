/**
 * What a view's rows show from the session stream (`createLiveSessionRows`),
 * over the real owner and a stream these tests drive. The owner's maps are
 * plain ones here, so a read is the value at that moment;
 * `liveSessionHints.component.test.ts` covers the reactivity.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { normalizeDisplayRoots } from '../sessions/pathDisplayRoots';
import {
  ESTABLISH_GRACE_MS,
  INTEREST_DELAY_MS,
  LINGER_MS,
  SessionStreamOwner,
} from '../../stores/sessionStream.svelte';
import type { SessionStreamHandlers } from '../../transport';
import type { SessionStreamInterest, SessionSummary, SessionSummaryRow } from '../../types';
import {
  collectRunningSessionIds,
  createLiveSessionRows,
  fallbackHintForPendingType,
  type LiveSessionRows,
} from './liveSessionHints';

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

describe('createLiveSessionRows', () => {
  let owner: SessionStreamOwner;
  let handlers: SessionStreamHandlers;
  let interests: SessionStreamInterest[];
  /** What the backend does with an interest: accept it, unless a test says otherwise. */
  let acceptInterest: (interest: SessionStreamInterest) => Promise<void>;
  let seq: { generation: number; next: number };
  let views: LiveSessionRows[];

  beforeEach(() => {
    vi.useFakeTimers();
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    interests = [];
    acceptInterest = async () => {};
    seq = { generation: 0, next: 1 };
    views = [];
    owner = new SessionStreamOwner({
      open: (given) => {
        handlers = given;
        return {
          setInterest: async (interest) => {
            interests.push(interest);
            await acceptInterest(interest);
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
  });

  afterEach(() => {
    for (const view of views) view.destroy();
    owner.stop();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  /** A view, and the switch for whether its rows can be seen. */
  function create(roots: string[] = ['/repo']) {
    const shown = { visible: true };
    const rows = createLiveSessionRows({
      owner,
      visible: () => shown.visible,
      displayRoots: () => roots,
    });
    views.push(rows);
    return { rows, shown };
  }

  /** Let a declared interest go out and be accepted. */
  const settle = () => vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS);

  /** Start the stream and have the backend accept its first interest. */
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
    // Root aliases resolve in a microtask.
    await vi.advanceTimersByTimeAsync(0);
  }

  function levels(): Record<string, string> {
    const last = interests[interests.length - 1];
    return Object.fromEntries(last.sessions.map((s) => [s.sessionId, s.level]));
  }

  it('shows a session’s subtitle and title as the frames bring them', async () => {
    await establish();
    const { rows } = create();
    rows.sync(['s1']);
    await settle();
    expect(levels()).toEqual({ s1: 'summary' });
    expect(rows.hint('s1')).toBeUndefined();
    expect(rows.title('s1')).toBeUndefined();

    await frame('snapshot', 's1', summary([OPENING, reading(2, '/repo/src/a.ts')]));
    expect(rows.hint('s1')).toBe('Reading src/a.ts…');
    expect(rows.title('s1')).toBeUndefined();

    await frame('delta', 's1', summary([says(3, 'Looking at the cache')], { acpTitle: 'Caching' }));
    expect(rows.hint('s1')).toBe('Looking at the cache');
    expect(rows.title('s1')).toBe('Caching');
  });

  it('shows a reply as soon as its row arrives', async () => {
    await establish();
    const { rows } = create();
    rows.sync(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Done')]));
    expect(rows.hint('s1')).toBe('Done');

    await frame('delta', 's1', summary([{ id: 3, role: 'user', content: 'Also cover eviction' }]));

    expect(rows.hint('s1')).toBe('Also cover eviction');
  });

  it('asks for the title alone for a title-only session and shows no hint for it', async () => {
    await establish();
    const { rows } = create();
    rows.sync(['s1'], ['child', 's1']);
    await settle();
    // An id in both lists is shown in full.
    expect(levels()).toEqual({ s1: 'summary', child: 'title' });

    await frame('snapshot', 'child', {
      status: 'running',
      acpTitle: '  Child <action>x</action>  title ',
      openingPromptId: null,
      rows: [],
    });

    expect(rows.title('child')).toBe('Child title');
    expect(rows.hint('child')).toBeUndefined();
  });

  it('formats tool paths against the roots the view gives', async () => {
    await establish();
    const { rows } = create(['/repo/apps/staged']);
    rows.sync(['s1']);
    await settle();

    await frame('snapshot', 's1', summary([OPENING, reading(2, '/repo/apps/staged/src/a.ts')]));

    expect(rows.hint('s1')).toBe('Reading src/a.ts…');
  });

  it('shows nothing for a session the stream says has stopped running', async () => {
    await establish();
    const { rows } = create();
    rows.sync(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')], { acpTitle: 'Title' }));
    expect(rows.hint('s1')).toBe('Working');

    await frame('delta', 's1', summary([], { status: 'completed', acpTitle: 'Title' }));

    expect(rows.hint('s1')).toBeUndefined();
    expect(rows.title('s1')).toBeUndefined();
  });

  it('drops the pipeline step once the transcript has rows with nothing to show', async () => {
    await establish();
    const { rows } = create();
    rows.sync(['s1']);
    await settle();
    await frame(
      'snapshot',
      's1',
      summary([], {
        openingPromptId: null,
        pipeline: {
          currentStep: 0,
          steps: [{ label: 'Preparing the worktree', status: 'running' }],
        } as SessionSummary['pipeline'],
      })
    );
    expect(rows.hint('s1')).toBe('Preparing the worktree…');

    // The opening prompt is not a reply, so there is nothing to show yet:
    // that is the session's subtitle now, and not a subtitle still to come.
    await frame('delta', 's1', summary([OPENING]));

    expect(rows.hint('s1')).toBeUndefined();
  });

  it('has the hint at once for a view that comes back to a session', async () => {
    await establish();
    const first = create();
    first.rows.sync(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')], { acpTitle: 'Title' }));
    first.rows.destroy();

    // A branch card scrolled out and back: a new view, before any frame.
    const second = create();
    second.rows.sync(['s1']);

    expect(second.rows.hint('s1')).toBe('Working');
    expect(second.rows.title('s1')).toBe('Title');
  });

  it('keeps what it shows while off screen, and is current when shown again', async () => {
    await establish();
    const { rows, shown } = create();
    rows.sync(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')]));
    expect(rows.hint('s1')).toBe('Working');

    shown.visible = false;
    rows.sync(['s1']);
    // The owner still holds the session for a while, and frames still come.
    await frame('delta', 's1', summary([says(3, 'Still working')]));
    expect(rows.hint('s1')).toBe('Working');

    shown.visible = true;
    rows.sync(['s1']);
    expect(rows.hint('s1')).toBe('Still working');
  });

  it('lets go of its sessions while off screen, which the owner outlasts for a while', async () => {
    await establish();
    const { rows, shown } = create();
    rows.sync(['s1']);
    await settle();
    expect(levels()).toEqual({ s1: 'summary' });

    shown.visible = false;
    rows.sync(['s1']);
    await settle();
    expect(levels()).toEqual({ s1: 'summary' });

    await vi.advanceTimersByTimeAsync(LINGER_MS + INTEREST_DELAY_MS);
    expect(levels()).toEqual({});
  });

  it('keeps what it showed until the snapshot when it was off screen too long', async () => {
    await establish();
    const { rows, shown } = create();
    rows.sync(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')], { acpTitle: 'Title' }));
    expect(rows.hint('s1')).toBe('Working');
    expect(rows.title('s1')).toBe('Title');

    shown.visible = false;
    rows.sync(['s1']);
    await vi.advanceTimersByTimeAsync(LINGER_MS + INTEREST_DELAY_MS);
    expect(owner.hint('s1', ['/repo'])).toBeUndefined();

    // Shown again: the owner has nothing for the session until it is read
    // again, and the row does not blank meanwhile.
    shown.visible = true;
    rows.sync(['s1']);
    await settle();
    expect(rows.hint('s1')).toBe('Working');
    expect(rows.title('s1')).toBe('Title');

    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working'), says(3, 'Nearly done')]));
    expect(rows.hint('s1')).toBe('Nearly done');
    expect(rows.title('s1')).toBeUndefined();
  });

  it('forgets what it showed for a session the view no longer lists', async () => {
    await establish();
    const { rows } = create();
    rows.sync(['s1', 's2']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'One')]));
    await frame('snapshot', 's2', summary([OPENING, says(2, 'Two')]));

    rows.sync(['s2']);
    await vi.advanceTimersByTimeAsync(LINGER_MS + INTEREST_DELAY_MS);
    expect(levels()).toEqual({ s2: 'summary' });

    // Listed again later, it starts with nothing: not what it showed before.
    rows.sync(['s1', 's2']);
    expect(rows.hint('s1')).toBeUndefined();
    expect(rows.hint('s2')).toBe('Two');
  });

  it('shows nothing, and does not poll, before the stream is first established', async () => {
    owner.start();
    const { rows } = create();
    rows.sync(['s1']);
    await vi.advanceTimersByTimeAsync(ESTABLISH_GRACE_MS + 1000);

    expect(owner.availability).toBe('unavailable');
    expect(rows.hint('s1')).toBeUndefined();
    expect(rows.title('s1')).toBeUndefined();
    expect(interests).toEqual([]);
  });

  it('holds what it shows while the stream is being re-established', async () => {
    await establish();
    const { rows } = create();
    rows.sync(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')], { acpTitle: 'Title' }));

    handlers.onClose();
    await vi.advanceTimersByTimeAsync(ESTABLISH_GRACE_MS - 1);

    expect(owner.availability).toBe('connecting');
    expect(rows.hint('s1')).toBe('Working');
    expect(rows.title('s1')).toBe('Title');
  });

  it('falls back to the static label once the stream is given up on, until it is back', async () => {
    await establish();
    const { rows } = create();
    rows.sync(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')], { acpTitle: 'Title' }));

    handlers.onClose();
    await vi.advanceTimersByTimeAsync(ESTABLISH_GRACE_MS);

    // `undefined` is what a row turns into its static label.
    expect(owner.availability).toBe('unavailable');
    expect(rows.hint('s1')).toBeUndefined();
    expect(rows.title('s1')).toBeUndefined();

    // The socket is back: the owner declares again and is sent snapshots.
    handlers.onOpen();
    await settle();
    expect(owner.availability).toBe('established');
    expect(levels()).toEqual({ s1: 'summary' });
    expect(rows.hint('s1')).toBeUndefined();

    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working'), says(3, 'Back')]));
    expect(rows.hint('s1')).toBe('Back');
  });

  it('falls back to the static label while the interest keeps being refused', async () => {
    await establish();
    const { rows } = create();
    rows.sync(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')]));

    // The hub has stopped: every interest is refused from here on.
    acceptInterest = async () => {
      throw new Error('session stream stopped');
    };
    rows.sync(['s1', 's2']);
    await settle();
    expect(owner.availability).toBe('connecting');
    expect(rows.hint('s1')).toBe('Working');

    await vi.advanceTimersByTimeAsync(ESTABLISH_GRACE_MS);
    expect(owner.availability).toBe('unavailable');
    expect(rows.hint('s1')).toBeUndefined();

    // It is accepted again: a new generation, so both sessions are read.
    acceptInterest = async () => {};
    await vi.advanceTimersByTimeAsync(30_000);
    expect(owner.availability).toBe('established');
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')]));
    expect(rows.hint('s1')).toBe('Working');
  });

  it('releases its sessions and shows nothing once destroyed', async () => {
    await establish();
    const { rows } = create();
    rows.sync(['s1']);
    await settle();
    await frame('snapshot', 's1', summary([OPENING, says(2, 'Working')], { acpTitle: 'Title' }));

    rows.destroy();
    rows.sync(['s1']);

    expect(rows.hint('s1')).toBeUndefined();
    expect(rows.title('s1')).toBeUndefined();
    await vi.advanceTimersByTimeAsync(LINGER_MS + INTEREST_DELAY_MS);
    expect(levels()).toEqual({});
  });
});

describe('collectRunningSessionIds', () => {
  const timeline = (overrides: object) =>
    ({
      commits: [],
      notes: [],
      reviews: [],
      ...overrides,
    }) as unknown as Parameters<typeof collectRunningSessionIds>[0];

  it('collects the running commits, notes and reviews, and every pending item with a session', () => {
    const ids = collectRunningSessionIds(
      timeline({
        commits: [
          { sessionStatus: 'running', sessionId: 'commit' },
          { sessionStatus: 'completed', sessionId: 'done-commit' },
          { sessionStatus: 'running', sessionId: null },
        ],
        notes: [
          { sessionStatus: 'running', sessionId: 'note' },
          { sessionStatus: 'queued', sessionId: 'queued-note' },
        ],
        reviews: [{ sessionStatus: 'running', sessionId: 'review' }],
      }),
      [{ sessionId: 'pending' }, {}, { sessionId: 'note' }]
    );

    expect(ids).toEqual(['commit', 'note', 'review', 'pending']);
  });
});

describe('fallbackHintForPendingType', () => {
  it('names what a pending row is waiting for', () => {
    expect(fallbackHintForPendingType('pending-commit')).toBe('Generating commit');
    expect(fallbackHintForPendingType('generating-review')).toBe('Generating review');
    expect(fallbackHintForPendingType('generating-note')).toBe('Generating note');
    expect(fallbackHintForPendingType('queued-commit')).toBe('Queued commit');
    expect(fallbackHintForPendingType('queued-review')).toBe('Queued review');
    expect(fallbackHintForPendingType('queued-note')).toBe('Queued note');
  });
});
