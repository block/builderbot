import { describe, expect, it } from 'vitest';
import { anchoredScrollTop, firstVisibleIndex } from './scrollAnchor';

describe('firstVisibleIndex', () => {
  // Group i spans [i * 100, i * 100 + 100) in client coordinates.
  const bottoms = (count: number) => (i: number) => i * 100 + 100;

  it('finds the first group whose bottom edge is below the box top', () => {
    expect(firstVisibleIndex(10, bottoms(10), 0)).toBe(0);
    expect(firstVisibleIndex(10, bottoms(10), 250)).toBe(2);
    // A bottom exactly on the box top is fully scrolled out.
    expect(firstVisibleIndex(10, bottoms(10), 300)).toBe(3);
    expect(firstVisibleIndex(10, bottoms(10), 999)).toBe(9);
  });

  it('reports none when every group is above the box or there are no groups', () => {
    expect(firstVisibleIndex(10, bottoms(10), 1000)).toBe(-1);
    expect(firstVisibleIndex(0, bottoms(0), 0)).toBe(-1);
  });

  it('reads only O(log n) bottoms', () => {
    let reads = 0;
    const read = (i: number) => {
      reads += 1;
      return bottoms(1024)(i);
    };
    expect(firstVisibleIndex(1024, read, 51_250)).toBe(512);
    expect(reads).toBeLessThanOrEqual(11);
  });
});

describe('anchoredScrollTop', () => {
  const anchor = { key: 'm-7', offset: 40 };

  it('scrolls by however far the group drifted from its offset', () => {
    // Group now 900px below the box top, wants to be 40.
    expect(anchoredScrollTop(0, anchor, 900)).toBe(860);
    // Already in place.
    expect(anchoredScrollTop(500, anchor, 40)).toBe(500);
    // Group above where it belongs: scroll up.
    expect(anchoredScrollTop(500, anchor, 20)).toBe(480);
  });
});
