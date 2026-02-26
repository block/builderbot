import type { SearchStateHandle } from '../state/searchState.svelte';
import type { FileDiffSummary } from '../types';

export interface SearchNavigationConfig {
	searchState: SearchStateHandle;
	selectFile: (path: string) => Promise<void>;
	files: FileDiffSummary[];
	onJumpToLine: (lineIndex: number) => void;
}

/**
 * Creates navigation handlers for search results that automatically expand collapsed
 * results and jump to the matched line.
 */
export function createSearchNavigationHandlers(config: SearchNavigationConfig) {
	const { searchState, selectFile, files, onJumpToLine } = config;

	async function onNextSearchResult() {
		const result = await searchState.goToNextResult(files);
		if (result) {
			// Auto-expand search results for this file
			if (searchState.areSearchResultsCollapsed(result.filePath)) {
				searchState.toggleSearchResults(result.filePath);
			}
			await selectFile(result.filePath);
			// Scroll to the specific line
			onJumpToLine(result.match.lineIndex);
		}
	}

	async function onPrevSearchResult() {
		const result = await searchState.goToPrevResult(files);
		if (result) {
			// Auto-expand search results for this file
			if (searchState.areSearchResultsCollapsed(result.filePath)) {
				searchState.toggleSearchResults(result.filePath);
			}
			await selectFile(result.filePath);
			// Scroll to the specific line
			onJumpToLine(result.match.lineIndex);
		}
	}

	return {
		onNextSearchResult,
		onPrevSearchResult
	};
}
