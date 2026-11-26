/**
 * The session stream owner's values are reactive per session: a row that
 * reads its own title and subtitle is rendered again when those change and
 * not when another session's do. Runs under the component project, where the
 * owner's maps are Svelte's reactive ones (`sessionStream.test.ts` runs them
 * as plain maps and covers everything else).
 */
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { normalizeDisplayRoots } from '../features/sessions/pathDisplayRoots';
import type { SessionStreamHandlers } from '../transport';
import type { SessionSummary, SessionSummaryRow } from '../types';
import { INTEREST_DELAY_MS, LINGER_MS, SessionStreamOwner } from './sessionStream.svelte';
import SessionStreamRows from './testStubs/SessionStreamRows.svelte';

const ROOTS = ['/repo'];
const OPENING_PROMPT: SessionSummaryRow = { id: 1, role: 'user' };

function assistant(id: number, content: string): SessionSummaryRow {
  return { id, role: 'assistant', content };
}

function summary(rows: SessionSummaryRow[], acpTitle: string | null = null): SessionSummary {
  return { status: 'running', acpTitle, workingDir: '/repo', openingPromptId: 1, rows };
}

describe('session stream rows', () => {
  let owner: SessionStreamOwner;
  let handlers: SessionStreamHandlers | undefined;
  let seq: number;
  let renders: string[];
  let component: ReturnType<typeof mount> | undefined;
  let target: HTMLElement;

  beforeEach(async () => {
    vi.useFakeTimers();
    seq = 0;
    renders = [];
    owner = new SessionStreamOwner({
      open: (given) => {
        handlers = given;
        return { setInterest: async () => {}, check: async () => true, close: () => {} };
      },
      resolveRoots: async (roots) => normalizeDisplayRoots(roots),
      onStoreReset: () => () => {},
      onHealthTrigger: () => () => {},
      visibility: null,
    });
    owner.start();
    handlers?.onOpen();
    await vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS);

    target = document.createElement('div');
    document.body.append(target);
  });

  afterEach(() => {
    if (component) void unmount(component);
    component = undefined;
    target.remove();
    owner.stop();
    vi.useRealTimers();
  });

  /**
   * Show rows for `sessionIds`. They are rendered first and declared to the
   * owner after, the order a view does it in: its markup runs before the
   * effect that syncs its sessions.
   */
  async function showRows(sessionIds: string[]) {
    component = mount(SessionStreamRows, {
      target,
      props: {
        owner,
        sessionIds,
        roots: ROOTS,
        onRender: (sessionId: string) => renders.push(sessionId),
      },
    });
    flushSync();
    owner.consume().sync({ summary: sessionIds, title: [], displayRoots: ROOTS });
    await vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS);
    flushSync();
  }

  /** Deliver the next frame of generation 1 and let the rows catch up. */
  async function frame(kind: 'snapshot' | 'delta', sessionId: string, given: SessionSummary) {
    seq += 1;
    handlers?.onFrame({ generation: 1, seq, sessionId, kind, summary: given });
    // Root aliases resolve in a microtask.
    await vi.advanceTimersByTimeAsync(0);
    flushSync();
  }

  function part(sessionId: string, name: 'title' | 'hint'): string {
    const row = target.querySelector(`[data-session="${sessionId}"]`);
    return row?.querySelector(`[data-part="${name}"]`)?.textContent ?? '';
  }

  it('renders a session’s row again for its own delta and nobody else’s', async () => {
    await showRows(['a', 'b']);
    await frame('snapshot', 'a', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')], 'A'));
    await frame('snapshot', 'b', summary([OPENING_PROMPT, assistant(2, 'Running tests')], 'B'));
    expect(part('a', 'hint')).toBe('Reading the cache');
    expect(part('b', 'hint')).toBe('Running tests');
    expect(part('a', 'title')).toBe('A');

    renders.length = 0;
    await frame('delta', 'a', summary([assistant(3, 'Writing the note')], 'A'));

    expect(part('a', 'hint')).toBe('Writing the note');
    expect(part('b', 'hint')).toBe('Running tests');
    expect(renders).toEqual(['a']);

    renders.length = 0;
    await frame('delta', 'b', summary([], 'B, renamed'));

    expect(part('b', 'title')).toBe('B, renamed');
    expect(renders).toEqual(['b']);
  });

  it('renders nothing again for a delta that leaves the subtitle as it was', async () => {
    await showRows(['a', 'b']);
    await frame('snapshot', 'a', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')]));
    await frame('snapshot', 'b', summary([OPENING_PROMPT, assistant(2, 'Running tests')]));

    renders.length = 0;
    // A tool result is not read, so the newest row with something to show
    // is the one it already was.
    await frame('delta', 'a', summary([{ id: 3, role: 'tool_result' }]));

    expect(renders).toEqual([]);
  });

  it('leaves a row with no title or subtitle alone when another session’s change', async () => {
    await showRows(['a', 'b']);
    await frame('snapshot', 'a', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')], 'A'));
    // `b` has neither yet: its row reads two values that are empty.
    await frame('snapshot', 'b', summary([OPENING_PROMPT]));
    expect(part('b', 'title')).toBe('');
    expect(part('b', 'hint')).toBe('');

    renders.length = 0;
    await frame('delta', 'a', summary([assistant(3, 'Writing the note')], 'A, renamed'));
    expect(part('a', 'hint')).toBe('Writing the note');
    expect(renders).toEqual(['a']);

    renders.length = 0;
    await frame('delta', 'b', summary([assistant(2, 'Starting')], 'B'));
    expect(part('b', 'hint')).toBe('Starting');
    expect(part('b', 'title')).toBe('B');
    expect(renders).toEqual(['b']);
  });

  it('leaves the rows alone when another session is taken up, updated and dropped', async () => {
    await showRows(['a', 'b']);
    await frame('snapshot', 'a', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')], 'A'));
    await frame('snapshot', 'b', summary([OPENING_PROMPT]));

    renders.length = 0;
    const other = owner.consume();
    other.sync({ summary: ['c'], title: [], displayRoots: ROOTS });
    await vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS);
    await frame('snapshot', 'c', summary([OPENING_PROMPT, assistant(2, 'Elsewhere')], 'C'));
    expect(owner.hint('c', ROOTS)).toBe('Elsewhere');

    other.release();
    await vi.advanceTimersByTimeAsync(LINGER_MS + INTEREST_DELAY_MS);
    flushSync();
    expect(owner.hint('c', ROOTS)).toBeUndefined();

    expect(renders).toEqual([]);
  });
});
