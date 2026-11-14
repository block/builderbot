import { describe, expect, it } from 'vitest';
import { indexTagCatalog } from '../tags/tagState';
import { tagState } from '../tags/testStubs/tagFixtures';
import {
  columnAtPoint,
  columnDropSide,
  isCardDrag,
  PROJECT_DRAG_TYPE,
  readProjectDrag,
  resolveBoardDrop,
  writeProjectDrag,
  type ColumnBox,
} from './boardDrop';
import { NO_TAG_COLUMN_KEY } from './boardPrefs';

const catalog = indexTagCatalog(tagState());

/** Enough of a DataTransfer for the drag helpers. */
function dataTransfer(data: Record<string, string> = {}): DataTransfer {
  const store = new Map(Object.entries(data));
  return {
    effectAllowed: 'none',
    dropEffect: 'none',
    get types() {
      return [...store.keys()];
    },
    setData: (type: string, value: string) => void store.set(type, value),
    getData: (type: string) => store.get(type) ?? '',
  } as unknown as DataTransfer;
}

describe('writeProjectDrag', () => {
  it('carries the project under its own type only, as a move', () => {
    const dt = dataTransfer();
    writeProjectDrag(dt, 'p1');

    expect(dt.effectAllowed).toBe('move');
    expect(dt.getData(PROJECT_DRAG_TYPE)).toBe('p1');
    // No text copy: a card released over the search field, or any text
    // input, must not paste its id there.
    expect(dt.types).toEqual([PROJECT_DRAG_TYPE]);
    expect(dt.getData('text/plain')).toBe('');
  });
});

describe('readProjectDrag', () => {
  const ids = new Set(['p1', 'p2']);

  it('reads the project type first, then the fallback', () => {
    expect(readProjectDrag(dataTransfer({ [PROJECT_DRAG_TYPE]: 'p1' }), 'p2', ids)).toBe('p1');
    // An engine that drops the data on the way: the board saw the drag start.
    expect(readProjectDrag(dataTransfer(), 'p2', ids)).toBe('p2');
    expect(readProjectDrag(null, 'p1', ids)).toBe('p1');
  });

  it('ignores anything that isn’t one of the projects', () => {
    // Text that happens to spell a project id is not a card.
    expect(readProjectDrag(dataTransfer({ 'text/plain': 'p2' }), null, ids)).toBeNull();
    expect(readProjectDrag(dataTransfer({ 'text/plain': 'hello' }), 'p1', ids)).toBe('p1');
    expect(readProjectDrag(dataTransfer(), 'gone', ids)).toBeNull();
  });
});

describe('isCardDrag', () => {
  it('goes by the type the drag carries', () => {
    expect(isCardDrag(dataTransfer({ [PROJECT_DRAG_TYPE]: 'p1' }), false)).toBe(true);
    expect(isCardDrag(dataTransfer({ [PROJECT_DRAG_TYPE]: 'p1' }), true)).toBe(true);
  });

  it('is not one for files and text', () => {
    expect(isCardDrag(dataTransfer({ 'text/plain': 'p1' }), false)).toBe(false);
    expect(isCardDrag(dataTransfer({ Files: '' }), false)).toBe(false);
    expect(isCardDrag(dataTransfer(), false)).toBe(false);
    expect(isCardDrag(null, false)).toBe(false);
  });

  it('falls back to the drag the board saw start when the types are dropped', () => {
    expect(isCardDrag(dataTransfer(), true)).toBe(true);
    expect(isCardDrag(null, true)).toBe(true);
  });

  it('is not one for text and files dragged after a card drag that never ended', () => {
    // The drag the board saw start speaks only for a drag with no types.
    expect(isCardDrag(dataTransfer({ 'text/plain': 'p1' }), true)).toBe(false);
    expect(isCardDrag(dataTransfer({ Files: '' }), true)).toBe(false);
  });
});

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
  // apart: a short one, a tall one, and one that runs past the row's right
  // edge, as when the row has more columns than fit.
  const row = { left: 0, right: 800, top: 100, bottom: 400 };
  const box = (key: string, left: number, bottom: number): ColumnBox => ({
    key,
    rect: { left, right: left + 272, top: 100, bottom, width: 272 },
  });
  const columns = [box('idea', 0, 200), box('doing', 284, 400), box('done', 568, 400)];
  const at = (clientX: number, clientY: number) => ({ clientX, clientY });
  const last = { columnKey: 'doing', place: 'after' } as const;

  it('names the column under the pointer, and the side its half gives', () => {
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

  it('takes a column from its left and top edges up to its right and bottom', () => {
    expect(columnAtPoint(at(284, 100), row, columns, null)?.columnKey).toBe('doing');
    expect(columnAtPoint(at(555, 399), row, columns, null)?.columnKey).toBe('doing');
    // 556 is the first pixel of the gap.
    expect(columnAtPoint(at(556, 150), row, columns, null)).toBeNull();
  });

  it('only ever lands after "No ‹group›"', () => {
    const withNone = [box(NO_TAG_COLUMN_KEY, 0, 400), box('idea', 284, 400)];
    expect(columnAtPoint(at(10, 150), row, withNone, null)).toEqual({
      columnKey: NO_TAG_COLUMN_KEY,
      place: 'after',
    });
  });

  it('keeps the last place in the gap between two columns', () => {
    expect(columnAtPoint(at(278, 150), row, columns, last)).toBe(last);
    expect(columnAtPoint(at(560, 150), row, columns, last)).toBe(last);
  });

  it('keeps the last place under a short column', () => {
    expect(columnAtPoint(at(100, 300), row, columns, last)).toBe(last);
  });

  it('lands nowhere in the row before any column has been crossed', () => {
    expect(columnAtPoint(at(278, 150), row, columns, null)).toBeNull();
  });

  it.each([
    ['above', at(300, 99)],
    ['below', at(300, 400)],
    ['left of', at(-1, 150)],
    ['right of', at(800, 150)],
  ])('lands nowhere %s the row, whatever the last place was', (_side, point) => {
    expect(columnAtPoint(point, row, columns, last)).toBeNull();
  });

  it('does not count the part of a column that runs past the row’s edge', () => {
    // Done spans 568 to 840, and the row ends at 800.
    expect(columnAtPoint(at(799, 150), row, columns, null)?.columnKey).toBe('done');
    expect(columnAtPoint(at(820, 150), row, columns, last)).toBeNull();
  });

  it('follows the columns as the row scrolls under a still pointer', () => {
    const scrolled = (by: number) =>
      columns.map(({ key, rect }) => box(key, rect.left - by, rect.bottom));
    expect(columnAtPoint(at(560, 150), row, scrolled(0), last)).toBe(last);
    expect(columnAtPoint(at(560, 150), row, scrolled(100), last)).toEqual({
      columnKey: 'done',
      place: 'before',
    });
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
