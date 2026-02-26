<!--
  FileSearchResults.svelte - Display search results inline under a file

  Shows collapsible search results for a single file with:
  - Individual result items with snippets
  - Pagination (Show more/less buttons)
  - Click handlers for navigation
  - Current result highlighting
-->
<script lang="ts">
	import type { SearchMatch } from '../utils/diffSearch';
	import type { FileSearchResult } from '../state/searchState.svelte';
	import SearchResultItem from './SearchResultItem.svelte';

	interface Props {
		fileResult: FileSearchResult;
		filePath: string;
		depth: number; // Indentation depth (for tree views)
		getSnippet: (match: SearchMatch, filePath: string) => string;
		isCurrentResult: (filePath: string, localIndex: number) => boolean;
		onResultClick: (filePath: string, match: SearchMatch, globalIndex: number) => void;
		getGlobalIndex: (filePath: string, localIndex: number) => number;
		onExpandResults: (filePath: string) => void;
		onCollapseResults: (filePath: string) => void;
	}

	let {
		fileResult,
		filePath,
		depth = 0,
		getSnippet,
		isCurrentResult,
		onResultClick,
		getGlobalIndex,
		onExpandResults,
		onCollapseResults
	}: Props = $props();
</script>

<div class="search-results-container" style="margin-left: {8 + (depth + 1) * 12}px">
	{#each fileResult.matches.slice(0, fileResult.displayLimit) as match, i}
		{@const snippet = getSnippet(match, filePath)}
		{@const isCurrent = isCurrentResult(filePath, i)}
		{@const globalIndex = getGlobalIndex(filePath, i)}
		<SearchResultItem
			{match}
			{snippet}
			{isCurrent}
			onclick={() => onResultClick(filePath, match, globalIndex)}
		/>
	{/each}

	{#if fileResult.matches.length > fileResult.displayLimit}
		<button class="show-more-btn" onclick={() => onExpandResults(filePath)}>
			Show {fileResult.matches.length - fileResult.displayLimit} more
		</button>
	{:else if fileResult.displayLimit > 5}
		<button class="show-less-btn" onclick={() => onCollapseResults(filePath)}>
			Show less
		</button>
	{/if}
</div>

<style>
	.search-results-container {
		display: flex;
		flex-direction: column;
		background-color: var(--bg-secondary);
		border-left: 2px solid var(--border-subtle);
		margin-left: 8px;
	}

	.show-more-btn,
	.show-less-btn {
		display: block;
		width: 100%;
		padding: 4px 12px;
		background: none;
		border: none;
		color: var(--text-faint);
		font-size: var(--size-xs);
		text-align: center;
		cursor: pointer;
		transition: background-color 0.1s;
	}

	.show-more-btn:hover,
	.show-less-btn:hover {
		background-color: var(--bg-hover);
		color: var(--text-muted);
	}
</style>
