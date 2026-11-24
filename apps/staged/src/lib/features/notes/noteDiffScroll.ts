/**
 * Where a note diff's blocks are, and how two panes showing the two
 * versions scroll together.
 *
 * Everything here is arithmetic over measured boxes (noteDiffUnits.ts takes
 * the measurements). Offsets are distances down a pane's scrolled content,
 * the coordinate `scrollTop` moves in.
 *
 * The panes are aligned by mapping, not by padding one out to the other's
 * height. Every block both versions hold is a pair of anchors, its top and
 * its bottom in each pane, and an offset in one pane maps to the other by
 * interpolating between the anchors around it. The line kept level is the
 * reference line, a third of the way down each pane: a block there in one
 * pane is there in the other, however different the text above it. Through a
 * stretch only one version has, the other pane holds still, so a change is
 * scrolled to from the pane that has its first block (`hunkLead`).
 *
 * What is drawn for a change in a version is placed in the same coordinate,
 * inside the element that scrolls it, so it moves with the text: the block
 * painted behind the change's text (`hunkBlock`), the line where the other
 * version has text this one has not (`hunkGapOffset`), and the marker beside
 * either (`hunkMarkerBox`).
 */

import {
  changeUnitIndex,
  type NoteDiffBlockKind,
  type NoteDiffHunk,
  type NoteDiffOp,
  type NoteDiffSide,
} from './noteDiff';

/** A unit's place in its pane's content. */
export interface NoteDiffUnitBox {
  top: number;
  height: number;
}

/** The same point of the note in each pane's content. */
export interface NoteDiffScrollAnchor {
  previous: number;
  current: number;
}

/** A pane's scroll state, as its element reports it. */
export interface NoteDiffPaneScroll {
  scrollTop: number;
  clientHeight: number;
  scrollHeight: number;
}

/** How far down a pane the line that is kept level sits, as a share of its height. */
export const REFERENCE_LINE_FRACTION = 1 / 3;

/**
 * Where a hunk is on one side: from the top of its first unit there to the
 * bottom of its last. A hunk with no unit on that side is the gap its blocks
 * would fill, a box of no height at the top of the unit after it, or under
 * the last unit when it is at the end. `null` when the side has nothing
 * measured to place it by.
 */
export function hunkExtent(
  hunk: NoteDiffHunk,
  side: NoteDiffSide,
  boxes: readonly NoteDiffUnitBox[]
): NoteDiffUnitBox | null {
  const indices = side === 'previous' ? hunk.previousIndices : hunk.currentIndices;
  if (indices.length > 0) {
    const first = boxes[indices[0]];
    const last = boxes[indices[indices.length - 1]];
    if (!first || !last) return null;
    return { top: first.top, height: Math.max(0, last.top + last.height - first.top) };
  }

  const start = side === 'previous' ? hunk.previousStart : hunk.currentStart;
  const next = boxes[start];
  if (next) return { top: next.top, height: 0 };
  const last = boxes[boxes.length - 1];
  return last ? { top: last.top + last.height, height: 0 } : null;
}

/** A stretch of a change's block that is one kind of change, and so one colour. */
export interface NoteDiffBlockPart {
  kind: NoteDiffBlockKind;
  height: number;
}

/** The one rectangle painted behind a change's text on a side. */
export interface NoteDiffBlock extends NoteDiffUnitBox {
  /** Its stretches from the top down, which fill it: their heights add up to its own. */
  parts: NoteDiffBlockPart[];
}

/**
 * How far a change's rectangle reaches past its first and its last block,
 * into the margin there: a heading's box ends at its letters, and a
 * rectangle cut off at them reads as cramped.
 */
export const BLOCK_BLEED = 4;

/**
 * How far a rectangle reaches into the room between its end block and the
 * block next to it: `BLOCK_BLEED`, and never past half-way, so it stays off
 * the neighbour. Blocks with nothing between them (the rows of a table)
 * leave no room, and the rectangle ends at the block's own edge. At either
 * end of the note there is no neighbour to stay off.
 */
function blockBleed(edge: number, neighbourEdge: number | undefined): number {
  if (neighbourEdge === undefined) return BLOCK_BLEED;
  return Math.min(BLOCK_BLEED, Math.max(0, Math.abs(edge - neighbourEdge) / 2));
}

/**
 * The box of the rectangle painted behind a hunk's text on one side: from
 * the top of its first block there to the bottom of its last
 * (`hunkExtent`), and a little into the margin above and below
 * (`BLOCK_BLEED`). `null` for a hunk with no block on that side, and when
 * the side is not measured.
 *
 * This is the one box everything drawn for the change on that side shares:
 * the rectangle (`hunkBlock`), the marker at its end (`hunkMarkerBox`) and
 * the end of the band that joins it to the other version
 * (`hunkConnectorSpan` in noteDiffConnectors.ts).
 */
export function hunkBlockExtent(
  hunk: NoteDiffHunk,
  side: NoteDiffSide,
  boxes: readonly NoteDiffUnitBox[]
): NoteDiffUnitBox | null {
  const indices = side === 'previous' ? hunk.previousIndices : hunk.currentIndices;
  if (indices.length === 0) return null;
  const extent = hunkExtent(hunk, side, boxes);
  if (!extent) return null;
  const before = boxes[indices[0] - 1];
  const after = boxes[indices[indices.length - 1] + 1];
  const bottom = extent.top + extent.height;
  const top = extent.top - blockBleed(extent.top, before && before.top + before.height);
  return { top, height: bottom + blockBleed(bottom, after?.top) - top };
}

/**
 * The rectangle to paint behind a hunk's text on one side
 * (`hunkBlockExtent`): whole, so the margins between its blocks are painted
 * with them. `null` for a hunk with no block on that side, which has a line
 * instead (`hunkGapOffset`), and when the side is not measured.
 *
 * Each block keeps the colour of its own change (see `NOTE_DIFF_ATTRIBUTE`),
 * so the rectangle is cut into parts, one per run of blocks of a kind. Two
 * runs meet half-way across the margin between them.
 */
export function hunkBlock(
  hunk: NoteDiffHunk,
  side: NoteDiffSide,
  boxes: readonly NoteDiffUnitBox[]
): NoteDiffBlock | null {
  const extent = hunkBlockExtent(hunk, side, boxes);
  if (!extent) return null;

  const bottom = extent.top + extent.height;
  const parts: NoteDiffBlockPart[] = [];
  /** Where the part being filled started, and the bottom of the last block in it. */
  let partTop = extent.top;
  let lastBottom = extent.top;
  for (const op of hunk.ops) {
    const index = changeUnitIndex(op, side);
    const box = index === null ? undefined : boxes[index];
    if (!box) continue;
    const open = parts[parts.length - 1];
    if (!open) {
      parts.push({ kind: op.type, height: 0 });
    } else if (open.kind !== op.type) {
      const seam = Math.min(Math.max((lastBottom + box.top) / 2, partTop), bottom);
      open.height = seam - partTop;
      partTop = seam;
      parts.push({ kind: op.type, height: 0 });
    }
    lastBottom = box.top + box.height;
  }
  const last = parts[parts.length - 1];
  if (!last) return null;
  last.height = bottom - partTop;
  return { ...extent, parts };
}

/** How far from a version's first or last block a gap at that end is drawn. */
const GAP_EDGE_OFFSET = 4;

/** The height of the marker of a hunk with no block on a side: a tick across its gap line. */
export const GAP_MARKER_HEIGHT = 8;

/** The least a marker is tall, for a block shorter than that (a rule). */
export const MIN_MARKER_HEIGHT = 4;

/**
 * Where the line for a hunk with no unit on a side is drawn: the middle of
 * the gap its blocks would fill, between the units either side of it, or
 * just off the one there is at an end of the version. `null` when the side
 * has nothing measured to place it by.
 */
export function hunkGapOffset(
  hunk: NoteDiffHunk,
  side: NoteDiffSide,
  boxes: readonly NoteDiffUnitBox[]
): number | null {
  const start = side === 'previous' ? hunk.previousStart : hunk.currentStart;
  const before = boxes[start - 1];
  const after = boxes[start];
  if (before && after) return (before.top + before.height + after.top) / 2;
  if (after) return after.top - GAP_EDGE_OFFSET;
  if (before) return before.top + before.height + GAP_EDGE_OFFSET;
  return null;
}

/**
 * Where a hunk's marker stands beside one side's text: at the end of the
 * rectangle painted behind the hunk's blocks there, and as tall as it
 * (`hunkBlockExtent`). A hunk with no block on that side gets a short
 * marker centred on its gap line (`hunkGapOffset`). `null` when there is
 * nothing measured to place it by.
 */
export function hunkMarkerBox(
  hunk: NoteDiffHunk,
  side: NoteDiffSide,
  boxes: readonly NoteDiffUnitBox[]
): NoteDiffUnitBox | null {
  const indices = side === 'previous' ? hunk.previousIndices : hunk.currentIndices;
  if (indices.length === 0) {
    const gap = hunkGapOffset(hunk, side, boxes);
    return gap === null ? null : { top: gap - GAP_MARKER_HEIGHT / 2, height: GAP_MARKER_HEIGHT };
  }
  const extent = hunkBlockExtent(hunk, side, boxes);
  if (!extent || extent.height >= MIN_MARKER_HEIGHT) return extent;
  // Too short to see or press: kept centred on the block.
  return {
    top: extent.top - (MIN_MARKER_HEIGHT - extent.height) / 2,
    height: MIN_MARKER_HEIGHT,
  };
}

/** Where a hunk is put on a pane's reference line: the pane, and the offset in it. */
export interface NoteDiffHunkLead {
  side: NoteDiffSide;
  offset: number;
}

/**
 * The pane a hunk is placed from when it is scrolled to, and the offset
 * that goes on that pane's reference line. The other pane follows through
 * the scroll map.
 *
 * A hunk is placed from the note, at its first block there, unless it
 * starts with blocks only the previous version has. On the note's side
 * those are a gap of no height, and the map sends a gap to the block after
 * it: the previous pane would show where the removed text ends, with its
 * start above the line and out of view when it is tall. Such a hunk is
 * placed from the previous version instead, its first block on that pane's
 * line, and the note follows to the gap.
 *
 * The previous version is measured only while it is shown. Without it the
 * hunk is placed from the note. `null` when the side it would be placed
 * from has nothing measured to place it by.
 */
export function hunkLead(
  hunk: NoteDiffHunk,
  boxes: Record<NoteDiffSide, readonly NoteDiffUnitBox[]>
): NoteDiffHunkLead | null {
  if (hunk.ops[0]?.type === 'removed') {
    const extent = hunkExtent(hunk, 'previous', boxes.previous);
    if (extent) return { side: 'previous', offset: extent.top };
  }
  const extent = hunkExtent(hunk, 'current', boxes.current);
  return extent ? { side: 'current', offset: extent.top } : null;
}

/**
 * The anchors of a diff: the top and the bottom of every block both
 * versions hold, unchanged or modified.
 *
 * The mapping needs them to rise together. Blocks are laid out in order, so
 * they do; an anchor that would step back in either pane (a layout that
 * breaks the order, a box not yet measured) is left out rather than fold
 * the map over.
 */
export function buildScrollAnchors(
  ops: readonly NoteDiffOp[],
  previousBoxes: readonly NoteDiffUnitBox[],
  currentBoxes: readonly NoteDiffUnitBox[]
): NoteDiffScrollAnchor[] {
  const anchors: NoteDiffScrollAnchor[] = [];
  const add = (anchor: NoteDiffScrollAnchor) => {
    const last = anchors[anchors.length - 1];
    if (last && (anchor.previous < last.previous || anchor.current < last.current)) return;
    anchors.push(anchor);
  };
  for (const op of ops) {
    if (op.type !== 'equal' && op.type !== 'modified') continue;
    const previous = previousBoxes[op.previousIndex];
    const current = currentBoxes[op.currentIndex];
    if (!previous || !current) continue;
    add({ previous: previous.top, current: current.top });
    add({ previous: previous.top + previous.height, current: current.top + current.height });
  }
  return anchors;
}

/**
 * The offset in the other pane that shows the same point of the note as
 * `offset` does in the `from` pane.
 *
 * The two contents' tops and bottoms are anchors too, so text before the
 * first shared block and after the last still maps, in proportion.
 */
export function mapScrollOffset(
  anchors: readonly NoteDiffScrollAnchor[],
  offset: number,
  from: NoteDiffSide,
  contentHeights: NoteDiffScrollAnchor
): number {
  const to: NoteDiffSide = from === 'previous' ? 'current' : 'previous';
  let below: NoteDiffScrollAnchor = { previous: 0, current: 0 };
  if (offset <= 0) return 0;
  for (const anchor of [...anchors, contentHeights]) {
    // Behind the last anchor in either pane: using it would fold the map over.
    if (anchor[from] < below[from] || anchor[to] < below[to]) continue;
    if (anchor[from] > offset) {
      const share = (offset - below[from]) / (anchor[from] - below[from]);
      return below[to] + share * (anchor[to] - below[to]);
    }
    below = anchor;
  }
  return below[to];
}

function maxScrollTop(pane: NoteDiffPaneScroll): number {
  return Math.max(0, pane.scrollHeight - pane.clientHeight);
}

/** The `scrollTop` that puts `offset` on a pane's reference line, as near as it can scroll. */
export function scrollTopForOffset(offset: number, pane: NoteDiffPaneScroll): number {
  const top = offset - pane.clientHeight * REFERENCE_LINE_FRACTION;
  return Math.min(Math.max(0, top), maxScrollTop(pane));
}

/**
 * Where the `target` pane scrolls to follow the `source` pane, which shows
 * the `from` version.
 *
 * With no shared block there is nothing to keep level, and the panes scroll
 * in proportion instead: each as far through its own content as the other.
 */
export function syncedScrollTop(
  anchors: readonly NoteDiffScrollAnchor[],
  from: NoteDiffSide,
  source: NoteDiffPaneScroll,
  target: NoteDiffPaneScroll
): number {
  if (anchors.length === 0) {
    const sourceMax = maxScrollTop(source);
    return sourceMax > 0 ? (source.scrollTop / sourceMax) * maxScrollTop(target) : 0;
  }
  const heights =
    from === 'previous'
      ? { previous: source.scrollHeight, current: target.scrollHeight }
      : { previous: target.scrollHeight, current: source.scrollHeight };
  const line = source.scrollTop + source.clientHeight * REFERENCE_LINE_FRACTION;
  return scrollTopForOffset(mapScrollOffset(anchors, line, from, heights), target);
}
