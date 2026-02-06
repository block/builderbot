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

/**
 * Create a scroll controller for synchronized diff panes.
 */
export function createScrollController() {
  // Internal state
  let alignments: Alignment[] = [];
  let beforeDims: PaneDimensions = { viewportHeight: 0, contentHeight: 0, lineHeight: 20 };
  let afterDims: PaneDimensions = { viewportHeight: 0, contentHeight: 0, lineHeight: 20 };

  // Track current file to detect file changes vs content refreshes
  let currentFilePath: string | null = null;

  // Current scroll positions
  let beforeScrollY = $state(0);
  let afterScrollY = $state(0);
  let beforeScrollX = $state(0);
  let afterScrollX = $state(0);

  /**
   * Clamp a vertical scroll position to valid bounds.
   */
  function clampScroll(scrollY: number, dims: PaneDimensions): number {
    const maxScroll = Math.max(0, dims.contentHeight - dims.viewportHeight);
    return Math.max(0, Math.min(maxScroll, scrollY));
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
     * Scroll by a delta amount. Updates both panes in sync.
     */
    scrollBy(side: 'before' | 'after', deltaY: number) {
      if (side === 'before') {
        // Update before, compute after
        const newBefore = clampScroll(beforeScrollY + deltaY, beforeDims);
        beforeScrollY = newBefore;
        afterScrollY = transferScroll(newBefore, 'before', beforeDims, afterDims);
      } else {
        // Update after, compute before
        const newAfter = clampScroll(afterScrollY + deltaY, afterDims);
        afterScrollY = newAfter;
        beforeScrollY = transferScroll(newAfter, 'after', afterDims, beforeDims);
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
