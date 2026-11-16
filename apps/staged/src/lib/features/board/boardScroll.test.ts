import { describe, expect, it } from 'vitest';
import { overflowEdges, revealOffset, revealStep } from './boardScroll';

describe('overflowEdges', () => {
  it('shows neither edge when the columns fit', () => {
    expect(overflowEdges({ scrollLeft: 0, scrollWidth: 800, clientWidth: 800 })).toEqual({
      start: false,
      end: false,
    });
  });

  it('shows the end edge at the start of an overflowing row', () => {
    expect(overflowEdges({ scrollLeft: 0, scrollWidth: 1400, clientWidth: 800 })).toEqual({
      start: false,
      end: true,
    });
  });

  it('shows both edges in the middle, and only the start edge at the end', () => {
    expect(overflowEdges({ scrollLeft: 300, scrollWidth: 1400, clientWidth: 800 })).toEqual({
      start: true,
      end: true,
    });
    expect(overflowEdges({ scrollLeft: 600, scrollWidth: 1400, clientWidth: 800 })).toEqual({
      start: true,
      end: false,
    });
  });

  it('ignores a sub-pixel remainder under zoom', () => {
    expect(overflowEdges({ scrollLeft: 599.5, scrollWidth: 1400, clientWidth: 800 })).toEqual({
      start: true,
      end: false,
    });
    expect(overflowEdges({ scrollLeft: 0.5, scrollWidth: 800.5, clientWidth: 800 })).toEqual({
      start: false,
      end: false,
    });
  });
});

describe('revealStep', () => {
  // A view from 48 to 752 (an 800px scroller less two 48px fades), and 272px
  // columns 12px apart starting at 0.
  const view = { left: 48, right: 752 };
  const columns = [0, 1, 2, 3, 4].map((i) => ({ left: i * 284, right: i * 284 + 272 }));

  it('lines the first column clipped on the right up with the view’s right edge', () => {
    // Column 2 ends at 840, past 752.
    expect(revealStep(1, view, columns)).toBe(840 - 752);
  });

  it('lines the last column clipped on the left up with the view’s left edge', () => {
    const scrolled = columns.map(({ left, right }) => ({ left: left - 600, right: right - 600 }));
    // Column 2 now starts at -32, before 48; column 3 starts at 252, inside.
    expect(revealStep(-1, view, scrolled)).toBe(-32 - 48);
  });

  it('scrolls by the view’s width when no column is clipped on that side', () => {
    expect(revealStep(-1, view, columns.slice(1))).toBe(-(752 - 48));
    expect(revealStep(1, view, [{ left: 0, right: 700 }])).toBe(752 - 48);
  });
});

describe('revealOffset', () => {
  const view = { left: 48, right: 752 };

  it('leaves a column that is in view where it is', () => {
    expect(revealOffset(view, { left: 284, right: 556 })).toBe(0);
    expect(revealOffset(view, { left: 48, right: 320 })).toBe(0);
    // A sub-pixel overhang under zoom is in view too.
    expect(revealOffset(view, { left: 480.5, right: 752.5 })).toBe(0);
  });

  it('lines a column clipped on the right up with the view’s right edge', () => {
    expect(revealOffset(view, { left: 568, right: 840 })).toBe(840 - 752);
  });

  it('lines a column clipped on the left up with the view’s left edge', () => {
    expect(revealOffset(view, { left: -32, right: 240 })).toBe(-32 - 48);
  });

  it('pages to a full-width column from either side', () => {
    const page = { left: 0, right: 390 };
    expect(revealOffset(page, { left: 390, right: 780 })).toBe(390);
    expect(revealOffset(page, { left: -390, right: 0 })).toBe(-390);
    expect(revealOffset(page, { left: 0, right: 390 })).toBe(0);
  });

  it('lines a column wider than the view up with its left edge', () => {
    expect(revealOffset(view, { left: 100, right: 900 })).toBe(100 - 48);
  });
});
