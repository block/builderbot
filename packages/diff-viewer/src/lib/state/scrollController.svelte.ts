/**
 * Scroll Controller
 *
 * Manages synchronized scrolling between two diff panes with frame-perfect sync.
 * Instead of reacting to scroll events after the fact, this controller owns both
 * scroll positions and updates them together in the same frame.
 *
 * Key features:
 * - Single source of truth for both scroll positions
 * - Alignment-based transfer (same algorithm as before)
 * - No feedback loops or lag
 * - Momentum scrolling support
 */

import type { Alignment, Span } from '../types';

/** Anchor point as fraction of viewport height (1/3 from top keeps context visible) */
const SCROLL_ANCHOR_FRACTION = 1 / 3;

/** Dimensions for a pane */
export interface PaneDimensions {
  /** Height of the visible viewport */
  viewportHeight: number;
  /** Total height of content */
  contentHeight: number;
  /** Height of a single line */
  lineHeight: number;
  /** Width of the visible viewport */
  viewportWidth?: number;
  /** Total width of content (max line width) */
  contentWidth?: number;
}

/** State returned by the controller */
export interface ScrollState {
  beforeScrollY: number;
  afterScrollY: number;
  beforeScrollX: number;
  afterScrollX: number;
}

/**
 * The scrollable extent a pane's scrollbar should present: the pane's own
 * range extended by the edge handoff headroom (see scrollBy), so dragging the
 * thumb to an end carries the other pane through rows that exist on its side
 * alone. Invariant: contentHeight - viewportHeight === maxScrollY.
 */
export interface ScrollExtent {
  /** Current position within the extent */
  scrollY: number;
  /** Maximum position within the extent */
  maxScrollY: number;
  /** Height of the extent: the pane's own content plus headroom at both edges */
  contentHeight: number;
  /** Where the pane's own content starts within the extent (its top headroom) */
  contentOffset: number;
}

/**
 * Find the alignment containing a given row index.
 */
function findAlignment(
  row: number,
  alignments: Alignment[],
  side: 'before' | 'after'
): Alignment | null {
  for (const alignment of alignments) {
    const span = side === 'before' ? alignment.before : alignment.after;
    if (row < span.end) {
      return alignment;
    }
  }
  return alignments.length > 0 ? alignments[alignments.length - 1] : null;
}

/**
 * Transfer a row index from one side to the corresponding position on the other side.
 */
function transferRow(row: number, alignment: Alignment, side: 'before' | 'after'): number {
  const [source, target]: [Span, Span] =
    side === 'before' ? [alignment.before, alignment.after] : [alignment.after, alignment.before];

  // Exact boundary matches
  if (source.start === row) return target.start;
  if (source.end === row) return target.end;

  // Past the alignment - linear offset from end
  if (source.end < row) return row - source.end + target.end;

  // Within the alignment
  const sourceSize = source.end - source.start;
  const targetSize = target.end - target.start;

  if (sourceSize === 0) {
    return target.start;
  }

  if (targetSize === 0) {
    return target.start;
  }

  // Proportional mapping within the alignment
  const offset = row - source.start;
  const ratio = offset / sourceSize;
  const targetOffset = Math.floor(ratio * targetSize);

  return Math.min(target.start + targetOffset, target.end - 1);
}

/** Dimensions of a pane that is not rendered: nothing to scroll, no headroom to hand off. */
function emptyDimensions(): PaneDimensions {
  return { viewportHeight: 0, contentHeight: 0, lineHeight: 20 };
}

/**
 * Create a scroll controller for synchronized diff panes.
 */
export function createScrollController() {
  // Internal state. Reactive (raw, replaced wholesale) so derived extents
  // recompute when the diff or pane geometry changes, not just on scroll.
  let alignments: Alignment[] = $state.raw([]);
  let beforeDims: PaneDimensions = $state.raw(emptyDimensions());
  let afterDims: PaneDimensions = $state.raw(emptyDimensions());

  // Track current file to detect file changes vs content refreshes
  let currentFilePath: string | null = null;

  // Current scroll positions
  let beforeScrollY = $state(0);
  let afterScrollY = $state(0);
  let beforeScrollX = $state(0);
  let afterScrollX = $state(0);

  /**
   * Maximum vertical scroll position for a pane.
   */
  function maxScrollY(dims: PaneDimensions): number {
    return Math.max(0, dims.contentHeight - dims.viewportHeight);
  }

  /**
   * Clamp a vertical scroll position to valid bounds.
   */
  function clampScroll(scrollY: number, dims: PaneDimensions): number {
    return Math.max(0, Math.min(maxScrollY(dims), scrollY));
  }

  /**
   * Clamp a horizontal scroll position to valid bounds.
   */
  function clampScrollX(scrollX: number, dims: PaneDimensions): number {
    const viewportWidth = dims.viewportWidth ?? 0;
    const contentWidth = dims.contentWidth ?? 0;
    const maxScroll = Math.max(0, contentWidth - viewportWidth);
    return Math.max(0, Math.min(maxScroll, scrollX));
  }

  /**
   * Transfer scroll position from one pane to the other using alignment mapping.
   */
  function transferScroll(
    sourceScrollY: number,
    sourceSide: 'before' | 'after',
    sourceDims: PaneDimensions,
    targetDims: PaneDimensions
  ): number {
    if (alignments.length === 0) {
      // No alignments - just use same scroll position
      return clampScroll(sourceScrollY, targetDims);
    }

    const lineHeight = sourceDims.lineHeight;
    if (lineHeight <= 0) return 0;

    // Calculate anchor point (1/3 down the viewport)
    const anchorOffset = sourceDims.viewportHeight * SCROLL_ANCHOR_FRACTION;
    const sourceY = sourceScrollY + anchorOffset;

    // Convert to row index with sub-row offset
    const sourceRow = Math.floor(sourceY / lineHeight);
    const subRowOffset = sourceY % lineHeight;

    // Find alignment and transfer row
    const alignment = findAlignment(sourceRow, alignments, sourceSide);
    if (!alignment) {
      return clampScroll(sourceScrollY, targetDims);
    }

    const targetRow = transferRow(sourceRow, alignment, sourceSide);

    // Calculate alignment sizes for proportional sub-row offset scaling
    const sourceSpan = sourceSide === 'before' ? alignment.before : alignment.after;
    const targetSpan = sourceSide === 'before' ? alignment.after : alignment.before;
    const sourceAlignmentSize = sourceSpan.end - sourceSpan.start;
    const targetAlignmentSize = targetSpan.end - targetSpan.start;

    // Scale sub-row offset proportionally
    let adjustedSubRowOffset = 0;
    if (targetAlignmentSize > 0 && sourceAlignmentSize > 0) {
      const ratio = targetAlignmentSize / sourceAlignmentSize;
      adjustedSubRowOffset = subRowOffset * ratio;
    } else if (!alignment.changed) {
      adjustedSubRowOffset = subRowOffset;
    }

    // Convert back to pixels (use target line height)
    const targetLineHeight = targetDims.lineHeight;
    const targetY = targetRow * targetLineHeight + adjustedSubRowOffset - anchorOffset;

    return clampScroll(targetY, targetDims);
  }

  /**
   * How far the target pane sits past its alignment-derived position because
   * scrolling was handed off to it while the source pane was pinned at an edge
   * (see scrollBy). Only counts while the source is still at that edge; any
   * other discrepancy is alignment rounding and keeps snapping as before.
   */
  function edgeOverscroll(
    sourceScrollY: number,
    sourceSide: 'before' | 'after',
    sourceDims: PaneDimensions,
    targetScrollY: number,
    targetDims: PaneDimensions
  ): number {
    const aligned = transferScroll(sourceScrollY, sourceSide, sourceDims, targetDims);
    const overscroll = targetScrollY - aligned;
    if (overscroll > 0 && sourceScrollY >= maxScrollY(sourceDims)) return overscroll;
    if (overscroll < 0 && sourceScrollY <= 0) return overscroll;
    return 0;
  }

  /**
   * How far the other pane can travel past its aligned position once this
   * pane is pinned at the top or bottom: the rows that exist on the other
   * side alone before the first (or after the last) shared row.
   */
  function edgeHeadroom(side: 'before' | 'after'): { top: number; bottom: number } {
    const sourceDims = side === 'before' ? beforeDims : afterDims;
    const targetDims = side === 'before' ? afterDims : beforeDims;
    const top = transferScroll(0, side, sourceDims, targetDims);
    const alignedAtMax = transferScroll(maxScrollY(sourceDims), side, sourceDims, targetDims);
    return { top, bottom: maxScrollY(targetDims) - alignedAtMax };
  }

  return {
    // Expose reactive state
    get beforeScrollY() {
      return beforeScrollY;
    },
    get afterScrollY() {
      return afterScrollY;
    },
    get beforeScrollX() {
      return beforeScrollX;
    },
    get afterScrollX() {
      return afterScrollX;
    },

    /**
     * Update alignments when diff content changes.
     * Only resets scroll when file changes, not on content refresh.
     *
     * @param newAlignments - The new alignments
     * @param filePath - Optional file path to track file identity
     */
    setAlignments(newAlignments: Alignment[], filePath?: string | null) {
      alignments = newAlignments;

      // Only reset scroll when switching to a different file
      const fileChanged = filePath !== undefined && filePath !== currentFilePath;
      if (fileChanged) {
        currentFilePath = filePath ?? null;
        beforeScrollY = 0;
        afterScrollY = 0;
        beforeScrollX = 0;
        afterScrollX = 0;
      } else {
        // Content refresh - clamp scroll to new bounds but preserve position
        beforeScrollY = clampScroll(beforeScrollY, beforeDims);
        afterScrollY = clampScroll(afterScrollY, afterDims);
      }
    },

    /**
     * Update dimensions for a pane.
     */
    setDimensions(side: 'before' | 'after', dims: PaneDimensions) {
      if (side === 'before') {
        beforeDims = dims;
      } else {
        afterDims = dims;
      }
    },

    /**
     * Forget a pane's dimensions when it is not rendered (created and deleted
     * files show a single pane). Stale dimensions from a previous file would
     * otherwise give the visible pane phantom edge headroom and let scrollBy
     * hand input off to a pane that does not exist.
     */
    clearDimensions(side: 'before' | 'after') {
      this.setDimensions(side, emptyDimensions());
    },

    /**
     * Scroll by a delta amount. Updates both panes in sync.
     *
     * The alignment transfer keeps shared rows level, so content that exists on
     * only one side past the last shared row (e.g. lines deleted from the end
     * of a file) sits beyond the other pane's scroll range. Once the source
     * pane is pinned at an edge, the delta it cannot consume is handed off to
     * the other pane so that content stays reachable; reversing direction
     * unwinds the handoff first so neither pane jumps.
     */
    scrollBy(side: 'before' | 'after', deltaY: number) {
      const sourceDims = side === 'before' ? beforeDims : afterDims;
      const targetDims = side === 'before' ? afterDims : beforeDims;
      const sourceScrollY = side === 'before' ? beforeScrollY : afterScrollY;
      const targetScrollY = side === 'before' ? afterScrollY : beforeScrollY;

      let overscroll = edgeOverscroll(sourceScrollY, side, sourceDims, targetScrollY, targetDims);
      let remaining = deltaY;
      if (overscroll !== 0 && Math.sign(remaining) === -Math.sign(overscroll)) {
        const unwind = Math.sign(remaining) * Math.min(Math.abs(remaining), Math.abs(overscroll));
        overscroll += unwind;
        remaining -= unwind;
      }

      const newSource = clampScroll(sourceScrollY + remaining, sourceDims);
      const residual = sourceScrollY + remaining - newSource;
      const aligned = transferScroll(newSource, side, sourceDims, targetDims);
      const newTarget = clampScroll(aligned + overscroll + residual, targetDims);

      if (side === 'before') {
        beforeScrollY = newSource;
        afterScrollY = newTarget;
      } else {
        afterScrollY = newSource;
        beforeScrollY = newTarget;
      }
    },

    /**
     * Scroll horizontally by a delta amount.
     * Horizontal scroll is independent for each pane (no alignment sync needed).
     */
    scrollByX(side: 'before' | 'after', deltaX: number) {
      if (side === 'before') {
        beforeScrollX = clampScrollX(beforeScrollX + deltaX, beforeDims);
      } else {
        afterScrollX = clampScrollX(afterScrollX + deltaX, afterDims);
      }
    },

    /**
     * Scroll both panes horizontally by the same amount.
     */
    scrollByXBoth(deltaX: number) {
      beforeScrollX = clampScrollX(beforeScrollX + deltaX, beforeDims);
      afterScrollX = clampScrollX(afterScrollX + deltaX, afterDims);
    },

    /**
     * The extent a pane's scrollbar should run over (see ScrollExtent). The
     * position includes any outstanding edge handoff, so a scrollbar bound to
     * it tracks scrollBy and scrollToEffective without jumping.
     */
    getEffectiveExtent(side: 'before' | 'after'): ScrollExtent {
      const sourceDims = side === 'before' ? beforeDims : afterDims;
      const targetDims = side === 'before' ? afterDims : beforeDims;
      const sourceScrollY = side === 'before' ? beforeScrollY : afterScrollY;
      const targetScrollY = side === 'before' ? afterScrollY : beforeScrollY;

      const { top, bottom } = edgeHeadroom(side);
      const overscroll = edgeOverscroll(sourceScrollY, side, sourceDims, targetScrollY, targetDims);
      const maxScroll = top + maxScrollY(sourceDims) + bottom;
      return {
        scrollY: Math.max(0, Math.min(maxScroll, top + sourceScrollY + overscroll)),
        maxScrollY: maxScroll,
        contentHeight: Math.max(sourceDims.contentHeight, sourceDims.viewportHeight) + top + bottom,
        contentOffset: top,
      };
    },

    /**
     * Set the position within a pane's effective extent (see getEffectiveExtent).
     * Inside the pane's own range this is scrollTo; in the headroom beyond
     * either edge the pane stays pinned and the remainder moves the other pane,
     * the same handoff scrollBy performs incrementally.
     */
    scrollToEffective(side: 'before' | 'after', effectiveY: number) {
      const sourceDims = side === 'before' ? beforeDims : afterDims;
      const targetDims = side === 'before' ? afterDims : beforeDims;

      const { top, bottom } = edgeHeadroom(side);
      const clamped = Math.max(0, Math.min(top + maxScrollY(sourceDims) + bottom, effectiveY));
      const newSource = clampScroll(clamped - top, sourceDims);
      const residual = clamped - top - newSource;
      const aligned = transferScroll(newSource, side, sourceDims, targetDims);
      const newTarget = clampScroll(aligned + residual, targetDims);

      if (side === 'before') {
        beforeScrollY = newSource;
        afterScrollY = newTarget;
      } else {
        afterScrollY = newSource;
        beforeScrollY = newTarget;
      }
    },

    /**
     * Set scroll position directly. Updates both panes in sync.
     */
    scrollTo(side: 'before' | 'after', scrollY: number) {
      if (side === 'before') {
        const newBefore = clampScroll(scrollY, beforeDims);
        beforeScrollY = newBefore;
        afterScrollY = transferScroll(newBefore, 'before', beforeDims, afterDims);
      } else {
        const newAfter = clampScroll(scrollY, afterDims);
        afterScrollY = newAfter;
        beforeScrollY = transferScroll(newAfter, 'after', afterDims, beforeDims);
      }
    },

    /**
     * Scroll to a specific row, centering it in the viewport.
     */
    scrollToRow(row: number, side: 'before' | 'after') {
      const dims = side === 'before' ? beforeDims : afterDims;
      const lineHeight = dims.lineHeight;

      // Position row at 1/3 from top
      const targetY = row * lineHeight - dims.viewportHeight * SCROLL_ANCHOR_FRACTION;
      this.scrollTo(side, targetY);
    },

    /**
     * Reset scroll positions to top/left.
     */
    reset() {
      beforeScrollY = 0;
      afterScrollY = 0;
      beforeScrollX = 0;
      afterScrollX = 0;
    },

    /**
     * Get current dimensions for a side.
     */
    getDimensions(side: 'before' | 'after'): PaneDimensions {
      return side === 'before' ? beforeDims : afterDims;
    },
  };
}

export type ScrollController = ReturnType<typeof createScrollController>;
