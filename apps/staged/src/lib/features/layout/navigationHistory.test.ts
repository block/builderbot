import { describe, expect, it } from 'vitest';
import {
  MAX_HISTORY_ENTRIES,
  canGoBack,
  canGoForward,
  createHistory,
  currentEntry,
  goBack,
  goForward,
  replaceCurrent,
  rewriteEntries,
  visit,
  type NavigationHistory,
} from './navigationHistory';

/** A history that visited each entry in turn, then stepped back `back` times. */
function visited(entries: string[], back = 0): NavigationHistory<string> {
  let history = createHistory(entries[0]);
  for (const entry of entries.slice(1)) history = visit(history, entry);
  for (let i = 0; i < back; i++) history = goBack(history);
  return history;
}

describe('a new history', () => {
  it('holds the one entry, with nowhere to go back or forward to', () => {
    const history = createHistory('grid');

    expect(history).toEqual({ entries: ['grid'], index: 0 });
    expect(currentEntry(history)).toBe('grid');
    expect(canGoBack(history)).toBe(false);
    expect(canGoForward(history)).toBe(false);
  });
});

describe('visit', () => {
  it('adds the entry after the current one and makes it current', () => {
    const history = visit(visit(createHistory('grid'), 'a'), 'b');

    expect(history).toEqual({ entries: ['grid', 'a', 'b'], index: 2 });
    expect(currentEntry(history)).toBe('b');
  });

  it('enables Back once there is an earlier entry, and leaves Forward disabled', () => {
    const history = visit(createHistory('grid'), 'a');

    expect(canGoBack(history)).toBe(true);
    expect(canGoForward(history)).toBe(false);
  });

  it('drops the forward entries when visiting after going back', () => {
    const history = visit(visited(['grid', 'a', 'b', 'c'], 2), 'd');

    expect(history).toEqual({ entries: ['grid', 'a', 'd'], index: 2 });
    expect(canGoForward(history)).toBe(false);
  });

  it('changes nothing, forward entries included, when the entry is already current', () => {
    const history = visited(['grid', 'a', 'b'], 1);

    expect(visit(history, 'a')).toBe(history);
    expect(canGoForward(history)).toBe(true);
  });

  it('visits an entry again when it is not the current one', () => {
    const history = visit(visited(['grid', 'a']), 'grid');

    expect(history).toEqual({ entries: ['grid', 'a', 'grid'], index: 2 });
  });

  it('decides what counts as the current entry with the given comparison', () => {
    const sameId = (a: { id: string }, b: { id: string }) => a.id === b.id;
    const history = createHistory({ id: 'a' });

    expect(visit(history, { id: 'a' }, sameId)).toBe(history);
    expect(visit(history, { id: 'a' }).entries).toHaveLength(2);
  });

  it('does not mutate the history it was given', () => {
    const history = visited(['grid', 'a', 'b'], 1);
    const before = structuredClone(history);

    visit(history, 'c');

    expect(history).toEqual(before);
  });

  it('drops the oldest entries past the limit and stays on the newest', () => {
    let history = createHistory('entry-0');
    for (let i = 1; i <= MAX_HISTORY_ENTRIES + 2; i++) history = visit(history, `entry-${i}`);

    expect(history.entries).toHaveLength(MAX_HISTORY_ENTRIES);
    expect(history.entries[0]).toBe('entry-3');
    expect(currentEntry(history)).toBe(`entry-${MAX_HISTORY_ENTRIES + 2}`);
    expect(canGoForward(history)).toBe(false);
  });
});

describe('goBack and goForward', () => {
  it('step one entry at a time through what was visited', () => {
    let history = visited(['grid', 'a', 'b']);

    history = goBack(history);
    expect(currentEntry(history)).toBe('a');
    history = goBack(history);
    expect(currentEntry(history)).toBe('grid');
    history = goForward(history);
    expect(currentEntry(history)).toBe('a');
    history = goForward(history);
    expect(currentEntry(history)).toBe('b');
  });

  it('keep every entry, so a step back can be undone by a step forward', () => {
    const history = visited(['grid', 'a', 'b']);

    expect(goForward(goBack(history))).toEqual(history);
    expect(goBack(history).entries).toEqual(['grid', 'a', 'b']);
  });

  it('report Back and Forward as available only where there is an entry', () => {
    const atEnd = visited(['grid', 'a', 'b']);
    const inMiddle = goBack(atEnd);
    const atStart = goBack(inMiddle);

    expect([canGoBack(atEnd), canGoForward(atEnd)]).toEqual([true, false]);
    expect([canGoBack(inMiddle), canGoForward(inMiddle)]).toEqual([true, true]);
    expect([canGoBack(atStart), canGoForward(atStart)]).toEqual([false, true]);
  });

  it('leave the history as it is at either end', () => {
    const atStart = visited(['grid', 'a'], 1);
    const atEnd = visited(['grid', 'a']);

    expect(goBack(atStart)).toBe(atStart);
    expect(goForward(atEnd)).toBe(atEnd);
  });
});

describe('replaceCurrent', () => {
  it('swaps the current entry and keeps the ones either side of it', () => {
    const history = replaceCurrent(visited(['grid', 'a', 'b'], 1), 'a2');

    expect(history).toEqual({ entries: ['grid', 'a2', 'b'], index: 1 });
    expect(canGoBack(history)).toBe(true);
    expect(canGoForward(history)).toBe(true);
  });

  it('returns the history it was given for the entry already there', () => {
    const history = visited(['grid', 'a']);

    expect(replaceCurrent(history, 'a')).toBe(history);
  });
});

describe('rewriteEntries', () => {
  const drop = (gone: string) => (entry: string) => (entry === gone ? null : entry);

  it('returns the history it was given when no entry changes', () => {
    const history = visited(['grid', 'a', 'b'], 1);

    expect(rewriteEntries(history, (entry) => entry, 'grid')).toBe(history);
  });

  it('drops an entry behind the current one and stays on the same entry', () => {
    const history = rewriteEntries(visited(['grid', 'a', 'b']), drop('a'), 'grid');

    expect(history).toEqual({ entries: ['grid', 'b'], index: 1 });
  });

  it('drops an entry ahead of the current one and stays on the same entry', () => {
    const history = rewriteEntries(visited(['grid', 'a', 'b'], 2), drop('b'), 'grid');

    expect(history).toEqual({ entries: ['grid', 'a'], index: 0 });
    expect(canGoForward(history)).toBe(true);
  });

  it('moves to the nearest earlier entry when the current one is dropped', () => {
    const history = rewriteEntries(visited(['grid', 'a', 'b', 'c'], 1), drop('b'), 'grid');

    expect(history).toEqual({ entries: ['grid', 'a', 'c'], index: 1 });
  });

  it('puts the fallback first when nothing at or before the current entry survives', () => {
    const history = rewriteEntries(visited(['a', 'b'], 1), drop('a'), 'grid');

    expect(history).toEqual({ entries: ['grid', 'b'], index: 0 });
    expect(canGoBack(history)).toBe(false);
    expect(canGoForward(history)).toBe(true);
  });

  it('is left holding only the fallback when every entry is dropped', () => {
    const history = rewriteEntries(visited(['a', 'a2']), () => null, 'grid');

    expect(history).toEqual({ entries: ['grid'], index: 0 });
  });

  it('does not repeat the fallback in front of the same entry', () => {
    const history = rewriteEntries(visited(['a', 'grid'], 1), drop('a'), 'grid');

    expect(history).toEqual({ entries: ['grid'], index: 0 });
  });

  it('merges neighbours a dropped entry leaves the same, wherever the current one is', () => {
    const entries = ['grid', 'a', 'grid', 'b'];

    expect(rewriteEntries(visited(entries), drop('a'), 'grid')).toEqual({
      entries: ['grid', 'b'],
      index: 1,
    });
    // On the entry that merges into its neighbour.
    expect(rewriteEntries(visited(entries, 1), drop('a'), 'grid')).toEqual({
      entries: ['grid', 'b'],
      index: 0,
    });
    // On the dropped entry itself.
    expect(rewriteEntries(visited(entries, 2), drop('a'), 'grid')).toEqual({
      entries: ['grid', 'b'],
      index: 0,
    });
  });

  it('replaces entries in place', () => {
    const history = rewriteEntries(
      visited(['grid', 'a', 'b'], 1),
      (entry) => (entry === 'a' ? 'a2' : entry),
      'grid'
    );

    expect(history).toEqual({ entries: ['grid', 'a2', 'b'], index: 1 });
  });

  it('merges entries a replacement makes the same', () => {
    const history = rewriteEntries(
      visited(['grid', 'a', 'b']),
      (entry) => (entry === 'a' ? 'grid' : entry),
      'grid'
    );

    expect(history).toEqual({ entries: ['grid', 'b'], index: 1 });
  });
});
