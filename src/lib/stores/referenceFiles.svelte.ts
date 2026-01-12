/**
 * Reference Files Store
 *
 * Manages files that are pinned for viewing outside the current diff.
 * These are files from the repository that weren't changed in the diff
 * but the user wants to view/comment on during the review.
 *
 * Reference file paths are persisted in the review DB, but content is
 * fetched fresh when needed (to avoid storing large file contents).
 */

import type { FileContent, DiffSpec } from '../types';
import { getFileAtRef } from '../services/files';
import { addReferenceFilePath, removeReferenceFilePath } from '../services/review';

// =============================================================================
// State
// =============================================================================

export interface ReferenceFile {
  /** File path in the repository */
  path: string;
  /** File content (text lines or binary marker) */
  content: FileContent;
}

interface ReferenceFilesState {
  /** Pinned reference files with loaded content */
  files: ReferenceFile[];
  /** Loading state for file fetching */
  loading: boolean;
  /** Error message if loading failed */
  error: string | null;
}

export const referenceFilesState: ReferenceFilesState = $state({
  files: [],
  loading: false,
  error: null,
});

// =============================================================================
// Getters
// =============================================================================

/**
 * Check if a path is a reference file (not a diff file).
 */
export function isReferenceFile(path: string): boolean {
  return referenceFilesState.files.some((f) => f.path === path);
}

/**
 * Get a reference file by path.
 */
export function getReferenceFile(path: string): ReferenceFile | undefined {
  return referenceFilesState.files.find((f) => f.path === path);
}

/**
 * Get all reference file paths.
 */
export function getReferenceFilePaths(): string[] {
  return referenceFilesState.files.map((f) => f.path);
}

// =============================================================================
// Actions
// =============================================================================

/**
 * Add a reference file by loading it from the repository.
 * Also persists the path to the review DB.
 *
 * @param refName - The git ref to load the file from (e.g., HEAD, branch name, SHA)
 * @param path - The file path in the repository
 * @param spec - The current diff spec (for persistence)
 * @param repoPath - Optional repository path
 */
export async function addReferenceFile(
  refName: string,
  path: string,
  spec: DiffSpec,
  repoPath?: string
): Promise<void> {
  // Don't add duplicates
  if (isReferenceFile(path)) {
    return;
  }

  referenceFilesState.loading = true;
  referenceFilesState.error = null;

  try {
    // Load file content
    const file = await getFileAtRef(refName, path, repoPath);
    referenceFilesState.files = [
      ...referenceFilesState.files,
      { path: file.path, content: file.content },
    ];

    // Persist the path to the review DB
    await addReferenceFilePath(spec, path, repoPath);
  } catch (e) {
    referenceFilesState.error = e instanceof Error ? e.message : String(e);
    throw e; // Re-throw so caller can handle
  } finally {
    referenceFilesState.loading = false;
  }
}

/**
 * Remove a reference file.
 * Also removes the path from the review DB.
 *
 * @param path - The file path to remove
 * @param spec - The current diff spec (for persistence)
 * @param repoPath - Optional repository path
 */
export async function removeReferenceFile(
  path: string,
  spec: DiffSpec,
  repoPath?: string
): Promise<void> {
  referenceFilesState.files = referenceFilesState.files.filter((f) => f.path !== path);

  // Remove from the review DB (fire and forget, don't block UI)
  removeReferenceFilePath(spec, path, repoPath).catch((e) => {
    console.error('Failed to remove reference file from DB:', e);
  });
}

/**
 * Load reference files from persisted paths.
 * Called when loading a review to restore previously added reference files.
 *
 * @param paths - The persisted reference file paths
 * @param refName - The git ref to load files from
 * @param repoPath - Optional repository path
 */
export async function loadReferenceFiles(
  paths: string[],
  refName: string,
  repoPath?: string
): Promise<void> {
  if (paths.length === 0) {
    referenceFilesState.files = [];
    return;
  }

  referenceFilesState.loading = true;
  referenceFilesState.error = null;

  try {
    // Load all files in parallel
    const results = await Promise.allSettled(
      paths.map(async (path) => {
        const file = await getFileAtRef(refName, path, repoPath);
        return { path: file.path, content: file.content };
      })
    );

    // Collect successfully loaded files
    const loadedFiles: ReferenceFile[] = [];
    for (const result of results) {
      if (result.status === 'fulfilled') {
        loadedFiles.push(result.value);
      } else {
        console.warn('Failed to load reference file:', result.reason);
      }
    }

    referenceFilesState.files = loadedFiles;
  } catch (e) {
    referenceFilesState.error = e instanceof Error ? e.message : String(e);
  } finally {
    referenceFilesState.loading = false;
  }
}

/**
 * Clear all reference files.
 * Call this when the diff spec changes.
 */
export function clearReferenceFiles(): void {
  referenceFilesState.files = [];
  referenceFilesState.error = null;
}

/**
 * Reset error state.
 */
export function clearReferenceFilesError(): void {
  referenceFilesState.error = null;
}
