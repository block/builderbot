/**
 * Keeps keyboard focus with a card or row through a re-sort nobody picked.
 *
 * Every project list is in the window's sort (projectOrder), and that order
 * follows live data: a project's last activity moving, a tag set in another
 * window or by an agent, a tag group reordered in settings. A keyed
 * `{#each}` shows the new order by taking nodes out and putting them back,
 * and a node that moves while it holds focus loses it, to `<body>`. The
 * pointer is covered where the order is made, since projectActivity holds a
 * fresh map through a press. The keyboard is covered here, in each list: a
 * row reached with Tab would otherwise drop focus whenever a session
 * elsewhere wrote a note.
 *
 * Before each render that changes the rows, this notes which project's row
 * holds focus. After the render, focus that dropped goes back to that row,
 * or to the row that took its place when the change took it off the list.
 * Focus anywhere else is left where it is (focusIfDropped), and with no row
 * focused nothing happens.
 *
 * A move the user picks from a row's menu is focusThroughMove's: focus is
 * in the menu while the list re-renders, so there is no focused row to note.
 */

import { untrack } from 'svelte';
import { refocusAfterRender, rowOrNeighbour } from './focusAfterRemove';

export interface KeepFocusThroughReorder {
  /** The rows in screen order, read reactively: a change is a render that may move rows. */
  shownIds: () => readonly string[];
  /**
   * What the `{#each}` renders, when that is not the shown list (a search
   * lays its rows out with CSS `order`). Read only to follow it.
   */
  rendered?: () => unknown;
  /** The project whose row holds focus now, if one does. */
  focusedId: () => string | null;
  /** Answers for what is on screen now, as for neighbourAfterRemove. */
  elementFor: (projectId: string) => HTMLElement | null | undefined;
}

/** Call from component setup, before the list's markup. */
export function keepFocusThroughReorder(options: KeepFocusThroughReorder): void {
  // The rows as last rendered: where a row's neighbours are read from once
  // it has left the list.
  let shown: readonly string[] = [];
  // A pre effect: it runs before the render it is about, while the row
  // still holds the focus that render will drop.
  $effect.pre(() => {
    options.rendered?.();
    const before = shown;
    shown = options.shownIds();
    untrack(() => {
      const focused = options.focusedId();
      if (focused === null) return;
      refocusAfterRender(rowOrNeighbour(before, focused, options.elementFor));
    });
  });
}
