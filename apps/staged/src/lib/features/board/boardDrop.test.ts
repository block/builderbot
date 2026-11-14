import { describe, expect, it } from 'vitest';
import { indexTagCatalog } from '../tags/tagState';
import { tagState } from '../tags/testStubs/tagFixtures';
import {
  boardDragKind,
  COLUMN_DRAG_TYPE,
  columnDropSide,
  PROJECT_DRAG_TYPE,
  readColumnDrag,
  readProjectDrag,
  resolveBoardDrop,
  writeColumnDrag,
  writeProjectDrag,
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

describe('writeColumnDrag', () => {
  it('carries the column under its own type only, as a move', () => {
    const dt = dataTransfer();
    writeColumnDrag(dt, 'doing');

    expect(dt.effectAllowed).toBe('move');
    // Neither a card's type, which would land it as a card, nor text.
    expect(dt.types).toEqual([COLUMN_DRAG_TYPE]);
    expect(dt.getData(COLUMN_DRAG_TYPE)).toBe('doing');
  });
});

describe('readColumnDrag', () => {
  it('reads the column type first, then the fallback', () => {
    expect(readColumnDrag(dataTransfer({ [COLUMN_DRAG_TYPE]: 'doing' }), 'done')).toBe('doing');
    // An engine that drops the data on the way: the board saw the drag start.
    expect(readColumnDrag(dataTransfer(), 'done')).toBe('done');
    expect(readColumnDrag(null, 'done')).toBe('done');
  });

  it('reads no column from a card or from text', () => {
    expect(readColumnDrag(dataTransfer({ [PROJECT_DRAG_TYPE]: 'p1' }), null)).toBeNull();
    expect(readColumnDrag(dataTransfer({ 'text/plain': 'doing' }), null)).toBeNull();
  });
});

describe('boardDragKind', () => {
  it('goes by the type the drag carries', () => {
    expect(boardDragKind(dataTransfer({ [PROJECT_DRAG_TYPE]: 'p1' }), null)).toBe('card');
    expect(boardDragKind(dataTransfer({ [COLUMN_DRAG_TYPE]: 'doing' }), null)).toBe('column');
  });

  it('is neither for files and text', () => {
    expect(boardDragKind(dataTransfer({ 'text/plain': 'doing' }), null)).toBeNull();
    expect(boardDragKind(dataTransfer({ Files: '' }), null)).toBeNull();
    expect(boardDragKind(null, null)).toBeNull();
  });

  it('falls back to the drag the board saw start when the types are dropped', () => {
    expect(boardDragKind(dataTransfer(), 'column')).toBe('column');
    expect(boardDragKind(null, 'card')).toBe('card');
  });

  it('believes the types over a drag left over from before', () => {
    // A card whose dragend never came, then a column dragged.
    expect(boardDragKind(dataTransfer({ [COLUMN_DRAG_TYPE]: 'doing' }), 'card')).toBe('column');
    expect(boardDragKind(dataTransfer({ [PROJECT_DRAG_TYPE]: 'p1' }), 'column')).toBe('card');
  });

  it('is neither for text and files dragged after a drag that never ended', () => {
    // The drag the board saw start speaks only for a drag with no types.
    expect(boardDragKind(dataTransfer({ 'text/plain': 'doing' }), 'column')).toBeNull();
    expect(boardDragKind(dataTransfer({ 'text/plain': 'p1' }), 'card')).toBeNull();
    expect(boardDragKind(dataTransfer({ Files: '' }), 'column')).toBeNull();
    expect(boardDragKind(dataTransfer({ Files: '' }), 'card')).toBeNull();
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
