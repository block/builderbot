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
import { anchorSideForHunk } from './diffViewerHelpers';

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
  /**
   * Hunk the host currently shows as focused (fed back from onHunkFocus), or
   * null once the user clicked away or the diff changed. Lets navigation trust
   * its last jump when the scroll position alone cannot identify the hunk.
   */
  getFocusedHunkIndex?: () => number | null;
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

interface NavState {
  /** Last keyboard hunk jump and the after-pane scroll position it landed on. */
  lastJump: { index: number; scrollY: number } | null;
}

/**
 * Row at the scroll anchor (1/3 down the after pane's viewport).
 */
function anchorRow(config: DiffNavConfig): number {
  const anchorY = config.getCurrentScrollY() + config.getViewportHeight() / 3;
  return Math.floor(anchorY / config.getLineHeight());
}

/**
 * Find the index of the current hunk based on scroll position.
 * Returns the index of the hunk that's currently visible (or just passed).
 */
function findCurrentHunkIndexFromScroll(config: DiffNavConfig): number {
  const alignments = config.getChangedAlignments();
  if (alignments.length === 0) return -1;

  // Consider a hunk "current" if its start is within the top third of the viewport
  const anchor = anchorRow(config);

  // Find the last hunk whose start is at or before the anchor
  let currentIndex = -1;
  for (let i = 0; i < alignments.length; i++) {
    const hunkStart = alignments[i].alignment.after.start;
    if (hunkStart <= anchor) {
      currentIndex = i;
    } else {
      break;
    }
  }

  return currentIndex;
}

/**
 * Resolve the hunk keyboard navigation should treat as current.
 *
 * The scroll-derived answer breaks down whenever the after pane cannot move far
 * enough to bring a hunk up to the anchor: pinned at its max by a trailing
 * deletion, or a file shorter than the viewport. Any hunk in that last stretch
 * is never "reached", so n/p would keep re-selecting the hunk above it. The
 * last keyboard jump is authoritative there, as long as the host still reports
 * it focused and the after pane has not scrolled since; that jump also means
 * the viewport sits at the hunk's start.
 */
function findCurrentHunk(
  config: DiffNavConfig,
  state: NavState
): { index: number; atStart: boolean } {
  const { lastJump } = state;
  if (lastJump) {
    const focused = config.getFocusedHunkIndex ? config.getFocusedHunkIndex() : lastJump.index;
    const inRange = lastJump.index < config.getChangedAlignments().length;
    if (focused === lastJump.index && inRange && config.getCurrentScrollY() === lastJump.scrollY) {
      return { index: lastJump.index, atStart: true };
    }
    state.lastJump = null;
  }
  return { index: findCurrentHunkIndexFromScroll(config), atStart: false };
}

/**
 * Scroll a hunk into view, anchored on the pane that actually has its rows,
 * and remember where that left the after pane.
 */
function scrollToHunk(config: DiffNavConfig, state: NavState, index: number): void {
  const hunk = config.getChangedAlignments()[index].alignment;
  const side = anchorSideForHunk(hunk);
  config.scrollToRow(hunk[side].start, side);
  state.lastJump = { index, scrollY: config.getCurrentScrollY() };
  config.onHunkFocus?.(index);
}

/**
 * Navigate to the next diff hunk.
 */
function goToNextHunk(config: DiffNavConfig, state: NavState): boolean {
  const alignments = config.getChangedAlignments();
  if (alignments.length === 0) return false;

  const nextIndex = findCurrentHunk(config, state).index + 1;

  if (nextIndex < alignments.length) {
    scrollToHunk(config, state, nextIndex);
    return true;
  }

  return false;
}

/**
 * Start a comment on the current hunk.
 */
function commentOnCurrentHunk(config: DiffNavConfig, state: NavState): boolean {
  const currentIndex = findCurrentHunk(config, state).index;
  if (currentIndex >= 0) {
    config.startCommentOnHunk(currentIndex);
    return true;
  }
  return false;
}

/**
 * Navigate to the previous diff hunk.
 */
function goToPreviousHunk(config: DiffNavConfig, state: NavState): boolean {
  const alignments = config.getChangedAlignments();
  if (alignments.length === 0) return false;

  const { index: currentIndex, atStart } = findCurrentHunk(config, state);

  // If we're past the first hunk, go to current or previous
  // We need to check if we're at the very start of the current hunk
  if (currentIndex >= 0) {
    const currentHunk = alignments[currentIndex].alignment;

    // If we're more than 2 lines into the current hunk, go to its start
    if (!atStart && anchorRow(config) > currentHunk.after.start + 2) {
      scrollToHunk(config, state, currentIndex);
      return true;
    }

    // Otherwise go to previous hunk
    if (currentIndex > 0) {
      scrollToHunk(config, state, currentIndex - 1);
      return true;
    }
  }

  // If at or before first hunk, go to first hunk
  scrollToHunk(config, state, 0);
  return true;
}

/**
 * Set up diff navigation keyboard shortcuts.
 * Attaches a keydown listener to `window` and returns a cleanup function.
 */
export function setupDiffKeyboardNav(config: Partial<DiffNavConfig> = {}): () => void {
  const cfg = { ...DEFAULT_CONFIG, ...config };
  const state: NavState = { lastJump: null };

  function handleKeydown(event: KeyboardEvent): void {
    const target = event.target as HTMLElement;
    const inInput =
      target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable;

    const key = event.key.toLowerCase();
    const ctrl = event.ctrlKey;
    const meta = event.metaKey;
    const shift = event.shiftKey;
    const alt = event.altKey;

    // Search shortcuts work even when focus is in an input/textarea
    // (so you can navigate results while typing in the search box)

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

    // Skip all other shortcuts when focus is in an input or textarea
    if (inInput) return;

    // J or ArrowDown — next hunk (no modifiers)
    if ((key === 'j' || key === 'arrowdown') && !ctrl && !meta && !shift && !alt) {
      if (goToNextHunk(cfg, state)) event.preventDefault();
      return;
    }

    // K or ArrowUp — previous hunk (no modifiers)
    if ((key === 'k' || key === 'arrowup') && !ctrl && !meta && !shift && !alt) {
      if (goToPreviousHunk(cfg, state)) event.preventDefault();
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
      if (commentOnCurrentHunk(cfg, state)) event.preventDefault();
      return;
    }
  }

  window.addEventListener('keydown', handleKeydown);

  return () => {
    window.removeEventListener('keydown', handleKeydown);
  };
}
