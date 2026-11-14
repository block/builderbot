import { describe, expect, it } from 'vitest';
import {
  DRAG_CHIP_MARGIN,
  DRAG_CHIP_OFFSET,
  DRAG_REACH,
  DRAG_THRESHOLD,
  EDGE_SCROLL_MAX_SPEED,
  EDGE_SCROLL_ZONE,
  dragChipOrigin,
  dragPositionAlong,
  edgeScrollSpeed,
  pastDragThreshold,
  scrollStep,
  spanAlong,
  withinDragReach,
} from './pointerDrag';

const at = (clientX: number, clientY: number) => ({ clientX, clientY });

describe('pastDragThreshold', () => {
  const pressed = at(100, 100);

  it('is false for a press that has not moved', () => {
    expect(pastDragThreshold(pressed, pressed)).toBe(false);
  });

  it('is false short of the threshold, in any direction', () => {
    expect(pastDragThreshold(pressed, at(103, 100))).toBe(false);
    expect(pastDragThreshold(pressed, at(100, 97))).toBe(false);
    expect(pastDragThreshold(pressed, at(98, 102))).toBe(false);
  });

  it('is true at the threshold, in any direction', () => {
    expect(pastDragThreshold(pressed, at(100 + DRAG_THRESHOLD, 100))).toBe(true);
    expect(pastDragThreshold(pressed, at(100 - DRAG_THRESHOLD, 100))).toBe(true);
    expect(pastDragThreshold(pressed, at(100, 100 + DRAG_THRESHOLD))).toBe(true);
    expect(pastDragThreshold(pressed, at(100, 100 - DRAG_THRESHOLD))).toBe(true);
  });

  it('measures the straight-line distance, not each axis alone', () => {
    // 3 px across and 3 px down is 4.24 px away, though neither axis is 4.
    expect(pastDragThreshold(pressed, at(103, 103))).toBe(true);
    expect(pastDragThreshold(pressed, at(102, 102))).toBe(false);
  });

  it('takes another threshold', () => {
    expect(pastDragThreshold(pressed, at(109, 100), 10)).toBe(false);
    expect(pastDragThreshold(pressed, at(110, 100), 10)).toBe(true);
  });
});

describe('edgeScrollSpeed', () => {
  // A container from 100 to 500: its zones are 100–148 and 452–500.
  const span = { start: 100, end: 500 };

  it('is zero in the middle', () => {
    expect(edgeScrollSpeed(300, span)).toBe(0);
  });

  it('is zero at the inner side of each zone', () => {
    expect(edgeScrollSpeed(100 + EDGE_SCROLL_ZONE, span)).toBe(0);
    expect(edgeScrollSpeed(500 - EDGE_SCROLL_ZONE, span)).toBe(0);
  });

  it('climbs towards the start edge, scrolling back', () => {
    expect(edgeScrollSpeed(136, span)).toBeCloseTo(-EDGE_SCROLL_MAX_SPEED / 4);
    expect(edgeScrollSpeed(124, span)).toBeCloseTo(-EDGE_SCROLL_MAX_SPEED / 2);
    expect(edgeScrollSpeed(100, span)).toBe(-EDGE_SCROLL_MAX_SPEED);
  });

  it('climbs towards the end edge, scrolling on', () => {
    expect(edgeScrollSpeed(464, span)).toBeCloseTo(EDGE_SCROLL_MAX_SPEED / 4);
    expect(edgeScrollSpeed(476, span)).toBeCloseTo(EDGE_SCROLL_MAX_SPEED / 2);
    expect(edgeScrollSpeed(500, span)).toBe(EDGE_SCROLL_MAX_SPEED);
  });

  it('stays at full speed for a pointer past an edge', () => {
    expect(edgeScrollSpeed(-2000, span)).toBe(-EDGE_SCROLL_MAX_SPEED);
    expect(edgeScrollSpeed(2000, span)).toBe(EDGE_SCROLL_MAX_SPEED);
  });

  it('keeps a still middle in a short container', () => {
    // 90 px long: each zone is a third of it, 30 px, not 48.
    const short = { start: 0, end: 90 };
    expect(edgeScrollSpeed(45, short)).toBe(0);
    expect(edgeScrollSpeed(30, short)).toBe(0);
    expect(edgeScrollSpeed(60, short)).toBe(0);
    expect(edgeScrollSpeed(15, short)).toBeCloseTo(-EDGE_SCROLL_MAX_SPEED / 2);
    expect(edgeScrollSpeed(75, short)).toBeCloseTo(EDGE_SCROLL_MAX_SPEED / 2);
  });

  it('is zero for a container with no length', () => {
    expect(edgeScrollSpeed(10, { start: 10, end: 10 })).toBe(0);
    expect(edgeScrollSpeed(10, { start: 20, end: 0 })).toBe(0);
  });
});

describe('scrollStep', () => {
  it('scrolls the whole px a frame covers', () => {
    // 900 px a second over a 20 ms frame is 18 px.
    expect(scrollStep(900, 20, 0)).toEqual({ step: 18, carry: 0 });
    expect(scrollStep(-900, 20, 0)).toEqual({ step: -18, carry: 0 });
  });

  it('carries the fraction a frame leaves, so a slow speed still moves', () => {
    // 25 px a second is 0.4 px a 16 ms frame: a pixel every third frame.
    let carry = 0;
    const steps: number[] = [];
    for (let frame = 0; frame < 5; frame++) {
      const next = scrollStep(25, 16, carry);
      steps.push(next.step);
      carry = next.carry;
    }
    expect(steps).toEqual([0, 0, 1, 0, 1]);
  });

  it('carries the fraction towards the start too', () => {
    let carry = 0;
    const steps: number[] = [];
    for (let frame = 0; frame < 5; frame++) {
      const next = scrollStep(-25, 16, carry);
      steps.push(next.step);
      carry = next.carry;
    }
    expect(steps).toEqual([-0, -0, -1, -0, -1]);
  });

  it('counts a stalled frame as a tenth of a second', () => {
    expect(scrollStep(900, 5000, 0).step).toBe(90);
    expect(scrollStep(900, 100, 0).step).toBe(90);
  });

  it('scrolls nothing over no time', () => {
    expect(scrollStep(900, 0, 0)).toEqual({ step: 0, carry: 0 });
    expect(scrollStep(900, -16, 0)).toEqual({ step: 0, carry: 0 });
  });
});

// A box from 100 to 500 across and 200 to 300 down.
const box = { left: 100, right: 500, top: 200, bottom: 300 };

describe('withinDragReach', () => {
  it('holds a point inside the box', () => {
    expect(withinDragReach(at(100, 200), box)).toBe(true);
    expect(withinDragReach(at(300, 250), box)).toBe(true);
    expect(withinDragReach(at(499, 299), box)).toBe(true);
  });

  it.each([
    ['above', at(300, 200 - DRAG_REACH), at(300, 199 - DRAG_REACH)],
    ['below', at(300, 299 + DRAG_REACH), at(300, 300 + DRAG_REACH)],
    ['left of', at(100 - DRAG_REACH, 250), at(99 - DRAG_REACH, 250)],
    ['right of', at(499 + DRAG_REACH, 250), at(500 + DRAG_REACH, 250)],
  ])('holds a point up to the reach %s the box, and none further', (_side, near, far) => {
    expect(withinDragReach(near, box)).toBe(true);
    expect(withinDragReach(far, box)).toBe(false);
  });

  it('reaches as far past a corner as past a side', () => {
    expect(withinDragReach(at(100 - DRAG_REACH, 200 - DRAG_REACH), box)).toBe(true);
    expect(withinDragReach(at(99 - DRAG_REACH, 200 - DRAG_REACH), box)).toBe(false);
    expect(withinDragReach(at(100 - DRAG_REACH, 199 - DRAG_REACH), box)).toBe(false);
  });

  it('reaches well past the drift of a hand, and no further than a nearby panel', () => {
    // A column's header is within 34px of its row's top, and a group's
    // handle within 32px of its list's side: neither is given up at once.
    expect(DRAG_REACH).toBeGreaterThanOrEqual(48);
    expect(DRAG_REACH).toBeLessThanOrEqual(96);
  });

  it('takes another reach, and none', () => {
    expect(withinDragReach(at(300, 190), box, 10)).toBe(true);
    expect(withinDragReach(at(300, 189), box, 10)).toBe(false);
    expect(withinDragReach(at(300, 199), box, 0)).toBe(false);
    expect(withinDragReach(at(300, 200), box, 0)).toBe(true);
  });
});

describe('dragPositionAlong', () => {
  it('is the coordinate along the axis for a point inside the box', () => {
    expect(dragPositionAlong(at(340, 250), box, 'x')).toBe(340);
    expect(dragPositionAlong(at(340, 250), box, 'y')).toBe(250);
  });

  it('is the same for a point that strays off the other axis, within reach', () => {
    // Above and below a row.
    expect(dragPositionAlong(at(340, 200 - DRAG_REACH), box, 'x')).toBe(340);
    expect(dragPositionAlong(at(340, 299 + DRAG_REACH), box, 'x')).toBe(340);
    // Left and right of a list.
    expect(dragPositionAlong(at(100 - DRAG_REACH, 250), box, 'y')).toBe(250);
    expect(dragPositionAlong(at(499 + DRAG_REACH, 250), box, 'y')).toBe(250);
  });

  it('is the nearer end for a point past an end of the axis, within reach', () => {
    expect(dragPositionAlong(at(99, 250), box, 'x')).toBe(100);
    expect(dragPositionAlong(at(100 - DRAG_REACH, 250), box, 'x')).toBe(100);
    expect(dragPositionAlong(at(499 + DRAG_REACH, 250), box, 'x')).toBe(500);
    expect(dragPositionAlong(at(300, 200 - DRAG_REACH), box, 'y')).toBe(200);
    expect(dragPositionAlong(at(300, 299 + DRAG_REACH), box, 'y')).toBe(300);
  });

  it('is both at once past a corner', () => {
    expect(dragPositionAlong(at(90, 190), box, 'x')).toBe(100);
    expect(dragPositionAlong(at(510, 310), box, 'y')).toBe(300);
  });

  it.each([
    ['above', at(300, 199 - DRAG_REACH)],
    ['below', at(300, 300 + DRAG_REACH)],
    ['left of', at(99 - DRAG_REACH, 250)],
    ['right of', at(500 + DRAG_REACH, 250)],
  ])('is nowhere out of reach %s the box, on either axis', (_side, point) => {
    expect(dragPositionAlong(point, box, 'x')).toBeNull();
    expect(dragPositionAlong(point, box, 'y')).toBeNull();
  });

  it('takes another reach', () => {
    expect(dragPositionAlong(at(340, 190), box, 'x', 10)).toBe(340);
    expect(dragPositionAlong(at(340, 189), box, 'x', 10)).toBeNull();
  });
});

describe('spanAlong', () => {
  // Three items 12 apart, the first starting 16 in.
  const spans = [
    { start: 16, end: 116 },
    { start: 128, end: 228 },
    { start: 240, end: 340 },
  ];
  const on = (index: number) => ({ index, gap: false });
  const afterItem = (index: number) => ({ index, gap: true });

  it('is on the item that holds the position, from its start up to its end', () => {
    expect(spanAlong(16, spans)).toEqual(on(0));
    expect(spanAlong(115, spans)).toEqual(on(0));
    expect(spanAlong(128, spans)).toEqual(on(1));
    expect(spanAlong(227, spans)).toEqual(on(1));
    expect(spanAlong(300, spans)).toEqual(on(2));
  });

  it('is in the gap after an item, from its end up to the next one’s start', () => {
    expect(spanAlong(116, spans)).toEqual(afterItem(0));
    expect(spanAlong(127, spans)).toEqual(afterItem(0));
    expect(spanAlong(228, spans)).toEqual(afterItem(1));
    expect(spanAlong(239, spans)).toEqual(afterItem(1));
  });

  it('is on the first item before them all, and on the last past them all', () => {
    expect(spanAlong(15, spans)).toEqual(on(0));
    expect(spanAlong(-500, spans)).toEqual(on(0));
    expect(spanAlong(340, spans)).toEqual(on(2));
    expect(spanAlong(5000, spans)).toEqual(on(2));
  });

  it('is on the only item wherever the position is', () => {
    const only = [{ start: 100, end: 200 }];
    expect(spanAlong(0, only)).toEqual(on(0));
    expect(spanAlong(150, only)).toEqual(on(0));
    expect(spanAlong(200, only)).toEqual(on(0));
  });

  it('has no gap between items that touch', () => {
    const touching = [
      { start: 0, end: 100 },
      { start: 100, end: 200 },
    ];
    expect(spanAlong(99, touching)).toEqual(on(0));
    expect(spanAlong(100, touching)).toEqual(on(1));
  });

  it('is nothing with no items', () => {
    expect(spanAlong(100, [])).toBeNull();
  });
});

describe('dragChipOrigin', () => {
  const chip = { width: 120, height: 28 };
  const viewport = { width: 1000, height: 700 };

  it('hangs the chip below the pointer and to its left', () => {
    const origin = dragChipOrigin(at(500, 300), chip, viewport);
    // Its right edge is short of the pointer, and its top below it.
    expect(origin.left + chip.width).toBe(500 - DRAG_CHIP_OFFSET.x);
    expect(origin.top).toBe(300 + DRAG_CHIP_OFFSET.y);
  });

  it('keeps clear of the pointer and of what is level with it', () => {
    // Far enough below that the chip is not over a name the pointer is on:
    // a line of text is no taller than this either side of the pointer.
    expect(DRAG_CHIP_OFFSET.y).toBeGreaterThanOrEqual(12);
    expect(DRAG_CHIP_OFFSET.x).toBeGreaterThan(0);
  });

  it('follows the pointer', () => {
    const from = dragChipOrigin(at(500, 300), chip, viewport);
    const to = dragChipOrigin(at(530, 340), chip, viewport);
    expect(to).toEqual({ left: from.left + 30, top: from.top + 40 });
  });

  it('slides right to stay whole at the window’s left edge, still below the pointer', () => {
    expect(dragChipOrigin(at(60, 300), chip, viewport)).toEqual({
      left: DRAG_CHIP_MARGIN,
      top: 300 + DRAG_CHIP_OFFSET.y,
    });
  });

  it('slides up to stay whole at the window’s bottom edge, still left of the pointer', () => {
    expect(dragChipOrigin(at(500, 690), chip, viewport)).toEqual({
      left: 500 - DRAG_CHIP_OFFSET.x - chip.width,
      top: 700 - DRAG_CHIP_MARGIN - chip.height,
    });
  });

  it('stays in the window for a pointer past its right edge or above its top', () => {
    expect(dragChipOrigin(at(1400, -50), chip, viewport)).toEqual({
      left: 1000 - DRAG_CHIP_MARGIN - chip.width,
      top: DRAG_CHIP_MARGIN,
    });
  });

  it('keeps to the left and top edges of a window too small for it', () => {
    expect(dragChipOrigin(at(50, 10), chip, { width: 100, height: 20 })).toEqual({
      left: DRAG_CHIP_MARGIN,
      top: DRAG_CHIP_MARGIN,
    });
  });
});
