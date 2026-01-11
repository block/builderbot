/**
 * Diff State Store
 *
 * Manages the file list and on-demand diff loading.
 *
 * New pattern:
 * - Load file list (fast) via list_diff_files
 * - Load individual file diffs on demand via get_file_diff
 * - Cache loaded diffs for the current spec
 *
 * Rebuildable: This module owns diff loading state. Components import
 * the reactive state object directly.
 */

import { listDiffFiles, getFileDiff } from '../services/git';
import type { DiffSpec, FileDiffSummary, FileDiff } from '../types';

// =============================================================================
// Reactive State
// =============================================================================

export const diffState = $state({
  /** Current diff spec (needed for on-demand loading) */
  currentSpec: null as DiffSpec | null,
  /** Current repo path */
  currentRepoPath: null as string | null,
  /** File summaries for the sidebar */
  files: [] as FileDiffSummary[],
  /** Cached full diffs by path */
  diffCache: new Map<string, FileDiff>(),
  /** Currently selected file path */
  selectedFile: null as string | null,
  /** Target line to scroll to after file selection (0-indexed, null = no scroll) */
  scrollTargetLine: null as number | null,
  /** Whether file list is loading */
  loading: true,
  /** Whether a specific file diff is loading */
  loadingFile: null as string | null,
  /** Error message if loading failed */
  error: null as string | null,
});

// =============================================================================
// Helpers
// =============================================================================

/** Get the primary path for a file summary */
function getFilePath(summary: FileDiffSummary): string {
  return summary.after ?? summary.before ?? '';
}

/**
 * Apply selection logic after loading files.
 * Returns the path that should be loaded (if any).
 */
function updateSelection(): string | null {
  let pathToLoad: string | null = null;

  // Auto-select first file if none selected
  if (!diffState.selectedFile && diffState.files.length > 0) {
    diffState.selectedFile = getFilePath(diffState.files[0]);
    pathToLoad = diffState.selectedFile;
  }

  // Check if currently selected file still exists
  if (diffState.selectedFile) {
    const stillExists = diffState.files.some((f) => getFilePath(f) === diffState.selectedFile);
    if (!stillExists) {
      diffState.selectedFile = diffState.files.length > 0 ? getFilePath(diffState.files[0]) : null;
      pathToLoad = diffState.selectedFile;
    }
  }

  return pathToLoad;
}

// =============================================================================
// Getters
// =============================================================================

/**
 * Get the cached diff for the currently selected file, or null if not loaded.
 */
export function getCurrentDiff(): FileDiff | null {
  if (!diffState.selectedFile) return null;
  return diffState.diffCache.get(diffState.selectedFile) ?? null;
}

/**
 * Get a cached diff by path.
 */
export function getCachedDiff(path: string): FileDiff | null {
  return diffState.diffCache.get(path) ?? null;
}

// =============================================================================
// Actions
// =============================================================================

/**
 * Load file list for the given spec.
 * Clears the diff cache since we're loading a new spec.
 */
export async function loadFiles(spec: DiffSpec, repoPath?: string): Promise<void> {
  diffState.loading = true;
  diffState.error = null;
  diffState.currentSpec = spec;
  diffState.currentRepoPath = repoPath ?? null;
  diffState.diffCache = new Map();

  try {
    diffState.files = await listDiffFiles(spec, repoPath);
    const pathToLoad = updateSelection();
    // Load the diff for the auto-selected file
    if (pathToLoad) {
      await loadFileDiff(pathToLoad);
    }
  } catch (e) {
    diffState.error = e instanceof Error ? e.message : String(e);
    diffState.files = [];
  } finally {
    diffState.loading = false;
  }
}

/**
 * Refresh file list without showing loading state.
 * Clears the diff cache since file contents may have changed.
 */
export async function refreshFiles(spec: DiffSpec, repoPath?: string): Promise<void> {
  try {
    const newFiles = await listDiffFiles(spec, repoPath);

    diffState.files = newFiles;
    diffState.currentSpec = spec;
    diffState.currentRepoPath = repoPath ?? null;

    // updateSelection() handles auto-select and checks if selected file still exists
    updateSelection();

    // Reload the selected file's diff if it exists
    // Don't clear cache first - fetch new diff, then swap atomically to avoid flicker
    if (diffState.selectedFile) {
      const diff = await getFileDiff(spec, diffState.selectedFile, repoPath);
      const newCache = new Map<string, FileDiff>();
      newCache.set(diffState.selectedFile, diff);
      diffState.diffCache = newCache;
    } else {
      diffState.diffCache = new Map();
    }
  } catch (e) {
    // On refresh errors, keep existing state
    console.error('Refresh failed:', e);
  }
}

/**
 * Load a specific file's diff content.
 * Returns the diff, also caches it.
 */
export async function loadFileDiff(path: string): Promise<FileDiff | null> {
  if (!diffState.currentSpec) return null;

  // Return cached if available
  const cached = diffState.diffCache.get(path);
  if (cached) return cached;

  diffState.loadingFile = path;

  try {
    const diff = await getFileDiff(
      diffState.currentSpec,
      path,
      diffState.currentRepoPath ?? undefined
    );
    // Create a new Map to trigger Svelte reactivity
    const newCache = new Map(diffState.diffCache);
    newCache.set(path, diff);
    diffState.diffCache = newCache;
    return diff;
  } catch (e) {
    console.error(`Failed to load diff for ${path}:`, e);
    return null;
  } finally {
    diffState.loadingFile = null;
  }
}

/**
 * Clear the scroll target (called after scrolling completes).
 */
export function clearScrollTarget(): void {
  diffState.scrollTargetLine = null;
}

/** Counter to track the current selection and ignore stale async results */
let selectionId = 0;

/**
 * Select a file by path, optionally scrolling to a specific line.
 * Triggers loading the diff if not cached.
 * Handles rapid selection changes by ignoring stale loads.
 */
export async function selectFile(path: string | null, scrollToLine?: number): Promise<void> {
  const thisSelection = ++selectionId;
  diffState.selectedFile = path;
  diffState.scrollTargetLine = scrollToLine ?? null;
  if (path && !diffState.diffCache.has(path)) {
    await loadFileDiff(path);
    // If user selected a different file while we were loading, don't update
    if (selectionId !== thisSelection) return;
  }
}

/**
 * Invalidate a specific file's cached diff (e.g., after edit).
 */
export function invalidateFile(path: string): void {
  diffState.diffCache.delete(path);
}

/**
 * Reset all state (for spec changes).
 */
export function resetState(): void {
  diffState.selectedFile = null;
  diffState.files = [];
  diffState.diffCache = new Map();
  diffState.error = null;
  diffState.loading = true;
  diffState.loadingFile = null;
  diffState.currentSpec = null;
  diffState.currentRepoPath = null;
}
