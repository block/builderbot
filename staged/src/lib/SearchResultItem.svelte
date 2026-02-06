<!--
  SearchResultItem.svelte - Individual search result display

  Shows a single match with line number and snippet preview.
-->
<script lang="ts">
  import type { SearchMatch } from './services/diffSearch';

  interface Props {
    match: SearchMatch;
    snippet: string;
    isCurrent: boolean;
    onclick: () => void;
  }

  let { match, snippet, isCurrent, onclick }: Props = $props();
</script>

<button class="search-result-item" class:current={isCurrent} {onclick}>
  <span class="result-snippet" title={snippet}>
    {snippet}
  </span>
</button>

<style>
  .search-result-item {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 4px 12px;
    width: 100%;
    background: none;
    border: none;
    color: var(--text-primary);
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: var(--size-xs);
    text-align: left;
    cursor: pointer;
    border-left: 2px solid transparent;
    transition:
      background-color 0.1s,
      border-color 0.1s;
  }

  .search-result-item:hover {
    background-color: var(--bg-hover);
  }

  .search-result-item.current {
    background-color: var(--accent-primary-muted, rgba(59, 130, 246, 0.12));
    border-left-color: var(--accent-primary);
  }

  .result-snippet {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
  }

  .search-result-item.current .result-snippet {
    color: var(--text-primary);
  }
</style>
