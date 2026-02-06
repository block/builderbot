<!--
  CrossFileSearchBar.svelte - Search bar for cross-file search

  Displays at the top of the sidebar with input, match counter, and close button.
-->
<script lang="ts">
  import { Search, X, Loader2, FileText, GitCompareArrows } from 'lucide-svelte';
  import {
    globalSearchState,
    performSearch,
    closeSearch,
    setSearchScope,
    type SearchScope,
  } from './stores/globalSearch.svelte';
  import type { FileDiffSummary, FileDiff } from './types';

  interface Props {
    files: FileDiffSummary[];
    loadFileDiff: (path: string) => Promise<FileDiff | null>;
  }

  let { files, loadFileDiff }: Props = $props();

  let inputElement: HTMLInputElement | null = $state(null);
  let localQuery = $state('');
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;

  const DEBOUNCE_MS = 300;

  // Dynamic placeholder based on scope
  let placeholderText = $derived(
    globalSearchState.scope === 'changes' ? 'Search changed lines...' : 'Search all files...'
  );

  // Match counter text
  let matchCounterText = $derived.by(() => {
    if (!globalSearchState.isOpen) return '';
    if (globalSearchState.loading) {
      return `Searching... (${globalSearchState.searchedFileCount}/${globalSearchState.totalFileCount} files)`;
    }
    if (!globalSearchState.query) return '';
    if (globalSearchState.totalMatches === 0) return 'No results';

    const fileCount = globalSearchState.fileResults.size;
    return `${globalSearchState.totalMatches} result${globalSearchState.totalMatches !== 1 ? 's' : ''} across ${fileCount} file${fileCount !== 1 ? 's' : ''}`;
  });

  // Auto-focus input when search opens
  $effect(() => {
    if (globalSearchState.isOpen && inputElement) {
      inputElement.focus();
    }
  });

  // Sync local query when search opens
  $effect(() => {
    if (globalSearchState.isOpen) {
      localQuery = globalSearchState.query;
    }
  });

  function handleInput(event: Event) {
    const value = (event.target as HTMLInputElement).value;
    localQuery = value;

    // Debounce search
    if (debounceTimer) {
      clearTimeout(debounceTimer);
    }

    debounceTimer = setTimeout(() => {
      performSearch(value, files, loadFileDiff);
      debounceTimer = null;
    }, DEBOUNCE_MS);
  }

  function handleClose() {
    closeSearch();
    localQuery = '';
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      handleClose();
    }
  }

  function handleScopeChange(newScope: SearchScope) {
    setSearchScope(newScope);
    // Re-run search with new scope if we have a query
    if (globalSearchState.query) {
      performSearch(globalSearchState.query, files, loadFileDiff);
    }
  }
</script>

{#if globalSearchState.isOpen}
  <div class="search-bar-container">
    <div class="search-input-wrapper">
      <Search size={14} class="search-icon" />
      <input
        bind:this={inputElement}
        type="text"
        class="search-input"
        placeholder={placeholderText}
        value={localQuery}
        oninput={handleInput}
        onkeydown={handleKeydown}
      />
      {#if globalSearchState.loading}
        <Loader2 size={14} class="loading-icon" />
      {/if}
      <button class="close-btn" onclick={handleClose} title="Close search (Esc)">
        <X size={14} />
      </button>
    </div>

    <div class="search-options">
      <div class="scope-toggle">
        <button
          class="scope-btn"
          class:active={globalSearchState.scope === 'all'}
          onclick={() => handleScopeChange('all')}
          title="Search all lines in file"
        >
          <FileText size={14} />
        </button>
        <button
          class="scope-btn"
          class:active={globalSearchState.scope === 'changes'}
          onclick={() => handleScopeChange('changes')}
          title="Search only changed lines"
        >
          <GitCompareArrows size={14} />
        </button>
      </div>

      {#if matchCounterText}
        <div class="match-counter">{matchCounterText}</div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .search-bar-container {
    padding: 12px;
    border-bottom: 1px solid var(--border-subtle);
    background-color: var(--bg-secondary);
  }

  .search-input-wrapper {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    padding: 6px 10px;
    transition: border-color 0.1s;
  }

  .search-input-wrapper:focus-within {
    border-color: var(--text-accent);
  }

  .search-input-wrapper :global(.search-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .search-input {
    flex: 1;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    outline: none;
    min-width: 0;
  }

  .search-input::placeholder {
    color: var(--text-faint);
  }

  .search-input-wrapper :global(.loading-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px;
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    border-radius: 4px;
    flex-shrink: 0;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .close-btn:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .search-options {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-top: 8px;
  }

  .scope-toggle {
    display: flex;
    gap: 2px;
    background-color: var(--bg-hover);
    border-radius: 5px;
    padding: 2px;
  }

  .scope-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 4px 6px;
    background: none;
    border: none;
    color: var(--text-faint);
    cursor: pointer;
    border-radius: 3px;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .scope-btn:hover:not(.active) {
    color: var(--text-muted);
  }

  .scope-btn.active {
    background-color: var(--bg-primary);
    color: var(--text-accent);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.1);
  }

  .match-counter {
    font-size: var(--size-xs);
    color: var(--text-muted);
    margin-left: auto;
  }
</style>
