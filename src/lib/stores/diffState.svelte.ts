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
  /** Whether diffs are currently loading */
  loading: true,
  /** Error message if loading failed */
  error: null as string | null,
  /** Currently selected file path */
  selectedFile: null as string | null,
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
// Actions
// =============================================================================

/**
 * Load all diffs for the given base..head.
 * Auto-selects the first file if none is selected.
 */
export async function loadDiffs(base: string, head: string): Promise<void> {
  diffState.loading = true;
  diffState.error = null;

  try {
    diffState.diffs = await getDiff(base, head);

    // Auto-select first file if none selected
    if (!diffState.selectedFile && diffState.diffs.length > 0) {
      diffState.selectedFile = getFilePath(diffState.diffs[0]);
    }

    // Check if currently selected file still exists
    if (diffState.selectedFile) {
      const stillExists = diffState.diffs.some((d) => getFilePath(d) === diffState.selectedFile);
      if (!stillExists) {
        diffState.selectedFile =
          diffState.diffs.length > 0 ? getFilePath(diffState.diffs[0]) : null;
      }
    }
  } catch (e) {
    diffState.error = e instanceof Error ? e.message : String(e);
    diffState.diffs = [];
  } finally {
    diffState.loading = false;
  }
}

/**
 * Select a file by path.
 */
export function selectFile(path: string | null): void {
  diffState.selectedFile = path;
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
