/**
 * Scroll Synchronization
 *
 * Handles synchronized scrolling between two diff panes using an alignment-based
 * line transfer algorithm.
 *
 * The approach maps corresponding regions (alignments) between panes and uses
 * proportional interpolation within change blocks. This was developed after
 * studying IntelliJ IDEA Community Edition's diff viewer (Apache 2.0 license)
 * to understand the general technique, then implemented independently.
 *
 * Key features:
 * - Anchor point at 1/3 viewport height (keeps context visible)
 * - Sub-line offset preservation (smooth scrolling)
 * - Feedback loop prevention via "primary" pane tracking
 * - Proportional mapping within change regions
 */

import type { Alignment, Span } from '../types';

const LINE_HEIGHT = 20; // Must match CSS .line min-height

export interface ScrollSyncConfig {
  /** Line height in pixels */
  lineHeight: number;
  /** Anchor point as fraction of viewport height (0.33 = 1/3 from top) */
  anchorFraction: number;
  /** Minimum pixel difference to trigger scroll update */
  scrollThreshold: number;
}

const DEFAULT_CONFIG: ScrollSyncConfig = {
  lineHeight: LINE_HEIGHT,
  anchorFraction: 1 / 3,
  scrollThreshold: 2,
};

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
 *
 * Within unchanged regions: 1:1 mapping
 * Within change regions: proportional mapping, clamped to alignment bounds
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
    // Source is empty (pure insertion/deletion on other side)
    return target.start;
  }

  if (targetSize === 0) {
    // Target is empty - clamp to target position
    return target.start;
  }

  // Proportional mapping within the alignment
  const offset = row - source.start;
  const ratio = offset / sourceSize;
  const targetOffset = Math.floor(ratio * targetSize);

  return Math.min(target.start + targetOffset, target.end - 1);
}

/**
 * Create a scroll sync controller for two panes.
 *
 * Uses a "primary" approach: whichever pane the user is actively scrolling
 * becomes the primary, and we ignore scroll events from the secondary until
 * user interaction stops.
 */
export function createScrollSync(config: Partial<ScrollSyncConfig> = {}) {
  const cfg = { ...DEFAULT_CONFIG, ...config };

  let alignments: Alignment[] = [];

  // Track which pane is currently the "primary" (user is scrolling it)
  // null = no active scrolling, accept events from either
  let primarySide: 'before' | 'after' | null = null;
  let primaryTimeout: ReturnType<typeof setTimeout> | null = null;

  // Track the last scroll position we set on each pane
  // This lets us ignore scroll events that are just the browser "catching up"
  let lastSetScrollTop: { before: number | null; after: number | null } = {
    before: null,
    after: null,
  };

  return {
    /**
     * Update the alignments when diff content changes.
     */
    setAlignments(newAlignments: Alignment[]) {
      alignments = newAlignments;
      // Reset tracking when content changes
      lastSetScrollTop = { before: null, after: null };
      primarySide = null;
    },

    /**
     * Handle scroll event from one pane, sync to the other.
     *
     * @param side - Which pane triggered the scroll ('before' or 'after')
     * @param source - The scrolling pane element
     * @param target - The pane to sync
     * @returns true if sync was performed
     */
    onScroll(side: 'before' | 'after', source: HTMLElement, target: HTMLElement | null): boolean {
      if (!target || alignments.length === 0) return false;

      const otherSide = side === 'before' ? 'after' : 'before';

      // Check if this scroll event is just the browser settling to a position we set
      const expectedPos = lastSetScrollTop[side];
      if (expectedPos !== null && Math.abs(source.scrollTop - expectedPos) < 3) {
        // This is the secondary responding to our programmatic scroll - ignore it
        lastSetScrollTop[side] = null;
        return false;
      }
      lastSetScrollTop[side] = null;

      // If another pane is primary, ignore this event
      if (primarySide !== null && primarySide !== side) {
        return false;
      }

      // This pane becomes primary
      primarySide = side;

      // Reset primary after a pause in scrolling
      if (primaryTimeout) clearTimeout(primaryTimeout);
      primaryTimeout = setTimeout(() => {
        primarySide = null;
      }, 150);

      // Calculate anchor point (1/3 down the viewport)
      const anchorOffset = source.clientHeight * cfg.anchorFraction;
      const sourceY = source.scrollTop + anchorOffset;

      // Convert to row index with sub-row offset
      const sourceRow = Math.floor(sourceY / cfg.lineHeight);
      const subRowOffset = sourceY % cfg.lineHeight;

      // Find alignment and transfer row
      const alignment = findAlignment(sourceRow, alignments, side);
      if (!alignment) {
        return false;
      }

      const targetRow = transferRow(sourceRow, alignment, side);

      // Calculate alignment sizes for proportional sub-row offset scaling
      const sourceSpan = side === 'before' ? alignment.before : alignment.after;
      const targetSpan = side === 'before' ? alignment.after : alignment.before;
      const sourceAlignmentSize = sourceSpan.end - sourceSpan.start;
      const targetAlignmentSize = targetSpan.end - targetSpan.start;

      // Scale sub-row offset proportionally to the alignment size ratio
      // If source has 9 rows and target has 1, sub-row offset should be scaled by 1/9
      // If target alignment is empty, no sub-row offset at all
      let adjustedSubRowOffset = 0;
      if (targetAlignmentSize > 0 && sourceAlignmentSize > 0) {
        const ratio = targetAlignmentSize / sourceAlignmentSize;
        adjustedSubRowOffset = subRowOffset * ratio;
      } else if (!alignment.changed) {
        // Context regions are 1:1, use full sub-row offset
        adjustedSubRowOffset = subRowOffset;
      }
      // else: changed region with empty target = no sub-row offset (stay still)

      // Convert back to pixels
      const targetY = targetRow * cfg.lineHeight + adjustedSubRowOffset - anchorOffset;
      const clampedTargetY = Math.max(0, targetY);

      // Only update if difference is significant
      const verticalDiff = Math.abs(target.scrollTop - clampedTargetY);
      if (verticalDiff > cfg.scrollThreshold) {
        // Record what we're setting so we can ignore the resulting event
        lastSetScrollTop[otherSide] = clampedTargetY;
        target.scrollTop = clampedTargetY;
      }

      // Sync horizontal scroll directly (1:1)
      const horizontalDiff = Math.abs(target.scrollLeft - source.scrollLeft);
      if (horizontalDiff > cfg.scrollThreshold) {
        target.scrollLeft = source.scrollLeft;
      }

      return true;
    },

    /**
     * Programmatically scroll to a specific row, syncing both panes.
     */
    scrollToRow(
      row: number,
      side: 'before' | 'after',
      beforePane: HTMLElement,
      afterPane: HTMLElement
    ) {
      const alignment = findAlignment(row, alignments, side);
      if (!alignment) return;

      const otherRow = transferRow(row, alignment, side);

      const beforeRow = side === 'before' ? row : otherRow;
      const afterRow = side === 'after' ? row : otherRow;

      // Center the row in viewport
      const beforeOffset = Math.max(0, beforeRow * cfg.lineHeight - beforePane.clientHeight / 3);
      const afterOffset = Math.max(0, afterRow * cfg.lineHeight - afterPane.clientHeight / 3);

      // Record positions to ignore resulting events
      lastSetScrollTop.before = beforeOffset;
      lastSetScrollTop.after = afterOffset;

      beforePane.scrollTop = beforeOffset;
      afterPane.scrollTop = afterOffset;
    },

    /**
     * Temporarily disable sync.
     */
    disable() {
      primarySide = 'before'; // Lock to one side
    },

    /**
     * Re-enable sync.
     */
    enable() {
      primarySide = null;
      lastSetScrollTop = { before: null, after: null };
    },
  };
}

export type ScrollSync = ReturnType<typeof createScrollSync>;
