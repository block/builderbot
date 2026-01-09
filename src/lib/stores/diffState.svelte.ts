/**
 * Diff State Store
 *
 * Manages the loaded diffs and file selection state.
 *
 * Rebuildable: This module owns diff loading state. Components import
 * the reactive state object directly.
 */

import { getDiff } from '../services/git';
import { getFilePath } from '../diffUtils';
import type { FileDiff } from '../types';

// =============================================================================
// Reactive State
// =============================================================================

export const diffState = $state({
  /** All diffs for the current base..head */
  diffs: [] as FileDiff[],
  /** Whether diffs are currently loading (initial load only) */
  loading: true,
  /** Error message if loading failed */
  error: null as string | null,
  /** Currently selected file path */
  selectedFile: null as string | null,
  /** Target line to scroll to after file selection (0-indexed, null = no scroll) */
  scrollTargetLine: null as number | null,
});

// =============================================================================
// Getters
// =============================================================================

/**
 * Get the diff for the currently selected file.
 */
export function getCurrentDiff(): FileDiff | null {
  if (!diffState.selectedFile) return null;
  return diffState.diffs.find((d) => getFilePath(d) === diffState.selectedFile) ?? null;
}

// =============================================================================
// Helpers
// =============================================================================

/**
 * Apply selection logic after loading diffs.
 */
function updateSelection(): void {
  // Auto-select first file if none selected
  if (!diffState.selectedFile && diffState.diffs.length > 0) {
    diffState.selectedFile = getFilePath(diffState.diffs[0]);
  }

  // Check if currently selected file still exists
  if (diffState.selectedFile) {
    const stillExists = diffState.diffs.some((d) => getFilePath(d) === diffState.selectedFile);
    if (!stillExists) {
      diffState.selectedFile = diffState.diffs.length > 0 ? getFilePath(diffState.diffs[0]) : null;
    }
  }
}

// =============================================================================
// Actions
// =============================================================================

/**
 * Load all diffs for the given base..head.
 * Shows loading state - use for initial load or spec changes.
 */
export async function loadDiffs(
  base: string,
  head: string,
  repoPath?: string,
  useMergeBase?: boolean
): Promise<void> {
  diffState.loading = true;
  diffState.error = null;

  try {
    diffState.diffs = await getDiff(base, head, repoPath, useMergeBase);
    updateSelection();
  } catch (e) {
    diffState.error = e instanceof Error ? e.message : String(e);
    diffState.diffs = [];
  } finally {
    diffState.loading = false;
  }
}

/**
 * Refresh diffs without showing loading state.
 * Use for file watcher updates - keeps existing content visible during fetch.
 */
export async function refreshDiffs(
  base: string,
  head: string,
  repoPath?: string,
  useMergeBase?: boolean
): Promise<void> {
  try {
    diffState.diffs = await getDiff(base, head, repoPath, useMergeBase);
    updateSelection();
  } catch (e) {
    // On refresh errors, keep existing state (don't disrupt UI)
    console.error('Refresh failed:', e);
  }
}

/**
 * Select a file by path, optionally scrolling to a specific line.
 */
export function selectFile(path: string | null, scrollToLine?: number): void {
  diffState.selectedFile = path;
  diffState.scrollTargetLine = scrollToLine ?? null;
}

/**
 * Clear the scroll target (called after scrolling completes).
 */
export function clearScrollTarget(): void {
  diffState.scrollTargetLine = null;
}

/**
 * Reset all state (for diff spec changes).
 */
export function resetState(): void {
  diffState.selectedFile = null;
  diffState.diffs = [];
  diffState.error = null;
  diffState.loading = true;
}
