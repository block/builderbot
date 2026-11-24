/**
 * A view's rows on the session stream are reactive per session: a row is
 * evaluated again for its own session's frames and nobody else's, keeps what
 * it showed while its view is off screen, and falls back to its static label
 * when the stream is given up on. Runs under the component project, where
 * the owner's maps are Svelte's reactive ones (`liveSessionHints.test.ts`
 * runs them as plain maps and covers the values).
 */
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { normalizeDisplayRoots } from '../sessions/pathDisplayRoots';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';
import {
  ESTABLISH_GRACE_MS,
  INTEREST_DELAY_MS,
  LINGER_MS,
  SessionStreamOwner,
} from '../../stores/sessionStream.svelte';
import type { SessionStreamHandlers } from '../../transport';
import type { SessionSummary, SessionSummaryRow } from '../../types';
import LiveSessionRowsView from './testStubs/LiveSessionRowsView.svelte';

const ROOTS = ['/repo'];
const OPENING_PROMPT: SessionSummaryRow = { id: 1, role: 'user' };
/** What the test view shows for a session with no subtitle, as a row does. */
const STATIC_LABEL = 'Generating note';

function assistant(id: number, content: string): SessionSummaryRow {
  return { id, role: 'assistant', content };
}

function summary(rows: SessionSummaryRow[], acpTitle: string | null = null): SessionSummary {
  return { status: 'running', acpTitle, workingDir: '/repo', openingPromptId: 1, rows };
}

describe('live session rows', () => {
  let owner: SessionStreamOwner;
  let handlers: SessionStreamHandlers | undefined;
  let generation: number;
  let seq: number;
  let renders: string[];
  let props: { visible: boolean; sessionIds: string[] };
  let component: ReturnType<typeof mount> | undefined;
  let target: HTMLElement;

  beforeEach(async () => {
    vi.useFakeTimers();
    generation = 0;
    seq = 0;
    renders = [];
    owner = new SessionStreamOwner({
      open: (given) => {
        handlers = given;
        return {
          setInterest: async (interest) => {
            if (interest.generation !== generation) seq = 0;
            generation = interest.generation;
          },
          check: async () => true,
          close: () => {},
        };
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

  /** Mount the view on `sessionIds` and let its interest be accepted. */
  async function showRows(sessionIds: string[]) {
    props = reactiveProps({ visible: true, sessionIds });
    component = mount(LiveSessionRowsView, {
      target,
      props: {
        owner,
        roots: ROOTS,
        onRender: (sessionId: string) => renders.push(sessionId),
        get sessionIds() {
          return props.sessionIds;
        },
        get visible() {
          return props.visible;
        },
      },
    });
    flushSync();
    await vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS);
    flushSync();
  }

  /** Deliver the next frame of the accepted generation and let the rows catch up. */
  async function frame(kind: 'snapshot' | 'delta', sessionId: string, given: SessionSummary) {
    seq += 1;
    handlers?.onFrame({ generation, seq, sessionId, kind, summary: given });
    // Root aliases resolve in a microtask.
    await vi.advanceTimersByTimeAsync(0);
    flushSync();
  }

  function part(sessionId: string, name: 'title' | 'hint'): string {
    const row = target.querySelector(`[data-session="${sessionId}"]`);
    return row?.querySelector(`[data-part="${name}"]`)?.textContent ?? '';
  }

  it('evaluates a row again for its own session’s delta and nobody else’s', async () => {
    await showRows(['a', 'b']);
    expect(part('a', 'hint')).toBe(STATIC_LABEL);
    await frame('snapshot', 'a', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')], 'A'));
    await frame('snapshot', 'b', summary([OPENING_PROMPT, assistant(2, 'Running tests')], 'B'));
    expect(part('a', 'hint')).toBe('Reading the cache');
    expect(part('a', 'title')).toBe('A');
    expect(part('b', 'hint')).toBe('Running tests');

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

  it('leaves the rows alone when another view’s session is taken up, updated and dropped', async () => {
    await showRows(['a']);
    await frame('snapshot', 'a', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')], 'A'));

    renders.length = 0;
    const other = owner.consume();
    other.sync({ summary: ['c'], title: [], displayRoots: ROOTS });
    await vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS);
    await frame('snapshot', 'c', summary([OPENING_PROMPT, assistant(2, 'Elsewhere')], 'C'));
    other.release();
    await vi.advanceTimersByTimeAsync(LINGER_MS + INTEREST_DELAY_MS);
    flushSync();

    expect(renders).toEqual([]);
  });

  it('keeps its rows as they were while off screen, and brings them up to date when shown', async () => {
    await showRows(['a', 'b']);
    await frame('snapshot', 'a', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')], 'A'));
    await frame('snapshot', 'b', summary([OPENING_PROMPT, assistant(2, 'Running tests')], 'B'));

    props.visible = false;
    flushSync();
    expect(part('a', 'hint')).toBe('Reading the cache');
    expect(part('a', 'title')).toBe('A');

    // The owner keeps a session nobody shows for a while, so frames for it
    // still arrive. A row nobody can see is not evaluated for them.
    renders.length = 0;
    await frame('delta', 'a', summary([assistant(3, 'Writing the note')], 'A'));
    await frame('delta', 'b', summary([assistant(3, 'Tests pass')], 'B'));
    expect(renders).toEqual([]);
    expect(part('a', 'hint')).toBe('Reading the cache');

    props.visible = true;
    flushSync();
    expect(part('a', 'hint')).toBe('Writing the note');
    expect(part('b', 'hint')).toBe('Tests pass');
  });

  it('does not blank a row whose view was off screen for longer than the owner keeps it', async () => {
    await showRows(['a']);
    await frame('snapshot', 'a', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')], 'A'));

    props.visible = false;
    flushSync();
    await vi.advanceTimersByTimeAsync(LINGER_MS + INTEREST_DELAY_MS);
    flushSync();
    expect(owner.status('a')).toBeUndefined();

    props.visible = true;
    flushSync();
    await vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS);
    flushSync();
    expect(part('a', 'hint')).toBe('Reading the cache');
    expect(part('a', 'title')).toBe('A');

    await frame(
      'snapshot',
      'a',
      summary([OPENING_PROMPT, assistant(2, 'Reading the cache'), assistant(3, 'Done reading')])
    );
    expect(part('a', 'hint')).toBe('Done reading');
    expect(part('a', 'title')).toBe('');
  });

  it('shows a row’s static label once the stream is given up on, and its subtitle when it is back', async () => {
    await showRows(['a']);
    await frame('snapshot', 'a', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')], 'A'));

    handlers?.onClose();
    await vi.advanceTimersByTimeAsync(ESTABLISH_GRACE_MS - 1);
    flushSync();
    expect(part('a', 'hint')).toBe('Reading the cache');

    await vi.advanceTimersByTimeAsync(1);
    flushSync();
    expect(owner.availability).toBe('unavailable');
    expect(part('a', 'hint')).toBe(STATIC_LABEL);
    expect(part('a', 'title')).toBe('');

    handlers?.onOpen();
    await vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS);
    await frame('snapshot', 'a', summary([OPENING_PROMPT, assistant(2, 'Still reading')], 'A'));
    expect(part('a', 'hint')).toBe('Still reading');
    expect(part('a', 'title')).toBe('A');
  });

  it('takes up a session added to the view', async () => {
    await showRows(['a']);
    await frame('snapshot', 'a', summary([OPENING_PROMPT, assistant(2, 'Reading the cache')]));

    props.sessionIds = ['a', 'b'];
    flushSync();
    await vi.advanceTimersByTimeAsync(INTEREST_DELAY_MS);
    await frame('snapshot', 'b', summary([OPENING_PROMPT, assistant(2, 'Running tests')]));

    expect(part('b', 'hint')).toBe('Running tests');
    expect(part('a', 'hint')).toBe('Reading the cache');
  });
});
