import { describe, expect, it } from 'vitest';
import { activeDescendantId, listboxOptionId } from './listboxIds';

describe('listboxOptionId', () => {
  it('derives a stable id from the listbox id and the index', () => {
    expect(listboxOptionId('repo-listbox', 0)).toBe('repo-listbox-option-0');
    expect(listboxOptionId('repo-listbox', 12)).toBe('repo-listbox-option-12');
  });

  it('gives different listboxes different option ids', () => {
    expect(listboxOptionId('a', 3)).not.toBe(listboxOptionId('b', 3));
  });
});

describe('activeDescendantId', () => {
  it('points at the highlighted option while the listbox is open', () => {
    expect(activeDescendantId('lb', { open: true, count: 4, selectedIndex: 2 })).toBe(
      listboxOptionId('lb', 2)
    );
  });

  it('is cleared when the listbox is closed, whatever the index', () => {
    expect(activeDescendantId('lb', { open: false, count: 4, selectedIndex: 2 })).toBeUndefined();
  });

  it('is cleared when nothing is highlighted (-1)', () => {
    expect(activeDescendantId('lb', { open: true, count: 4, selectedIndex: -1 })).toBeUndefined();
  });

  it('is cleared when the highlight is past the end of a list that shrank', () => {
    expect(activeDescendantId('lb', { open: true, count: 2, selectedIndex: 2 })).toBeUndefined();
    expect(activeDescendantId('lb', { open: true, count: 0, selectedIndex: 0 })).toBeUndefined();
  });

  it('ignores a non-integer index', () => {
    expect(activeDescendantId('lb', { open: true, count: 4, selectedIndex: NaN })).toBeUndefined();
    expect(activeDescendantId('lb', { open: true, count: 4, selectedIndex: 1.5 })).toBeUndefined();
  });
});
