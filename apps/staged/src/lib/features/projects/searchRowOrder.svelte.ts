/**
 * The order a project search surface (the landing grid, the sidebar) renders
 * its rows in, and the CSS `order` each row needs on top of that.
 *
 * Per keystroke the rows stay where they are in the DOM: the search hides
 * what it rules out and CSS `order` lays the rest out by rank, so typing
 * flips attributes and styles on rows that already exist. Once the query
 * settles — SEARCH_SETTLE_MS after the ranking last changed, or as soon as
 * the search field loses focus — the {#each} is re-keyed into the order on
 * screen, once, and the `order` values drop. From then on DOM order, which
 * is what Tab and a screen reader walk, is the order the user sees. A
 * cleared query settles back to the unsearched order the same way, with CSS
 * putting the rows back in place until it does.
 *
 * Re-keying moves rows rather than rebuilding them (keyed {#each}). It only
 * runs while the surface's layer is on screen; a ranking made while it was
 * hidden settles after it comes back.
 */

import { tick, untrack } from 'svelte';
import type { Project } from '../../types';
import { useWorkspaceLayer } from '../layout/workspaceLayer';
import { basePositions, inRankOrder, rowsFollowRank } from './projectSearch';

/** How long a ranking has to stay put before the DOM order catches up:
 *  longer than the gap between keystrokes, shorter than a reach for Tab or
 *  the mouse. */
export const SEARCH_SETTLE_MS = 300;

export interface SearchRowOrder {
  /** Every row, in the order to render them. */
  readonly rows: Project[];
  /** The CSS `order` a row needs, or undefined while the DOM order is
   *  already the order on screen. */
  orderOf(id: string): number | undefined;
  /** Catch the DOM order up now, rather than when the wait runs out: the
   *  search field lost focus, and Tab may head for the rows next. */
  settle(): void;
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
    // Moving the focused row (remove + insert) can drop its focus to the
    // body; give it back once the rows have moved.
    if (focused instanceof HTMLElement && focused !== document.body) {
      void tick().then(() => {
        const now = document.activeElement;
        if (focused.isConnected && (now === null || now === document.body)) {
          focused.focus({ preventScroll: true });
        }
      });
    }
  }

  $effect(() => {
    if (!offset || !layer.visible) return;
    // Every new ranking, keystroke or data change, restarts the wait.
    void target;
    const timer = setTimeout(settle, SEARCH_SETTLE_MS);
    return () => clearTimeout(timer);
  });

  return {
    get rows() {
      return rows;
    },
    orderOf(id) {
      return offset ? target?.get(id) : undefined;
    },
    settle,
  };
}
