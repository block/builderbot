import { describe, expect, it } from 'vitest';
import {
  groupAtPoint,
  isTagDrag,
  TAG_DRAG_TYPE,
  writeTagDrag,
  type GroupSectionBox,
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

describe('writeTagDrag', () => {
  it('carries the id under its own type, as a move', () => {
    const data = dataTransfer();
    writeTagDrag(data, 'idea');

    expect(data.effectAllowed).toBe('move');
    expect(data.getData(TAG_DRAG_TYPE)).toBe('idea');
  });

  it('carries no text for a name field to take', () => {
    const data = dataTransfer();
    writeTagDrag(data, 'idea');

    expect(data.types).toEqual([TAG_DRAG_TYPE]);
    expect(data.getData('text/plain')).toBe('');
  });
});

describe('isTagDrag', () => {
  it('is the drag the panel saw start, carrying its type', () => {
    expect(isTagDrag(dataTransfer({ [TAG_DRAG_TYPE]: 'id' }), true)).toBe(true);
  });

  it('believes the panel when the engine dropped the types', () => {
    expect(isTagDrag(dataTransfer(), true)).toBe(true);
    expect(isTagDrag(null, true)).toBe(true);
  });

  it('is not text or files dragged after a drag that never ended', () => {
    expect(isTagDrag(dataTransfer({ 'text/plain': 'Status' }), true)).toBe(false);
    expect(isTagDrag(dataTransfer({ Files: '' }), true)).toBe(false);
  });

  it('is never a drag the panel did not see start', () => {
    expect(isTagDrag(dataTransfer({ [TAG_DRAG_TYPE]: 'id' }), false)).toBe(false);
    expect(isTagDrag(dataTransfer(), false)).toBe(false);
    expect(isTagDrag(null, false)).toBe(false);
  });
});

describe('groupAtPoint', () => {
  // A list from 100 to 500 down and 0 to 600 across. Three 560px sections
  // 24px apart, inset 16px: Status and Priority, 120px tall, and Area, which
  // runs past the list's bottom edge, as when the list is longer than fits.
  const list = { left: 0, right: 600, top: 100, bottom: 500 };
  const box = (groupId: string, top: number, height: number): GroupSectionBox => ({
    groupId,
    rect: { left: 16, right: 576, top, bottom: top + height, height },
  });
  const sections = [box('status', 116, 120), box('priority', 260, 120), box('area', 404, 200)];
  const at = (clientX: number, clientY: number) => ({ clientX, clientY });
  const last = { anchorGroupId: 'priority', place: 'after' } as const;

  it('names the group under the pointer, and the side its half gives', () => {
    expect(groupAtPoint(at(300, 270), list, sections, null)).toEqual({
      anchorGroupId: 'priority',
      place: 'before',
    });
    expect(groupAtPoint(at(300, 370), list, sections, null)).toEqual({
      anchorGroupId: 'priority',
      place: 'after',
    });
    // Whatever the pointer last named.
    expect(groupAtPoint(at(300, 120), list, sections, last)).toEqual({
      anchorGroupId: 'status',
      place: 'before',
    });
  });

  it('splits a section at its middle', () => {
    // Priority spans 260 to 380, so 320 is the first pixel of its lower half.
    expect(groupAtPoint(at(300, 319), list, sections, null)?.place).toBe('before');
    expect(groupAtPoint(at(300, 320), list, sections, null)?.place).toBe('after');
  });

  it('takes a section from its top and left edges up to its bottom and right', () => {
    expect(groupAtPoint(at(16, 260), list, sections, null)?.anchorGroupId).toBe('priority');
    expect(groupAtPoint(at(575, 379), list, sections, null)?.anchorGroupId).toBe('priority');
    // 380 is the first pixel of the gap.
    expect(groupAtPoint(at(300, 380), list, sections, null)).toBeNull();
  });

  it('keeps the last place in the gap between two sections', () => {
    expect(groupAtPoint(at(300, 248), list, sections, last)).toBe(last);
    expect(groupAtPoint(at(300, 392), list, sections, last)).toBe(last);
  });

  it('keeps the last place beside a section, in the list’s padding', () => {
    expect(groupAtPoint(at(8, 300), list, sections, last)).toBe(last);
    expect(groupAtPoint(at(590, 300), list, sections, last)).toBe(last);
  });

  it('keeps the last place below the last section', () => {
    const short = [box('status', 116, 120), box('priority', 260, 120)];
    expect(groupAtPoint(at(300, 450), list, short, last)).toBe(last);
  });

  it('lands nowhere in the list before any section has been crossed', () => {
    expect(groupAtPoint(at(300, 248), list, sections, null)).toBeNull();
  });

  it.each([
    ['above', at(300, 99)],
    ['below', at(300, 500)],
    ['left of', at(-1, 300)],
    ['right of', at(600, 300)],
  ])('lands nowhere %s the list, whatever the last place was', (_side, point) => {
    expect(groupAtPoint(point, list, sections, last)).toBeNull();
  });

  it('does not count the part of a section that runs past the list’s edge', () => {
    // Area spans 404 to 604, and the list ends at 500.
    expect(groupAtPoint(at(300, 499), list, sections, null)?.anchorGroupId).toBe('area');
    expect(groupAtPoint(at(300, 520), list, sections, last)).toBeNull();
  });

  it('follows the sections as the list scrolls under a still pointer', () => {
    const scrolled = (by: number) =>
      sections.map(({ groupId, rect }) => box(groupId, rect.top - by, rect.height));
    // In the gap above Area, then over Area's upper half, then its lower.
    expect(groupAtPoint(at(300, 392), list, scrolled(0), last)).toBe(last);
    expect(groupAtPoint(at(300, 392), list, scrolled(20), last)).toEqual({
      anchorGroupId: 'area',
      place: 'before',
    });
    expect(groupAtPoint(at(300, 392), list, scrolled(120), last)).toEqual({
      anchorGroupId: 'area',
      place: 'after',
    });
  });
});
