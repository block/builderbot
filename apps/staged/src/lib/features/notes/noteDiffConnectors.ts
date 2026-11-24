/**
 * The bands that join each change of a note diff across the gutter between
 * its two panes: a change's place in the previous version, on the left, to
 * its place in the note, on the right.
 *
 * They are the code diff viewer's spine connectors (DiffViewer.svelte in
 * packages/diff-viewer), and the outline is the viewer's own: the path of
 * a band comes from its `connectorGeometry.ts`, which its canvas renderer
 * draws from too. What is worked out here is where a band meets each pane.
 * The viewer counts that in lines of one height; a note's blocks are
 * measured (noteDiffUnits.ts), so a band's edge runs from the top of the
 * change's first block on a side to the bottom of its last. A side with no
 * block of the change (in the note, text the revision removed; in the
 * previous version, text it added) is a point at the line drawn across that
 * gap (`hunkGapOffset`), and the band tapers to it.
 *
 * The panes scroll separately, so a band is placed from both scroll offsets
 * and moves whenever either does. Its coordinates are the gutter's, whose
 * top is the top of both scroll areas and whose height is theirs. A band
 * that reaches past the gutter is cut off there by whatever draws it (an
 * `<svg>` clips to its box), so a change scrolled out of one pane leads off
 * the edge; `clipConnectorEdges` drops the bands with nothing in view and
 * keeps the rest from running arbitrarily far out.
 *
 * Nothing here touches the DOM.
 */

import {
  connectorFillPathData,
  connectorStrokePathData,
  type ConnectorEdges,
} from '@builderbot/diff-viewer/utils';

import type { NoteDiffHunk, NoteDiffHunkKind, NoteDiffSide } from './noteDiff';
import { hunkExtent, hunkGapOffset, type NoteDiffUnitBox } from './noteDiffScroll';

/**
 * How far the gutter reaches into the previous version's pane: to the right
 * edge of that pane's markers (NoteDiffMarkers.svelte stands them 8px from
 * the scroll area's edge), which a band starts from.
 */
export const CONNECTOR_PREVIOUS_REACH = 8;

/**
 * How far the gutter reaches into the note's pane: across its left padding
 * (24px) to the tint of a changed block, which is carried 3px past the text
 * (noteDiffStyles.css), where a band lands.
 */
export const CONNECTOR_CURRENT_REACH = 21;

/** The gutter's width. */
export const CONNECTOR_WIDTH = CONNECTOR_PREVIOUS_REACH + CONNECTOR_CURRENT_REACH;

/**
 * How far a band's edge is drawn inside its blocks' edge, so that the 1px
 * line along it lies within the change, as the viewer's does within its
 * lines.
 */
const EDGE_INSET = 0.5;

/** The gutter as it stands: how far each pane is scrolled, and its size. */
export interface NoteDiffConnectorViewport {
  scrollTops: Record<NoteDiffSide, number>;
  width: number;
  /** The visible height of the panes' scroll areas. */
  height: number;
}

/** One change's band, ready to draw. */
export interface NoteDiffConnector {
  hunkId: string;
  kind: NoteDiffHunkKind;
  /** Where the band meets each pane, in the gutter's coordinates. */
  edges: ConnectorEdges;
  /** SVG path data for the band's fill, and for the lines along its top and bottom. */
  fill: string;
  stroke: string;
}

/**
 * Where a band meets one side, as offsets in that pane's content: the
 * extent of the hunk's blocks there, or the one point of its gap line.
 * `null` when the side has nothing measured to place it by.
 */
export function hunkConnectorSpan(
  hunk: NoteDiffHunk,
  side: NoteDiffSide,
  boxes: readonly NoteDiffUnitBox[]
): { top: number; bottom: number } | null {
  const indices = side === 'previous' ? hunk.previousIndices : hunk.currentIndices;
  if (indices.length === 0) {
    const gap = hunkGapOffset(hunk, side, boxes);
    return gap === null ? null : { top: gap, bottom: gap };
  }
  const extent = hunkExtent(hunk, side, boxes);
  if (!extent) return null;
  // A block too short to hold the inset twice is a point at its middle.
  const inset = Math.min(EDGE_INSET, extent.height / 2);
  return { top: extent.top + inset, bottom: extent.top + extent.height - inset };
}

/**
 * Where a hunk's band meets each pane, in the gutter's coordinates: the
 * previous version's side moved by how far that pane is scrolled, and the
 * note's by how far the note is. `null` until both sides are measured.
 */
export function hunkConnectorEdges(
  hunk: NoteDiffHunk,
  boxes: Record<NoteDiffSide, readonly NoteDiffUnitBox[]>,
  scrollTops: Record<NoteDiffSide, number>
): ConnectorEdges | null {
  const before = hunkConnectorSpan(hunk, 'previous', boxes.previous);
  const after = hunkConnectorSpan(hunk, 'current', boxes.current);
  if (!before || !after) return null;
  return {
    beforeTop: before.top - scrollTops.previous,
    beforeBottom: before.bottom - scrollTops.previous,
    afterTop: after.top - scrollTops.current,
    afterBottom: after.bottom - scrollTops.current,
  };
}

/**
 * A band's edges cut to a gutter `height` tall, or `null` when none of the
 * band is in it.
 *
 * A band lies between the higher of its two tops and the lower of its two
 * bottoms: its curves leave each side level, so they never pass either. One
 * with that whole stretch above the gutter or below it shows nothing.
 *
 * A band with a side in view and the other far off still shows, leading off
 * the top or the bottom. Its far side is held one gutter height out: near
 * enough that the band keeps some width where it leaves, and a continuous
 * move as the pane scrolls, so nothing jumps when a side comes into range.
 */
export function clipConnectorEdges(edges: ConnectorEdges, height: number): ConnectorEdges | null {
  if (!(height > 0)) return null;
  const top = Math.min(edges.beforeTop, edges.afterTop);
  const bottom = Math.max(edges.beforeBottom, edges.afterBottom);
  if (bottom <= 0 || top >= height) return null;
  const hold = (y: number) => Math.min(Math.max(y, -height), 2 * height);
  return {
    beforeTop: hold(edges.beforeTop),
    beforeBottom: hold(edges.beforeBottom),
    afterTop: hold(edges.afterTop),
    afterBottom: hold(edges.afterBottom),
  };
}

/**
 * The bands to draw for a diff: one per hunk that has any part in the
 * gutter, in the hunks' order, each as the viewer's connector shape between
 * x = 0 (the previous version's side) and x = `width` (the note's).
 */
export function layoutNoteDiffConnectors(
  hunks: readonly NoteDiffHunk[],
  boxes: Record<NoteDiffSide, readonly NoteDiffUnitBox[]>,
  viewport: NoteDiffConnectorViewport
): NoteDiffConnector[] {
  const { scrollTops, width, height } = viewport;
  if (!(width > 0)) return [];
  return hunks.flatMap((hunk) => {
    const placed = hunkConnectorEdges(hunk, boxes, scrollTops);
    const edges = placed && clipConnectorEdges(placed, height);
    if (!edges) return [];
    return [
      {
        hunkId: hunk.id,
        kind: hunk.kind,
        edges,
        fill: connectorFillPathData(edges, width),
        stroke: connectorStrokePathData(edges, width),
      },
    ];
  });
}
