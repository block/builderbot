import { describe, expect, it } from 'vitest';
import { DRAG_REACH } from '../../../shared/pointerDrag';
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
  // A list from 100 to 500 down and 0 to 600 across. Three sections 24px
  // apart, starting 16px down: Status and Priority, 120px tall, and Area,
  // which runs past the list's bottom edge, as when the list is longer than
  // fits.
  const list = { left: 0, right: 600, top: 100, bottom: 500 };
  const box = (groupId: string, top: number, height: number): GroupSectionBox => ({
    groupId,
    rect: { top, bottom: top + height, height },
  });
  const sections = [box('status', 116, 120), box('priority', 260, 120), box('area', 404, 200)];
  const at = (clientX: number, clientY: number) => ({ clientX, clientY });
  const last = { anchorGroupId: 'priority', place: 'after' } as const;

  it('names the group the pointer is level with, and the side its half gives', () => {
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

  it('takes a section from its top edge up to its bottom', () => {
    expect(groupAtPoint(at(16, 260), list, sections, null)?.anchorGroupId).toBe('priority');
    expect(groupAtPoint(at(575, 379), list, sections, null)?.anchorGroupId).toBe('priority');
    // 380 is the first pixel of the gap under it.
    expect(groupAtPoint(at(300, 380), list, sections, null)).toEqual({
      anchorGroupId: 'priority',
      place: 'after',
    });
  });

  it('goes by how far down the list the pointer is, anywhere across it', () => {
    // In the list's padding, and right of sections that stop short of a
    // wide list's edge.
    expect(groupAtPoint(at(1, 300), list, sections, last)).toEqual({
      anchorGroupId: 'priority',
      place: 'before',
    });
    expect(groupAtPoint(at(599, 300), list, sections, last)).toEqual({
      anchorGroupId: 'priority',
      place: 'before',
    });
  });

  // The handle a group is dragged by is at the list's left edge, so a
  // pointer dragging straight down drifts off it.
  it.each([
    ['just left of', -20],
    ['as far left as the reach of', -DRAG_REACH],
    ['just right of', 620],
    ['as far right as the reach of', 599 + DRAG_REACH],
  ])('names the same group for a pointer that drifts %s the list', (_drift, clientX) => {
    expect(groupAtPoint(at(clientX, 270), list, sections, null)).toEqual({
      anchorGroupId: 'priority',
      place: 'before',
    });
    expect(groupAtPoint(at(clientX, 370), list, sections, last)).toEqual({
      anchorGroupId: 'priority',
      place: 'after',
    });
    // And the gap under Priority is the place below it there, as in the list.
    expect(groupAtPoint(at(clientX, 392), list, sections, null)).toEqual(last);
  });

  it.each([
    ['above', at(300, 99 - DRAG_REACH)],
    ['below', at(300, 500 + DRAG_REACH)],
    ['left of', at(-1 - DRAG_REACH, 300)],
    ['right of', at(600 + DRAG_REACH, 300)],
  ])('lands nowhere clearly %s the list, whatever the last place was', (_side, point) => {
    expect(groupAtPoint(point, list, sections, last)).toBeNull();
  });

  // The gap between two sections is where the line for that place is drawn.

  it('lands below the group above a gap, with no section named yet', () => {
    expect(groupAtPoint(at(300, 248), list, sections, null)).toEqual({
      anchorGroupId: 'status',
      place: 'after',
    });
    expect(groupAtPoint(at(300, 392), list, sections, null)).toEqual({
      anchorGroupId: 'priority',
      place: 'after',
    });
  });

  it('keeps either name the last place had for a gap: below the one group, above the other', () => {
    // The gap from 380 to 404 is below Priority and above Area.
    const belowPriority = { anchorGroupId: 'priority', place: 'after' } as const;
    const aboveArea = { anchorGroupId: 'area', place: 'before' } as const;
    expect(groupAtPoint(at(300, 392), list, sections, belowPriority)).toBe(belowPriority);
    expect(groupAtPoint(at(300, 392), list, sections, aboveArea)).toBe(aboveArea);
  });

  it('does not keep a last place that is not the gap’s', () => {
    // The far sides of the gap's two sections, and a place elsewhere.
    for (const elsewhere of [
      { anchorGroupId: 'priority', place: 'before' },
      { anchorGroupId: 'area', place: 'after' },
      { anchorGroupId: 'status', place: 'after' },
    ] as const) {
      expect(groupAtPoint(at(300, 392), list, sections, elsewhere)).toEqual({
        anchorGroupId: 'priority',
        place: 'after',
      });
    }
  });

  it('lands below the last group for a pointer under its section, in the list', () => {
    // Two groups, then the ungrouped tags and empty list from 380 on.
    const short = [box('status', 116, 120), box('priority', 260, 120)];
    expect(groupAtPoint(at(300, 380), list, short, null)).toEqual({
      anchorGroupId: 'priority',
      place: 'after',
    });
    expect(groupAtPoint(at(300, 499), list, short, last)).toEqual({
      anchorGroupId: 'priority',
      place: 'after',
    });
  });

  it('lands above the first group for a pointer over the list’s padding above it', () => {
    expect(groupAtPoint(at(300, 100), list, sections, last)).toEqual({
      anchorGroupId: 'status',
      place: 'before',
    });
    expect(groupAtPoint(at(300, 115), list, sections, null)).toEqual({
      anchorGroupId: 'status',
      place: 'before',
    });
  });

  it('takes a pointer past the list’s end, within reach, to be at that end', () => {
    // Above the list: its first pixel, which is above Status.
    expect(groupAtPoint(at(300, 99), list, sections, last)).toEqual({
      anchorGroupId: 'status',
      place: 'before',
    });
    expect(groupAtPoint(at(300, 100 - DRAG_REACH), list, sections, last)).toEqual({
      anchorGroupId: 'status',
      place: 'before',
    });
    // Below it: Area spans 404 to 604 and the list ends at 500, which is in
    // Area's upper half.
    expect(groupAtPoint(at(300, 500), list, sections, last)).toEqual({
      anchorGroupId: 'area',
      place: 'before',
    });
    expect(groupAtPoint(at(300, 499 + DRAG_REACH), list, sections, null)).toEqual({
      anchorGroupId: 'area',
      place: 'before',
    });
  });

  it('does not count the part of a section that runs past the list’s edge', () => {
    // A fourth section from 628 on does not show in a list that ends at
    // 500, though the pointer past the end is level with it.
    const more = [...sections, box('later', 628, 120)];
    expect(groupAtPoint(at(300, 540), list, more, null)).toEqual({
      anchorGroupId: 'area',
      place: 'before',
    });
    // Nor Area's lower half, which the same pointer is level with.
    expect(groupAtPoint(at(300, 520), list, sections, null)?.place).toBe('before');
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

  it('lands nowhere in a list with no groups', () => {
    expect(groupAtPoint(at(300, 300), list, [], null)).toBeNull();
    expect(groupAtPoint(at(300, 300), list, [], last)).toBeNull();
  });
});
