import { describe, expect, it } from 'vitest';
import { alignDelta, revealWithin } from './revealWithin';

interface FakeOptions {
  /** Top-left of the border box in the viewport, and its size. */
  rect: { top: number; left: number; width: number; height: number };
  scrollHeight?: number;
  scrollWidth?: number;
  scrollTop?: number;
  scrollLeft?: number;
  scroller?: boolean;
}

interface Fake {
  parentElement: Fake | null;
  scroller: boolean;
  scrolls: ScrollToOptions[];
  clientTop: number;
  clientLeft: number;
  clientHeight: number;
  clientWidth: number;
  scrollHeight: number;
  scrollWidth: number;
  scrollTop: number;
  scrollLeft: number;
  getBoundingClientRect(): DOMRect;
  contains(other: Fake): boolean;
  scrollTo(options: ScrollToOptions): void;
}

/** A box laid out at `rect` that records its scrolls (applied at once). */
function fake(parent: Fake | null, options: FakeOptions): Fake {
  const { rect } = options;
  const node: Fake = {
    parentElement: parent,
    scroller: options.scroller ?? false,
    scrolls: [],
    clientTop: 0,
    clientLeft: 0,
    clientHeight: rect.height,
    clientWidth: rect.width,
    scrollHeight: options.scrollHeight ?? rect.height,
    scrollWidth: options.scrollWidth ?? rect.width,
    scrollTop: options.scrollTop ?? 0,
    scrollLeft: options.scrollLeft ?? 0,
    getBoundingClientRect: () =>
      ({
        ...rect,
        bottom: rect.top + rect.height,
        right: rect.left + rect.width,
      }) as DOMRect,
    contains(other) {
      for (let n: Fake | null = other; n; n = n.parentElement) if (n === node) return true;
      return false;
    },
    scrollTo(to) {
      node.scrolls.push(to);
    },
  };
  return node;
}

const reveal = (el: Fake, box: Fake, options?: Parameters<typeof revealWithin>[2]) =>
  revealWithin(
    el as unknown as Element,
    box as unknown as Element,
    options,
    (node) => (node as unknown as Fake).scroller
  );

/** The pane around the transcript, which must never scroll, and the transcript. */
function transcript(scrollTop = 1000) {
  const pane = fake(null, {
    rect: { top: 0, left: 0, width: 800, height: 600 },
    scrollHeight: 900,
    scroller: true,
  });
  const box = fake(pane, {
    rect: { top: 100, left: 0, width: 800, height: 400 },
    scrollHeight: 5000,
    scrollTop,
  });
  return { pane, box };
}

describe('alignDelta', () => {
  it('aligns as scrollIntoView does', () => {
    expect(alignDelta('start', 300, 320, 100, 500)).toBe(200);
    expect(alignDelta('end', 300, 320, 100, 500)).toBe(-180);
    expect(alignDelta('center', 300, 320, 100, 500)).toBe(10);
  });

  it('leaves a span in view, or one covering the port, where it is', () => {
    expect(alignDelta('nearest', 200, 300, 100, 500)).toBe(0);
    expect(alignDelta('nearest', 0, 600, 100, 500)).toBe(0);
  });

  it('aligns the edge a smaller span overflows, the far edge of a larger one', () => {
    expect(alignDelta('nearest', 480, 520, 100, 500)).toBe(20);
    expect(alignDelta('nearest', 80, 120, 100, 500)).toBe(-20);
    expect(alignDelta('nearest', 300, 900, 100, 500)).toBe(200);
    expect(alignDelta('nearest', -300, 300, 100, 500)).toBe(-200);
  });
});

describe('revealWithin', () => {
  it('scrolls the box, and nothing around it', () => {
    const { pane, box } = transcript();
    const row = fake(box, { rect: { top: 900, left: 20, width: 600, height: 20 } });
    reveal(row, box, { block: 'center', behavior: 'smooth' });
    expect(box.scrolls).toEqual([{ top: 1610, left: 0, behavior: 'smooth' }]);
    expect(pane.scrolls).toEqual([]);
  });

  it('does nothing for a row already in view with nearest', () => {
    const { box } = transcript();
    const row = fake(box, { rect: { top: 300, left: 20, width: 600, height: 20 } });
    reveal(row, box, { block: 'nearest' });
    expect(box.scrolls).toEqual([]);
  });

  it('brings a row below the fold up to the bottom edge with nearest', () => {
    const { box } = transcript();
    const row = fake(box, { rect: { top: 560, left: 20, width: 600, height: 60 } });
    reveal(row, box, { block: 'nearest' });
    expect(box.scrolls).toEqual([{ top: 1120, left: 0, behavior: 'auto' }]);
  });

  it('scrolls a code block inside the box to the match, then the box', () => {
    const { pane, box } = transcript();
    const pre = fake(box, {
      rect: { top: 700, left: 20, width: 300, height: 100 },
      scrollWidth: 1200,
      scroller: true,
    });
    const match = fake(pre, { rect: { top: 740, left: 500, width: 40, height: 20 } });
    reveal(match, box, { block: 'center', inline: 'nearest' });
    // Its right edge to the block's: 540 - 320.
    expect(pre.scrolls).toEqual([{ top: 0, left: 220, behavior: 'auto' }]);
    // In view across the box once the block has scrolled, centred down it.
    expect(box.scrolls).toEqual([{ top: 1450, left: 0, behavior: 'auto' }]);
    expect(pane.scrolls).toEqual([]);
  });

  it('stops at the end of the box', () => {
    const { box } = transcript(4500);
    const row = fake(box, { rect: { top: 480, left: 20, width: 600, height: 20 } });
    reveal(row, box, { block: 'center' });
    expect(box.scrolls).toEqual([{ top: 4600, left: 0, behavior: 'auto' }]);
  });

  it('leaves an element outside the box alone', () => {
    const { pane, box } = transcript();
    const composer = fake(pane, { rect: { top: 560, left: 0, width: 800, height: 40 } });
    reveal(composer, box);
    expect(box.scrolls).toEqual([]);
    expect(pane.scrolls).toEqual([]);
  });
});
