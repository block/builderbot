import { describe, expect, it } from 'vitest';
import { indexTagCatalog } from '../tags/tagState';
import { DRAG_REACH } from '../../shared/pointerDrag';
import { tagState } from '../tags/testStubs/tagFixtures';
import type { BoardDropTarget } from './boardCardDrag';
import {
  columnAtPoint,
  columnDropSide,
  judgeBoardDrop,
  resolveBoardDrop,
  type ColumnBox,
} from './boardDrop';
import { NO_TAG_COLUMN_KEY } from './boardPrefs';

const catalog = indexTagCatalog(tagState());

function over(key: string): BoardDropTarget {
  return { kind: 'column', key };
}

describe('columnDropSide', () => {
  const rect = { left: 100, width: 272 };

  it('is before on the left half and after on the right', () => {
    expect(columnDropSide('doing', rect, 100)).toBe('before');
    expect(columnDropSide('doing', rect, 235)).toBe('before');
    expect(columnDropSide('doing', rect, 236)).toBe('after');
    expect(columnDropSide('doing', rect, 371)).toBe('after');
  });

  it('is always after "No ‹group›", which stays first', () => {
    expect(columnDropSide(NO_TAG_COLUMN_KEY, rect, 100)).toBe('after');
    expect(columnDropSide(NO_TAG_COLUMN_KEY, rect, 371)).toBe('after');
  });
});

describe('columnAtPoint', () => {
  // A row from 0 to 800 across and 100 to 400 down. Three 272px columns 12px
  // apart, the last of which runs past the row's right edge, as when the
  // row has more columns than fit.
  const row = { left: 0, right: 800, top: 100, bottom: 400 };
  const box = (key: string, left: number): ColumnBox => ({
    key,
    rect: { left, right: left + 272, width: 272 },
  });
  const columns = [box('idea', 0), box('doing', 284), box('done', 568)];
  const at = (clientX: number, clientY: number) => ({ clientX, clientY });
  const last = { columnKey: 'doing', place: 'after' } as const;

  it('names the column the pointer is level with, and the side its half gives', () => {
    expect(columnAtPoint(at(300, 150), row, columns, null)).toEqual({
      columnKey: 'doing',
      place: 'before',
    });
    expect(columnAtPoint(at(500, 150), row, columns, null)).toEqual({
      columnKey: 'doing',
      place: 'after',
    });
    // Whatever the pointer last named.
    expect(columnAtPoint(at(100, 150), row, columns, last)).toEqual({
      columnKey: 'idea',
      place: 'before',
    });
  });

  it('takes a column from its left edge up to its right', () => {
    expect(columnAtPoint(at(284, 100), row, columns, null)?.columnKey).toBe('doing');
    expect(columnAtPoint(at(555, 399), row, columns, null)?.columnKey).toBe('doing');
    // 556 is the first pixel of the gap after it.
    expect(columnAtPoint(at(556, 150), row, columns, null)).toEqual({
      columnKey: 'doing',
      place: 'after',
    });
  });

  it('only ever lands after "No ‹group›"', () => {
    const withNone = [box(NO_TAG_COLUMN_KEY, 0), box('idea', 284)];
    expect(columnAtPoint(at(10, 150), row, withNone, null)).toEqual({
      columnKey: NO_TAG_COLUMN_KEY,
      place: 'after',
    });
  });

  it('goes by how far across the row the pointer is, anywhere down it', () => {
    // A short column has nothing under it but the row, and still counts.
    expect(columnAtPoint(at(100, 399), row, columns, last)).toEqual({
      columnKey: 'idea',
      place: 'before',
    });
  });

  // The header a column is dragged by is at the row's top edge, so a
  // pointer dragging level drifts above it.
  it.each([
    ['just above', 80],
    ['as far above as the reach', 100 - DRAG_REACH],
    ['just below', 420],
    ['as far below as the reach', 399 + DRAG_REACH],
  ])('names the same column for a pointer that drifts %s the row', (_drift, clientY) => {
    expect(columnAtPoint(at(300, clientY), row, columns, null)).toEqual({
      columnKey: 'doing',
      place: 'before',
    });
    expect(columnAtPoint(at(500, clientY), row, columns, last)).toEqual({
      columnKey: 'doing',
      place: 'after',
    });
    // And the gap after Doing is the place after it there, as in the row.
    expect(columnAtPoint(at(560, clientY), row, columns, null)).toEqual(last);
  });

  it.each([
    ['above', at(300, 99 - DRAG_REACH)],
    ['below', at(300, 400 + DRAG_REACH)],
    ['left of', at(-1 - DRAG_REACH, 150)],
    ['right of', at(800 + DRAG_REACH, 150)],
  ])('lands nowhere clearly %s the row, whatever the last place was', (_side, point) => {
    expect(columnAtPoint(point, row, columns, last)).toBeNull();
  });

  // The gap between two columns is where the line for that place is drawn.

  it('lands after the column before a gap, with no column named yet', () => {
    expect(columnAtPoint(at(278, 150), row, columns, null)).toEqual({
      columnKey: 'idea',
      place: 'after',
    });
    expect(columnAtPoint(at(560, 150), row, columns, null)).toEqual({
      columnKey: 'doing',
      place: 'after',
    });
  });

  it('keeps either name the last place had for a gap: after the one column, before the other', () => {
    // The gap from 556 to 568 is after Doing and before Done.
    const afterDoing = { columnKey: 'doing', place: 'after' } as const;
    const beforeDone = { columnKey: 'done', place: 'before' } as const;
    expect(columnAtPoint(at(560, 150), row, columns, afterDoing)).toBe(afterDoing);
    expect(columnAtPoint(at(560, 150), row, columns, beforeDone)).toBe(beforeDone);
  });

  it('does not keep a last place that is not the gap’s', () => {
    // The far sides of the gap's two columns, and a place elsewhere.
    for (const elsewhere of [
      { columnKey: 'doing', place: 'before' },
      { columnKey: 'done', place: 'after' },
      { columnKey: 'idea', place: 'after' },
    ] as const) {
      expect(columnAtPoint(at(560, 150), row, columns, elsewhere)).toEqual({
        columnKey: 'doing',
        place: 'after',
      });
    }
  });

  it('lands after "No ‹group›" in the gap that follows it', () => {
    const withNone = [box(NO_TAG_COLUMN_KEY, 0), box('idea', 284)];
    expect(columnAtPoint(at(278, 150), row, withNone, null)).toEqual({
      columnKey: NO_TAG_COLUMN_KEY,
      place: 'after',
    });
  });

  it('lands after the last column for a pointer past it, in the row', () => {
    // Two columns, then the + column and empty row from 556 on.
    const short = [box('idea', 0), box('doing', 284)];
    expect(columnAtPoint(at(556, 150), row, short, null)).toEqual({
      columnKey: 'doing',
      place: 'after',
    });
    expect(columnAtPoint(at(799, 150), row, short, last)).toEqual({
      columnKey: 'doing',
      place: 'after',
    });
  });

  it('lands before the first column for a pointer short of it, and after "No ‹group›"', () => {
    const inset = [box('idea', 16), box('doing', 300)];
    expect(columnAtPoint(at(8, 150), row, inset, last)).toEqual({
      columnKey: 'idea',
      place: 'before',
    });
    const withNone = [box(NO_TAG_COLUMN_KEY, 16), box('idea', 300)];
    expect(columnAtPoint(at(8, 150), row, withNone, last)).toEqual({
      columnKey: NO_TAG_COLUMN_KEY,
      place: 'after',
    });
  });

  it('takes a pointer past the row’s end, within reach, to be at that end', () => {
    // Past the left end: the row's first pixel, which Idea's left half holds.
    expect(columnAtPoint(at(-1, 150), row, columns, last)).toEqual({
      columnKey: 'idea',
      place: 'before',
    });
    expect(columnAtPoint(at(-DRAG_REACH, 150), row, columns, last)).toEqual({
      columnKey: 'idea',
      place: 'before',
    });
    // Past the right end: Done spans 568 to 840 and the row ends at 800,
    // which is in Done's right half.
    expect(columnAtPoint(at(800, 150), row, columns, last)).toEqual({
      columnKey: 'done',
      place: 'after',
    });
    expect(columnAtPoint(at(799 + DRAG_REACH, 150), row, columns, null)).toEqual({
      columnKey: 'done',
      place: 'after',
    });
  });

  it('does not count the part of a column that runs past the row’s edge', () => {
    // A fourth column from 852 on does not show in a row that ends at 800,
    // though the pointer past the end is level with it.
    const more = [...columns, box('later', 852)];
    expect(columnAtPoint(at(860, 150), row, more, null)).toEqual({
      columnKey: 'done',
      place: 'after',
    });
    // Scrolled on so the row's edge is in Done's left half, that is its side.
    const scrolled = more.map(({ key, rect }) => box(key, rect.left + 200));
    expect(columnAtPoint(at(860, 150), row, scrolled, null)).toEqual({
      columnKey: 'done',
      place: 'before',
    });
  });

  it('follows the columns as the row scrolls under a still pointer', () => {
    const scrolled = (by: number) => columns.map(({ key, rect }) => box(key, rect.left - by));
    // In the gap after Doing, then over Done's left half.
    expect(columnAtPoint(at(560, 150), row, scrolled(0), last)).toBe(last);
    expect(columnAtPoint(at(560, 150), row, scrolled(100), last)).toEqual({
      columnKey: 'done',
      place: 'before',
    });
  });

  it('lands nowhere in a row with no columns', () => {
    expect(columnAtPoint(at(300, 150), row, [], null)).toBeNull();
    expect(columnAtPoint(at(300, 150), row, [], last)).toBeNull();
  });
});

describe('resolveBoardDrop', () => {
  it('sets the column’s tag, or clears the group on "No ‹group›"', () => {
    expect(resolveBoardDrop('done', 'status', catalog)).toEqual({
      groupId: 'status',
      tagId: 'done',
    });
    expect(resolveBoardDrop(NO_TAG_COLUMN_KEY, 'status', catalog)).toEqual({
      groupId: 'status',
      tagId: null,
    });
  });

  it('is null when the tag vanished or left the group mid-drag', () => {
    expect(resolveBoardDrop('gone', 'status', catalog)).toBeNull();
    expect(resolveBoardDrop('p0', 'status', catalog)).toBeNull();
    expect(resolveBoardDrop('q4', 'status', catalog)).toBeNull();
  });

  it('is null when the group itself is gone', () => {
    expect(resolveBoardDrop(NO_TAG_COLUMN_KEY, 'deleted', catalog)).toBeNull();
    expect(resolveBoardDrop('done', 'deleted', catalog)).toBeNull();
  });
});

describe('judgeBoardDrop', () => {
  it('moves a card dropped on another column to that column’s tag', () => {
    expect(judgeBoardDrop(over('done'), 'idea', 'status', catalog)).toEqual({
      kind: 'move',
      key: 'done',
      write: { groupId: 'status', tagId: 'done' },
    });
    // From "No ‹group›", which is a column like any other to leave.
    expect(judgeBoardDrop(over('idea'), NO_TAG_COLUMN_KEY, 'status', catalog)).toEqual({
      kind: 'move',
      key: 'idea',
      write: { groupId: 'status', tagId: 'idea' },
    });
  });

  it('clears the group’s tag on "No ‹group›"', () => {
    expect(judgeBoardDrop(over(NO_TAG_COLUMN_KEY), 'done', 'status', catalog)).toEqual({
      kind: 'move',
      key: NO_TAG_COLUMN_KEY,
      write: { groupId: 'status', tagId: null },
    });
  });

  it('changes nothing on the card’s own column', () => {
    expect(judgeBoardDrop(over('done'), 'done', 'status', catalog)).toEqual({ kind: 'home' });
    expect(judgeBoardDrop(over(NO_TAG_COLUMN_KEY), NO_TAG_COLUMN_KEY, 'status', catalog)).toEqual({
      kind: 'home',
    });
  });

  it('changes nothing off the columns', () => {
    expect(judgeBoardDrop(null, 'done', 'status', catalog)).toEqual({ kind: 'none' });
  });

  it('refuses the + column, whichever column the card came from', () => {
    expect(judgeBoardDrop({ kind: 'add-column' }, 'done', 'status', catalog)).toEqual({
      kind: 'refused',
    });
    expect(judgeBoardDrop({ kind: 'add-column' }, null, 'status', catalog)).toEqual({
      kind: 'refused',
    });
  });

  it('refuses a column whose tag vanished or left the group mid-drag', () => {
    expect(judgeBoardDrop(over('gone'), 'idea', 'status', catalog)).toEqual({ kind: 'refused' });
    expect(judgeBoardDrop(over('p0'), 'idea', 'status', catalog)).toEqual({ kind: 'refused' });
  });

  it('refuses every column once the group itself is gone', () => {
    expect(judgeBoardDrop(over('done'), 'idea', 'deleted', catalog)).toEqual({ kind: 'refused' });
    expect(judgeBoardDrop(over(NO_TAG_COLUMN_KEY), 'idea', 'deleted', catalog)).toEqual({
      kind: 'refused',
    });
  });

  it('moves a card whose own column is unknown (hidden, or gone)', () => {
    expect(judgeBoardDrop(over('done'), null, 'status', catalog)).toMatchObject({
      kind: 'move',
      write: { groupId: 'status', tagId: 'done' },
    });
  });
});
