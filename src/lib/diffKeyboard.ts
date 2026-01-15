/**
 * Diff Keyboard Navigation
 *
 * Registers keyboard shortcuts for navigating the diff viewer:
 * - J/Down: Jump to next diff hunk
 * - K/Up: Jump to previous diff hunk
 * - Ctrl+N: Scroll down
 * - Ctrl+P: Scroll up
 */

import type { Alignment } from './types';
import { registerShortcuts, type Shortcut } from './services/keyboard';

export interface DiffNavConfig {
  scrollAmount: number; // pixels per keypress for smooth scroll
  getChangedAlignments: () => Array<{ alignment: Alignment; index: number }>;
  scrollToRow: (row: number, side: 'before' | 'after') => void;
  scrollBy: (deltaY: number) => void;
  getCurrentScrollY: () => number;
  getLineHeight: () => number;
  getViewportHeight: () => number;
  startCommentOnHunk: (hunkIndex: number) => void;
}

const DEFAULT_CONFIG: DiffNavConfig = {
  scrollAmount: 60, // ~3 lines
  getChangedAlignments: () => [],
  scrollToRow: () => {},
  scrollBy: () => {},
  getCurrentScrollY: () => 0,
  getLineHeight: () => 20,
  getViewportHeight: () => 400,
  startCommentOnHunk: () => {},
};

/**
 * Find the index of the current hunk based on scroll position.
 * Returns the index of the hunk that's currently visible (or just passed).
 */
function findCurrentHunkIndex(config: DiffNavConfig): number {
  const alignments = config.getChangedAlignments();
  if (alignments.length === 0) return -1;

  const scrollY = config.getCurrentScrollY();
  const lineHeight = config.getLineHeight();
  const viewportHeight = config.getViewportHeight();

  // Consider a hunk "current" if its start is within the top third of the viewport
  const anchorY = scrollY + viewportHeight / 3;
  const anchorRow = Math.floor(anchorY / lineHeight);

  // Find the last hunk whose start is at or before the anchor
  let currentIndex = -1;
  for (let i = 0; i < alignments.length; i++) {
    const hunkStart = alignments[i].alignment.after.start;
    if (hunkStart <= anchorRow) {
      currentIndex = i;
    } else {
      break;
    }
  }

  return currentIndex;
}

/**
 * Navigate to the next diff hunk.
 */
function goToNextHunk(config: DiffNavConfig): boolean {
  const alignments = config.getChangedAlignments();
  if (alignments.length === 0) return false;

  const currentIndex = findCurrentHunkIndex(config);
  const nextIndex = currentIndex + 1;

  if (nextIndex < alignments.length) {
    const nextHunk = alignments[nextIndex].alignment;
    config.scrollToRow(nextHunk.after.start, 'after');
    return true;
  }

  return false;
}

/**
 * Start a comment on the current hunk.
 */
function commentOnCurrentHunk(config: DiffNavConfig): boolean {
  const currentIndex = findCurrentHunkIndex(config);
  if (currentIndex >= 0) {
    config.startCommentOnHunk(currentIndex);
    return true;
  }
  return false;
}

/**
 * Navigate to the previous diff hunk.
 */
function goToPreviousHunk(config: DiffNavConfig): boolean {
  const alignments = config.getChangedAlignments();
  if (alignments.length === 0) return false;

  const currentIndex = findCurrentHunkIndex(config);

  // If we're past the first hunk, go to current or previous
  // We need to check if we're at the very start of the current hunk
  if (currentIndex >= 0) {
    const currentHunk = alignments[currentIndex].alignment;
    const scrollY = config.getCurrentScrollY();
    const lineHeight = config.getLineHeight();
    const viewportHeight = config.getViewportHeight();
    const anchorY = scrollY + viewportHeight / 3;
    const anchorRow = Math.floor(anchorY / lineHeight);

    // If we're more than 2 lines into the current hunk, go to its start
    if (anchorRow > currentHunk.after.start + 2) {
      config.scrollToRow(currentHunk.after.start, 'after');
      return true;
    }

    // Otherwise go to previous hunk
    if (currentIndex > 0) {
      const prevHunk = alignments[currentIndex - 1].alignment;
      config.scrollToRow(prevHunk.after.start, 'after');
      return true;
    }
  }

  // If at or before first hunk, go to first hunk
  if (alignments.length > 0) {
    config.scrollToRow(alignments[0].alignment.after.start, 'after');
    return true;
  }

  return false;
}

/**
 * Set up diff navigation keyboard shortcuts.
 * Returns a cleanup function.
 */
export function setupDiffKeyboardNav(config: Partial<DiffNavConfig> = {}): () => void {
  const cfg = { ...DEFAULT_CONFIG, ...config };

  const shortcuts: Shortcut[] = [
    {
      id: 'diff-next-hunk',
      keys: ['j', 'ArrowDown'],
      description: 'Next diff hunk',
      category: 'navigation',
      handler: () => goToNextHunk(cfg),
    },
    {
      id: 'diff-prev-hunk',
      keys: ['k', 'ArrowUp'],
      description: 'Previous diff hunk',
      category: 'navigation',
      handler: () => goToPreviousHunk(cfg),
    },
    {
      id: 'diff-scroll-down',
      keys: ['n'],
      modifiers: { ctrl: true },
      description: 'Scroll down',
      category: 'navigation',
      handler: () => cfg.scrollBy(cfg.scrollAmount),
    },
    {
      id: 'diff-scroll-up',
      keys: ['p'],
      modifiers: { ctrl: true },
      description: 'Scroll up',
      category: 'navigation',
      handler: () => cfg.scrollBy(-cfg.scrollAmount),
    },
    {
      id: 'diff-add-comment',
      keys: ['i'],
      description: 'Add comment on hunk',
      category: 'comments',
      handler: () => commentOnCurrentHunk(cfg),
    },
  ];

  return registerShortcuts(shortcuts);
}
