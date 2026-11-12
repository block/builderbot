import { describe, expect, it } from 'vitest';
import { indexTagCatalog } from '../tags/tagState';
import { tagState } from '../tags/testStubs/tagFixtures';
import {
  PROJECT_DRAG_TYPE,
  readProjectDrag,
  resolveBoardDrop,
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
