/**
 * Search State Factory
 *
 * Manages cross-file search state including query, results grouped by file,
 * and global navigation across all matches.
 */

import { findMatches, MAX_MATCHES, type SearchMatch } from '../utils/diffSearch';
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

export interface SearchState {
	isOpen: boolean;
	query: string;
	scope: SearchScope; // 'all' = search all lines, 'changes' = only changed lines
	fileResults: Map<string, FileSearchResult>;
	currentResultIndex: number; // Global index across all files
	totalMatches: number;
	loading: boolean;
	searchedFileCount: number;
	totalFileCount: number;
	collapsedSearchResults: Set<string>; // File paths with collapsed search results
	focusTrigger: number; // Increment to trigger input focus
}

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
// Utility Functions
// =============================================================================

/**
 * Get the primary path for a file summary.
 */
function getFilePath(file: FileDiffSummary): string {
	return file.after ?? file.before ?? '';
}

/**
 * Get text lines from a diff's file content.
 */
function getTextLines(diff: FileDiff, side: 'before' | 'after'): string[] {
	const file = side === 'before' ? diff.before : diff.after;
	if (!file) return [];

	const content = file.content;
	if (content.type === 'Binary') return [];

	return content.lines;
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

// =============================================================================
// Factory Function
// =============================================================================

export function createSearchState() {
	// Reactive State
	const state = $state<SearchState>({
		isOpen: false,
		query: '',
		scope: 'all',
		fileResults: new Map(),
		currentResultIndex: 0,
		totalMatches: 0,
		loading: false,
		searchedFileCount: 0,
		totalFileCount: 0,
		collapsedSearchResults: new Set(),
		focusTrigger: 0
	});

	// =============================================================================
	// Actions
	// =============================================================================

	/**
	 * Open the search bar and trigger focus.
	 */
	function openSearch(): void {
		state.isOpen = true;
		state.focusTrigger++;
	}

	/**
	 * Close the search bar and clear search state.
	 */
	function closeSearch(): void {
		state.isOpen = false;
		state.query = '';
		state.scope = 'all';
		state.fileResults = new Map();
		state.currentResultIndex = 0;
		state.totalMatches = 0;
		state.loading = false;
		state.searchedFileCount = 0;
		state.totalFileCount = 0;
		state.collapsedSearchResults = new Set();
	}

	/**
	 * Set the search scope.
	 */
	function setSearchScope(scope: SearchScope): void {
		state.scope = scope;
	}

	/**
	 * Clear all search results.
	 */
	function clearSearch(): void {
		state.fileResults = new Map();
		state.currentResultIndex = 0;
		state.totalMatches = 0;
		state.searchedFileCount = 0;
	}

	/**
	 * Perform search across all files.
	 */
	async function performSearch(
		query: string,
		files: FileDiffSummary[],
		loadFileDiff: (path: string) => Promise<FileDiff | null>
	): Promise<void> {
		if (!query) {
			clearSearch();
			return;
		}

		state.loading = true;
		state.query = query;
		state.searchedFileCount = 0;
		state.totalFileCount = files.length;

		// Build new Map instead of mutating existing one (for Svelte 5 reactivity)
		const newResults = new Map<string, FileSearchResult>();
		let totalMatches = 0;

		const scope = state.scope;

		for (const fileSummary of files) {
			const path = getFilePath(fileSummary);

			// Load diff (uses cache if available, loads on-demand if not)
			const diff = await loadFileDiff(path);
			state.searchedFileCount++;

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
					displayLimit: 5 // Initial display limit
				});
				totalMatches += matches.length;
			}
		}

		// Update state once at the end to avoid UI flashing during search
		state.fileResults = newResults;
		state.totalMatches = totalMatches;

		// Reset to first result if we have matches
		if (state.totalMatches > 0) {
			state.currentResultIndex = 0;
		}

		state.loading = false;
	}

	/**
	 * Flatten all results into a single ordered list for navigation.
	 * Order follows the file list order.
	 */
	function getFlattenedResults(files: FileDiffSummary[]): FlattenedResult[] {
		const flattened: FlattenedResult[] = [];
		let globalIndex = 0;

		// Iterate in file list order
		for (const fileSummary of files) {
			const path = getFilePath(fileSummary);
			const fileResult = state.fileResults.get(path);

			if (!fileResult) continue;

			for (let localIndex = 0; localIndex < fileResult.matches.length; localIndex++) {
				flattened.push({
					filePath: path,
					match: fileResult.matches[localIndex],
					globalIndex: globalIndex++,
					localIndex
				});
			}
		}

		return flattened;
	}

	/**
	 * Navigate to the next search result (with wrap-around).
	 * Returns the result to navigate to, or null if no results.
	 */
	async function goToNextResult(
		files: FileDiffSummary[]
	): Promise<{ filePath: string; match: SearchMatch; needsLoad: boolean } | null> {
		const flattened = getFlattenedResults(files);
		if (flattened.length === 0) return null;

		// Find next result (with wrap-around)
		const nextIndex = (state.currentResultIndex + 1) % flattened.length;
		const result = flattened[nextIndex];

		// Auto-expand if result is hidden by "Show More"
		const fileResult = state.fileResults.get(result.filePath);
		if (fileResult && result.localIndex >= fileResult.displayLimit) {
			// Expand by standard increment (10), but ensure we show at least the current result
			const newLimit = Math.max(
				result.localIndex + 1, // At minimum, show the current result
				fileResult.displayLimit + 10 // Expand by standard increment
			);
			const newResults = new Map(state.fileResults);
			newResults.set(result.filePath, {
				...fileResult,
				displayLimit: Math.min(newLimit, fileResult.matches.length)
			});
			state.fileResults = newResults;
		}

		// Update current index
		state.currentResultIndex = nextIndex;

		return {
			filePath: result.filePath,
			match: result.match,
			needsLoad: false // Caller will check cache
		};
	}

	/**
	 * Navigate to the previous search result (with wrap-around).
	 * Returns the result to navigate to, or null if no results.
	 */
	async function goToPrevResult(
		files: FileDiffSummary[]
	): Promise<{ filePath: string; match: SearchMatch; needsLoad: boolean } | null> {
		const flattened = getFlattenedResults(files);
		if (flattened.length === 0) return null;

		// Find previous result (with wrap-around)
		const prevIndex = (state.currentResultIndex - 1 + flattened.length) % flattened.length;
		const result = flattened[prevIndex];

		// Auto-expand if hidden
		const fileResult = state.fileResults.get(result.filePath);
		if (fileResult && result.localIndex >= fileResult.displayLimit) {
			// Expand by standard increment (10), but ensure we show at least the current result
			const newLimit = Math.max(
				result.localIndex + 1, // At minimum, show the current result
				fileResult.displayLimit + 10 // Expand by standard increment
			);
			const newResults = new Map(state.fileResults);
			newResults.set(result.filePath, {
				...fileResult,
				displayLimit: Math.min(newLimit, fileResult.matches.length)
			});
			state.fileResults = newResults;
		}

		// Update current index
		state.currentResultIndex = prevIndex;

		return {
			filePath: result.filePath,
			match: result.match,
			needsLoad: false
		};
	}

	/**
	 * Expand results for a file (show more matches).
	 */
	function expandFileResults(filePath: string): void {
		const fileResult = state.fileResults.get(filePath);
		if (!fileResult) return;

		// Expand by 10 more, or show all
		const newLimit = Math.min(fileResult.displayLimit + 10, fileResult.matches.length);

		// Create new Map to trigger reactivity
		const newResults = new Map(state.fileResults);
		newResults.set(filePath, {
			...fileResult,
			displayLimit: newLimit
		});
		state.fileResults = newResults;
	}

	/**
	 * Collapse results for a file (reset to initial limit).
	 */
	function collapseFileResults(filePath: string): void {
		const fileResult = state.fileResults.get(filePath);
		if (!fileResult) return;

		// Create new Map to trigger reactivity
		const newResults = new Map(state.fileResults);
		newResults.set(filePath, {
			...fileResult,
			displayLimit: 5 // Reset to initial
		});
		state.fileResults = newResults;
	}

	/**
	 * Check if a specific result is the current one.
	 */
	function isCurrentResult(files: FileDiffSummary[], filePath: string, localIndex: number): boolean {
		const flattened = getFlattenedResults(files);
		const current = flattened[state.currentResultIndex];

		if (!current) return false;

		return current.filePath === filePath && current.localIndex === localIndex;
	}

	/**
	 * Get the global index for a specific file and local match index.
	 */
	function getGlobalIndex(files: FileDiffSummary[], filePath: string, localIndex: number): number {
		const flattened = getFlattenedResults(files);

		const result = flattened.find((r) => r.filePath === filePath && r.localIndex === localIndex);

		return result?.globalIndex ?? -1;
	}

	/**
	 * Set the current result by global index.
	 */
	function setCurrentResult(globalIndex: number): void {
		state.currentResultIndex = globalIndex;
	}

	/**
	 * Toggle the collapsed state of search results for a file.
	 */
	function toggleSearchResults(filePath: string): void {
		const newCollapsed = new Set(state.collapsedSearchResults);
		if (newCollapsed.has(filePath)) {
			newCollapsed.delete(filePath);
		} else {
			newCollapsed.add(filePath);
		}
		state.collapsedSearchResults = newCollapsed;
	}

	/**
	 * Check if search results for a file are collapsed.
	 */
	function areSearchResultsCollapsed(filePath: string): boolean {
		return state.collapsedSearchResults.has(filePath);
	}

	/**
	 * Initialize collapsed state after a search.
	 * Collapses all files except the first result's file.
	 */
	function initializeCollapsedState(files: FileDiffSummary[]): void {
		if (!state.isOpen || state.fileResults.size === 0) return;

		const flattened = getFlattenedResults(files);
		const firstResultPath = flattened.length > 0 ? flattened[0].filePath : null;

		const newCollapsed = new Set<string>();
		for (const filePath of state.fileResults.keys()) {
			// Collapse all except the first result's file
			if (filePath !== firstResultPath) {
				newCollapsed.add(filePath);
			}
		}

		state.collapsedSearchResults = newCollapsed;
	}

	/**
	 * Select the first search result for a given file.
	 * If the file has search results, sets the current result to the first match.
	 * Returns true if a result was selected, false otherwise.
	 */
	function selectFirstResultInFile(files: FileDiffSummary[], filePath: string): boolean {
		if (!state.isOpen || !state.fileResults.has(filePath)) return false;

		const flattened = getFlattenedResults(files);
		const firstResult = flattened.find((r) => r.filePath === filePath);

		if (firstResult) {
			state.currentResultIndex = firstResult.globalIndex;
			return true;
		}

		return false;
	}

	return {
		get state() {
			return state;
		},
		openSearch,
		closeSearch,
		setSearchScope,
		performSearch,
		getFlattenedResults,
		goToNextResult,
		goToPrevResult,
		expandFileResults,
		collapseFileResults,
		isCurrentResult,
		getGlobalIndex,
		setCurrentResult,
		toggleSearchResults,
		areSearchResultsCollapsed,
		initializeCollapsedState,
		selectFirstResultInFile
	};
}

export type SearchStateHandle = ReturnType<typeof createSearchState>;
