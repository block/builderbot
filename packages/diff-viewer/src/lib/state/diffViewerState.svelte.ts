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

  // Counter to ignore stale context switches (e.g. user rapidly clicks commit A then B).
  let contextGeneration = 0;

  // =========================================================================
  // Actions
  // =========================================================================

  /** Load the file list. Called once on creation, and again on context switch. */
  async function loadFiles(generation: number): Promise<void> {
    state.loading = true;
    state.error = null;

    try {
      const t0 = performance.now();
      console.info('[diff] getDiffFiles start', { branchId: state.branchId, scope: state.scope, commitSha: state.commitSha });
      const response = await commands.getDiffFiles(
        state.branchId,
        state.commitSha ?? undefined,
        state.scope
      );
      console.info('[diff] getDiffFiles done', { files: response.files.length, elapsed: `${(performance.now() - t0).toFixed(1)}ms` });

      // Discard result if a newer context switch happened while we were loading.
      if (generation !== contextGeneration) return;

      console.info('[diff] getDiffFiles done', { files: response.files.length, elapsed: `${(performance.now() - t0).toFixed(1)}ms` });

      state.commitSha = response.commitSha;
      state.files = response.files;

      // Auto-select first file
      if (state.files.length > 0) {
        await selectFile(fileSummaryPath(state.files[0]));
      }
    } catch (e) {
      // Discard error if a newer context switch happened while we were loading.
      if (generation !== contextGeneration) return;
      state.error = e instanceof Error ? e.message : String(e);
      state.files = [];
    } finally {
      if (generation === contextGeneration) {
        state.loading = false;
      }
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
      const t0 = performance.now();
      console.info('[diff] getFileDiff start', { path, commitSha: state.commitSha, scope: state.scope });
      const diff = await commands.getFileDiff(state.branchId, state.commitSha, state.scope, path);
      const beforeLineCount = diff.before?.content?.type === 'Text' ? diff.before.content.lines.length : 0;
      const afterLineCount = diff.after?.content?.type === 'Text' ? diff.after.content.lines.length : 0;
      const beforeMaxLen = diff.before?.content?.type === 'Text' ? diff.before.content.lines.reduce((max: number, l: string) => Math.max(max, l.length), 0) : 0;
      const afterMaxLen = diff.after?.content?.type === 'Text' ? diff.after.content.lines.reduce((max: number, l: string) => Math.max(max, l.length), 0) : 0;
      console.info('[diff] getFileDiff done', {
        path,
        elapsed: `${(performance.now() - t0).toFixed(1)}ms`,
        beforeLines: beforeLineCount,
        afterLines: afterLineCount,
        beforeMaxLineLen: beforeMaxLen,
        afterMaxLineLen: afterMaxLen,
        alignments: diff.alignments.length,
      });
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

  /**
   * Switch to a different diff context (branch vs commit scope).
   * Clears the cache and reloads the file list.
   *
   * Uses a generation counter so that rapid switches (e.g. user clicks
   * commit A then immediately commit B) don't race — only the latest
   * loadFiles result is applied.
   */
  async function switchContext(
    newScope: 'branch' | 'commit',
    newCommitSha?: string
  ): Promise<void> {
    const generation = ++contextGeneration;
    state.scope = newScope;
    state.commitSha = newCommitSha ?? null;
    state.diffCache = new Map();
    state.selectedFile = null;
    state.loadingFile = null;
    state.files = [];
    state.error = null;
    await loadFiles(generation);
  }

  // Kick off initial load.
  loadFiles(contextGeneration);

  return {
    state,
    selectFile,
    loadFileDiff,
    getCurrentDiff,
    switchContext,
  };
}
