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
    statusOverride: null,
  };
}

const [a, b, c, d] = ['a', 'b', 'c', 'd'].map(project);
const rankOf = (...ids: string[]) => new Map(ids.map((id, i) => [id, i]));

let rank = $state.raw<ReadonlyMap<string, number> | null>(null);
let base = $state.raw<Project[]>([a, b, c, d]);
let order: SearchRowOrder;
let cleanup: () => void;

const ids = () => order.rows.map((p) => p.id);
const orders = () => order.rows.map((p) => order.orderOf(p.id));
function setRank(next: ReadonlyMap<string, number> | null) {
  rank = next;
  flushSync();
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
  });
  flushSync();
});

afterEach(() => {
  cleanup();
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

  it('settles at once when asked, as the search field blurs', () => {
    setRank(rankOf('c', 'a'));
    order.settle();
    flushSync();
    expect(ids()).toEqual(['c', 'a', 'b', 'd']);
    expect(orders()).toEqual([undefined, undefined, undefined, undefined]);
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
