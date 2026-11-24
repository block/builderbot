import { describe, expect, it } from 'vitest';

import type { NoteDiffHunk } from './noteDiff';
import {
  clipConnectorEdges,
  connectorLineEdges,
  CONNECTOR_WIDTH,
  hunkConnectorEdges,
  hunkConnectorSpan,
  layoutNoteDiffConnectors,
} from './noteDiffConnectors';
import { BLOCK_BLEED, hunkBlock, hunkGapOffset, type NoteDiffUnitBox } from './noteDiffScroll';

function box(top: number, height: number): NoteDiffUnitBox {
  return { top, height };
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

/** The numbers of a path, in the order it is written. */
function numbers(d: string): number[] {
  return (d.match(/-?\d+(?:\.\d+)?/g) ?? []).map(Number);
}

// Blocks 10px apart, as margins leave. The previous version has one block
// more than the note, the second, which the revision removed. The rectangle
// painted behind a change reaches 4px into the margin above and below it
// (`BLOCK_BLEED`), and a band's ends are that rectangle's.
const previous = [box(20, 40), box(70, 30), box(110, 60), box(180, 50), box(240, 20)];
const current = [box(20, 40), box(70, 80), box(160, 50), box(220, 20), box(250, 90)];
const boxes = { previous, current };

const removed = hunk({ id: 'removed', kind: 'removed', previousIndices: [1], currentStart: 1 });
const modified = hunk({
  id: 'modified',
  kind: 'modified',
  previousIndices: [2],
  currentIndices: [1],
  previousStart: 2,
  currentStart: 1,
});
const added = hunk({ id: 'added', kind: 'added', currentIndices: [4], previousStart: 5 });
const NOT_SCROLLED = { previous: 0, current: 0 };

describe('hunkConnectorSpan', () => {
  it('runs from just above the change’s first block to just below its last', () => {
    expect(hunkConnectorSpan(modified, 'previous', previous)).toEqual({
      top: 110 - BLOCK_BLEED,
      bottom: 170 + BLOCK_BLEED,
    });
    expect(hunkConnectorSpan(hunk({ currentIndices: [1, 2, 3] }), 'current', current)).toEqual({
      top: 70 - BLOCK_BLEED,
      bottom: 240 + BLOCK_BLEED,
    });
  });

  it('is a point at the gap line on a side with no block of the change', () => {
    const gap = hunkGapOffset(removed, 'current', current);

    expect(gap).toBe(65);
    expect(hunkConnectorSpan(removed, 'current', current)).toEqual({ top: 65, bottom: 65 });
  });

  it('has the top and the bottom of the rectangle painted behind the change', () => {
    const section = hunk({
      currentIndices: [1, 2, 3],
      ops: [
        { type: 'added', currentIndex: 1 },
        { type: 'added', currentIndex: 2 },
        { type: 'added', currentIndex: 3 },
      ],
    });
    const block = hunkBlock(section, 'current', current)!;
    const span = hunkConnectorSpan(section, 'current', current)!;

    expect(span.top).toBe(block.top);
    expect(span.bottom).toBe(block.top + block.height);
  });

  it('is nowhere on a side with nothing measured', () => {
    expect(hunkConnectorSpan(modified, 'previous', [])).toBeNull();
    expect(hunkConnectorSpan(removed, 'current', [])).toBeNull();
  });
});

describe('connectorLineEdges', () => {
  it('runs half a pixel inside the band’s edges, where a 1px line lies within the change', () => {
    expect(
      connectorLineEdges({ beforeTop: 110, beforeBottom: 170, afterTop: 70, afterBottom: 150 })
    ).toEqual({ beforeTop: 110.5, beforeBottom: 169.5, afterTop: 70.5, afterBottom: 149.5 });
  });

  it('leaves a side that is a point where it is, and makes one of a side too short to line', () => {
    expect(
      connectorLineEdges({ beforeTop: 65, beforeBottom: 65, afterTop: 30, afterBottom: 30.4 })
    ).toEqual({ beforeTop: 65, beforeBottom: 65, afterTop: 30.2, afterBottom: 30.2 });
  });
});

describe('hunkConnectorEdges', () => {
  it('puts the previous version’s extent on the left and the note’s on the right', () => {
    expect(hunkConnectorEdges(modified, boxes, NOT_SCROLLED)).toEqual({
      beforeTop: 106,
      beforeBottom: 174,
      afterTop: 66,
      afterBottom: 154,
    });
  });

  it('moves each side by how far its own pane is scrolled', () => {
    expect(hunkConnectorEdges(modified, boxes, { previous: 100, current: 30 })).toEqual({
      beforeTop: 6,
      beforeBottom: 74,
      afterTop: 36,
      afterBottom: 124,
    });
  });

  it('collapses the note’s side of a removal, and the previous version’s side of an addition', () => {
    const removal = hunkConnectorEdges(removed, boxes, NOT_SCROLLED)!;
    expect([removal.beforeTop, removal.beforeBottom]).toEqual([66, 104]);
    expect([removal.afterTop, removal.afterBottom]).toEqual([65, 65]);

    const addition = hunkConnectorEdges(added, boxes, NOT_SCROLLED)!;
    // Under the previous version's last block, where its gap line is drawn.
    expect([addition.beforeTop, addition.beforeBottom]).toEqual([264, 264]);
    // The note ends with the added block: the rectangle runs its full reach past it.
    expect([addition.afterTop, addition.afterBottom]).toEqual([246, 344]);
  });

  it('is not placed until both panes are measured', () => {
    expect(hunkConnectorEdges(modified, { previous: [], current }, NOT_SCROLLED)).toBeNull();
    expect(hunkConnectorEdges(modified, { previous, current: [] }, NOT_SCROLLED)).toBeNull();
  });
});

describe('clipConnectorEdges', () => {
  const HEIGHT = 300;
  const edges = (
    beforeTop: number,
    beforeBottom: number,
    afterTop: number,
    afterBottom: number
  ) => ({
    beforeTop,
    beforeBottom,
    afterTop,
    afterBottom,
  });

  it('leaves a band that is in the gutter as it is', () => {
    expect(clipConnectorEdges(edges(10, 50, 30, 90), HEIGHT)).toEqual(edges(10, 50, 30, 90));
  });

  it('drops a band that is wholly above the gutter, or wholly below it', () => {
    expect(clipConnectorEdges(edges(-80, -40, -60, -10), HEIGHT)).toBeNull();
    expect(clipConnectorEdges(edges(310, 350, 330, 400), HEIGHT)).toBeNull();
    // Touching the edge shows nothing either.
    expect(clipConnectorEdges(edges(-80, -40, -60, 0), HEIGHT)).toBeNull();
    expect(clipConnectorEdges(edges(300, 350, 330, 400), HEIGHT)).toBeNull();
  });

  it('keeps a band with one side scrolled out, which leads off the edge', () => {
    // The previous version's side is above the gutter, the note's is in it.
    expect(clipConnectorEdges(edges(-120, -60, 20, 80), HEIGHT)).toEqual(edges(-120, -60, 20, 80));
    // And the other way up, off the bottom.
    expect(clipConnectorEdges(edges(250, 290, 340, 420), HEIGHT)).toEqual(
      edges(250, 290, 340, 420)
    );
  });

  it('keeps a band that covers the gutter from above it to below it', () => {
    expect(clipConnectorEdges(edges(-50, 400, -20, 500), HEIGHT)).toEqual(
      edges(-50, 400, -20, 500)
    );
  });

  it('holds a side that is far off one gutter height out', () => {
    expect(clipConnectorEdges(edges(-5000, -4000, 20, 80), HEIGHT)).toEqual(
      edges(-HEIGHT, -HEIGHT, 20, 80)
    );
    expect(clipConnectorEdges(edges(100, 200, 4000, 9000), HEIGHT)).toEqual(
      edges(100, 200, 2 * HEIGHT, 2 * HEIGHT)
    );
  });

  it('holds a side continuously: just inside the limit it is where it was', () => {
    expect(clipConnectorEdges(edges(-299, -250, 20, 80), HEIGHT)).toEqual(
      edges(-299, -250, 20, 80)
    );
    expect(clipConnectorEdges(edges(-301, -250, 20, 80), HEIGHT)).toEqual(
      edges(-300, -250, 20, 80)
    );
  });

  it('has nothing to show in a gutter with no height', () => {
    expect(clipConnectorEdges(edges(10, 50, 30, 90), 0)).toBeNull();
  });
});

describe('layoutNoteDiffConnectors', () => {
  const viewport = (previousTop = 0, currentTop = 0, height = 400) => ({
    scrollTops: { previous: previousTop, current: currentTop },
    width: CONNECTOR_WIDTH,
    height,
  });
  const hunks = [removed, modified, added];

  it('draws one band per change, in order, named and coloured for its hunk', () => {
    const connectors = layoutNoteDiffConnectors(hunks, boxes, viewport());

    expect(connectors.map(({ hunkId, kind }) => [hunkId, kind])).toEqual([
      ['removed', 'removed'],
      ['modified', 'modified'],
      ['added', 'added'],
    ]);
  });

  it('spans each change’s extent in the previous version on the left and in the note on the right', () => {
    const [, band] = layoutNoteDiffConnectors(hunks, boxes, viewport());

    // "M 0 beforeTop C … width afterTop L width afterBottom C … 0 beforeBottom Z"
    const fill = numbers(band.fill);
    expect(fill.slice(0, 2)).toEqual([0, 106]);
    expect(fill.slice(6, 8)).toEqual([CONNECTOR_WIDTH, 66]);
    expect(fill.slice(8, 10)).toEqual([CONNECTOR_WIDTH, 154]);
    expect(fill.slice(-2)).toEqual([0, 174]);
    expect(band.fill.endsWith('Z')).toBe(true);
  });

  it('tapers a removal to a point at the note’s gap line', () => {
    const [band] = layoutNoteDiffConnectors(hunks, boxes, viewport());

    // No line down the right edge: the top curve arrives where the bottom one leaves.
    expect(band.fill).not.toContain('L');
    const fill = numbers(band.fill);
    expect(fill.slice(0, 2)).toEqual([0, 66]);
    expect(fill.slice(6, 8)).toEqual([CONNECTOR_WIDTH, 65]);
    expect(fill.slice(-2)).toEqual([0, 104]);
  });

  it('opens an addition from a point at the previous version’s gap line', () => {
    const [, , band] = layoutNoteDiffConnectors(hunks, boxes, viewport());

    const fill = numbers(band.fill);
    expect(fill.slice(0, 2)).toEqual([0, 264]);
    expect(fill.slice(6, 10)).toEqual([CONNECTOR_WIDTH, 246, CONNECTOR_WIDTH, 344]);
    expect(fill.slice(-2)).toEqual([0, 264]);
  });

  it('gives each end the kind of the change’s blocks there, for a band between two colours', () => {
    const replaced = hunk({
      id: 'replaced',
      previousIndices: [2],
      currentIndices: [1],
      previousStart: 2,
      currentStart: 1,
      ops: [
        { type: 'removed', previousIndex: 2 },
        { type: 'added', currentIndex: 1 },
      ],
    });
    const connectors = layoutNoteDiffConnectors([removed, modified, replaced], boxes, viewport());

    expect(connectors.map(({ kind, before, after }) => [kind, before, after])).toEqual([
      ['removed', 'removed', 'removed'],
      ['modified', 'modified', 'modified'],
      ['modified', 'removed', 'added'],
    ]);
  });

  it('draws the lines along the top and the bottom of the same band, half a pixel inside it', () => {
    const [, band] = layoutNoteDiffConnectors(hunks, boxes, viewport());

    const stroke = numbers(band.stroke);
    expect(stroke.slice(0, 2)).toEqual([0, 106.5]);
    expect(stroke.slice(6, 8)).toEqual([CONNECTOR_WIDTH, 66.5]);
    expect(stroke.slice(8, 10)).toEqual([CONNECTOR_WIDTH, 153.5]);
    expect(stroke.slice(-2)).toEqual([0, 173.5]);
    expect(band.stroke).not.toContain('Z');
  });

  it('moves the left edges with the previous version’s scroll and the right edges with the note’s', () => {
    const at = (previousTop: number, currentTop: number) =>
      layoutNoteDiffConnectors(hunks, boxes, viewport(previousTop, currentTop)).find(
        ({ hunkId }) => hunkId === 'modified'
      )!.edges;

    expect(at(40, 0)).toEqual({
      beforeTop: 66,
      beforeBottom: 134,
      afterTop: 66,
      afterBottom: 154,
    });
    expect(at(40, 25)).toEqual({
      beforeTop: 66,
      beforeBottom: 134,
      afterTop: 41,
      afterBottom: 129,
    });
  });

  it('leaves out the changes scrolled out of both panes', () => {
    // Both panes 200px down, in a gutter 60px tall: only the addition at the end is in it.
    const connectors = layoutNoteDiffConnectors(hunks, boxes, viewport(200, 200, 60));

    expect(connectors.map(({ hunkId }) => hunkId)).toEqual(['added']);
  });

  it('keeps a change that is in one pane only, leading off the gutter’s edge', () => {
    // The note has scrolled the modified block out; the previous version still shows its.
    const connectors = layoutNoteDiffConnectors(hunks, boxes, viewport(100, 300, 100));
    const band = connectors.find(({ hunkId }) => hunkId === 'modified')!;

    expect(band.edges.beforeTop).toBe(6);
    expect(band.edges.afterBottom).toBeLessThan(0);
  });

  it('draws nothing until both versions are measured', () => {
    expect(layoutNoteDiffConnectors(hunks, { previous: [], current }, viewport())).toEqual([]);
    expect(layoutNoteDiffConnectors(hunks, { previous, current: [] }, viewport())).toEqual([]);
  });

  it('draws nothing in a gutter with no size', () => {
    expect(layoutNoteDiffConnectors(hunks, boxes, { ...viewport(), height: 0 })).toEqual([]);
    expect(layoutNoteDiffConnectors(hunks, boxes, { ...viewport(), width: 0 })).toEqual([]);
  });
});
