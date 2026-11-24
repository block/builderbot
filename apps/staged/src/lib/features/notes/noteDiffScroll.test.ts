import { describe, expect, it } from 'vitest';

import type { NoteDiffHunk, NoteDiffOp } from './noteDiff';
import {
  buildScrollAnchors,
  GAP_MARKER_HEIGHT,
  hunkExtent,
  hunkGapOffset,
  hunkLead,
  hunkMarkerBox,
  mapScrollOffset,
  REFERENCE_LINE_FRACTION,
  scrollTopForOffset,
  syncedScrollTop,
  type NoteDiffPaneScroll,
  type NoteDiffScrollAnchor,
  type NoteDiffUnitBox,
} from './noteDiffScroll';

function box(top: number, height: number): NoteDiffUnitBox {
  return { top, height };
}

function pane(scrollTop: number, scrollHeight: number, clientHeight = 300): NoteDiffPaneScroll {
  return { scrollTop, clientHeight, scrollHeight };
}

function hunk(overrides: Partial<NoteDiffHunk>): NoteDiffHunk {
  return {
    id: 'hunk-0',
    kind: 'modified',
    ops: [],
    previousIndices: [],
    currentIndices: [],
    previousStart: 0,
    currentStart: 0,
    ...overrides,
  };
}

describe('hunkExtent', () => {
  const boxes = [box(0, 40), box(50, 30), box(90, 60), box(160, 20)];

  it('runs from the top of the first unit to the bottom of the last', () => {
    expect(hunkExtent(hunk({ currentIndices: [1, 2] }), 'current', boxes)).toEqual(box(50, 100));
    expect(hunkExtent(hunk({ previousIndices: [3] }), 'previous', boxes)).toEqual(box(160, 20));
  });

  it('is the gap above the next unit on a side the hunk has no unit on', () => {
    const removed = hunk({ kind: 'removed', previousIndices: [1], currentStart: 2 });

    expect(hunkExtent(removed, 'current', boxes)).toEqual(box(90, 0));
  });

  it('is the gap under the last unit when the hunk is at the end', () => {
    const removed = hunk({ kind: 'removed', previousIndices: [4], currentStart: 4 });

    expect(hunkExtent(removed, 'current', boxes)).toEqual(box(180, 0));
  });

  it('is nowhere on a side with nothing measured', () => {
    expect(hunkExtent(hunk({ kind: 'removed', previousIndices: [0] }), 'current', [])).toBeNull();
    expect(hunkExtent(hunk({ currentIndices: [7] }), 'current', boxes)).toBeNull();
  });
});

describe('hunkGapOffset', () => {
  // 10px between blocks, as margins leave.
  const boxes = [box(20, 40), box(70, 30), box(110, 60)];

  it('is the middle of the gap between the units either side', () => {
    const removed = hunk({ kind: 'removed', previousIndices: [1], currentStart: 2 });

    expect(hunkGapOffset(removed, 'current', boxes)).toBe(105);
  });

  it('is just above the first unit, and just under the last', () => {
    const first = hunk({ kind: 'removed', previousIndices: [0], currentStart: 0 });
    const last = hunk({ kind: 'removed', previousIndices: [4], currentStart: 3 });

    expect(hunkGapOffset(first, 'current', boxes)).toBe(16);
    expect(hunkGapOffset(last, 'current', boxes)).toBe(174);
  });

  it('reads the side asked for', () => {
    const added = hunk({ kind: 'added', currentIndices: [1], previousStart: 1, currentStart: 1 });

    expect(hunkGapOffset(added, 'previous', boxes)).toBe(65);
  });

  it('is nowhere on a side with nothing measured', () => {
    expect(
      hunkGapOffset(hunk({ kind: 'removed', previousIndices: [0] }), 'current', [])
    ).toBeNull();
  });
});

describe('hunkMarkerBox', () => {
  const boxes = [box(20, 40), box(70, 30), box(110, 60), box(180, 1)];

  it('is as tall as the hunk’s blocks and level with them', () => {
    expect(hunkMarkerBox(hunk({ currentIndices: [1] }), 'current', boxes)).toEqual(box(70, 30));
    expect(hunkMarkerBox(hunk({ currentIndices: [0, 1, 2] }), 'current', boxes)).toEqual(
      box(20, 150)
    );
    expect(hunkMarkerBox(hunk({ previousIndices: [2] }), 'previous', boxes)).toEqual(box(110, 60));
  });

  it('is a short marker centred on the gap line where the side has no block', () => {
    const removed = hunk({ kind: 'removed', previousIndices: [1], currentStart: 2 });

    const marker = hunkMarkerBox(removed, 'current', boxes)!;
    expect(marker.height).toBe(GAP_MARKER_HEIGHT);
    expect(marker.top + marker.height / 2).toBe(hunkGapOffset(removed, 'current', boxes));
  });

  it('keeps a marker tall enough to see beside a block that is not, centred on it', () => {
    expect(hunkMarkerBox(hunk({ currentIndices: [3] }), 'current', boxes)).toEqual(box(178.5, 4));
  });

  it('is nowhere on a side with nothing measured', () => {
    expect(hunkMarkerBox(hunk({ currentIndices: [7] }), 'current', boxes)).toBeNull();
    expect(
      hunkMarkerBox(hunk({ kind: 'removed', previousIndices: [0] }), 'current', [])
    ).toBeNull();
  });
});

describe('hunkLead', () => {
  // The previous version has a 300px paragraph the current one lacks, with a
  // shared block either side and 16px between blocks.
  const previousBoxes = [box(0, 1000), box(1016, 300), box(1332, 100), box(1448, 2000)];
  const currentBoxes = [box(0, 1000), box(1016, 100), box(1132, 2000)];
  const boxes = { previous: previousBoxes, current: currentBoxes };
  const removed = hunk({
    kind: 'removed',
    ops: [{ type: 'removed', previousIndex: 1 }],
    previousIndices: [1],
    previousStart: 1,
    currentStart: 1,
  });

  it('places a change from the note where the note has its first block', () => {
    const added = hunk({
      kind: 'added',
      ops: [{ type: 'added', currentIndex: 1 }],
      currentIndices: [1],
      previousStart: 2,
      currentStart: 1,
    });
    expect(hunkLead(added, boxes)).toEqual({ side: 'current', offset: 1016 });

    const reworded = hunk({
      ops: [
        {
          type: 'modified',
          previousIndex: 2,
          currentIndex: 1,
          previousRanges: [],
          currentRanges: [],
          wholeUnit: true,
        },
      ],
      previousIndices: [2],
      currentIndices: [1],
      previousStart: 2,
      currentStart: 1,
    });
    expect(hunkLead(reworded, boxes)).toEqual({ side: 'current', offset: 1016 });
  });

  it('places a removal from the previous version, at its first block', () => {
    expect(hunkLead(removed, boxes)).toEqual({ side: 'previous', offset: 1016 });
  });

  it('places a change that starts with a removal from the previous version', () => {
    const replaced = hunk({
      ops: [
        { type: 'removed', previousIndex: 1 },
        { type: 'added', currentIndex: 1 },
      ],
      previousIndices: [1],
      currentIndices: [1],
      previousStart: 1,
      currentStart: 1,
    });
    expect(hunkLead(replaced, boxes)).toEqual({ side: 'previous', offset: 1016 });
  });

  it('places a removal from the note, at the gap, while the previous version is not measured', () => {
    expect(hunkLead(removed, { previous: [], current: currentBoxes })).toEqual({
      side: 'current',
      offset: 1016,
    });
  });

  it('places nothing with neither side measured', () => {
    expect(hunkLead(removed, { previous: [], current: [] })).toBeNull();
  });

  it('starts removed text on the previous pane’s line, the note following to the gap', () => {
    const anchors = buildScrollAnchors(
      [
        { type: 'equal', previousIndex: 0, currentIndex: 0 },
        { type: 'removed', previousIndex: 1 },
        { type: 'equal', previousIndex: 2, currentIndex: 1 },
        { type: 'equal', previousIndex: 3, currentIndex: 2 },
      ],
      previousBoxes,
      currentBoxes
    );
    const previous = pane(0, 3448, 600);
    const current = pane(0, 3132, 600);
    const line = 600 * REFERENCE_LINE_FRACTION;

    const lead = hunkLead(removed, boxes)!;
    previous.scrollTop = scrollTopForOffset(lead.offset, previous);
    current.scrollTop = syncedScrollTop(anchors, 'previous', previous, current);

    // The removed paragraph starts on the line and runs down the pane from
    // it, where the note has the gap it left: between the block before it,
    // which ends 1000px down the note, and the one after, 1016px down.
    expect(previousBoxes[1].top - previous.scrollTop).toBe(line);
    expect(current.scrollTop + line).toBeGreaterThan(1000);
    expect(current.scrollTop + line).toBeLessThan(1016);
  });
});

describe('buildScrollAnchors', () => {
  const ops: NoteDiffOp[] = [
    { type: 'equal', previousIndex: 0, currentIndex: 0 },
    { type: 'added', currentIndex: 1 },
    {
      type: 'modified',
      previousIndex: 1,
      currentIndex: 2,
      previousRanges: [],
      currentRanges: [],
      wholeUnit: true,
    },
    { type: 'removed', previousIndex: 2 },
  ];

  it('anchors the top and bottom of every block both versions hold', () => {
    const previous = [box(0, 40), box(50, 30), box(90, 60)];
    const current = [box(0, 40), box(50, 200), box(260, 80)];

    expect(buildScrollAnchors(ops, previous, current)).toEqual([
      { previous: 0, current: 0 },
      { previous: 40, current: 40 },
      { previous: 50, current: 260 },
      { previous: 80, current: 340 },
    ]);
  });

  it('leaves out an anchor that steps back in either pane', () => {
    const previous = [box(100, 40), box(50, 30), box(90, 60)];
    const current = [box(0, 40), box(50, 200), box(260, 80)];

    expect(buildScrollAnchors(ops, previous, current)).toEqual([
      { previous: 100, current: 0 },
      { previous: 140, current: 40 },
    ]);
  });

  it('skips a block with no box', () => {
    expect(buildScrollAnchors(ops, [box(0, 40)], [box(0, 40)])).toEqual([
      { previous: 0, current: 0 },
      { previous: 40, current: 40 },
    ]);
  });
});

describe('mapScrollOffset', () => {
  // A shared block at the top; 300px only the current version has; a shared
  // block that is taller in the previous version; then the end.
  const anchors: NoteDiffScrollAnchor[] = [
    { previous: 0, current: 0 },
    { previous: 100, current: 100 },
    { previous: 100, current: 400 },
    { previous: 300, current: 500 },
  ];
  const heights = { previous: 400, current: 700 };

  it('keeps a shared block level', () => {
    expect(mapScrollOffset(anchors, 60, 'current', heights)).toBe(60);
    expect(mapScrollOffset(anchors, 60, 'previous', heights)).toBe(60);
  });

  it('holds the other pane still through text only one version has', () => {
    expect(mapScrollOffset(anchors, 150, 'current', heights)).toBe(100);
    expect(mapScrollOffset(anchors, 399, 'current', heights)).toBe(100);
  });

  it('interpolates through a block whose height differs', () => {
    expect(mapScrollOffset(anchors, 450, 'current', heights)).toBe(200);
    expect(mapScrollOffset(anchors, 200, 'previous', heights)).toBe(450);
  });

  it('lands past text only the other version has when scrolled the other way', () => {
    // Just under the first block's bottom is just under it in the other
    // pane; at it, the previous pane has reached the next shared block.
    expect(mapScrollOffset(anchors, 99, 'previous', heights)).toBe(99);
    expect(mapScrollOffset(anchors, 100, 'previous', heights)).toBe(400);
  });

  it('maps the text after the last shared block in proportion', () => {
    expect(mapScrollOffset(anchors, 600, 'current', heights)).toBe(350);
    expect(mapScrollOffset(anchors, 350, 'previous', heights)).toBe(600);
  });

  it('maps the text before the first shared block in proportion', () => {
    const later: NoteDiffScrollAnchor[] = [{ previous: 100, current: 300 }];

    expect(mapScrollOffset(later, 150, 'current', heights)).toBe(50);
    expect(mapScrollOffset(later, 50, 'previous', heights)).toBe(150);
  });

  it('stays within the other content at both ends', () => {
    expect(mapScrollOffset(anchors, -40, 'current', heights)).toBe(0);
    expect(mapScrollOffset(anchors, 0, 'current', heights)).toBe(0);
    expect(mapScrollOffset(anchors, 700, 'current', heights)).toBe(400);
    expect(mapScrollOffset(anchors, 5000, 'current', heights)).toBe(400);
    expect(mapScrollOffset(anchors, 5000, 'previous', heights)).toBe(700);
  });

  it('ignores an anchor that lies behind the one before it', () => {
    const folded: NoteDiffScrollAnchor[] = [
      { previous: 200, current: 200 },
      { previous: 150, current: 300 },
    ];

    expect(mapScrollOffset(folded, 250, 'current', heights)).toBe(220);
  });
});

describe('scrollTopForOffset', () => {
  it('puts the offset a third of the way down the pane', () => {
    expect(scrollTopForOffset(500, pane(0, 2000))).toBe(400);
  });

  it('stops at the top and at the bottom of what the pane can scroll', () => {
    expect(scrollTopForOffset(40, pane(0, 2000))).toBe(0);
    expect(scrollTopForOffset(1990, pane(0, 2000))).toBe(1700);
    expect(scrollTopForOffset(500, pane(0, 200))).toBe(0);
  });
});

describe('syncedScrollTop', () => {
  const anchors: NoteDiffScrollAnchor[] = [
    { previous: 0, current: 0 },
    { previous: 100, current: 100 },
    { previous: 100, current: 400 },
    { previous: 1100, current: 1400 },
  ];

  it('keeps a shared block on the reference line of both panes', () => {
    // The current pane's line is at 700 + 100: 400 into the second shared block.
    const top = syncedScrollTop(anchors, 'current', pane(700, 1500), pane(0, 1200));

    expect(top).toBe(400);
    // And back: the previous pane's line at 500 is the same point.
    expect(syncedScrollTop(anchors, 'previous', pane(400, 1200), pane(0, 1500))).toBe(700);
  });

  it('holds the other pane while this one scrolls through text only it has', () => {
    const at = (scrollTop: number) =>
      syncedScrollTop(anchors, 'current', pane(scrollTop, 1500), pane(0, 1200));

    expect(at(100)).toBe(0);
    expect(at(250)).toBe(0);
  });

  it('does not scroll the other pane past its top or its bottom', () => {
    expect(syncedScrollTop(anchors, 'current', pane(0, 1500), pane(50, 1200))).toBe(0);
    expect(syncedScrollTop(anchors, 'current', pane(1200, 1500), pane(0, 1200))).toBe(900);
  });

  it('scrolls in proportion when the versions share no block', () => {
    expect(syncedScrollTop([], 'current', pane(300, 1500), pane(0, 900))).toBe(150);
    expect(syncedScrollTop([], 'previous', pane(600, 900), pane(0, 1500))).toBe(1200);
  });

  it('leaves the other pane at the top when this one cannot scroll', () => {
    expect(syncedScrollTop([], 'current', pane(0, 200), pane(40, 900))).toBe(0);
  });
});
