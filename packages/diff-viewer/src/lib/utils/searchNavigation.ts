import type { SearchStateHandle } from '../state/searchState.svelte';
import type { FileDiffSummary } from '../types';

export interface SearchNavigationConfig {
  searchState: SearchStateHandle;
  selectFile: (path: string) => Promise<boolean | void>;
  getFiles: () => FileDiffSummary[];
  onJumpToLine: (lineIndex: number) => void;
}

/**
 * Creates navigation handlers for search results that automatically expand collapsed
 * results and jump to the matched line.
 */
export function createSearchNavigationHandlers(config: SearchNavigationConfig) {
  const { searchState, selectFile, getFiles, onJumpToLine } = config;

  async function navigateToResult(
    getResult: () => Promise<{ filePath: string; match: { lineIndex: number } } | null>
  ) {
    const previousResultIndex = searchState.state.currentResultIndex;
    const result = await getResult();
    if (result) {
      // Auto-expand search results for this file
      const wasCollapsed = searchState.areSearchResultsCollapsed(result.filePath);
      if (wasCollapsed) {
        searchState.toggleSearchResults(result.filePath);
      }
      const selected = (await selectFile(result.filePath)) !== false;
      if (!selected) {
        searchState.setCurrentResult(previousResultIndex);
        if (wasCollapsed) {
          searchState.toggleSearchResults(result.filePath);
        }
        return;
      }
      // Scroll to the specific line
      onJumpToLine(result.match.lineIndex);
    }
  }

  async function onNextSearchResult() {
    await navigateToResult(() => searchState.goToNextResult(getFiles()));
  }

  async function onPrevSearchResult() {
    await navigateToResult(() => searchState.goToPrevResult(getFiles()));
  }

  return {
    onNextSearchResult,
    onPrevSearchResult,
  };
}
