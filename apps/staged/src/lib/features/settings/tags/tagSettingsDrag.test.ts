import { describe, expect, it } from 'vitest';
import {
  isTagDrag,
  isTagGroupDrag,
  TAG_DRAG_TYPE,
  TAG_GROUP_DRAG_TYPE,
  writeTagDrag,
  writeTagGroupDrag,
} from './tagSettingsDrag';

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

describe.each([
  ['writeTagDrag', writeTagDrag, TAG_DRAG_TYPE, 'idea'],
  ['writeTagGroupDrag', writeTagGroupDrag, TAG_GROUP_DRAG_TYPE, 'status'],
] as const)('%s', (_name, write, type, id) => {
  it('carries the id under its own type, as a move', () => {
    const data = dataTransfer();
    write(data, id);

    expect(data.effectAllowed).toBe('move');
    expect(data.getData(type)).toBe(id);
  });

  it('carries no text for a name field to take', () => {
    const data = dataTransfer();
    write(data, id);

    expect(data.types).toEqual([type]);
    expect(data.getData('text/plain')).toBe('');
  });
});

describe.each([
  ['isTagDrag', isTagDrag, TAG_DRAG_TYPE, TAG_GROUP_DRAG_TYPE],
  ['isTagGroupDrag', isTagGroupDrag, TAG_GROUP_DRAG_TYPE, TAG_DRAG_TYPE],
] as const)('%s', (_name, isDrag, type, otherType) => {
  it('is the drag the panel saw start, carrying its type', () => {
    expect(isDrag(dataTransfer({ [type]: 'id' }), true)).toBe(true);
  });

  it('believes the panel when the engine dropped the types', () => {
    expect(isDrag(dataTransfer(), true)).toBe(true);
    expect(isDrag(null, true)).toBe(true);
  });

  it('is not text, files or the other kind dragged after a drag that never ended', () => {
    expect(isDrag(dataTransfer({ 'text/plain': 'Status' }), true)).toBe(false);
    expect(isDrag(dataTransfer({ Files: '' }), true)).toBe(false);
    expect(isDrag(dataTransfer({ [otherType]: 'id' }), true)).toBe(false);
  });

  it('is never a drag the panel did not see start', () => {
    expect(isDrag(dataTransfer({ [type]: 'id' }), false)).toBe(false);
    expect(isDrag(dataTransfer(), false)).toBe(false);
    expect(isDrag(null, false)).toBe(false);
  });
});
