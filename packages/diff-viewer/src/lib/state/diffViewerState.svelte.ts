/**
 * Diff Viewer State
 *
 * Manages file list and on-demand diff loading for the diff modal.
 *
 * Pattern:
 * - Load file list (fast) via get_diff_files
 * - Load individual file diffs on demand via get_file_diff
 * - Cache loaded diffs for the current session
 * - Auto-select first file when opened
 *
 * This is a factory — each DiffModal creates its own instance.
 */

import type { DiffCommands, FileDiffSummary, FileDiff } from '../types';

// =============================================================================
// Type
// =============================================================================

export interface DiffViewerState {
  /** Branch ID for API calls. */
  branchId: string;
  /** Resolved commit SHA (set after initial load). */
  commitSha: string | null;
  /** Diff scope. */
  scope: 'branch' | 'commit';
  /** File summaries for the sidebar. */
  files: FileDiffSummary[];
  /** Cached full diffs by path. */
  diffCache: Map<string, FileDiff>;
  /** Currently selected file path. */
  selectedFile: string | null;
  /** Whether the file list is loading. */
  loading: boolean;
  /** Whether a specific file diff is loading. */
  loadingFile: string | null;
  /** Error message if loading failed. */
  error: string | null;
}

// =============================================================================
// Helpers
// =============================================================================

/** Get the display path for a file summary. */
export function fileSummaryPath(summary: FileDiffSummary): string {
  return summary.after ?? summary.before ?? '';
}

// =============================================================================
// Factory
// =============================================================================

/**
 * Create a reactive diff viewer state instance.
 *
 * Immediately begins loading the file list. Returns a reactive `$state` object
 * and action functions scoped to this instance.
 */
export function createDiffViewerState(
  commands: DiffCommands,
  branchId: string,
  scope: 'branch' | 'commit',
  commitSha?: string
) {
  const state: DiffViewerState = $state({
    branchId,
    commitSha: commitSha ?? null,
    scope,
    files: [],
    diffCache: new Map(),
    selectedFile: null,
    loading: true,
    loadingFile: null,
    error: null,
  });

  // Counter to ignore stale async loads after rapid file selection.
  let selectionGeneration = 0;

  // =========================================================================
  // Actions
  // =========================================================================

  /** Load the file list. Called once on creation. */
  async function loadFiles(): Promise<void> {
    state.loading = true;
    state.error = null;

    try {
      const response = await commands.getDiffFiles(
        state.branchId,
        state.commitSha ?? undefined,
        state.scope
      );

      state.commitSha = response.commitSha;
      state.files = response.files;

      // Auto-select first file
      if (state.files.length > 0) {
        await selectFile(fileSummaryPath(state.files[0]));
      }
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
      state.files = [];
    } finally {
      state.loading = false;
    }
  }

  /**
   * Select a file by path and load its diff if not cached.
   */
  async function selectFile(path: string | null): Promise<void> {
    const thisGeneration = ++selectionGeneration;
    state.selectedFile = path;

    if (path && !state.diffCache.has(path)) {
      await loadFileDiff(path);
      // Ignore if user selected another file while we were loading.
      if (selectionGeneration !== thisGeneration) return;
    }
  }

  /**
   * Load a single file's diff content and cache it.
   */
  async function loadFileDiff(path: string): Promise<FileDiff | null> {
    if (!state.commitSha) return null;

    const cached = state.diffCache.get(path);
    if (cached) return cached;

    state.loadingFile = path;

    try {
      const diff = await commands.getFileDiff(state.branchId, state.commitSha, state.scope, path);
      // New Map for reactivity.
      const newCache = new Map(state.diffCache);
      newCache.set(path, diff);
      state.diffCache = newCache;
      return diff;
    } catch (e) {
      console.error(`Failed to load diff for ${path}:`, e);
      return null;
    } finally {
      state.loadingFile = null;
    }
  }

  /** Get the cached diff for the currently selected file. */
  function getCurrentDiff(): FileDiff | null {
    if (!state.selectedFile) return null;
    return state.diffCache.get(state.selectedFile) ?? null;
  }

  // Kick off initial load.
  loadFiles();

  return {
    state,
    selectFile,
    getCurrentDiff,
  };
}
