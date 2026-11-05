/**
 * The order a project search surface (the landing grid, the sidebar) renders
 * its rows in, and the CSS `order` each row needs on top of that.
 *
 * Per keystroke the rows stay where they are in the DOM: the search hides
 * what it rules out and CSS `order` lays the rest out by rank, so typing
 * flips attributes and styles on rows that already exist. Once the query
 * settles — SEARCH_SETTLE_MS after the ranking last changed, or as soon as
 * Tab is pressed in the search field — the {#each} is re-keyed into the
 * order on screen, once, and the `order` values drop. From then on DOM
 * order, which is what Tab and a screen reader walk, is the order the user
 * sees. A cleared query settles back to the unsearched order the same way,
 * with CSS putting the rows back in place until it does.
 *
 * Re-keying moves rows rather than rebuilding them (keyed {#each}), and a
 * row moved between a press and its click loses the click in WebKit, so the
 * wait holds while a pointer is down on the rows and starts over once it is
 * released. Settling only runs while the surface's layer is on screen; a
 * ranking made while it was hidden settles after it comes back.
 */

import { flushSync, untrack } from 'svelte';
import type { Project } from '../../types';
import { useWorkspaceLayer } from '../layout/workspaceLayer';
import { basePositions, inRankOrder, rowsFollowRank } from './projectSearch';

/** How long a ranking has to stay put before the DOM order catches up:
 *  longer than the gap between keystrokes, shorter than a reach for Tab or
 *  the mouse. */
export const SEARCH_SETTLE_MS = 300;

/** What ends a press: the click it makes, or the release or cancel when
 *  there is none. */
const PRESS_END_EVENTS = ['pointerup', 'pointercancel', 'click'] as const;

export interface SearchRowOrder {
  /** Every row, in the order to render them. */
  readonly rows: Project[];
  /** The CSS `order` a row needs, or undefined while the DOM order is
   *  already the order on screen. */
  orderOf(id: string): number | undefined;
  /** Catch the DOM order up now, rather than when the wait runs out. */
  settle(): void;
  /** The search field's keydown: Tab settles before the browser moves
   *  focus, so it lands on the rows in the order on screen. */
  fieldKeydown(event: KeyboardEvent): void;
  /** The rows container's pointerdown: the wait holds until the press ends,
   *  so the rows never move under a click. */
  rowsPointerdown(): void;
}

/**
 * Call from component setup. `base` is every row in its unsearched order;
 * `rank` the search's (searchProjects' `rank`, after searchOrderHold), null
 * while the query is blank.
 */
export function searchRowOrder(
  base: () => Project[],
  rank: () => ReadonlyMap<string, number> | null
): SearchRowOrder {
  const layer = useWorkspaceLayer();
  // The ranking the DOM order was last settled on; null for base order.
  let settled = $state.raw<ReadonlyMap<string, number> | null>(null);
  // A pointer is down on the rows.
  let pressed = $state(false);
  const rows = $derived(inRankOrder(base(), settled));
  // Where each shown row belongs: by rank while searching; once the query
  // is cleared, back in base order.
  const target = $derived(rank() ?? (settled === null ? null : basePositions(base())));
  // Whether CSS has rows to move. Checked per ranking (a walk of the rows),
  // so a held query whose data changed only hides or appends where the DOM
  // order already agrees doesn't put `order` back on every row.
  const offset = $derived(target !== null && target !== settled && !rowsFollowRank(rows, target));

  function settle() {
    if (!untrack(() => offset)) return;
    const focused = document.activeElement;
    settled = untrack(rank);
    // Move the rows now, not in the next microtask, so a Tab keydown's
    // default focus move already sees them in their new order.
    flushSync();
    // Moving the focused row (remove + insert) can drop its focus to the
    // body; give it back now the rows have moved.
    if (focused instanceof HTMLElement && focused !== document.body && focused.isConnected) {
      const now = document.activeElement;
      if (now === null || now === document.body) focused.focus({ preventScroll: true });
    }
  }

  $effect(() => {
    if (!offset || !layer.visible || pressed) return;
    // Every new ranking, keystroke or data change, restarts the wait, and
    // so does the end of a press.
    void target;
    const timer = setTimeout(settle, SEARCH_SETTLE_MS);
    return () => clearTimeout(timer);
  });

  // Window-wide, capturing, so a press that ends off the rows (or whose
  // handler stops propagation) still ends.
  $effect(() => {
    const release = () => {
      if (pressed) pressed = false;
    };
    for (const type of PRESS_END_EVENTS) window.addEventListener(type, release, true);
    return () => {
      for (const type of PRESS_END_EVENTS) window.removeEventListener(type, release, true);
    };
  });

  return {
    get rows() {
      return rows;
    },
    orderOf(id) {
      return offset ? target?.get(id) : undefined;
    },
    settle,
    fieldKeydown(event) {
      if (event.key === 'Tab') settle();
    },
    rowsPointerdown() {
      pressed = true;
    },
  };
}
