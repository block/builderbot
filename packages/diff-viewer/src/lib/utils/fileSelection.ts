import type { SearchStateHandle } from '../state/searchState.svelte';
import type { FileDiffSummary } from '../types';

export interface FileSelectionWithSearchConfig {
	searchState: SearchStateHandle;
	getFiles: () => FileDiffSummary[];
}

/**
 * Creates a wrapper for file selection that automatically handles search-related behavior:
 * - Auto-expands collapsed search results when selecting a file with matches
 * - Selects the first search result in the file if search is open
 *
 * This should be called BEFORE the actual file selection logic to ensure proper timing.
 *
 * @example
 * ```ts
 * const handleSearchOnFileSelect = createFileSelectionWithSearch({
 *   searchState,
 *   getFiles: () => diffViewer.state.files
 * });
 *
 * function selectFile(path: string) {
 *   // Handle search-related behavior first
 *   handleSearchOnFileSelect(path);
 *   // Then do the actual file selection
 *   diffViewer.selectFile(path);
 * }
 * ```
 */
export function createFileSelectionWithSearch(config: FileSelectionWithSearchConfig) {
	const { searchState, getFiles } = config;

	/**
	 * Handles search-related logic when a file is selected.
	 * Call this before updating the selected file state.
	 */
	return (filePath: string) => {
		if (!searchState.state.isOpen) {
			return;
		}

		// Auto-expand search results if they are collapsed
		if (searchState.areSearchResultsCollapsed(filePath)) {
			searchState.toggleSearchResults(filePath);
		}

		// Select the first search result in this file
		searchState.selectFirstResultInFile(getFiles(), filePath);
	};
}
