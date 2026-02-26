import type { SearchStateHandle } from '../state/searchState.svelte';
import type { FileDiffSummary } from '../types';

export interface SearchInitializationConfig {
	searchState: SearchStateHandle;
	files: FileDiffSummary[];
}

/**
 * Tracks whether the search collapsed state has been initialized for the current search.
 * Returns a function that should be called in a Svelte $effect to automatically initialize
 * the collapsed state when search results are ready.
 *
 * This ensures the collapsed state is only initialized once per unique search (based on
 * query and result count), preventing it from resetting when the user interacts with files.
 *
 * @example
 * ```ts
 * let searchInitializedKey = $state<string>('');
 * const checkSearchInitialization = createSearchInitializationTracker({
 *   searchState,
 *   files
 * });
 *
 * $effect(() => {
 *   const newKey = checkSearchInitialization();
 *   if (newKey) {
 *     searchInitializedKey = newKey;
 *   }
 * });
 * ```
 */
export function createSearchInitializationTracker(config: SearchInitializationConfig) {
	const { searchState, files } = config;
	let lastInitializedKey = '';

	/**
	 * Check if search initialization is needed and perform it if so.
	 * Returns the new search key if initialization was performed, null otherwise.
	 */
	return (): string | null => {
		const searchKey = `${searchState.state.query}-${searchState.state.fileResults.size}`;

		if (
			searchState.state.isOpen &&
			searchState.state.fileResults.size > 0 &&
			!searchState.state.loading &&
			lastInitializedKey !== searchKey
		) {
			searchState.initializeCollapsedState(files);
			lastInitializedKey = searchKey;
			return searchKey;
		}

		return null;
	};
}
