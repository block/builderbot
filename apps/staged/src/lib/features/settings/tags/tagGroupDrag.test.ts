import { describe, expect, it } from 'vitest';
import { isTagGroupDrag, TAG_GROUP_DRAG_TYPE, writeTagGroupDrag } from './tagGroupDrag';

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

describe('writeTagGroupDrag', () => {
  it('carries the group under its own type, as a move', () => {
    const data = dataTransfer();
    writeTagGroupDrag(data, 'status');

    expect(data.effectAllowed).toBe('move');
    expect(data.getData(TAG_GROUP_DRAG_TYPE)).toBe('status');
  });

  it('carries no text for a name field to take', () => {
    const data = dataTransfer();
    writeTagGroupDrag(data, 'status');

    expect(data.types).toEqual([TAG_GROUP_DRAG_TYPE]);
    expect(data.getData('text/plain')).toBe('');
  });
});

describe('isTagGroupDrag', () => {
  it('is the drag the panel saw start, carrying the group type', () => {
    expect(isTagGroupDrag(dataTransfer({ [TAG_GROUP_DRAG_TYPE]: 'status' }), true)).toBe(true);
  });

  it('believes the panel when the engine dropped the types', () => {
    expect(isTagGroupDrag(dataTransfer(), true)).toBe(true);
    expect(isTagGroupDrag(null, true)).toBe(true);
  });

  it('is not text, files or a tag dragged after a group drag that never ended', () => {
    expect(isTagGroupDrag(dataTransfer({ 'text/plain': 'Status' }), true)).toBe(false);
    expect(isTagGroupDrag(dataTransfer({ Files: '' }), true)).toBe(false);
  });

  it('is never a drag the panel did not see start', () => {
    expect(isTagGroupDrag(dataTransfer({ [TAG_GROUP_DRAG_TYPE]: 'status' }), false)).toBe(false);
    expect(isTagGroupDrag(dataTransfer(), false)).toBe(false);
    expect(isTagGroupDrag(null, false)).toBe(false);
  });
});
