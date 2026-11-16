/**
 * Browser-style history: the entries visited, oldest first, and which one is
 * on screen.
 *
 * Pure and generic. What an entry is, and what showing one does, belong to
 * navigation.svelte.ts, which holds the app's one history in its `navigation`
 * state. Every function returns a new history, or the one it was given when
 * there is nothing to change, so the holder can tell whether anything moved
 * by identity.
 */

export interface NavigationHistory<T> {
  readonly entries: readonly T[];
  /** The entry on screen. Always a valid index: a history is never empty. */
  readonly index: number;
}

/**
 * The most entries a history keeps. A visit past it drops the oldest, as a
 * browser does, so a long-running window doesn't hold every view it was
 * ever on.
 */
export const MAX_HISTORY_ENTRIES = 100;

type Same<T> = (a: T, b: T) => boolean;

/** A history holding one entry, with nowhere to go back or forward to. */
export function createHistory<T>(entry: T): NavigationHistory<T> {
  return { entries: [entry], index: 0 };
}

export function currentEntry<T>(history: NavigationHistory<T>): T {
  return history.entries[history.index];
}

export function canGoBack<T>(history: NavigationHistory<T>): boolean {
  return history.index > 0;
}

export function canGoForward<T>(history: NavigationHistory<T>): boolean {
  return history.index < history.entries.length - 1;
}

/**
 * Go somewhere new: the entry is added after the current one and becomes the
 * current one, and the entries that were forward of it are dropped. Visiting
 * the entry already on screen changes nothing, so the forward entries stay.
 */
export function visit<T>(
  history: NavigationHistory<T>,
  entry: T,
  same: Same<T> = Object.is
): NavigationHistory<T> {
  if (same(currentEntry(history), entry)) return history;
  const entries = [...history.entries.slice(0, history.index + 1), entry];
  const overflow = Math.max(0, entries.length - MAX_HISTORY_ENTRIES);
  return { entries: entries.slice(overflow), index: entries.length - overflow - 1 };
}

/**
 * Swap the current entry for another without adding one: the entries back
 * and forward of it stay as they are.
 */
export function replaceCurrent<T>(history: NavigationHistory<T>, entry: T): NavigationHistory<T> {
  if (Object.is(currentEntry(history), entry)) return history;
  return {
    entries: history.entries.map((existing, i) => (i === history.index ? entry : existing)),
    index: history.index,
  };
}

/** The history one entry back, or unchanged when there is no earlier entry. */
export function goBack<T>(history: NavigationHistory<T>): NavigationHistory<T> {
  return canGoBack(history) ? { entries: history.entries, index: history.index - 1 } : history;
}

/** The history one entry forward, or unchanged when there is no later entry. */
export function goForward<T>(history: NavigationHistory<T>): NavigationHistory<T> {
  return canGoForward(history) ? { entries: history.entries, index: history.index + 1 } : history;
}

/**
 * Rewrite every entry: `rewrite` returns the entry to keep in its place
 * (itself, to leave it alone) or null to drop it. For entries whose target no
 * longer exists, which Back and Forward must not land on.
 *
 * Neighbours the rewrite leaves the same are merged into one, so stepping
 * through the history always changes the view. When the current entry is
 * dropped, the nearest earlier one that survives takes its place, as Back
 * would have; with none, `fallback` becomes the first and current entry.
 */
export function rewriteEntries<T>(
  history: NavigationHistory<T>,
  rewrite: (entry: T) => T | null,
  fallback: T,
  same: Same<T> = Object.is
): NavigationHistory<T> {
  const entries: T[] = [];
  let index = -1;
  let changed = false;
  history.entries.forEach((entry, i) => {
    const next = rewrite(entry);
    if (next !== entry) changed = true;
    if (next !== null) {
      if (entries.length > 0 && same(entries[entries.length - 1], next)) changed = true;
      else entries.push(next);
    }
    if (i <= history.index) index = entries.length - 1;
  });
  if (!changed) return history;
  if (index < 0) {
    if (entries.length === 0 || !same(entries[0], fallback)) entries.unshift(fallback);
    index = 0;
  }
  return { entries, index };
}
