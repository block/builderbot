/**
 * Global Search State Store
 *
 * Manages cross-file search state including query, results grouped by file,
 * and global navigation across all matches.
 */

import { findMatches, MAX_MATCHES, type SearchMatch } from '../services/diffSearch';
import { getTextLines } from '../diffUtils';
import type { FileDiff, FileDiffSummary } from '../types';

// =============================================================================
// Types
// =============================================================================

export interface FileSearchResult {
  filePath: string;
  matches: SearchMatch[];
  isLimited: boolean; // If results were truncated at MAX_MATCHES
  displayLimit: number; // How many to show (5 initially, expandable)
}

export type SearchScope = 'all' | 'changes';

export interface GlobalSearchState {
  isOpen: boolean;
  query: string;
  scope: SearchScope; // 'all' = search all lines, 'changes' = only changed lines
  fileResults: Map<string, FileSearchResult>;
  currentResultIndex: number; // Global index across all files
  totalMatches: number;
  loading: boolean;
  searchedFileCount: number;
  totalFileCount: number;
}

// =============================================================================
// Reactive State
// =============================================================================

export const globalSearchState = $state<GlobalSearchState>({
  isOpen: false,
  query: '',
  scope: 'all',
  fileResults: new Map(),
  currentResultIndex: 0,
  totalMatches: 0,
  loading: false,
  searchedFileCount: 0,
  totalFileCount: 0,
});

// =============================================================================
// Helper Types
// =============================================================================

interface FlattenedResult {
  filePath: string;
  match: SearchMatch;
  globalIndex: number;
  localIndex: number; // Index within file
}

// =============================================================================
// Actions
// =============================================================================

/**
 * Open the search bar.
 */
export function openSearch(): void {
  globalSearchState.isOpen = true;
}

/**
 * Close the search bar and clear search state.
 */
export function closeSearch(): void {
  globalSearchState.isOpen = false;
  globalSearchState.query = '';
  globalSearchState.scope = 'all';
  globalSearchState.fileResults = new Map();
  globalSearchState.currentResultIndex = 0;
  globalSearchState.totalMatches = 0;
  globalSearchState.loading = false;
  globalSearchState.searchedFileCount = 0;
  globalSearchState.totalFileCount = 0;
}

/**
 * Set the search scope.
 */
export function setSearchScope(scope: SearchScope): void {
  globalSearchState.scope = scope;
}

/**
 * Clear all search results.
 */
function clearSearch(): void {
  globalSearchState.fileResults = new Map();
  globalSearchState.currentResultIndex = 0;
  globalSearchState.totalMatches = 0;
  globalSearchState.searchedFileCount = 0;
}

/**
 * Get the primary path for a file summary.
 */
function getFilePath(file: FileDiffSummary): string {
  return file.after ?? file.before ?? '';
}

/**
 * Get set of line indices that are in changed alignments.
 */
function getChangedLineIndices(diff: FileDiff): Set<number> {
  const changedIndices = new Set<number>();

  for (const alignment of diff.alignments) {
    if (alignment.changed) {
      // Add all lines in the 'after' range of this changed alignment
      for (let i = alignment.after.start; i < alignment.after.end; i++) {
        changedIndices.add(i);
      }
    }
  }

  return changedIndices;
}

/**
 * Perform search across all files.
 */
export async function performSearch(
  query: string,
  files: FileDiffSummary[],
  loadFileDiff: (path: string) => Promise<FileDiff | null>
): Promise<void> {
  if (!query) {
    clearSearch();
    return;
  }

  globalSearchState.loading = true;
  globalSearchState.query = query;
  globalSearchState.searchedFileCount = 0;
  globalSearchState.totalFileCount = files.length;

  // Build new Map instead of mutating existing one (for Svelte 5 reactivity)
  const newResults = new Map<string, FileSearchResult>();
  let totalMatches = 0;

  const scope = globalSearchState.scope;

  for (const fileSummary of files) {
    const path = getFilePath(fileSummary);

    // Load diff (uses cache if available, loads on-demand if not)
    const diff = await loadFileDiff(path);
    globalSearchState.searchedFileCount++;

    if (!diff) continue;

    const beforeLines = getTextLines(diff, 'before');
    const afterLines = getTextLines(diff, 'after');

    // Get changed line indices if scope is 'changes'
    const changedLineIndices = scope === 'changes' ? getChangedLineIndices(diff) : undefined;

    // Reuse existing findMatches function
    const matches = findMatches(beforeLines, afterLines, query, scope, changedLineIndices);

    if (matches.length > 0) {
      newResults.set(path, {
        filePath: path,
        matches,
        isLimited: matches.length >= MAX_MATCHES,
        displayLimit: 5, // Initial display limit
      });
      totalMatches += matches.length;
    }
  }

  // Assign new Map to trigger reactivity
  globalSearchState.fileResults = newResults;
  globalSearchState.totalMatches = totalMatches;

  // Reset to first result if we have matches
  if (globalSearchState.totalMatches > 0) {
    globalSearchState.currentResultIndex = 0;
  }

  globalSearchState.loading = false;
}

/**
 * Flatten all results into a single ordered list for navigation.
 * Order follows the file list order.
 */
export function getFlattenedResults(files: FileDiffSummary[]): FlattenedResult[] {
  const flattened: FlattenedResult[] = [];
  let globalIndex = 0;

  // Iterate in file list order
  for (const fileSummary of files) {
    const path = getFilePath(fileSummary);
    const fileResult = globalSearchState.fileResults.get(path);

    if (!fileResult) continue;

    for (let localIndex = 0; localIndex < fileResult.matches.length; localIndex++) {
      flattened.push({
        filePath: path,
        match: fileResult.matches[localIndex],
        globalIndex: globalIndex++,
        localIndex,
      });
    }
  }

  return flattened;
}

/**
 * Navigate to the next search result (with wrap-around).
 * Returns the result to navigate to, or null if no results.
 */
export async function goToNextResult(
  files: FileDiffSummary[],
  loadFileDiff: (path: string) => Promise<FileDiff | null>
): Promise<{ filePath: string; match: SearchMatch; needsLoad: boolean } | null> {
  const flattened = getFlattenedResults(files);
  if (flattened.length === 0) return null;

  // Find next result (with wrap-around)
  const nextIndex = (globalSearchState.currentResultIndex + 1) % flattened.length;
  const result = flattened[nextIndex];

  // Auto-expand if result is hidden by "Show More"
  const fileResult = globalSearchState.fileResults.get(result.filePath);
  if (fileResult && result.localIndex >= fileResult.displayLimit) {
    // Expand by standard increment (10), but ensure we show at least the current result
    const newLimit = Math.max(
      result.localIndex + 1, // At minimum, show the current result
      fileResult.displayLimit + 10 // Expand by standard increment
    );
    const newResults = new Map(globalSearchState.fileResults);
    newResults.set(result.filePath, {
      ...fileResult,
      displayLimit: Math.min(newLimit, fileResult.matches.length),
    });
    globalSearchState.fileResults = newResults;
  }

  // Update current index
  globalSearchState.currentResultIndex = nextIndex;

  return {
    filePath: result.filePath,
    match: result.match,
    needsLoad: false, // Caller will check cache
  };
}

/**
 * Navigate to the previous search result (with wrap-around).
 * Returns the result to navigate to, or null if no results.
 */
export async function goToPrevResult(
  files: FileDiffSummary[],
  loadFileDiff: (path: string) => Promise<FileDiff | null>
): Promise<{ filePath: string; match: SearchMatch; needsLoad: boolean } | null> {
  const flattened = getFlattenedResults(files);
  if (flattened.length === 0) return null;

  // Find previous result (with wrap-around)
  const prevIndex =
    (globalSearchState.currentResultIndex - 1 + flattened.length) % flattened.length;
  const result = flattened[prevIndex];

  // Auto-expand if hidden
  const fileResult = globalSearchState.fileResults.get(result.filePath);
  if (fileResult && result.localIndex >= fileResult.displayLimit) {
    // Expand by standard increment (10), but ensure we show at least the current result
    const newLimit = Math.max(
      result.localIndex + 1, // At minimum, show the current result
      fileResult.displayLimit + 10 // Expand by standard increment
    );
    const newResults = new Map(globalSearchState.fileResults);
    newResults.set(result.filePath, {
      ...fileResult,
      displayLimit: Math.min(newLimit, fileResult.matches.length),
    });
    globalSearchState.fileResults = newResults;
  }

  // Update current index
  globalSearchState.currentResultIndex = prevIndex;

  return {
    filePath: result.filePath,
    match: result.match,
    needsLoad: false,
  };
}

/**
 * Expand results for a file (show more matches).
 */
export function expandFileResults(filePath: string): void {
  const fileResult = globalSearchState.fileResults.get(filePath);
  if (!fileResult) return;

  // Expand by 10 more, or show all
  const newLimit = Math.min(fileResult.displayLimit + 10, fileResult.matches.length);

  // Create new Map to trigger reactivity
  const newResults = new Map(globalSearchState.fileResults);
  newResults.set(filePath, {
    ...fileResult,
    displayLimit: newLimit,
  });
  globalSearchState.fileResults = newResults;
}

/**
 * Collapse results for a file (reset to initial limit).
 */
export function collapseFileResults(filePath: string): void {
  const fileResult = globalSearchState.fileResults.get(filePath);
  if (!fileResult) return;

  // Create new Map to trigger reactivity
  const newResults = new Map(globalSearchState.fileResults);
  newResults.set(filePath, {
    ...fileResult,
    displayLimit: 5, // Reset to initial
  });
  globalSearchState.fileResults = newResults;
}

/**
 * Check if a specific result is the current one.
 */
export function isCurrentResult(
  files: FileDiffSummary[],
  filePath: string,
  localIndex: number
): boolean {
  const flattened = getFlattenedResults(files);
  const current = flattened[globalSearchState.currentResultIndex];

  if (!current) return false;

  return current.filePath === filePath && current.localIndex === localIndex;
}

/**
 * Get the global index for a specific file and local match index.
 */
export function getGlobalIndex(
  files: FileDiffSummary[],
  filePath: string,
  localIndex: number
): number {
  const flattened = getFlattenedResults(files);

  const result = flattened.find((r) => r.filePath === filePath && r.localIndex === localIndex);

  return result?.globalIndex ?? -1;
}

/**
 * Set the current result by global index.
 */
export function setCurrentResult(globalIndex: number): void {
  globalSearchState.currentResultIndex = globalIndex;
}
