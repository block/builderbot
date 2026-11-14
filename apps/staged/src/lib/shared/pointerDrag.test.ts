import { describe, expect, it } from 'vitest';
import {
  DRAG_THRESHOLD,
  EDGE_SCROLL_MAX_SPEED,
  EDGE_SCROLL_ZONE,
  edgeScrollSpeed,
  pastDragThreshold,
  scrollStep,
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
