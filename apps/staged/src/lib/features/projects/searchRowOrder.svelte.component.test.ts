import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '../../types';
import { SEARCH_SETTLE_MS, searchRowOrder, type SearchRowOrder } from './searchRowOrder.svelte';

// The surface's layer, outside any component: settling waits for it to be on
// screen.
const layer = $state({ visible: true });
vi.mock('../layout/workspaceLayer', () => ({
  useWorkspaceLayer: () => ({
    get visible() {
      return layer.visible;
    },
  }),
}));

function project(id: string): Project {
  return {
    id,
    name: id.toUpperCase(),
    githubRepo: null,
    location: 'local',
    subpath: null,
    createdAt: 0,
    updatedAt: 0,
  };
}

const [a, b, c, d] = ['a', 'b', 'c', 'd'].map(project);
const rankOf = (...ids: string[]) => new Map(ids.map((id, i) => [id, i]));

let rank = $state.raw<ReadonlyMap<string, number> | null>(null);
let base = $state.raw<Project[]>([a, b, c, d]);
let order: SearchRowOrder;
let cleanup: () => void;
// The row ids as an {#each} last rendered them: updated only when effects
// flush, as the DOM is.
let rendered: string[] = [];

const ids = () => order.rows.map((p) => p.id);
const orders = () => order.rows.map((p) => order.orderOf(p.id));
function setRank(next: ReadonlyMap<string, number> | null) {
  rank = next;
  flushSync();
}
function key(name: string) {
  return new KeyboardEvent('keydown', { key: name });
}
function wait(ms: number) {
  vi.advanceTimersByTime(ms);
  flushSync();
}

beforeEach(() => {
  vi.useFakeTimers();
  layer.visible = true;
  rank = null;
  base = [a, b, c, d];
  cleanup = $effect.root(() => {
    order = searchRowOrder(
      () => base,
      () => rank
    );
    $effect(() => {
      rendered = ids();
    });
  });
  flushSync();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('searchRowOrder', () => {
  it('renders the base list as it is, with no order, for a blank query', () => {
    expect(order.rows).toBe(base);
    expect(orders()).toEqual([undefined, undefined, undefined, undefined]);
  });

  it('moves rows with CSS order per ranking, leaving the DOM order alone', () => {
    setRank(rankOf('c', 'a'));
    expect(ids()).toEqual(['a', 'b', 'c', 'd']);
    expect(orders()).toEqual([1, undefined, 0, undefined]);
  });

  it('re-keys the rows into rank order once the ranking settles, dropping the order', () => {
    setRank(rankOf('c', 'a'));
    wait(SEARCH_SETTLE_MS - 1);
    expect(ids()).toEqual(['a', 'b', 'c', 'd']);
    wait(1);
    expect(ids()).toEqual(['c', 'a', 'b', 'd']);
    expect(orders()).toEqual([undefined, undefined, undefined, undefined]);
  });

  it('restarts the wait on every new ranking', () => {
    setRank(rankOf('c', 'a'));
    wait(200);
    setRank(rankOf('d', 'c'));
    wait(200);
    expect(ids()).toEqual(['a', 'b', 'c', 'd']);
    wait(SEARCH_SETTLE_MS - 200);
    expect(ids()).toEqual(['d', 'c', 'a', 'b']);
  });

  it('settles at once when asked', () => {
    setRank(rankOf('c', 'a'));
    order.settle();
    expect(ids()).toEqual(['c', 'a', 'b', 'd']);
    expect(orders()).toEqual([undefined, undefined, undefined, undefined]);
  });

  it('settles on Tab in the search field, before focus moves, and on no other key', () => {
    setRank(rankOf('c', 'a'));
    order.fieldKeydown(key('a'));
    order.fieldKeydown(key('ArrowDown'));
    flushSync();
    expect(rendered).toEqual(['a', 'b', 'c', 'd']);
    // No flush in between: the rows have moved by the time the keydown
    // handler returns and the browser picks the next focus target.
    order.fieldKeydown(key('Tab'));
    expect(rendered).toEqual(['c', 'a', 'b', 'd']);
  });

  it('gives focus back to a row the move dropped it from', () => {
    const row = document.createElement('button');
    document.body.append(row);
    row.focus();
    setRank(rankOf('c', 'a'));
    // What WebKit does to a focused node a keyed {#each} moves.
    const focus = vi.spyOn(row, 'focus');
    vi.spyOn(document, 'activeElement', 'get')
      .mockReturnValueOnce(row)
      .mockReturnValueOnce(document.body);
    order.settle();
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
    row.remove();
  });

  describe('while a pointer is down on the rows', () => {
    it.each(['pointerup', 'pointercancel', 'click'])(
      'holds the wait until %s, then waits afresh',
      (end) => {
        setRank(rankOf('c', 'a'));
        wait(SEARCH_SETTLE_MS - 100);
        order.rowsPointerdown();
        flushSync();
        wait(SEARCH_SETTLE_MS * 3);
        expect(ids()).toEqual(['a', 'b', 'c', 'd']);
        // Off the rows too: a press can end anywhere.
        document.body.dispatchEvent(new Event(end, { bubbles: true }));
        flushSync();
        wait(SEARCH_SETTLE_MS - 1);
        expect(ids()).toEqual(['a', 'b', 'c', 'd']);
        wait(1);
        expect(ids()).toEqual(['c', 'a', 'b', 'd']);
      }
    );

    it('holds the wait until the window loses focus, then waits afresh', () => {
      setRank(rankOf('c', 'a'));
      order.rowsPointerdown();
      flushSync();
      wait(SEARCH_SETTLE_MS * 3);
      // ⌘-Tab away mid-press: the release goes to another app.
      window.dispatchEvent(new FocusEvent('blur'));
      flushSync();
      wait(SEARCH_SETTLE_MS - 1);
      expect(ids()).toEqual(['a', 'b', 'c', 'd']);
      wait(1);
      expect(ids()).toEqual(['c', 'a', 'b', 'd']);
    });

    it("doesn't end on an element's blur, like the search field's as the press starts", () => {
      const field = document.createElement('input');
      document.body.append(field);
      setRank(rankOf('c', 'a'));
      order.rowsPointerdown();
      // What a row's pointerdown does to the focused field, right after:
      // a blur, which window listeners see in their capture phase.
      field.dispatchEvent(new FocusEvent('blur'));
      flushSync();
      wait(SEARCH_SETTLE_MS * 3);
      expect(ids()).toEqual(['a', 'b', 'c', 'd']);
      field.remove();
    });

    it('still settles on Tab', () => {
      setRank(rankOf('c', 'a'));
      order.rowsPointerdown();
      flushSync();
      order.fieldKeydown(key('Tab'));
      expect(ids()).toEqual(['c', 'a', 'b', 'd']);
    });

    it('stops listening for the press to end once torn down', () => {
      const remove = vi.spyOn(window, 'removeEventListener');
      cleanup();
      for (const type of ['pointerup', 'pointercancel', 'click']) {
        expect(remove).toHaveBeenCalledWith(type, expect.any(Function), true);
      }
      expect(remove).toHaveBeenCalledWith('blur', expect.any(Function));
      expect(remove).not.toHaveBeenCalledWith('blur', expect.any(Function), true);
      cleanup = () => {};
    });
  });

  it('needs no order for a ranking the settled rows already follow', () => {
    setRank(rankOf('c', 'a', 'b'));
    order.settle();
    flushSync();
    // A held query whose data changed: b stopped matching, d joined last.
    setRank(rankOf('c', 'a', 'd'));
    expect(orders()).toEqual([undefined, undefined, undefined, undefined]);
  });

  it('puts the rows back in base order with CSS when the query is cleared, then re-keys', () => {
    setRank(rankOf('c', 'a'));
    order.settle();
    flushSync();
    setRank(null);
    expect(ids()).toEqual(['c', 'a', 'b', 'd']);
    expect(orders()).toEqual([2, 0, 1, 3]);
    wait(SEARCH_SETTLE_MS);
    expect(order.rows).toBe(base);
    expect(orders()).toEqual([undefined, undefined, undefined, undefined]);
  });

  it('waits for its layer to be on screen before settling', () => {
    layer.visible = false;
    setRank(rankOf('c', 'a'));
    wait(SEARCH_SETTLE_MS * 3);
    expect(ids()).toEqual(['a', 'b', 'c', 'd']);
    layer.visible = true;
    flushSync();
    wait(SEARCH_SETTLE_MS);
    expect(ids()).toEqual(['c', 'a', 'b', 'd']);
  });

  it('keeps rows the base list gains or loses in base order after the ranked ones', () => {
    setRank(rankOf('c', 'a'));
    order.settle();
    flushSync();
    const e = project('e');
    base = [b, c, d, e];
    flushSync();
    expect(ids()).toEqual(['c', 'b', 'd', 'e']);
  });
});
