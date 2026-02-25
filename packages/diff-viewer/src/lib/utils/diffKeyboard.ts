/**
 * Diff Keyboard Navigation
 *
 * Registers keyboard shortcuts for navigating the diff viewer:
 * - J/Down: Jump to next diff hunk
 * - K/Up: Jump to previous diff hunk
 * - Ctrl+N: Scroll down
 * - Ctrl+P: Scroll up
 * - I: Add comment on current hunk
 * - Cmd/Ctrl+F: Open search
 * - Cmd/Ctrl+G: Next search result
 * - Cmd/Ctrl+Shift+G: Previous search result
 *
 * Uses plain DOM event listeners (no external keyboard service dependency).
 */

import type { Alignment } from '../types';

export interface DiffNavConfig {
  scrollAmount: number; // pixels per keypress for smooth scroll
  getChangedAlignments: () => Array<{ alignment: Alignment; index: number }>;
  scrollToRow: (row: number, side: 'before' | 'after') => void;
  scrollBy: (deltaY: number) => void;
  getCurrentScrollY: () => number;
  getLineHeight: () => number;
  getViewportHeight: () => number;
  startCommentOnHunk: (hunkIndex: number) => void;
  /** Called when keyboard navigation focuses a hunk */
  onHunkFocus?: (hunkIndex: number | null) => void;
  /** Search callbacks */
  onOpenSearch?: () => void;
  onNextSearchResult?: () => void;
  onPrevSearchResult?: () => void;
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
  onHunkFocus: () => {},
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
    config.onHunkFocus?.(nextIndex);
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
      config.onHunkFocus?.(currentIndex);
      return true;
    }

    // Otherwise go to previous hunk
    if (currentIndex > 0) {
      const prevHunk = alignments[currentIndex - 1].alignment;
      config.scrollToRow(prevHunk.after.start, 'after');
      config.onHunkFocus?.(currentIndex - 1);
      return true;
    }
  }

  // If at or before first hunk, go to first hunk
  if (alignments.length > 0) {
    config.scrollToRow(alignments[0].alignment.after.start, 'after');
    config.onHunkFocus?.(0);
    return true;
  }

  return false;
}

/**
 * Set up diff navigation keyboard shortcuts.
 * Attaches a keydown listener to `window` and returns a cleanup function.
 */
export function setupDiffKeyboardNav(config: Partial<DiffNavConfig> = {}): () => void {
  const cfg = { ...DEFAULT_CONFIG, ...config };

  function handleKeydown(event: KeyboardEvent): void {
    // Skip when focus is in an input or textarea
    const target = event.target as HTMLElement;
    if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') return;

    const key = event.key.toLowerCase();
    const ctrl = event.ctrlKey;
    const meta = event.metaKey;
    const shift = event.shiftKey;
    const alt = event.altKey;

    // J or ArrowDown — next hunk (no modifiers)
    if ((key === 'j' || key === 'arrowdown') && !ctrl && !meta && !shift && !alt) {
      if (goToNextHunk(cfg)) event.preventDefault();
      return;
    }

    // K or ArrowUp — previous hunk (no modifiers)
    if ((key === 'k' || key === 'arrowup') && !ctrl && !meta && !shift && !alt) {
      if (goToPreviousHunk(cfg)) event.preventDefault();
      return;
    }

    // Ctrl+N — scroll down
    if (key === 'n' && ctrl && !meta && !shift && !alt) {
      event.preventDefault();
      cfg.scrollBy(cfg.scrollAmount);
      return;
    }

    // Ctrl+P — scroll up
    if (key === 'p' && ctrl && !meta && !shift && !alt) {
      event.preventDefault();
      cfg.scrollBy(-cfg.scrollAmount);
      return;
    }

    // I — add comment on current hunk (no modifiers)
    if (key === 'i' && !ctrl && !meta && !shift && !alt) {
      if (commentOnCurrentHunk(cfg)) event.preventDefault();
      return;
    }

    // Cmd/Ctrl+F — open search
    if (key === 'f' && (ctrl || meta) && !shift && !alt) {
      event.preventDefault();
      cfg.onOpenSearch?.();
      return;
    }

    // Cmd/Ctrl+G — next search result
    if (key === 'g' && (ctrl || meta) && !shift && !alt) {
      event.preventDefault();
      cfg.onNextSearchResult?.();
      return;
    }

    // Cmd/Ctrl+Shift+G — previous search result
    if (key === 'g' && (ctrl || meta) && shift && !alt) {
      event.preventDefault();
      cfg.onPrevSearchResult?.();
      return;
    }
  }

  window.addEventListener('keydown', handleKeydown);

  return () => {
    window.removeEventListener('keydown', handleKeydown);
  };
}
